//! Downloads of movies and episodes (VOD ticket 10; SPEC 9.6): a plain
//! HTTP GET to `<file>.part`, renamed to the file once complete. A part
//! left by a pause, a failure or a quit resumes with `Range`, which the
//! provider honours (VOD ticket 01). The recorder queues them and decides
//! when each may take a connection.

use crate::recorder::name_part;
use crate::store::Download;
use reqwest::{StatusCode, header};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

/// How often a running download says how far it is.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);

/// The HTTP client for downloads: no overall timeout, as a movie takes
/// minutes, but one on each read, so a stalled one fails.
pub fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("vidicon/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .build()
        .expect("HTTP client")
}

/// A download's file name, without the extension: the movie's name, or
/// `Series - S02E04 - Name` for an episode (`S02E04` alone without a name,
/// `S02 - Name` without a number).
pub fn file_stem(d: &Download) -> String {
    let title = name_part(&d.title, 100);
    let Some(season) = d.season.filter(|_| d.series_id.is_some()) else {
        return title;
    };
    let mark = match d.episode_num {
        Some(n) => format!("S{season:02}E{n:02}"),
        None => format!("S{season:02}"),
    };
    match d.episode.as_deref().map(|e| name_part(e, 100)) {
        Some(e) if !e.is_empty() => format!("{title} - {mark} - {e}"),
        _ => format!("{title} - {mark}"),
    }
}

