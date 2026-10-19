//! Channel logos (ticket 13) and movie and series posters (VOD ticket 06):
//! fetched lazily, cached on disk, decoded and resized on the worker runtime.
//! The UI asks for a logo at the pixel size it draws and gets back ready
//! pixels, or `None` to keep the initials tile.
//!
//! Posters share the hosts' queues and limits with logos, but have their own
//! cache: there are far more of them (about 160k URLs), and they are kept as
//! JPEG, as they come, scaled down to the largest size drawn.
//!
//! A request first looks in the disk cache. On a miss it joins its host's
//! queue: a logo on a provider's own host goes through that provider's
//! [`Client`] (SPEC 6.2, behind its API requests); any other host gets two
//! requests in flight, 250 ms apart, and the same 429/5xx backoff. Queues are newest first
//! and short, so rows scrolled past are dropped rather than fetched.

use crate::xtream::{self, Client, Clients, Gate, Limits};
use image::{DynamicImage, ImageReader, RgbaImage, imageops::FilterType};
use reqwest::{StatusCode, Url};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::{HashMap, VecDeque};
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::{Notify, mpsc};

/// Disk cache size cap; pruning takes it down to 80%.
pub const CACHE_CAP: u64 = 100 << 20;
/// The posters' cap: about 20k at [`POSTER_STORED`].
pub const POSTER_CACHE_CAP: u64 = 256 << 20;
/// Logos are stored no larger than this: the largest drawn (88 px) at 2x.
const STORED: u32 = 176;
/// Posters are stored no taller than this: the largest drawn (120 px) at
/// 2x, 160 by 240 for a 2:3 poster, about 12 KB.
const POSTER_STORED: u32 = 240;
const POSTER_QUALITY: u8 = 85;
/// Requests waiting per queue; older ones are dropped.
const QUEUE: usize = 64;
/// Larger downloads are not logos.
const MAX_BYTES: usize = 2 << 20;
/// How long a broken logo (4xx, not an image) is left alone.
const BAD_FOR: Duration = Duration::from_secs(7 * 24 * 3600);
/// Requests in flight per third-party host (SPEC 6.2).
const PER_HOST: usize = 2;
/// Least time between request starts to one third-party host (SPEC 6.2).
const HOST_GAP: Duration = Duration::from_millis(250);
/// Tasks reading the disk cache.
const DISK_READERS: usize = 2;

/// A logo wanted `px` pixels square (physical); a poster `px` high.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub url: String,
    pub px: u32,
    pub poster: bool,
}

/// The answer to a [`Request`]: the logo fitted into `px` square, or `None`
/// if it failed and the tile stays.
pub struct Logo {
    pub url: String,
    pub px: u32,
    pub pixels: Option<SharedPixelBuffer<Rgba8Pixel>>,
}

/// Starts the loader on the current runtime, caching logos in `dir` and
/// posters in `poster_dir`. Requests go in on the returned sender; logos
/// come out on `out`.
pub fn spawn(
    dir: PathBuf,
    poster_dir: PathBuf,
    providers: Clients,
    limits: Limits,
    out: mpsc::UnboundedSender<Logo>,
) -> mpsc::UnboundedSender<Request> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Request>();
    let shared = Arc::new(Shared {
        cache: Cache::new(dir, CACHE_CAP, "png"),
        posters: Cache::new(poster_dir, POSTER_CACHE_CAP, "jpg"),
        http: xtream::http(),
        providers,
        limits,
        hosts: Mutex::default(),
        disk: Queue::default(),
        out,
    });
    {
        let s = shared.clone();
        tokio::task::spawn_blocking(move || {
            s.cache.scan();
            s.posters.scan();
        });
    }
    for _ in 0..DISK_READERS {
        tokio::spawn(disk_reader(shared.clone()));
    }
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            shared.disk.push(req.clone(), |r| *r == req);
        }
    });
    tx
}

struct Shared {
    cache: Cache,
    posters: Cache,
    http: reqwest::Client,
    /// The provider clients, found by host.
    providers: Clients,
    limits: Limits,
    hosts: Mutex<HashMap<String, Arc<Host>>>,
    disk: Queue<Request>,
    out: mpsc::UnboundedSender<Logo>,
}