/// Picks `<dir>/<stem>.<ext>`, or `<stem> (2).<ext>` and on, where neither
/// the file nor its part exists, and creates the part, making `dir` first.
/// Returns the file's path; it is written as `<file>.part`.
pub fn reserve(dir: &Path, stem: &str, ext: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    for n in 1.. {
        let file = match n {
            1 => dir.join(format!("{stem}.{ext}")),
            n => dir.join(format!("{stem} ({n}).{ext}")),
        };
        if file.exists() {
            continue;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(part_of(&file))
        {
            Ok(_) => return Ok(file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

/// `<file>.part`.
pub fn part_of(file: &Path) -> PathBuf {
    let mut part = file.as_os_str().to_owned();
    part.push(".part");
    part.into()
}

/// Renames a complete part to `file`, or to `<stem> (2).<ext>` and on if
/// something took that name meanwhile, passing over names reserved for
/// other downloads. Returns where it went.
pub fn finish(file: &Path) -> std::io::Result<PathBuf> {
    let part = part_of(file);
    let (stem, ext) = match (file.file_stem(), file.extension()) {
        (Some(s), Some(e)) => (s.to_string_lossy(), e.to_string_lossy()),
        _ => (file.to_string_lossy(), "".into()),
    };
    let dir = file.parent().unwrap_or(Path::new("."));
    for n in 1.. {
        let to = match n {
            1 => file.to_path_buf(),
            n => dir.join(format!("{stem} ({n}).{ext}")),
        };
        // Passing over names another download has reserved.
        if !to.exists() && (n == 1 || !part_of(&to).exists()) {
            std::fs::rename(&part, &to)?;
            return Ok(to);
        }
    }
    unreachable!()
}

/// How a download ended.
#[derive(Debug, PartialEq)]
pub enum Exit {
    /// The part holds the whole file.
    Done,
    /// Stopped by the recorder; the part stays to resume.
    Stopped,
    /// Why it failed, without the URL. The part stays.
    Failed(String),
}

/// What a running download says while it runs.
#[derive(Debug, PartialEq)]
pub enum Report {
    /// The whole file's size, in bytes, once the provider says.
    Total(u64),
    /// How many bytes the part holds, at most each second.
    Have(u64),
}

/// GETs `url` into `part`, from where it stops, until done, failed or
/// `stop`.
pub async fn fetch(
    http: &reqwest::Client,
    url: reqwest::Url,
    part: &Path,
    mut stop: oneshot::Receiver<()>,
    report: impl Fn(Report),
) -> Exit {
    let mut have = std::fs::metadata(part).map_or(0, |m| m.len());
    let mut request = http.get(url);
    if have > 0 {
        request = request.header(header::RANGE, format!("bytes={have}-"));
    }
    let response = tokio::select! {
        r = request.send() => r,
        _ = &mut stop => return Exit::Stopped,
    };
    let mut response = match response {
        Ok(r) => r,
        Err(e) => return Exit::Failed(e.without_url().to_string()),
    };
    let status = response.status();
    let header = |name| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let total = match status {
        StatusCode::PARTIAL_CONTENT => {
            header(header::CONTENT_RANGE).and_then(|r| r.rsplit_once('/')?.1.parse().ok())
        }
        // Asked for the rest of a part that is whole already.
        StatusCode::RANGE_NOT_SATISFIABLE if have > 0 => return Exit::Done,
        StatusCode::OK => {
            // A whole body, the range ignored: start the part again.
            have = 0;
            response.content_length()
        }
        s => return Exit::Failed(format!("the provider answered HTTP {}", s.as_u16())),
    };
    if let Some(total) = total {
        report(Report::Total(total));
    }
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(have > 0)
        .truncate(have == 0)
        .open(part)
        .await;
    let mut file = match file {
        Ok(f) => f,
        Err(e) => return Exit::Failed(format!("can't write {}: {e}", part.display())),
    };
    let mut said = Instant::now();
    loop {
        let chunk = tokio::select! {
            c = response.chunk() => c,
            _ = &mut stop => {
                let _ = file.flush().await;
                return Exit::Stopped;
            }
        };
        match chunk {
            Ok(Some(bytes)) => {
                if let Err(e) = file.write_all(&bytes).await {
                    return Exit::Failed(format!("can't write {}: {e}", part.display()));
                }
                have += bytes.len() as u64;
                if said.elapsed() >= PROGRESS_EVERY {
                    said = Instant::now();
                    report(Report::Have(have));
                }
            }
            Ok(None) => break,
            Err(e) => {
                let _ = file.flush().await;
                return Exit::Failed(e.without_url().to_string());
            }
        }
    }
    if let Err(e) = file.flush().await {
        return Exit::Failed(format!("can't write {}: {e}", part.display()));
    }
    report(Report::Have(have));
    match total {
        Some(total) if have < total => Exit::Failed(format!(
            "the provider stopped sending at {have} of {total} bytes"
        )),
        _ => Exit::Done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_xtream::{FakeFile, FileOpts};
    use crate::store::DlState;

    fn movie(title: &str) -> Download {
        Download {
            id: 1,
            provider_id: "p".into(),
            stream_id: 7,
            series_id: None,
            season: None,
            episode_num: None,
            title: title.into(),
            episode: None,
            extension: "mkv".into(),
            state: DlState::Queued,
            file: None,
            total: None,
            note: None,
            added: 0,
            ended: None,
            trashed: false,
        }
    }

    fn episode(season: i64, num: Option<i64>, name: Option<&str>) -> Download {
        Download {
            series_id: Some(3),
            season: Some(season),
            episode_num: num,
            episode: name.map(str::to_string),
            ..movie("Glass Kings")
        }
    }

    #[test]
    fn names_come_from_the_title_and_episode() {
        assert_eq!(file_stem(&movie("The Long Harbour")), "The Long Harbour");
        assert_eq!(file_stem(&movie("Who? What/When: 2")), "Who- What-When- 2");
        assert_eq!(
            file_stem(&episode(2, Some(4), Some("The Night Ferry"))),
            "Glass Kings - S02E04 - The Night Ferry"
        );
        assert_eq!(
            file_stem(&episode(2, Some(4), None)),
            "Glass Kings - S02E04"
        );
        assert_eq!(
            file_stem(&episode(11, None, Some("Pilot"))),
            "Glass Kings - S11 - Pilot"
        );
    }

    #[test]
    fn reserving_passes_over_taken_files_and_parts() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().join("Downloads");
        let first = reserve(&dir, "Film", "mkv").unwrap();
        assert_eq!(first, dir.join("Film.mkv"));
        assert!(part_of(&first).exists() && !first.exists());
        // Its part takes the name; so does a finished file.
        assert_eq!(
            reserve(&dir, "Film", "mkv").unwrap(),
            dir.join("Film (2).mkv")
        );
        std::fs::write(dir.join("Film (3).mkv"), b"x").unwrap();
        assert_eq!(
            reserve(&dir, "Film", "mkv").unwrap(),
            dir.join("Film (4).mkv")
        );

        // Finishing renames the part, around a file that took the name.
        std::fs::write(&first, b"other").unwrap();
        assert_eq!(finish(&first).unwrap(), dir.join("Film (5).mkv"));
        assert!(!part_of(&first).exists());
    }

    fn body() -> Vec<u8> {
        (0..50_000u32).map(|i| (i % 251) as u8).collect()
    }

    fn quick() -> FileOpts {
        FileOpts {
            chunk: 10_000,
            ..Default::default()
        }
    }

    async fn get(server: &FakeFile, part: &Path) -> (Exit, Vec<Report>) {
        let url = reqwest::Url::parse(&format!("{}/movie/u/p/7.mkv", server.url())).unwrap();
        let reports = std::sync::Mutex::new(Vec::new());
        let (_stop, stopped) = oneshot::channel();
        let exit = fetch(&http(), url, part, stopped, |r| {
            reports.lock().unwrap().push(r)
        })
        .await;
        (exit, reports.into_inner().unwrap())
    }

    #[tokio::test]
    async fn downloads_a_whole_file_and_resumes_a_part() {
        let server = FakeFile::start(body(), quick()).await;
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("Film.mkv.part");
        let (exit, reports) = get(&server, &part).await;
        assert_eq!(exit, Exit::Done);
        assert_eq!(reports.first(), Some(&Report::Total(50_000)));
        assert_eq!(reports.last(), Some(&Report::Have(50_000)));
        assert_eq!(std::fs::read(&part).unwrap(), body());

        // A part cut at 12 kB asks for the rest only.
        std::fs::write(&part, &body()[..12_000]).unwrap();
        let (exit, _) = get(&server, &part).await;
        assert_eq!(exit, Exit::Done);
        assert_eq!(server.ranges(), [0, 12_000]);
        assert_eq!(std::fs::read(&part).unwrap(), body());
    }

    #[tokio::test]
    async fn a_server_ignoring_the_range_starts_the_part_again() {
        let opts = FileOpts {
            ignore_range: true,
            ..quick()
        };
        let server = FakeFile::start(body(), opts).await;
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("Film.mkv.part");
        std::fs::write(&part, &body()[..12_000]).unwrap();
        assert_eq!(get(&server, &part).await.0, Exit::Done);
        assert_eq!(std::fs::read(&part).unwrap(), body());
    }

    #[tokio::test]
    async fn failures_say_why_and_keep_the_part() {
        let cut = FileOpts {
            cut_at: Some(20_000),
            ..quick()
        };
        let server = FakeFile::start(body(), cut).await;
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("Film.mkv.part");
        let Exit::Failed(note) = get(&server, &part).await.0 else {
            panic!("a cut download is a failure");
        };
        assert!(!note.contains("/u/p/"), "{note}");
        assert_eq!(std::fs::read(&part).unwrap(), &body()[..20_000]);

        let missing = FileOpts {
            status: Some(404),
            ..quick()
        };
        let server = FakeFile::start(body(), missing).await;
        let (exit, _) = get(&server, &part).await;
        assert_eq!(exit, Exit::Failed("the provider answered HTTP 404".into()));
        assert_eq!(std::fs::metadata(&part).unwrap().len(), 20_000);
    }

    #[tokio::test]
    async fn a_stop_ends_it_keeping_the_part() {
        let slow = FileOpts {
            chunk: 1_000,
            every: Duration::from_millis(20),
            ..Default::default()
        };
        let server = FakeFile::start(body(), slow).await;
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("Film.mkv.part");
        let url = reqwest::Url::parse(&format!("{}/movie/u/p/7.mkv", server.url())).unwrap();
        let (stop, stopped) = oneshot::channel();
        let http = http();
        let fetching = fetch(&http, url, &part, stopped, |_| {});
        let stopping = async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let _ = stop.send(());
        };
        let (exit, ()) = tokio::join!(fetching, stopping);
        assert_eq!(exit, Exit::Stopped);
        let have = std::fs::read(&part).unwrap();
        assert!(!have.is_empty() && have.len() < 50_000, "{}", have.len());
        assert_eq!(have, &body()[..have.len()]);
    }
}