/// A logo to download, and the sizes wanted from it.
struct Job {
    url: Url,
    poster: bool,
    sizes: Vec<u32>,
}

struct Host {
    queue: Queue<Job>,
    /// Downloads under way, by URL, with the sizes wanted from each.
    in_flight: Mutex<HashMap<String, Vec<u32>>>,
    via: Via,
}

enum Via {
    Provider(Arc<Client>),
    ThirdParty(Mutex<Gate>),
}

/// How a download went.
enum Fetched {
    Body(Vec<u8>),
    /// 429 or 5xx: try again once the backoff ends.
    Retry,
    /// Any other HTTP error: remembered as broken.
    Broken(String),
    /// Network trouble: given up on for this run only.
    Failed(String),
}

/// Newest first, at most [`QUEUE`] long.
struct Queue<T> {
    items: Mutex<VecDeque<T>>,
    ready: Notify,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self {
            items: Mutex::default(),
            ready: Notify::new(),
        }
    }
}

impl<T> Queue<T> {
    /// Puts `item` first, replacing any entry `same` matches.
    fn push(&self, item: T, same: impl Fn(&T) -> bool) {
        let mut items = self.items.lock().unwrap();
        items.retain(|i| !same(i));
        items.push_front(item);
        items.truncate(QUEUE);
        self.ready.notify_one();
    }

    async fn pop(&self) -> T {
        loop {
            if let Some(item) = self.items.lock().unwrap().pop_front() {
                return item;
            }
            self.ready.notified().await;
        }
    }
}

async fn disk_reader(s: Arc<Shared>) {
    loop {
        let req = s.disk.pop().await;
        let url = match Url::parse(&req.url) {
            Ok(u) if matches!(u.scheme(), "http" | "https") && u.host_str().is_some() => u,
            // Including the odd `data:` poster.
            _ => {
                s.send(&req.url, req.px, None);
                continue;
            }
        };
        let s2 = s.clone();
        let key = req.url.clone();
        let found = tokio::task::spawn_blocking(move || {
            let cache = s2.cache(req.poster);
            if let Some(bytes) = cache.get(&key) {
                return Some(decode(&bytes).map(|img| fit(&img, req.px)));
            }
            cache.is_bad(&key).then_some(None)
        })
        .await
        .unwrap_or(Some(None));
        match found {
            Some(pixels) => s.send(&req.url, req.px, pixels),
            None => s.enqueue(url, req.px, req.poster),
        }
    }
}

async fn host_worker(s: Arc<Shared>, host: Arc<Host>) {
    loop {
        let job = host.queue.pop().await;
        let key = job.url.as_str().to_owned();
        host.in_flight
            .lock()
            .unwrap()
            .insert(key.clone(), job.sizes);
        let fetched = s.download(&host, job.url.clone()).await;
        let sizes = host
            .in_flight
            .lock()
            .unwrap()
            .remove(&key)
            .unwrap_or_default();
        let pixels = match fetched {
            Fetched::Body(body) => {
                let s2 = s.clone();
                let (key, sizes, poster) = (key.clone(), sizes.clone(), job.poster);
                tokio::task::spawn_blocking(move || s2.store(&key, &body, &sizes, poster))
                    .await
                    .unwrap_or_default()
            }
            Fetched::Retry => {
                let (url, poster) = (job.url, job.poster);
                host.queue
                    .push(Job { url, poster, sizes }, |j| j.url.as_str() == key);
                continue;
            }
            Fetched::Broken(why) => {
                log::debug!("logo {key}: {why}");
                s.cache(job.poster).mark_bad(&key);
                Vec::new()
            }
            Fetched::Failed(why) => {
                log::debug!("logo {key}: {why}");
                Vec::new()
            }
        };
        let mut pixels = pixels.into_iter();
        for px in sizes {
            s.send(&key, px, pixels.next().flatten());
        }
    }
}

impl Shared {
    fn cache(&self, poster: bool) -> &Cache {
        if poster { &self.posters } else { &self.cache }
    }

    fn send(&self, url: &str, px: u32, pixels: Option<SharedPixelBuffer<Rgba8Pixel>>) {
        let _ = self.out.send(Logo {
            url: url.to_owned(),
            px,
            pixels,
        });
    }

    /// A cache miss: queue it on its host, joining a download already queued
    /// or under way for the same URL.
    fn enqueue(self: &Arc<Self>, url: Url, px: u32, poster: bool) {
        let host = self.host(url.host_str().unwrap_or_default());
        let key = url.as_str();
        if let Some(sizes) = host.in_flight.lock().unwrap().get_mut(key) {
            if !sizes.contains(&px) {
                sizes.push(px);
            }
            return;
        }
        let mut sizes = vec![px];
        {
            let items = host.queue.items.lock().unwrap();
            if let Some(queued) = items.iter().find(|j| j.url.as_str() == key) {
                sizes.extend(queued.sizes.iter().filter(|&&s| s != px));
            }
        }
        let key = key.to_owned();
        host.queue
            .push(Job { url, poster, sizes }, |j| j.url.as_str() == key);
    }

    fn host(self: &Arc<Self>, name: &str) -> Arc<Host> {
        let mut hosts = self.hosts.lock().unwrap();
        if let Some(h) = hosts.get(name) {
            return h.clone();
        }
        let (via, workers) = match self.providers.by_host(name) {
            Some(client) => (Via::Provider(client), 1),
            None => (Via::ThirdParty(Mutex::new(Gate::new())), PER_HOST),
        };
        let host = Arc::new(Host {
            queue: Queue::default(),
            in_flight: Mutex::default(),
            via,
        });
        for _ in 0..workers {
            tokio::spawn(host_worker(self.clone(), host.clone()));
        }
        hosts.insert(name.to_owned(), host.clone());
        host
    }

    async fn download(&self, host: &Host, url: Url) -> Fetched {
        let result = match &host.via {
            Via::Provider(client) => client.logo(url, MAX_BYTES).await,
            Via::ThirdParty(gate) => {
                // Takes the next start slot, or waits for it.
                loop {
                    let at = {
                        let mut gate = gate.lock().unwrap();
                        let now = Instant::now();
                        if gate.next_at <= now {
                            gate.next_at = now + HOST_GAP;
                            break;
                        }
                        gate.next_at
                    };
                    tokio::time::sleep_until(at.into()).await;
                }
                let result = xtream::get_small(&self.http, url.clone(), MAX_BYTES).await;
                if let Ok((status, _)) = &result {
                    let mut gate = gate.lock().unwrap();
                    if gate.record(*status, &self.limits) {
                        let host = url.host_str().unwrap_or_default();
                        log::warn!("logos from {host}: HTTP {status}, backing off");
                    }
                }
                result
            }
        };
        match result {
            Ok((status, body)) if status.is_success() => Fetched::Body(body),
            Ok((status, _))
                if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() =>
            {
                Fetched::Retry
            }
            Ok((status, _)) => Fetched::Broken(format!("HTTP {status}")),
            Err(e) => Fetched::Failed(e),
        }
    }

    /// Decodes a download, caches it at [`STORED`] size (a poster at
    /// [`POSTER_STORED`]) and fits it to each of `sizes`. A body that is not
    /// an image is remembered as broken.
    fn store(
        &self,
        url: &str,
        body: &[u8],
        sizes: &[u32],
        poster: bool,
    ) -> Vec<Option<SharedPixelBuffer<Rgba8Pixel>>> {
        let cache = self.cache(poster);
        let Some(img) = decode(body) else {
            log::debug!("logo {url}: not an image");
            cache.mark_bad(url);
            return Vec::new();
        };
        // A poster keeps its shape; a logo fills its box.
        let (img, stored) = match poster {
            true => (img, POSTER_STORED),
            false => (trim(img), STORED),
        };
        let img = if img.width().max(img.height()) > stored {
            DynamicImage::ImageRgba8(img)
                .resize(stored, stored, FilterType::Lanczos3)
                .into_rgba8()
        } else {
            img
        };
        let mut bytes = Vec::new();
        let encoded = match poster {
            true => DynamicImage::ImageRgba8(img.clone())
                .into_rgb8()
                .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(
                    &mut bytes,
                    POSTER_QUALITY,
                )),
            false => DynamicImage::ImageRgba8(img.clone())
                .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png),
        };
        match encoded {
            Ok(()) => cache.put(url, &bytes),
            Err(e) => log::warn!("logo cache: encoding failed: {e}"),
        }
        sizes.iter().map(|&px| Some(fit(&img, px))).collect()
    }
}

/// Decodes PNG, JPEG, WebP or GIF (first frame), refusing huge images.
fn decode(bytes: &[u8]) -> Option<RgbaImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 << 20);
    reader.limits(limits);
    Some(reader.decode().ok()?.into_rgba8())
}

/// Crops to the pixels that are not (nearly) transparent, so the logo
/// itself fills its box. An image with none is left as it is.
fn trim(img: RgbaImage) -> RgbaImage {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if p[3] > 8 {
            (x0, y0) = (x0.min(x), y0.min(y));
            (x1, y1) = (x1.max(x), y1.max(y));
        }
    }
    if x0 > x1 || (x0, y0, x1 + 1, y1 + 1) == (0, 0, img.width(), img.height()) {
        return img;
    }
    image::imageops::crop_imm(&img, x0, y0, x1 - x0 + 1, y1 - y0 + 1).to_image()
}

/// Scales `img` to fit within `px` square, keeping its aspect ratio.
fn fit(img: &RgbaImage, px: u32) -> SharedPixelBuffer<Rgba8Pixel> {
    let px = px.max(1);
    let (w, h) = (img.width().max(1), img.height().max(1));
    let (fw, fh) = if w >= h {
        (px, (px * h / w).max(1))
    } else {
        ((px * w / h).max(1), px)
    };
    let out = image::imageops::resize(img, fw, fh, FilterType::CatmullRom);
    SharedPixelBuffer::clone_from_slice(out.as_raw(), fw, fh)
}

/// The disk cache: `<hash>.png` per logo (`.jpg` per poster), normalised,
/// and an empty `<hash>.bad` for a broken one. Oldest-used files are pruned
/// past the cap.
pub struct Cache {
    dir: PathBuf,
    cap: u64,
    /// The images' extension.
    ext: &'static str,
    /// Bytes of images, once [`Cache::scan`] has run.
    size: Mutex<Option<u64>>,
}

impl Cache {
    pub fn new(dir: PathBuf, cap: u64, ext: &'static str) -> Self {
        Self {
            dir,
            cap,
            ext,
            size: Mutex::new(None),
        }
    }

    /// FNV-1a: stable across builds, unlike `std`'s hasher.
    fn path(&self, url: &str, ext: &str) -> PathBuf {
        let h = url.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
        self.dir.join(format!("{h:016x}.{ext}"))
    }

    /// The cached image, marked as just used.
    pub fn get(&self, url: &str) -> Option<Vec<u8>> {
        let path = self.path(url, self.ext);
        let bytes = std::fs::read(&path).ok()?;
        if let Ok(f) = std::fs::File::options().append(true).open(&path) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(bytes)
    }

    pub fn put(&self, url: &str, bytes: &[u8]) {
        let path = self.path(url, self.ext);
        let tmp = path.with_extension("tmp");
        let written = std::fs::create_dir_all(&self.dir)
            .and_then(|()| std::fs::write(&tmp, bytes))
            .and_then(|()| std::fs::rename(&tmp, &path));
        if let Err(e) = written {
            log::warn!("logo cache: writing {}: {e}", path.display());
            return;
        }
        let over = {
            let mut size = self.size.lock().unwrap();
            size.as_mut().map(|s| {
                *s += bytes.len() as u64;
                *s > self.cap
            })
        };
        if over == Some(true) {
            self.prune();
        }
    }

    pub fn mark_bad(&self, url: &str) {
        let _ = std::fs::create_dir_all(&self.dir)
            .and_then(|()| std::fs::write(self.path(url, "bad"), b""));
    }

    /// Marked broken less than [`BAD_FOR`] ago.
    pub fn is_bad(&self, url: &str) -> bool {
        std::fs::metadata(self.path(url, "bad"))
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|age| age < BAD_FOR))
    }

    /// Totals the cache at startup, pruning if it is over the cap.
    pub fn scan(&self) {
        let total: u64 = self.files(self.ext).iter().map(|(_, len, _)| len).sum();
        *self.size.lock().unwrap() = Some(total);
        if total > self.cap {
            self.prune();
        }
    }

    /// Deletes the least recently used logos until the cache is at 80% of
    /// its cap, and broken marks that have expired.
    pub fn prune(&self) {
        let mut files = self.files(self.ext);
        files.sort_by_key(|(_, _, used)| *used);
        let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
        let target = self.cap / 5 * 4;
        let mut removed = 0;
        for (path, len, _) in files {
            if total <= target {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= len;
                removed += 1;
            }
        }
        for (path, _, marked) in self.files("bad") {
            if marked.elapsed().is_ok_and(|age| age >= BAD_FOR) {
                let _ = std::fs::remove_file(path);
            }
        }
        *self.size.lock().unwrap() = Some(total);
        log::debug!("logo cache: pruned {removed} files, {total} bytes left");
    }

    /// Path, length and modification time of each `*.ext`.
    fn files(&self, ext: &str) -> Vec<(PathBuf, u64, SystemTime)> {
        let Ok(dir) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        dir.filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == ext))
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                Some((e.path(), m.len(), m.modified().ok()?))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_xtream::{FakeXtream, Reply};
    use std::path::Path;

    const MS: Duration = Duration::from_millis(1);

    const FAST: Limits = Limits {
        min_gap: Duration::from_millis(40),
        backoff_start: Duration::from_millis(150),
        backoff_max: Duration::from_millis(300),
    };

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = RgbaImage::from_pixel(w, h, image::Rgba([200, 30, 30, 255]));
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn ok(body: Vec<u8>, delay: Duration) -> Reply {
        Reply {
            status: 200,
            body,
            delay,
        }
    }

    struct Loader {
        tx: mpsc::UnboundedSender<Request>,
        rx: mpsc::UnboundedReceiver<Logo>,
    }

    impl Loader {
        fn start(dir: &Path, providers: Vec<Arc<Client>>) -> Self {
            let (out, rx) = mpsc::unbounded_channel();
            let providers = providers
                .into_iter()
                .enumerate()
                .map(|(i, c)| (i.to_string(), c))
                .collect();
            let tx = spawn(dir.join("logos"), dir.join("posters"), providers, FAST, out);
            Self { tx, rx }
        }

        fn ask(&self, url: &str, px: u32) {
            self.send(url, px, false);
        }

        fn ask_poster(&self, url: &str, px: u32) {
            self.send(url, px, true);
        }

        fn send(&self, url: &str, px: u32, poster: bool) {
            self.tx
                .send(Request {
                    url: url.into(),
                    px,
                    poster,
                })
                .unwrap();
        }

        async fn next(&mut self) -> Logo {
            tokio::time::timeout(Duration::from_secs(10), self.rx.recv())
                .await
                .expect("no logo")
                .unwrap()
        }
    }

    #[test]
    fn cache_hit_miss_and_prune() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().into(), 250, "png");
        cache.scan();
        assert_eq!(cache.get("http://a/1.png"), None, "miss");
        let t0 = SystemTime::now() - Duration::from_secs(3600);
        for i in 0..4 {
            let url = format!("http://a/{i}.png");
            cache.put(&url, &[i as u8; 60]);
            // Distinct use times, oldest first.
            let f = std::fs::File::options()
                .append(true)
                .open(cache.path(&url, "png"))
                .unwrap();
            f.set_modified(t0 + Duration::from_secs(i * 60)).unwrap();
        }
        assert_eq!(cache.get("http://a/0.png"), Some(vec![0; 60]), "hit");
        // 0 was just used, so 1 is now the oldest; 300 bytes is over the cap.
        cache.put("http://a/4.png", &[4; 60]);
        let kept: Vec<bool> = (0..5)
            .map(|i| cache.path(&format!("http://a/{i}.png"), "png").exists())
            .collect();
        // Pruned to 200 bytes: the two least recently used are gone.
        assert_eq!(kept, [true, false, false, true, true]);
        assert_eq!(*cache.size.lock().unwrap(), Some(180));
    }

    #[test]
    fn transparent_margins_are_trimmed() {
        let mut img = RgbaImage::new(100, 60);
        for x in 20..80 {
            for y in 15..45 {
                img.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let img = trim(decode(&bytes).unwrap());
        assert_eq!(img.dimensions(), (60, 30));
        let pixels = fit(&img, 40);
        assert_eq!((pixels.width(), pixels.height()), (40, 20));
        // Nothing but transparency: kept whole.
        assert_eq!(trim(RgbaImage::new(5, 5)).dimensions(), (5, 5));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn downloads_once_then_serves_from_disk() {
        let fake = FakeXtream::start_with(|_| ok(png(300, 150), Duration::ZERO)).await;
        let dir = tempfile::tempdir().unwrap();
        let url = format!("{}/logo/1.png", fake.url());
        let mut loader = Loader::start(dir.path(), Vec::new());
        loader.ask(&url, 32);
        let logo = loader.next().await;
        let pixels = logo.pixels.expect("decoded");
        // Fitted into the box, aspect kept.
        assert_eq!((pixels.width(), pixels.height()), (32, 16));

        // A new run, same cache: no request.
        let mut loader = Loader::start(dir.path(), Vec::new());
        loader.ask(&url, 88);
        let pixels = loader.next().await.pixels.expect("from disk");
        assert_eq!((pixels.width(), pixels.height()), (88, 44));
        assert_eq!(fake.targets(), ["/logo/1.png"]);
        // Stored no larger than STORED.
        let stored = Cache::new(dir.path().join("logos"), CACHE_CAP, "png").get(&url);
        assert_eq!(
            decode(&stored.unwrap()).unwrap().dimensions(),
            (STORED, STORED / 2)
        );
    }

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([30, 60, 200]));
        let mut out = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Jpeg)
            .unwrap();
        out
    }

    /// A poster (VOD ticket 06): fetched once, kept as a JPEG no taller than
    /// `POSTER_STORED` in its own cache, then read from disk at any size.
    #[tokio::test(flavor = "multi_thread")]
    async fn poster_downloads_once_then_serves_from_disk() {
        let fake = FakeXtream::start_with(|_| ok(jpeg(600, 900), Duration::ZERO)).await;
        let dir = tempfile::tempdir().unwrap();
        let url = format!("{}/poster/1.jpg", fake.url());
        let mut loader = Loader::start(dir.path(), Vec::new());
        loader.ask_poster(&url, 84);
        let pixels = loader.next().await.pixels.expect("decoded");
        // 2:3, `px` high.
        assert_eq!((pixels.width(), pixels.height()), (56, 84));

        let mut loader = Loader::start(dir.path(), Vec::new());
        loader.ask_poster(&url, 240);
        let pixels = loader.next().await.pixels.expect("from disk");
        assert_eq!((pixels.width(), pixels.height()), (160, 240));
        assert_eq!(fake.targets(), ["/poster/1.jpg"]);

        let posters = Cache::new(dir.path().join("posters"), POSTER_CACHE_CAP, "jpg");
        let stored = posters.get(&url).unwrap();
        assert_eq!(
            ImageReader::new(Cursor::new(&stored))
                .with_guessed_format()
                .unwrap()
                .format(),
            Some(image::ImageFormat::Jpeg)
        );
        assert_eq!(decode(&stored).unwrap().dimensions(), (160, POSTER_STORED));
        assert!(!dir.path().join("logos").exists(), "not with the logos");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn broken_poster_falls_back_and_is_not_retried() {
        let fake = FakeXtream::start_with(|target| match target {
            "/404.jpg" => Reply {
                status: 404,
                body: Vec::new(),
                delay: Duration::ZERO,
            },
            _ => ok(b"<html>not a poster</html>".to_vec(), Duration::ZERO),
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        for _ in 0..2 {
            let mut loader = Loader::start(dir.path(), Vec::new());
            for path in ["/html.jpg", "/404.jpg"] {
                loader.ask_poster(&format!("{}{path}", fake.url()), 84);
                assert!(loader.next().await.pixels.is_none(), "{path}: tile");
            }
            loader.ask_poster("data:image/jpeg;base64,/9j/", 84);
            assert!(loader.next().await.pixels.is_none());
        }
        assert_eq!(fake.targets().len(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn broken_image_falls_back_and_is_not_retried() {
        let fake = FakeXtream::start_with(|target| match target {
            "/404.png" => Reply {
                status: 404,
                body: Vec::new(),
                delay: Duration::ZERO,
            },
            _ => ok(b"<html>not a logo</html>".to_vec(), Duration::ZERO),
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let mut loader = Loader::start(dir.path(), Vec::new());
        for path in ["/html.png", "/404.png"] {
            loader.ask(&format!("{}{path}", fake.url()), 32);
            assert!(loader.next().await.pixels.is_none(), "{path}: tile");
        }
        loader.ask("not a url", 32);
        assert!(loader.next().await.pixels.is_none());

        // Remembered across runs.
        let mut loader = Loader::start(dir.path(), Vec::new());
        for path in ["/html.png", "/404.png"] {
            loader.ask(&format!("{}{path}", fake.url()), 32);
            assert!(loader.next().await.pixels.is_none());
        }
        assert_eq!(fake.targets().len(), 2);
    }

    /// A third-party host: two requests in flight at most, starting at
    /// least `HOST_GAP` apart.
    #[tokio::test(flavor = "multi_thread")]
    async fn third_party_hosts_get_two_in_flight() {
        let fake = FakeXtream::start_with(|_| ok(png(8, 8), 600 * MS)).await;
        let dir = tempfile::tempdir().unwrap();
        let mut loader = Loader::start(dir.path(), Vec::new());
        for i in 0..6 {
            loader.ask(&format!("{}/{i}.png", fake.url()), 32);
        }
        for _ in 0..6 {
            assert!(loader.next().await.pixels.is_some());
        }
        assert_eq!(fake.max_in_flight(), 2);
        let gaps: Vec<Duration> = fake.times().windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.iter().all(|&g| g >= HOST_GAP - 5 * MS), "{gaps:?}");
    }

    /// 429 and 5xx back off, doubling, and the logo still arrives after.
    #[tokio::test(flavor = "multi_thread")]
    async fn third_party_backoff() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let fake = FakeXtream::start_with(|_| match CALLS.fetch_add(1, Ordering::SeqCst) {
            0 => Reply {
                status: 503,
                body: Vec::new(),
                delay: Duration::ZERO,
            },
            1 => Reply {
                status: 429,
                body: Vec::new(),
                delay: Duration::ZERO,
            },
            _ => ok(png(8, 8), Duration::ZERO),
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let mut loader = Loader::start(dir.path(), Vec::new());
        loader.ask(&format!("{}/a.png", fake.url()), 32);
        assert!(loader.next().await.pixels.is_some());
        let t = fake.times();
        assert_eq!(t.len(), 3);
        let gaps: Vec<Duration> = t.windows(2).map(|w| w[1] - w[0]).collect();
        for (gap, want) in gaps.iter().zip([150, 300]) {
            let want = want * MS;
            assert!(*gap >= want && *gap < want + 100 * MS, "gaps {gaps:?}");
        }
    }

    /// A logo on the provider's host goes through its client: one at a
    /// time, spaced by its gap, behind a waiting API request.
    #[tokio::test(flavor = "multi_thread")]
    async fn provider_host_uses_the_provider_limits() {
        let fake = FakeXtream::start_with(|target| {
            if target.starts_with("/logo") {
                ok(png(8, 8), Duration::ZERO)
            } else {
                let (status, body) = crate::fake_xtream::active(target);
                Reply {
                    status,
                    body: body.into_bytes(),
                    delay: Duration::ZERO,
                }
            }
        })
        .await;
        let client = Arc::new(Client::with_limits(fake.provider(), FAST));
        let dir = tempfile::tempdir().unwrap();
        let mut loader = Loader::start(dir.path(), vec![client.clone()]);
        for i in 0..3 {
            loader.ask(&format!("{}/logo/{i}.png", fake.url()), 32);
        }
        // Asked while the logos wait: goes first after the one in flight.
        tokio::time::sleep(10 * MS).await;
        client.account().await.unwrap();
        for _ in 0..3 {
            assert!(loader.next().await.pixels.is_some());
        }
        let targets = fake.targets();
        assert_eq!(targets.len(), 4);
        assert!(
            targets[..2].iter().any(|t| !t.starts_with("/logo")),
            "{targets:?}"
        );
        assert_eq!(fake.max_in_flight(), 1);
        let gaps: Vec<Duration> = fake.times().windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.iter().all(|&g| g >= FAST.min_gap), "{gaps:?}");
    }
}
