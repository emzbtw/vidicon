//! Paths, settings and credential loading (SPEC 3, 4; ADR 0004).

use serde::Deserialize;
use std::fmt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    /// None is the setup screen's [`Setup::Empty`].
    #[serde(default)]
    pub provider: Vec<Provider>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub id: String,
    pub name: String,
    /// An Xtream provider's origin, or an M3U playlist's URL or absolute
    /// path, which can hold a token: never logged.
    pub url: String,
    /// Required for Xtream only, empty on an M3U provider.
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// The provider's guide sources in order (EPG sources ticket 02).
    #[serde(default)]
    pub epg: Vec<EpgSource>,
    /// `type` in the file (M3U ticket 02), Xtream when absent.
    #[serde(default, rename = "type")]
    pub source: Source,
    /// An M3U provider's `User-Agent` for the playlist, `vidicon/<version>`
    /// when absent.
    pub user_agent: Option<String>,
}

/// Where a provider's channels and streams come from (M3U ticket 02):
/// it decides the import and the stream URLs.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Xtream,
    M3u,
}

/// What a provider offers beyond live channels (M3U ticket 04). The UI
/// hides what it lacks rather than showing it disabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// Catch-up: past programmes from the archive, and the guide tables.
    pub archive: bool,
    /// Movies and series.
    pub vod: bool,
    /// The account check: the dashboard and the expiry.
    pub account: bool,
}

impl Source {
    pub fn caps(self) -> Capabilities {
        let xtream = self == Source::Xtream;
        Capabilities {
            archive: xtream,
            vod: xtream,
            account: xtream,
        }
    }
}

impl Provider {
    /// An M3U provider's playlist file, when its `url` is a path.
    pub fn playlist_path(&self) -> Option<&Path> {
        Some(Path::new(&self.url)).filter(|p| p.is_absolute())
    }

    /// What logs and messages call an M3U provider's playlist, holding no
    /// token: the URL's host or the path's file name.
    pub fn playlist_label(&self) -> String {
        source_label(&self.url)
    }
}

/// A `[[provider.epg]]` entry: an external XMLTV feed, by URL or local
/// path, or with `provider_guide` where the provider's own `xmltv.php`
/// sits in the order (first when no entry says). URLs often carry a token,
/// so they are as secret as the password.
#[derive(Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EpgSource {
    pub url: Option<String>,
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub provider_guide: bool,
    /// What the detail panel and the log call it; the host or the file
    /// name when absent.
    pub name: Option<String>,
    /// Added to every programme's times at import.
    #[serde(default)]
    pub shift_minutes: i64,
    #[serde(default = "enabled")]
    pub enabled: bool,
    /// A URL's least gap between scheduled fetches, in hours, 12 when
    /// absent (EPG sources ticket 03).
    pub every_hours: Option<u32>,
    /// A URL's most requests in a rolling 24 h, all kinds counted.
    pub max_per_day: Option<u32>,
}

fn enabled() -> bool {
    true
}

impl EpgSource {
    /// The provider's own guide's entry, as when no entry places it.
    pub fn provider_guide() -> Self {
        EpgSource {
            url: None,
            path: None,
            provider_guide: true,
            name: None,
            shift_minutes: 0,
            enabled: true,
            every_hours: None,
            max_per_day: None,
        }
    }

    /// The least gap between scheduled fetches, in seconds.
    pub fn every(&self) -> i64 {
        i64::from(self.every_hours.unwrap_or(12)) * 3600
    }

    /// What keeps a source's data across edits: the URL without its query
    /// string, so a renewed token keeps it, or the path; empty for the
    /// provider's guide.
    pub fn ident(&self) -> String {
        match (&self.url, &self.path) {
            (Some(url), _) => match reqwest::Url::parse(url) {
                Ok(mut u) => {
                    u.set_query(None);
                    u.set_fragment(None);
                    u.to_string()
                }
                Err(_) => url.split(['?', '#']).next().unwrap_or_default().to_owned(),
            },
            (None, Some(path)) => path.display().to_string(),
            (None, None) => String::new(),
        }
    }

    /// A name for logs and messages that holds no token: its `name`, else
    /// the URL's host or the path's file name.
    pub fn label(&self) -> String {
        if self.provider_guide {
            return "xmltv.php".into();
        }
        match self.name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => source_label(&self.ident()),
        }
    }
}

/// A source's label from its [`EpgSource::ident`] alone: the URL's host,
/// or the path's file name.
pub fn source_label(ident: &str) -> String {
    if let Ok(url) = reqwest::Url::parse(ident)
        && let Some(host) = url.host_str()
    {
        return host.to_owned();
    }
    Path::new(ident)
        .file_name()
        .map_or_else(|| "EPG source".into(), |f| f.to_string_lossy().into_owned())
}

/// A provider's guide sources in their order: its `[[provider.epg]]`
/// entries, the provider's own guide first when none places it.
pub fn epg_order(provider: &Provider) -> Vec<EpgSource> {
    let mut sources = provider.epg.clone();
    if !sources.iter().any(|s| s.provider_guide) {
        sources.insert(0, EpgSource::provider_guide());
    }
    sources
}

/// What the UI calls each of [`epg_order`]'s sources: the provider's guide,
/// and an unnamed feed on the provider's own host, by the provider's name;
/// another by its [`EpgSource::label`]. The provider's guide keeps its
/// name; a name met again gets ` 2`, ` 3` and on in order, so no two read
/// the same.
pub fn epg_labels(provider: &Provider) -> Vec<String> {
    let host = |url: &str| reqwest::Url::parse(url).ok()?.host_str().map(str::to_owned);
    let own = host(&provider.url);
    let order = epg_order(provider);
    let mut taken = vec![provider.name.clone()];
    order
        .iter()
        .map(|s| {
            if s.provider_guide {
                return provider.name.clone();
            }
            let unnamed = s.name.as_deref().is_none_or(|n| n.trim().is_empty());
            let base = if unnamed && own.is_some() && s.url.as_deref().and_then(host) == own {
                provider.name.clone()
            } else {
                s.label()
            };
            let label = std::iter::once(base.clone())
                .chain((2..).map(|n| format!("{base} {n}")))
                .find(|l| !taken.contains(l))
                .unwrap_or_default();
            taken.push(label.clone());
            label
        })
        .collect()
}

// The URL can hold a token.
impl fmt::Debug for EpgSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EpgSource")
            .field("label", &self.label())
            .field("shift_minutes", &self.shift_minutes)
            .field("enabled", &self.enabled)
            .field("every_hours", &self.every_hours)
            .field("max_per_day", &self.max_per_day)
            .finish_non_exhaustive()
    }
}

// Keeps credentials out of any `{:?}` that reaches a log line.
impl fmt::Debug for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Provider")
            .field("id", &self.id)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// `config.toml` (SPEC 4.2). A missing file means defaults.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub active_provider: Option<String>,
    /// Validated here; refresh scheduling (ticket 08) uses it.
    pub refresh_hours: u32,
    pub mpv_path: String,
    pub extra_mpv_args: Vec<String>,
    /// Where `R` records to; created when first needed.
    pub recordings_dir: PathBuf,
    /// Where `d` downloads movies and episodes to (VOD ticket 10).
    pub downloads_dir: PathBuf,
    /// Records scheduled programmes (recording ticket 02).
    pub ffmpeg_path: String,
    /// Minutes a scheduled recording starts before the guide's start.
    pub record_before: u32,
    /// Minutes it runs past the guide's stop.
    pub record_after: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            active_provider: None,
            refresh_hours: 12,
            mpv_path: "mpv".into(),
            extra_mpv_args: Vec::new(),
            recordings_dir: default_videos_dir().join("Recordings"),
            downloads_dir: default_videos_dir().join("Downloads"),
            ffmpeg_path: "ffmpeg".into(),
            record_before: 2,
            record_after: 5,
        }
    }
}

pub fn settings_path() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("vidicon/config.toml")
}

/// The keys `config.toml` may hold (SPEC 4.2), and the only ones the app
/// writes.
pub const SETTING_KEYS: [&str; 9] = [
    "active_provider",
    "refresh_hours",
    "mpv_path",
    "extra_mpv_args",
    "recordings_dir",
    "downloads_dir",
    "ffmpeg_path",
    "record_before",
    "record_after",
];

pub fn load_settings(path: &Path) -> Result<Settings, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(e) => return Err(format!("Cannot read {}: {e}", path.display())),
    };
    parse_settings(path, &text)
}

fn parse_settings(path: &Path, text: &str) -> Result<Settings, String> {
    let settings: Settings =
        toml::from_str(text).map_err(|e| format!("{} is not valid: {e}", path.display()))?;
    if !matches!(settings.refresh_hours, 6 | 12) {
        return Err(format!(
            "{}: refresh_hours must be 12 or 6, not {}",
            path.display(),
            settings.refresh_hours
        ));
    }
    Ok(settings)
}

/// Why the app cannot save to `path`, if it cannot (settings ticket 01): a
/// symlink into the Nix store, or a file or directory read-only to the
/// user. Its values still show; nothing saves.
pub fn managed(path: &Path) -> Option<String> {
    managed_in(path, Path::new("/nix/store"))
}

fn managed_in(path: &Path, nix_store: &Path) -> Option<String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let target = std::fs::canonicalize(path).ok();
    if target.as_ref().is_some_and(|t| t.starts_with(nix_store)) {
        return Some(format!("{name} is managed by Nix; edit it there"));
    }
    let writable = |p: &Path| access(p, libc::W_OK);
    // With no file yet, the directory decides; with none either, saving
    // creates it.
    let file = target.unwrap_or_else(|| path.to_owned());
    let read_only = (file.exists() && !writable(&file))
        || file.parent().is_some_and(|d| d.exists() && !writable(d));
    read_only.then(|| format!("{name} is read-only to you, so nothing here saves"))
}

/// Whether the user may use `path` as `mode` asks (`access(2)`).
fn access(path: &Path, mode: libc::c_int) -> bool {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a valid NUL-terminated path.
    unsafe { libc::access(c.as_ptr(), mode) == 0 }
}

/// Why a value typed into the Settings view is refused (settings ticket
/// 02). `Missing` is a directory that does not exist, which the view offers
/// to create.
#[derive(Debug, PartialEq)]
pub enum Refused {
    Reason(String),
    Missing(PathBuf),
}

/// `recordings_dir` or `downloads_dir` as typed: an absolute path to a
/// directory the user can write to.
pub fn check_dir(text: &str) -> Result<PathBuf, Refused> {
    let path = PathBuf::from(text.trim());
    if !path.is_absolute() {
        return Err(Refused::Reason("Must be an absolute path".into()));
    }
    let shown = path.display();
    match std::fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Refused::Missing(path)),
        Err(e) => Err(Refused::Reason(format!("{shown}: {e}"))),
        Ok(m) if !m.is_dir() => Err(Refused::Reason(format!("{shown}: not a folder"))),
        Ok(_) if !access(&path, libc::W_OK | libc::X_OK) => {
            Err(Refused::Reason(format!("{shown}: not writable by you")))
        }
        Ok(_) => Ok(path),
    }
}

/// `mpv_path` or `ffmpeg_path` as typed: a name found on `PATH`, or an
/// absolute path, to an executable file.
pub fn check_program(text: &str) -> Result<String, String> {
    check_program_in(text, &std::env::var_os("PATH").unwrap_or_default())
}

fn check_program_in(text: &str, path: &std::ffi::OsStr) -> Result<String, String> {
    let text = text.trim();
    let runnable = |p: &Path| p.is_file() && access(p, libc::X_OK);
    if text.is_empty() {
        return Err("Must name a program".into());
    }
    if !text.contains('/') {
        return std::env::split_paths(path)
            .any(|dir| runnable(&dir.join(text)))
            .then(|| text.to_owned())
            .ok_or_else(|| format!("{text}: not found on PATH"));
    }
    let path = Path::new(text);
    if !path.is_absolute() {
        Err("Must be an absolute path, or a name on PATH".into())
    } else if !path.exists() {
        Err(format!("{text}: not found"))
    } else if !runnable(path) {
        Err(format!("{text}: not executable"))
    } else {
        Ok(text.to_owned())
    }
}

/// `extra_mpv_args` as typed: one line, split as a shell would.
pub fn split_args(text: &str) -> Result<Vec<String>, String> {
    shlex::split(text).ok_or_else(|| "extra args: unmatched quote".into())
}

/// Checks `text`, typed for `key` in the Settings view, and saves it as
/// [`save_setting`] does. A refused value is not written.
pub fn save_typed(path: &Path, key: &str, text: &str) -> Result<Settings, Refused> {
    let value: toml_edit::Value = match key {
        "mpv_path" | "ffmpeg_path" => check_program(text).map_err(Refused::Reason)?.into(),
        "extra_mpv_args" => split_args(text)
            .map_err(Refused::Reason)?
            .into_iter()
            .collect::<toml_edit::Array>()
            .into(),
        "recordings_dir" | "downloads_dir" => check_dir(text)?.display().to_string().into(),
        _ => return Err(Refused::Reason(format!("{key} is not typed"))),
    };
    save_setting(path, key, Some(value)).map_err(Refused::Reason)
}

/// `args` as one line that [`split_args`] gives back.
pub fn join_args(args: &[String]) -> String {
    shlex::try_join(args.iter().map(String::as_str)).unwrap_or_default()
}

/// Sets `key` in `config.toml` to `value`, or with `None` removes it,
/// keeping the rest of the file as it was: comments, order and layout. The
/// file must still load; it is written to a temporary file beside it, then
/// renamed over it with its mode kept, so a crash never leaves half a
/// file. A symlink stays one: the file it points at is replaced. Returns
/// the settings as saved.
pub fn save_setting(
    path: &Path,
    key: &str,
    value: Option<toml_edit::Value>,
) -> Result<Settings, String> {
    if !SETTING_KEYS.contains(&key) {
        return Err(format!("{key} is not a setting"));
    }
    if let Some(why) = managed(path) {
        return Err(why);
    }
    let file = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("Cannot read {}: {e}", path.display())),
    };
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("{} is not valid: {e}", path.display()))?;
    match value {
        Some(value) => match doc.get_mut(key).and_then(toml_edit::Item::as_value_mut) {
            // The old value's spacing and trailing comment stay.
            Some(old) => {
                let decor = old.decor().clone();
                *old = value;
                *old.decor_mut() = decor;
            }
            None => doc[key] = toml_edit::value(value),
        },
        None => remove_key(&mut doc, key),
    }
    let text = doc.to_string();
    let settings = parse_settings(path, &text)?;
    replace_file(&file, &text, None).map_err(|e| format!("Cannot save {}: {e}", path.display()))?;
    Ok(settings)
}

/// Removes `key`, the comments above it moving to the key after it (or
/// the file's end), so a header comment stays.
fn remove_key(doc: &mut toml_edit::DocumentMut, key: &str) {
    let table = doc.as_table_mut();
    let Some(pos) = table.iter().position(|(k, _)| k == key) else {
        return;
    };
    let Some((removed, _)) = table.remove_entry(key) else {
        return;
    };
    let raw = |p: Option<&toml_edit::RawString>| {
        p.and_then(|p| p.as_str()).unwrap_or_default().to_owned()
    };
    let prefix = raw(removed.leaf_decor().prefix());
    if prefix.trim().is_empty() {
        return;
    }
    if let Some((mut next, _)) = table.iter_mut().nth(pos) {
        let after = raw(next.leaf_decor().prefix());
        next.leaf_decor_mut().set_prefix(prefix + &after);
    } else {
        let after = raw(Some(doc.trailing()));
        doc.set_trailing(prefix + &after);
    }
}

/// Writes `text` to `path` through a temporary file beside it, with
/// `mode` if given, else the old file's mode. With `mode`, the temporary
/// file has it from its creation, so the text is never readable by others.
fn replace_file(path: &Path, text: &str, mode: Option<u32>) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mode = match mode {
        Some(m) => Some(std::fs::Permissions::from_mode(m)),
        None => std::fs::metadata(path).ok().map(|m| m.permissions()),
    };
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let written = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode.as_ref().map_or(0o666, |m| m.mode()))
            .open(&tmp)?;
        f.write_all(text.as_bytes())?;
        if let Some(mode) = mode {
            f.set_permissions(mode)?;
        }
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Index of the provider to start with: `active_provider` if set (a pin),
/// else `last_used` (the last one switched to) if it still exists, else the
/// first.
pub fn start_provider(
    providers: &[Provider],
    settings: &Settings,
    last_used: Option<&str>,
) -> Result<usize, String> {
    let find = |id: &str| providers.iter().position(|p| p.id == id);
    match &settings.active_provider {
        Some(id) => find(id).ok_or_else(|| {
            let ids: Vec<&str> = providers.iter().map(|p| p.id.as_str()).collect();
            format!(
                "active_provider = \"{id}\" in {} matches no provider; the credentials file has: {}",
                settings_path().display(),
                ids.join(", ")
            )
        }),
        None => Ok(last_used.and_then(find).unwrap_or(0)),
    }
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback),
    }
}

pub fn default_credentials_path() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("vidicon/credentials.toml")
}

pub fn data_home() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share")
}

pub fn database_path() -> PathBuf {
    data_home().join("vidicon/vidicon.db")
}

/// `$XDG_VIDEOS_DIR/Vidicon`, else `~/Videos/Vidicon`: recordings and
/// downloads each have a folder in it.
fn default_videos_dir() -> PathBuf {
    xdg("XDG_VIDEOS_DIR", "Videos").join("Vidicon")
}

pub fn mpv_socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    runtime.join("vidicon/mpv.sock")
}

pub fn logos_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("vidicon/logos")
}

pub fn posters_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("vidicon/posters")
}

#[cfg(feature = "devtools")]
pub fn snapshots_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("vidicon/snapshots")
}

/// `--credentials` wins over `VIDICON_CREDENTIALS`, which wins over the default.
pub fn credentials_path(flag: Option<PathBuf>) -> PathBuf {
    flag.or_else(|| std::env::var_os("VIDICON_CREDENTIALS").map(PathBuf::from))
        .unwrap_or_else(default_credentials_path)
}

const FORMAT_HINT: &str = "\
Expected format (SPEC 4.1):

  [[provider]]
  id = \"main\"
  name = \"My provider\"
  url = \"http://example.com:8080\"
  username = \"...\"
  password = \"...\"

or an M3U playlist:

  [[provider]]
  id = \"lists\"
  name = \"Public lists\"
  type = \"m3u\"
  url = \"https://lists.example.net/tv.m3u\"  # or an absolute path
  user_agent = \"Example/1.0\"  # optional";

const EPG_HINT: &str = "\
Guide sources (SPEC 4.1), in order, after their [[provider]]:

  [[provider.epg]]
  url = \"https://epg.example.net/guide.xml.gz\"  # or path = \"/srv/epg/guide.xml\"
  name = \"World feed\"  # optional, shown in the detail panel
  shift_minutes = 0  # optional
  enabled = true     # optional
  every_hours = 12   # optional, a url's, at least 1
  max_per_day = 20   # optional, a url's, at least 3 (2 are kept for r)

  [[provider.epg]]
  provider_guide = true  # where the provider's own guide goes; first if absent";

/// A provider's `[[provider.epg]]` entries: each one URL (http or https),
/// absolute path or `provider_guide`, the last at most once, no source
/// twice. Says which entry, never quoting a URL.
fn check_epg(sources: &[EpgSource]) -> Result<(), String> {
    for (i, s) in sources.iter().enumerate() {
        let n = i + 1;
        match (&s.url, &s.path, s.provider_guide) {
            (Some(url), None, false) => {
                let ok = reqwest::Url::parse(url)
                    .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.has_host());
                if !ok {
                    return Err(format!(
                        "guide source {n}: url must be an http:// or https:// address"
                    ));
                }
            }
            (None, Some(path), false) if path.is_absolute() => {}
            (None, Some(_), false) => {
                return Err(format!("guide source {n}: path must be absolute"));
            }
            (None, None, true) => {
                if sources[..i].iter().any(|s| s.provider_guide) {
                    return Err(format!("guide source {n}: provider_guide is given twice"));
                }
            }
            _ => {
                return Err(format!(
                    "guide source {n}: give one of url, path or provider_guide = true"
                ));
            }
        }
        let limits = s.every_hours.is_some() || s.max_per_day.is_some();
        if limits && s.url.is_none() {
            return Err(format!(
                "guide source {n}: every_hours and max_per_day are for a url only"
            ));
        }
        if s.every_hours == Some(0) {
            return Err(format!("guide source {n}: every_hours must be at least 1"));
        }
        if s.max_per_day.is_some_and(|m| m < 3) {
            return Err(format!(
                "guide source {n}: max_per_day must be at least 3, as 2 are kept for r"
            ));
        }
        if !s.provider_guide && sources[..i].iter().any(|t| t.ident() == s.ident()) {
            return Err(format!("guide source {n}: the same source is given twice"));
        }
    }
    Ok(())
}

/// A provider's keys for its `type` (M3U ticket 02): an Xtream one needs
/// a username and password and takes no user agent; an M3U one's `url` is
/// an `http(s)` URL or an absolute path. Never quotes a value.
fn check_source_type(p: &Provider) -> Result<(), String> {
    match p.source {
        Source::Xtream if p.username.is_empty() || p.password.is_empty() => {
            Err("username and password are required for an Xtream provider".into())
        }
        Source::Xtream if p.user_agent.is_some() => {
            Err("user_agent is for an M3U provider (type = \"m3u\") only".into())
        }
        Source::Xtream => Ok(()),
        Source::M3u => {
            let url = reqwest::Url::parse(&p.url)
                .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.has_host());
            if url || Path::new(&p.url).is_absolute() {
                Ok(())
            } else {
                Err("url must be an http:// or https:// address or an absolute path".into())
            }
        }
    }
}

/// Why `credentials.toml` gives no provider to start with: the setup
/// screen shows it (onboarding ticket 03).
#[derive(Clone, Debug, PartialEq)]
pub enum Setup {
    Missing,
    /// It loads but names no provider.
    Empty,
    /// The file or its directory is open to others, saying which and the
    /// `chmod`; `fix` when [`fix_permissions`] may change them.
    Loose {
        why: String,
        fix: bool,
    },
    /// Unreadable or not valid, saying where without quoting a value.
    Invalid(String),
}

impl Setup {
    /// What to print when there is no window to show it in.
    pub fn message(&self, path: &Path) -> String {
        match self {
            Setup::Missing => {
                let dir = path.parent().unwrap_or(Path::new("."));
                format!(
                    "No credentials file at {}.\n\nCreate it with mode 0600 in a 0700 directory:\n\n  mkdir -p {d} && chmod 700 {d}\n  $EDITOR {f} && chmod 600 {f}\n\n{FORMAT_HINT}",
                    path.display(),
                    d = dir.display(),
                    f = path.display(),
                )
            }
            Setup::Empty => format!(
                "{} has no [[provider]] entries.\n\n{FORMAT_HINT}",
                path.display()
            ),
            Setup::Loose { why, .. } | Setup::Invalid(why) => why.clone(),
        }
    }
}

/// The providers in `credentials.toml`, checked in this order: the file
/// is there, its modes are strict (nothing is read until they are), it
/// parses, it names a provider.
pub fn load_credentials(path: &Path) -> Result<Vec<Provider>, Setup> {
    if !path.exists() {
        return Err(Setup::Missing);
    }
    if let Err(why) = check_permissions(path) {
        // SAFETY: `geteuid` takes nothing and cannot fail.
        let uid = unsafe { libc::geteuid() };
        let fix = fixable_in(path, Path::new("/nix/store"), uid);
        return Err(Setup::Loose { why, fix });
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| Setup::Invalid(format!("Cannot read {}: {e}", path.display())))?;
    let providers = parse_credentials(path, &text)
        .map_err(Setup::Invalid)?
        .provider;
    if providers.is_empty() {
        return Err(Setup::Empty);
    }
    Ok(providers)
}

/// Whether `Fix permissions` is offered: user `uid` owns the file and its
/// directory, and the file is not in the Nix store.
fn fixable_in(path: &Path, nix_store: &Path, uid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    if std::fs::canonicalize(path).is_ok_and(|t| t.starts_with(nix_store)) {
        return false;
    }
    [path, credentials_dir(path)]
        .iter()
        .all(|p| std::fs::metadata(p).is_ok_and(|m| m.uid() == uid))
}

/// `Fix permissions`: the file set to `0600` and its directory to `0700`,
/// whichever is open to others.
pub fn fix_permissions(path: &Path) -> Result<(), String> {
    for (p, strict) in [(path, 0o600), (credentials_dir(path), 0o700)] {
        let mode = std::fs::metadata(p)
            .map_err(|e| format!("Cannot stat {}: {e}", p.display()))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(strict))
                .map_err(|e| format!("Cannot change {}: {e}", p.display()))?;
        }
    }
    Ok(())
}

fn credentials_dir(file: &Path) -> &Path {
    match file.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    }
}

/// `credentials.toml`'s text, checked as at start: valid, with providers
/// whose ids are unique. Errors never quote a value.
fn parse_credentials(path: &Path, text: &str) -> Result<Credentials, String> {
    // toml's error text quotes the offending line, which could hold a password.
    let creds: Credentials = toml::from_str(text).map_err(|e| {
        let at = e
            .span()
            .map(|s| format!(" (line {})", text[..s.start].lines().count().max(1)))
            .unwrap_or_default();
        format!(
            "{} is not valid{at}: {}\n\n{FORMAT_HINT}",
            path.display(),
            e.message()
        )
    })?;
    // `id` keys every row in the database (SPEC 5), so it must be unique.
    for (i, p) in creds.provider.iter().enumerate() {
        if creds.provider[..i].iter().any(|q| q.id == p.id) {
            return Err(format!(
                "{}: two [[provider]] entries have id = \"{}\"; each id must be unique.",
                path.display(),
                p.id
            ));
        }
        check_source_type(p).map_err(|why| {
            format!(
                "{}: provider \"{}\": {why}\n\n{FORMAT_HINT}",
                path.display(),
                p.id
            )
        })?;
        check_epg(&p.epg).map_err(|why| {
            format!(
                "{}: provider \"{}\": {why}\n\n{EPG_HINT}",
                path.display(),
                p.id
            )
        })?;
    }
    Ok(creds)
}

/// A new provider's id, from its display name (onboarding ticket 01):
/// lowercase ASCII letters and digits, accents dropped, any other run of
/// characters one `-`; `provider` if that leaves nothing; `-2` and up when
/// `taken` has it. Made once and never changed, as it keys the database.
pub fn slug(name: &str, taken: &[&str]) -> String {
    use unicode_normalization::UnicodeNormalization;
    let mut slug = String::new();
    // Decomposed, `é` is `e` and a combining accent, which is dropped.
    for c in name
        .nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
    {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let base = match slug.trim_end_matches('-') {
        "" => "provider",
        s => s,
    };
    std::iter::once(base.to_owned())
        .chain((2..).map(|n| format!("{base}-{n}")))
        .find(|id| !taken.contains(&id.as_str()))
        .unwrap_or_default()
}

/// A server URL as typed, cut to its origin (SPEC 4.1): `http://` when it
/// names no scheme, and any path, query or login in it dropped, which the
/// second value says. Errors never quote it: a pasted playlist link holds
/// the password.
pub fn origin(text: &str) -> Result<(String, bool), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Must give the server's address".into());
    }
    let full = if text.contains("://") {
        text.to_owned()
    } else {
        format!("http://{text}")
    };
    let url = reqwest::Url::parse(&full).map_err(|_| "Not a web address".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Must be an http:// or https:// address".into());
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("Not a web address".into());
    }
    let dropped = url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some();
    Ok((url.origin().ascii_serialization(), dropped))
}

/// Appends `provider` to `credentials.toml` as a `[[provider]]` table
/// (onboarding ticket 01), keeping the rest of the file as it was, as
/// [`save_setting`] does. Returns the providers as saved.
pub fn add_provider(path: &Path, provider: &Provider) -> Result<Vec<Provider>, String> {
    write_credentials(path, |doc, text| {
        let mut table = toml_edit::Table::new();
        for (key, value) in provider_values(provider) {
            table[key] = toml_edit::value(value);
        }
        // At the very end, after any comments closing the file, one blank
        // line before it.
        let trailing = doc.trailing().as_str().unwrap_or_default();
        let trailing = trailing.trim_end_matches('\n');
        let prefix = match (text.trim().is_empty(), trailing.is_empty()) {
            (true, _) => String::new(),
            (false, true) => "\n".into(),
            (false, false) => format!("{trailing}\n\n"),
        };
        table.decor_mut().set_prefix(prefix);
        doc.set_trailing("");
        match doc.get_mut("provider") {
            None => {
                let mut tables = toml_edit::ArrayOfTables::new();
                tables.push(table);
                doc["provider"] = toml_edit::Item::ArrayOfTables(tables);
            }
            Some(item) => item
                .as_array_of_tables_mut()
                .ok_or_else(|| format!("{}: provider is not [[provider]] tables", path.display()))?
                .push(table),
        }
        Ok(())
    })
}

/// Sets the name, URL, username and password of the provider whose id is
/// `provider.id` (onboarding ticket 02), each value keeping its spacing and
/// trailing comment, the rest of the file as it was. Returns the providers
/// as saved.
pub fn edit_provider(path: &Path, provider: &Provider) -> Result<Vec<Provider>, String> {
    write_credentials(path, |doc, _| {
        let table = provider_tables(doc, path)?
            .iter_mut()
            .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(&provider.id))
            .ok_or_else(|| format!("{} has no provider {}", path.display(), provider.id))?;
        for (key, value) in provider_values(provider).into_iter().skip(1) {
            match table.get_mut(key).and_then(toml_edit::Item::as_value_mut) {
                Some(old) => {
                    let decor = old.decor().clone();
                    *old = value.into();
                    *old.decor_mut() = decor;
                }
                None => table[key] = toml_edit::value(value),
            }
        }
        Ok(())
    })
}

/// Removes the `[[provider]]` table whose id is `id` (onboarding ticket
/// 02), the comments above it moving to the table after it (or the file's
/// end), as [`save_setting`] does for a key. Returns the providers left.
pub fn remove_provider(path: &Path, id: &str) -> Result<Vec<Provider>, String> {
    write_credentials(path, |doc, _| {
        let tables = provider_tables(doc, path)?;
        let at = tables
            .iter()
            .position(|t| t.get("id").and_then(|v| v.as_str()) == Some(id))
            .ok_or_else(|| format!("{} has no provider {id}", path.display()))?;
        let raw = |t: &toml_edit::Table| {
            t.decor()
                .prefix()
                .and_then(|p| p.as_str())
                .unwrap_or_default()
                .to_owned()
        };
        let prefix = tables.get(at).map(raw).unwrap_or_default();
        tables.remove(at);
        if prefix.trim().is_empty() {
            return Ok(());
        }
        if let Some(next) = tables.get_mut(at) {
            let after = raw(next);
            next.decor_mut().set_prefix(prefix + &after);
        } else {
            let after = doc.trailing().as_str().unwrap_or_default().to_owned();
            doc.set_trailing(prefix + &after);
        }
        Ok(())
    })
}

/// The values of a provider's table, in the order they are written.
fn provider_values(p: &Provider) -> [(&'static str, &str); 5] {
    [
        ("id", &p.id),
        ("name", &p.name),
        ("url", &p.url),
        ("username", &p.username),
        ("password", &p.password),
    ]
}

fn provider_tables<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    path: &Path,
) -> Result<&'a mut toml_edit::ArrayOfTables, String> {
    doc.get_mut("provider")
        .and_then(toml_edit::Item::as_array_of_tables_mut)
        .ok_or_else(|| format!("{}: provider is not [[provider]] tables", path.display()))
}

/// A guide source as typed in Settings › EPG (EPG sources ticket 04): an
/// `http(s)` URL with a host, or an absolute path to a file that can be
/// read, checked without fetching. Errors never quote a URL.
pub fn check_source(text: &str) -> Result<(Option<String>, Option<PathBuf>), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Must give a URL or a path".into());
    }
    if text.starts_with('/') {
        let path = PathBuf::from(text);
        std::fs::File::open(&path).map_err(|e| format!("{text}: cannot read: {e}"))?;
        if !path.is_file() {
            return Err(format!("{text}: not a file"));
        }
        return Ok((None, Some(path)));
    }
    let ok = reqwest::Url::parse(text)
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.has_host());
    if !ok {
        return Err("Must be an http:// or https:// address, or an absolute path".into());
    }
    Ok((Some(text.to_owned()), None))
}

/// A whole number typed in Settings › EPG: `None` when blank, else at
/// least `min`. `what` names it in the refusal.
pub fn check_number(text: &str, min: i64, what: &str) -> Result<Option<i64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.strip_prefix('+').unwrap_or(text).parse::<i64>() {
        Ok(n) if n >= min => Ok(Some(n)),
        Ok(_) => Err(format!("{what} must be at least {min}")),
        Err(_) => Err(format!("{what} must be a whole number")),
    }
}

/// Appends `source` to the guide sources of provider `provider_id`
/// (EPG sources ticket 04), last in its order, the rest of the file as it
/// was. Returns the providers as saved.
pub fn add_epg_source(
    path: &Path,
    provider_id: &str,
    source: &EpgSource,
) -> Result<Vec<Provider>, String> {
    write_credentials(path, |doc, _| {
        let tables = epg_tables(doc, path, provider_id)?;
        let mut table = toml_edit::Table::new();
        table.decor_mut().set_prefix("\n");
        source_values(&mut table, source);
        tables.push(table);
        Ok(())
    })
}

/// Sets the source at `index` in provider `provider_id`'s order (as
/// [`epg_order`] gives it) to `source`: an external one's URL or path,
/// name, shift, limits and `enabled`; the provider's guide's shift and
/// `enabled`. Each value keeps its spacing and trailing comment; a value
/// back to its default is dropped unless the file gave it.
pub fn edit_epg_source(
    path: &Path,
    provider_id: &str,
    index: usize,
    source: &EpgSource,
) -> Result<Vec<Provider>, String> {
    write_credentials(path, |doc, _| {
        let tables = epg_tables(doc, path, provider_id)?;
        let at = epg_index(tables, index, source.provider_guide)?;
        source_values(tables.get_mut(at).ok_or("no such guide source")?, source);
        Ok(())
    })
}

/// Moves the source at `from` in provider `provider_id`'s order to `to`
/// (`J`/`K`), the others keeping theirs, each with its comments.
pub fn move_epg_source(
    path: &Path,
    provider_id: &str,
    from: usize,
    to: usize,
) -> Result<Vec<Provider>, String> {
    write_credentials(path, |doc, _| {
        let tables = epg_tables(doc, path, provider_id)?;
        let implicit = !tables.iter().any(is_provider_guide);
        // The provider's guide placed by no entry gets one when it moves,
        // or when another moves above it.
        let place = implicit && (from == 0 || to == 0);
        let (from, to) = if implicit && !place {
            (from - 1, to - 1)
        } else {
            (from, to)
        };
        if place {
            epg_index(tables, 0, true)?;
        }
        if from >= tables.len() || to >= tables.len() {
            return Err("no such guide source".into());
        }
        // A table is written where its position says, so the positions
        // stay in file order and the tables move through them.
        let positions: Vec<Option<isize>> = tables.iter().map(|t| t.position()).collect();
        let table = tables.remove(from);
        tables.insert(to, table);
        for (t, p) in tables.iter_mut().zip(positions) {
            t.set_position(p);
        }
        Ok(())
    })
}

/// Removes the source at `index` in provider `provider_id`'s order, the
/// comments above it moving to the entry after it. Its data goes at the
/// next refresh pass (SPEC 4.1). The provider's guide cannot be removed.
pub fn remove_epg_source(
    path: &Path,
    provider_id: &str,
    index: usize,
) -> Result<Vec<Provider>, String> {
    write_credentials(path, |doc, _| {
        let tables = epg_tables(doc, path, provider_id)?;
        let at = epg_index(tables, index, false)?;
        if tables.get(at).is_none_or(is_provider_guide) {
            return Err("The provider's guide cannot be removed, only turned off".into());
        }
        let prefix = tables.get(at).map(table_prefix).unwrap_or_default();
        tables.remove(at);
        if !prefix.trim().is_empty()
            && let Some(next) = tables.get_mut(at)
        {
            // Its comments, then any of the next entry's own.
            let after = table_prefix(next);
            let after = if after.trim().is_empty() {
                ""
            } else {
                after.trim_start_matches('\n')
            };
            next.decor_mut().set_prefix(prefix + after);
        }
        Ok(())
    })
}

fn table_prefix(t: &toml_edit::Table) -> String {
    t.decor()
        .prefix()
        .and_then(|p| p.as_str())
        .unwrap_or_default()
        .to_owned()
}

fn is_provider_guide(t: &toml_edit::Table) -> bool {
    t.get("provider_guide").and_then(|v| v.as_bool()) == Some(true)
}

/// Provider `provider_id`'s `[[provider.epg]]` tables, made if it has none.
fn epg_tables<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    path: &Path,
    provider_id: &str,
) -> Result<&'a mut toml_edit::ArrayOfTables, String> {
    let provider = provider_tables(doc, path)?
        .iter_mut()
        .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(provider_id))
        .ok_or_else(|| format!("{} has no provider {provider_id}", path.display()))?;
    if provider.get("epg").is_none() {
        provider.insert(
            "epg",
            toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()),
        );
    }
    provider
        .get_mut("epg")
        .and_then(toml_edit::Item::as_array_of_tables_mut)
        .ok_or_else(|| {
            format!(
                "{}: provider.epg is not [[provider.epg]] tables",
                path.display()
            )
        })
}

/// The table index of the source at `index` in the order. With no entry
/// placing the provider's guide, its index 0 is not in the file: `place`
/// writes an entry for it first, else the others are one further on.
fn epg_index(
    tables: &mut toml_edit::ArrayOfTables,
    index: usize,
    place: bool,
) -> Result<usize, String> {
    if tables.iter().any(is_provider_guide) {
        return Ok(index);
    }
    if place {
        if index != 0 {
            return Err("no such guide source".into());
        }
        let mut table = toml_edit::Table::new();
        table["provider_guide"] = toml_edit::value(true);
        table.decor_mut().set_prefix("\n");
        // Written just before the entry that was first.
        table.set_position(tables.get(0).and_then(toml_edit::Table::position));
        tables.insert(0, table);
        return Ok(0);
    }
    index
        .checked_sub(1)
        .ok_or_else(|| "no such guide source".into())
}

/// Writes `source`'s values into its table in the file's order of keys,
/// each kept with its decor; a default dropped unless the table had it.
fn source_values(table: &mut toml_edit::Table, source: &EpgSource) {
    use toml_edit::Value;
    let mut put = |key: &str, value: Option<Value>, default: bool| match value {
        Some(v) if !default || table.contains_key(key) => {
            match table.get_mut(key).and_then(toml_edit::Item::as_value_mut) {
                Some(old) => {
                    let decor = old.decor().clone();
                    *old = v;
                    *old.decor_mut() = decor;
                }
                None => table[key] = toml_edit::Item::Value(v),
            }
        }
        _ => {
            table.remove(key);
        }
    };
    if !source.provider_guide {
        put("url", source.url.as_deref().map(Value::from), false);
        let path = source.path.as_ref().map(|p| p.display().to_string());
        put("path", path.map(Value::from), false);
        let name = source
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty());
        put("name", name.map(Value::from), false);
    }
    put(
        "shift_minutes",
        Some(source.shift_minutes.into()),
        source.shift_minutes == 0,
    );
    put("enabled", Some(source.enabled.into()), source.enabled);
    if !source.provider_guide {
        let every = source.every_hours.map(|h| Value::from(i64::from(h)));
        put("every_hours", every, false);
        let max = source.max_per_day.map(|m| Value::from(i64::from(m)));
        put("max_per_day", max, false);
    }
}

/// Changes `credentials.toml` with `change`, given the document and its
/// text, keeping the rest of the file as it was. Refused unless the result
/// loads. The file is written `0600` and its directory made `0700`,
/// created if missing (ADR 0004). A managed file is not written. Returns
/// the providers as saved.
fn write_credentials(
    path: &Path,
    change: impl FnOnce(&mut toml_edit::DocumentMut, &str) -> Result<(), String>,
) -> Result<Vec<Provider>, String> {
    use std::os::unix::fs::DirBuilderExt;
    if let Some(why) = managed(path) {
        return Err(why);
    }
    let file = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("Cannot read {}: {e}", path.display())),
    };
    // toml_edit's error text quotes the line, as toml's does.
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|_| format!("{} is not valid TOML", path.display()))?;
    change(&mut doc, &text)?;
    let text = doc.to_string();
    let creds = parse_credentials(path, &text)?;
    let dir = credentials_dir(path);
    let saving = |e: std::io::Error| format!("Cannot save {}: {e}", path.display());
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(saving)?;
    let dir_mode = std::fs::metadata(dir).map_err(saving)?.permissions().mode();
    if dir_mode & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(dir_mode & 0o700))
            .map_err(saving)?;
    }
    replace_file(&file, &text, Some(0o600)).map_err(saving)?;
    Ok(creds.provider)
}

/// Refuse a file looser than 0600 or a directory looser than 0700, as ssh does.
pub fn check_permissions(file: &Path) -> Result<(), String> {
    let mode = |p: &Path| {
        std::fs::metadata(p)
            .map(|m| m.permissions().mode() & 0o777)
            .map_err(|e| format!("Cannot stat {}: {e}", p.display()))
    };
    let file_mode = mode(file)?;
    if file_mode & 0o077 != 0 {
        return Err(format!(
            "{} has mode {file_mode:04o}; it must not be readable by others.\nFix: chmod 600 {}",
            file.display(),
            file.display()
        ));
    }
    let dir = credentials_dir(file);
    let dir_mode = mode(dir)?;
    if dir_mode & 0o077 != 0 {
        return Err(format!(
            "{} has mode {dir_mode:04o}; the credentials directory must not be accessible by others.\nFix: chmod 700 {}",
            dir.display(),
            dir.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup(dir_mode: u32, file_mode: u32) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("vidicon");
        fs::create_dir(&dir).unwrap();
        let file = dir.join("credentials.toml");
        fs::write(&file, "").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(file_mode)).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(dir_mode)).unwrap();
        (root, file)
    }

    #[test]
    fn strict_modes_pass() {
        let (_root, file) = setup(0o700, 0o600);
        assert!(check_permissions(&file).is_ok());
        let (_root, file) = setup(0o700, 0o400);
        assert!(check_permissions(&file).is_ok());
    }

    #[test]
    fn loose_file_is_refused_with_chmod() {
        for m in [0o644, 0o640, 0o604, 0o660] {
            let (_root, file) = setup(0o700, m);
            let err = check_permissions(&file).unwrap_err();
            assert!(
                err.contains(&format!("chmod 600 {}", file.display())),
                "{err}"
            );
        }
    }

    #[test]
    fn loose_directory_is_refused_with_chmod() {
        for m in [0o755, 0o750, 0o705] {
            let (_root, file) = setup(m, 0o600);
            let err = check_permissions(&file).unwrap_err();
            assert!(
                err.contains(&format!("chmod 700 {}", file.parent().unwrap().display())),
                "{err}"
            );
        }
    }

    #[test]
    fn missing_file_names_path_and_format() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("nope/credentials.toml");
        assert_eq!(load_credentials(&file).err(), Some(Setup::Missing));
        let err = Setup::Missing.message(&file);
        assert!(err.contains(&file.display().to_string()));
        assert!(err.contains("[[provider]]"));
    }

    #[test]
    fn missing_settings_file_means_defaults() {
        let root = tempfile::tempdir().unwrap();
        let s = load_settings(&root.path().join("config.toml")).unwrap();
        assert_eq!((s.mpv_path.as_str(), s.refresh_hours), ("mpv", 12));
        assert!(s.extra_mpv_args.is_empty() && s.active_provider.is_none());
        assert!(s.recordings_dir.ends_with("Vidicon/Recordings"));
        assert!(s.downloads_dir.ends_with("Vidicon/Downloads"));
        assert_eq!(s.ffmpeg_path, "ffmpeg");
        assert_eq!((s.record_before, s.record_after), (2, 5));
    }

    #[test]
    fn settings_parse_and_reject_unknown_keys() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        fs::write(
            &path,
            "refresh_hours = 6\nextra_mpv_args = [\"--mute=yes\"]\nrecordings_dir = \"/srv/tv\"\n\
             downloads_dir = \"/srv/films\"\n\
             record_before = 0\nrecord_after = 15\n",
        )
        .unwrap();
        let s = load_settings(&path).unwrap();
        assert_eq!((s.record_before, s.record_after), (0, 15));
        assert_eq!(s.refresh_hours, 6);
        assert_eq!(s.extra_mpv_args, ["--mute=yes"]);
        assert_eq!(s.recordings_dir, Path::new("/srv/tv"));
        assert_eq!(s.downloads_dir, Path::new("/srv/films"));

        fs::write(&path, "mpv_pth = \"mpv\"\n").unwrap();
        assert!(load_settings(&path).unwrap_err().contains("mpv_pth"));
        fs::write(&path, "refresh_hours = 8\n").unwrap();
        assert!(load_settings(&path).is_err());
    }

    const COMMENTED: &str = "\
# Vidicon settings
active_provider = \"main\"

refresh_hours = 12          # 12 or 6
mpv_path = \"/opt/mpv\"      # a newer build
";

    #[test]
    fn saving_keeps_comments_and_other_keys() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        fs::write(&path, COMMENTED).unwrap();
        let s = save_setting(&path, "refresh_hours", Some(6.into())).unwrap();
        assert_eq!(s.refresh_hours, 6);
        assert_eq!(s.mpv_path, "/opt/mpv");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            COMMENTED.replace("= 12 ", "= 6 ")
        );
        // Removed, and added back at the end.
        save_setting(&path, "active_provider", None).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("active_provider"), "{text}");
        assert!(text.starts_with("# Vidicon settings\n"), "{text}");
        let s = save_setting(&path, "active_provider", Some("spare".into())).unwrap();
        assert_eq!(s.active_provider.as_deref(), Some("spare"));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("active_provider = \"spare\"\n"), "{text}");
        assert!(text.contains("# a newer build"), "{text}");
    }

    #[test]
    fn saving_creates_a_missing_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("vidicon/config.toml");
        save_setting(&path, "refresh_hours", Some(6.into())).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "refresh_hours = 6\n");
    }

    #[test]
    fn saving_keeps_the_mode_and_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("dotfiles.toml");
        fs::write(&real, COMMENTED).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
        let path = root.path().join("config.toml");
        std::os::unix::fs::symlink(&real, &path).unwrap();
        save_setting(&path, "refresh_hours", Some(6.into())).unwrap();
        assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
        let mode = fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
        assert!(fs::read_to_string(&real).unwrap().contains("= 6 "));
        // No temporary file left beside it.
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    }

    #[test]
    fn a_managed_file_is_not_written() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("store");
        fs::create_dir(&store).unwrap();
        let stored = store.join("config.toml");
        fs::write(&stored, COMMENTED).unwrap();
        let path = root.path().join("config.toml");
        std::os::unix::fs::symlink(&stored, &path).unwrap();
        assert_eq!(
            managed_in(&path, &store).as_deref(),
            Some("config.toml is managed by Nix; edit it there")
        );

        let path = root.path().join("ro.toml");
        fs::write(&path, COMMENTED).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        let err = save_setting(&path, "refresh_hours", Some(6.into())).unwrap_err();
        assert_eq!(err, "ro.toml is read-only to you, so nothing here saves");
        assert_eq!(fs::read_to_string(&path).unwrap(), COMMENTED);
        assert_eq!(managed(&root.path().join("none.toml")), None);
    }

    #[test]
    fn a_directory_is_absolute_existing_and_writable() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("tv");
        let reason = |s: &str| Err(Refused::Reason(s.into()));
        assert_eq!(check_dir("~/tv"), reason("Must be an absolute path"));
        assert_eq!(check_dir("tv"), reason("Must be an absolute path"));
        let typed = dir.display().to_string();
        assert_eq!(check_dir(&typed), Err(Refused::Missing(dir.clone())));
        fs::write(&dir, "").unwrap();
        assert_eq!(check_dir(&typed), reason(&format!("{typed}: not a folder")));
        fs::remove_file(&dir).unwrap();
        fs::create_dir(&dir).unwrap();
        assert_eq!(check_dir(&format!(" {typed} ")), Ok(dir.clone()));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        assert_eq!(
            check_dir(&typed),
            reason(&format!("{typed}: not writable by you"))
        );
    }

    #[test]
    fn a_program_is_on_path_or_absolute_and_executable() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let mpv = bin.join("mpv");
        fs::write(&mpv, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&mpv, fs::Permissions::from_mode(0o755)).unwrap();
        let notes = bin.join("notes");
        fs::write(&notes, "").unwrap();
        let path = std::ffi::OsString::from(format!("/nonexistent:{}", bin.display()));
        let check = |t: &str| check_program_in(t, &path);
        assert_eq!(check("mpv"), Ok("mpv".into()));
        assert_eq!(check("ffmpeg"), Err("ffmpeg: not found on PATH".into()));
        assert_eq!(check("notes"), Err("notes: not found on PATH".into()));
        assert_eq!(check(""), Err("Must name a program".into()));
        assert_eq!(
            check("bin/mpv"),
            Err("Must be an absolute path, or a name on PATH".into())
        );
        let typed = mpv.display().to_string();
        assert_eq!(check(&typed), Ok(typed.clone()));
        let typed = notes.display().to_string();
        assert_eq!(check(&typed), Err(format!("{typed}: not executable")));
        let typed = bin.display().to_string();
        assert_eq!(check(&typed), Err(format!("{typed}: not executable")));
        let typed = bin.join("none").display().to_string();
        assert_eq!(check(&typed), Err(format!("{typed}: not found")));
    }

    #[test]
    fn extra_args_split_as_a_shell_would() {
        assert_eq!(
            split_args(r#"--hwdec=auto  --title="Late film" '--sub-font=Inter Display'"#),
            Ok(vec![
                "--hwdec=auto".into(),
                "--title=Late film".into(),
                "--sub-font=Inter Display".into()
            ])
        );
        assert_eq!(split_args(""), Ok(vec![]));
        assert_eq!(
            split_args(r#"--title="Late film"#),
            Err("extra args: unmatched quote".into())
        );
        let args = split_args(r#"--a='b c' --d="e'f""#).unwrap();
        assert_eq!(split_args(&join_args(&args)), Ok(args));
    }

    #[test]
    fn a_typed_value_is_saved_only_once_it_passes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        fs::write(&path, COMMENTED).unwrap();
        let refused = [
            ("recordings_dir", "~/tv"),
            ("downloads_dir", "/nonexistent/films"),
            ("mpv_path", "./mpv"),
            ("ffmpeg_path", "vidicon-no-such-ffmpeg"),
            ("extra_mpv_args", "--title='x"),
        ];
        for (key, text) in refused {
            assert!(save_typed(&path, key, text).is_err(), "{key}");
            assert_eq!(fs::read_to_string(&path).unwrap(), COMMENTED);
        }
        let s = save_typed(&path, "extra_mpv_args", "--hwdec=auto --title='Late film'").unwrap();
        assert_eq!(s.extra_mpv_args, ["--hwdec=auto", "--title=Late film"]);
        let dir = root.path().display().to_string();
        let s = save_typed(&path, "downloads_dir", &dir).unwrap();
        assert_eq!(s.downloads_dir, root.path());
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.ends_with(&format!(
                "extra_mpv_args = [\"--hwdec=auto\", \"--title=Late film\"]\ndownloads_dir = \"{dir}\"\n"
            )),
            "{text}"
        );
    }

    #[test]
    fn saving_writes_only_known_keys_and_valid_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        assert!(save_setting(&path, "mpv_pth", Some("mpv".into())).is_err());
        assert!(save_setting(&path, "refresh_hours", Some(8.into())).is_err());
        assert!(!path.exists());
        // A typo made by hand since the start stays an error, and the file
        // stays as it is.
        let typo = "mpv_pth = \"mpv\"\n";
        fs::write(&path, typo).unwrap();
        let err = save_setting(&path, "refresh_hours", Some(6.into())).unwrap_err();
        assert!(err.contains("mpv_pth"), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), typo);
    }

    fn feed(url: &str) -> EpgSource {
        EpgSource {
            url: Some(url.into()),
            provider_guide: false,
            ..EpgSource::provider_guide()
        }
    }

    fn labels(providers: &[Provider], i: usize) -> Vec<String> {
        epg_order(&providers[i])
            .iter()
            .map(EpgSource::label)
            .collect()
    }

    #[test]
    fn guide_sources_are_added_edited_and_removed_in_the_file() {
        let (_root, file) = setup(0o700, 0o600);
        let text = provider_toml("main") + "\n" + &provider_toml("spare");
        fs::write(&file, &text).unwrap();
        let mut a = feed("https://epg.example.net/a.xml?token=t1");
        a.shift_minutes = -60;
        a.max_per_day = Some(20);
        add_epg_source(&file, "main", &a).unwrap();
        let local = EpgSource {
            url: None,
            path: Some("/srv/epg/b.xml".into()),
            name: Some("Local".into()),
            ..a.clone()
        };
        let local = EpgSource {
            max_per_day: None,
            shift_minutes: 0,
            ..local
        };
        let providers = add_epg_source(&file, "main", &local).unwrap();
        assert_eq!(
            labels(&providers, 0),
            ["xmltv.php", "epg.example.net", "Local"]
        );
        assert!(providers[1].epg.is_empty(), "the other provider untouched");
        let added = "\n[[provider.epg]]\nurl = \"https://epg.example.net/a.xml?token=t1\"\nshift_minutes = -60\nmax_per_day = 20\n\n[[provider.epg]]\npath = \"/srv/epg/b.xml\"\nname = \"Local\"\n";
        let want = provider_toml("main") + added + "\n" + &provider_toml("spare");
        assert_eq!(fs::read_to_string(&file).unwrap(), want);

        // Disabled, then on again, keeping the key and its comment.
        let off = EpgSource {
            enabled: false,
            ..a.clone()
        };
        edit_epg_source(&file, "main", 1, &off).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(
            text.contains("shift_minutes = -60\nmax_per_day = 20\nenabled = false\n"),
            "{text}"
        );
        fs::write(
            &file,
            text.replace("enabled = false", "enabled = false  # flaky"),
        )
        .unwrap();
        let providers = edit_epg_source(&file, "main", 1, &a).unwrap();
        assert!(providers[0].epg[0].enabled);
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("enabled = true  # flaky\n"), "{text}");
        // The provider's guide off writes an entry for it, first.
        let guide_off = EpgSource {
            enabled: false,
            ..EpgSource::provider_guide()
        };
        let providers = edit_epg_source(&file, "main", 0, &guide_off).unwrap();
        assert!(!epg_order(&providers[0])[0].enabled);
        assert!(
            fs::read_to_string(&file)
                .unwrap()
                .contains("password = \"secret-main\"\n\n[[provider.epg]]\nprovider_guide = true\nenabled = false\n\n[[provider.epg]]\nurl")
        );

        // Removed: the provider's guide refused, a source's comments kept.
        let err = remove_epg_source(&file, "main", 0).unwrap_err();
        assert!(err.contains("turned off"), "{err}");
        let text = fs::read_to_string(&file).unwrap().replace(
            "\n[[provider.epg]]\nurl",
            "\n# the world feed\n[[provider.epg]]\nurl",
        );
        fs::write(&file, &text).unwrap();
        let providers = remove_epg_source(&file, "main", 1).unwrap();
        assert_eq!(labels(&providers, 0), ["xmltv.php", "Local"]);
        let text = fs::read_to_string(&file).unwrap();
        assert!(!text.contains("token"), "{text}");
        assert!(
            text.contains("\n# the world feed\n[[provider.epg]]\npath"),
            "{text}"
        );
        assert!(text.ends_with(&provider_toml("spare")), "{text}");
    }

    #[test]
    fn guide_sources_are_reordered_in_the_file() {
        let (_root, file) = setup(0o700, 0o600);
        let sources = "\n# first\n[[provider.epg]]\nurl = \"https://a.example.net/g.xml\"\n\n[[provider.epg]]\nurl = \"https://b.example.net/g.xml\"  # second\n";
        let text = provider_toml("main") + sources + "\n" + &provider_toml("spare");
        fs::write(&file, &text).unwrap();
        // Among the external ones, the provider's guide stays unwritten.
        let providers = move_epg_source(&file, "main", 2, 1).unwrap();
        assert_eq!(
            labels(&providers, 0),
            ["xmltv.php", "b.example.net", "a.example.net"]
        );
        let swapped = "\n[[provider.epg]]\nurl = \"https://b.example.net/g.xml\"  # second\n\n# first\n[[provider.epg]]\nurl = \"https://a.example.net/g.xml\"\n";
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            provider_toml("main") + swapped + "\n" + &provider_toml("spare")
        );
        // Above the provider's guide: an entry places it.
        let providers = move_epg_source(&file, "main", 1, 0).unwrap();
        assert_eq!(
            labels(&providers, 0),
            ["b.example.net", "xmltv.php", "a.example.net"]
        );
        let providers = move_epg_source(&file, "main", 1, 2).unwrap();
        assert_eq!(
            labels(&providers, 0),
            ["b.example.net", "a.example.net", "xmltv.php"]
        );
        let text = fs::read_to_string(&file).unwrap();
        assert_eq!(text.matches("provider_guide = true").count(), 1);
        assert!(text.ends_with(&provider_toml("spare")), "{text}");
        assert!(move_epg_source(&file, "main", 2, 3).is_err());
    }

    #[test]
    fn sources_on_the_providers_host_take_its_name_and_repeats_a_number() {
        let mut p = added("main");
        p.name = "Demo TV".into();
        p.url = "http://127.0.0.1:47811".into();
        let named = EpgSource {
            name: Some("World feed".into()),
            ..feed("http://127.0.0.1:47811/epg/c.xml")
        };
        p.epg = vec![
            feed("http://127.0.0.1:47811/epg/a.xml?token=t"),
            EpgSource::provider_guide(),
            feed("http://127.0.0.1:47811/epg/b.xml.gz"),
            feed("https://epg.example.net/uk.xml"),
            feed("https://epg.example.net/us.xml"),
            named,
        ];
        assert_eq!(
            epg_labels(&p),
            [
                "Demo TV 2",
                "Demo TV",
                "Demo TV 3",
                "epg.example.net",
                "epg.example.net 2",
                "World feed"
            ]
        );
    }

    #[test]
    fn a_typed_source_is_checked_without_fetching() {
        let dir = tempfile::tempdir().unwrap();
        let feed = dir.path().join("g.xml");
        fs::write(&feed, "<tv/>").unwrap();
        let url = "https://epg.example.net/g.xml?token=tok-9x";
        assert_eq!(check_source(url), Ok((Some(url.into()), None)));
        assert_eq!(
            check_source(feed.to_str().unwrap()),
            Ok((None, Some(feed.clone())))
        );
        for (bad, why) in [
            ("", "Must give"),
            ("epg.example.net/g.xml?token=tok-9x", "http"),
            ("ftp://epg.example.net/g.xml?token=tok-9x", "http"),
            ("/nonexistent/g.xml", "cannot read"),
            (dir.path().to_str().unwrap(), "not a file"),
        ] {
            let err = check_source(bad).unwrap_err();
            assert!(err.contains(why), "{bad}: {err}");
            assert!(!err.contains("tok-9x"), "{err}");
        }
        assert_eq!(check_number(" -60 ", i64::MIN, "Shift"), Ok(Some(-60)));
        assert_eq!(check_number("", 3, "Per day"), Ok(None));
        assert!(
            check_number("2", 3, "Per day")
                .unwrap_err()
                .contains("at least 3")
        );
        assert!(
            check_number("1.5", 1, "Every")
                .unwrap_err()
                .contains("whole")
        );
    }

    fn provider_toml(id: &str) -> String {
        format!(
            "[[provider]]\nid = \"{id}\"\nname = \"P {id}\"\nurl = \"http://h\"\nusername = \"u\"\npassword = \"secret-{id}\"\n"
        )
    }

    #[test]
    fn duplicate_ids_are_refused() {
        let (_root, file) = setup(0o700, 0o600);
        let text = provider_toml("main") + &provider_toml("spare") + &provider_toml("main");
        fs::write(&file, text).unwrap();
        let Err(Setup::Invalid(err)) = load_credentials(&file) else {
            panic!("loaded");
        };
        assert!(
            err.contains("id = \"main\"") && err.contains("unique"),
            "{err}"
        );
        assert!(!err.contains("secret"), "{err}");

        fs::write(&file, provider_toml("main") + &provider_toml("spare")).unwrap();
        assert_eq!(load_credentials(&file).unwrap().len(), 2);
    }

    #[test]
    fn m3u_providers_load_and_are_checked() {
        let (_root, file) = setup(0o700, 0o600);
        let m3u = "[[provider]]\nid = \"lists\"\nname = \"Lists\"\ntype = \"m3u\"\nurl = \"https://lists.example.net/tv.m3u?token=tok-9x\"\nuser_agent = \"Example/1.0\"\n\n[[provider]]\nid = \"file\"\nname = \"File\"\ntype = \"m3u\"\nurl = \"/srv/tv/local.m3u\"\n";
        fs::write(&file, provider_toml("main") + m3u).unwrap();
        let p = load_credentials(&file).unwrap();
        let types: Vec<Source> = p.iter().map(|p| p.source).collect();
        assert_eq!(types, [Source::Xtream, Source::M3u, Source::M3u]);
        assert_eq!(p[1].user_agent.as_deref(), Some("Example/1.0"));
        assert_eq!(
            (p[1].playlist_label(), p[2].playlist_label()),
            ("lists.example.net".into(), "local.m3u".into())
        );

        for (bad, why) in [
            (
                "type = \"m3u\"\nurl = \"tv.m3u?token=tok-9x\"",
                "absolute path",
            ),
            (
                "type = \"m3u\"\nurl = \"ftp://h/tv.m3u?token=tok-9x\"",
                "absolute path",
            ),
            ("type = \"stalker\"\nurl = \"http://h\"", "line"),
        ] {
            let text = format!("[[provider]]\nid = \"x\"\nname = \"X\"\n{bad}\n");
            fs::write(&file, text).unwrap();
            let Err(Setup::Invalid(err)) = load_credentials(&file) else {
                panic!("loaded {bad}");
            };
            assert!(err.contains(why), "{bad}: {err}");
            assert!(!err.contains("tok-9x"), "{err}");
        }
        for (bad, why) in [
            ("username = \"u\"\n", "username and password"),
            ("password = \"secret\"\n", "username and password"),
            (
                "username = \"u\"\npassword = \"secret\"\nuser_agent = \"A\"\n",
                "user_agent",
            ),
        ] {
            let text = format!("[[provider]]\nid = \"x\"\nname = \"X\"\nurl = \"http://h\"\n{bad}");
            fs::write(&file, text).unwrap();
            let Err(Setup::Invalid(err)) = load_credentials(&file) else {
                panic!("loaded {bad}");
            };
            assert!(err.contains(why), "{bad}: {err}");
            assert!(!err.contains("secret"), "{err}");
        }
    }

    #[test]
    fn epg_sources_load_in_order_and_are_checked() {
        let (_root, file) = setup(0o700, 0o600);
        let sources = "[[provider.epg]]\nurl = \"https://epg.example.net/a/guide.xml.gz?token=tok-9x\"\nshift_minutes = -60\n\n[[provider.epg]]\nprovider_guide = true\nenabled = false\n\n[[provider.epg]]\npath = \"/srv/epg/local.xml\"\n\n[[provider.epg]]\nurl = \"https://sho.rt/a8f3kz\"\nname = \" World feed \"\nevery_hours = 2\nmax_per_day = 20\n";
        fs::write(&file, provider_toml("main") + sources).unwrap();
        let p = load_credentials(&file).unwrap().remove(0);
        let labels: Vec<String> = p.epg.iter().map(EpgSource::label).collect();
        assert_eq!(
            labels,
            ["epg.example.net", "xmltv.php", "local.xml", "World feed"]
        );
        assert_eq!(
            (p.epg[0].shift_minutes, p.epg[0].enabled, p.epg[1].enabled),
            (-60, true, false)
        );
        assert_eq!((p.epg[0].every(), p.epg[0].max_per_day), (12 * 3600, None));
        assert_eq!(
            (p.epg[3].every(), p.epg[3].max_per_day),
            (2 * 3600, Some(20))
        );
        // A renewed token keeps the source.
        assert_eq!(p.epg[0].ident(), "https://epg.example.net/a/guide.xml.gz");
        assert_eq!(p.epg[2].ident(), "/srv/epg/local.xml");
        let shown = format!("{p:?} {:?}", p.epg);
        assert!(!shown.contains("tok-9x"), "{shown}");

        let token = "url = \"https://epg.example.net/g.xml?token=tok-9x\"\n";
        for (bad, why) in [
            (
                "url = \"ftp://epg.example.net/g.xml?token=tok-9x\"\n",
                "http",
            ),
            ("path = \"guide.xml\"\n", "absolute"),
            ("shift_minutes = 5\n", "one of"),
            (&format!("{token}path = \"/srv/g.xml\"\n"), "one of"),
            (&format!("{token}provider_guide = true\n"), "one of"),
            (&format!("{token}every_hours = 0\n"), "at least 1"),
            (&format!("{token}max_per_day = 2\n"), "at least 3"),
            ("path = \"/srv/g.xml\"\nmax_per_day = 9\n", "url only"),
            ("provider_guide = true\nevery_hours = 2\n", "url only"),
        ] {
            fs::write(&file, provider_toml("main") + "[[provider.epg]]\n" + bad).unwrap();
            let Err(Setup::Invalid(err)) = load_credentials(&file) else {
                panic!("loaded {bad}");
            };
            assert!(err.contains(why), "{why}: {err}");
            assert!(!err.contains("tok-9x"), "{err}");
        }
        for twice in [
            "[[provider.epg]]\nprovider_guide = true\n[[provider.epg]]\nprovider_guide = true\n",
            "[[provider.epg]]\nurl = \"https://e.net/g.xml?t=1\"\n[[provider.epg]]\nurl = \"https://e.net/g.xml?t=2\"\n",
        ] {
            fs::write(&file, provider_toml("main") + twice).unwrap();
            let Err(Setup::Invalid(err)) = load_credentials(&file) else {
                panic!("loaded {twice}");
            };
            assert!(
                err.contains("guide source 2") && err.contains("twice"),
                "{err}"
            );
        }
    }

    #[test]
    fn start_provider_pin_then_last_used_then_first() {
        let (_root, file) = setup(0o700, 0o600);
        let text = provider_toml("a") + &provider_toml("b") + &provider_toml("c");
        fs::write(&file, text).unwrap();
        let providers = load_credentials(&file).unwrap();
        let mut settings = Settings::default();
        assert_eq!(start_provider(&providers, &settings, None), Ok(0));
        assert_eq!(start_provider(&providers, &settings, Some("c")), Ok(2));
        // A remembered provider since removed from the file.
        assert_eq!(start_provider(&providers, &settings, Some("gone")), Ok(0));
        settings.active_provider = Some("b".into());
        assert_eq!(start_provider(&providers, &settings, Some("c")), Ok(1));
        settings.active_provider = Some("x".into());
        let err = start_provider(&providers, &settings, None).unwrap_err();
        assert!(err.contains("a, b, c"), "{err}");
    }

    #[test]
    fn a_slug_is_made_from_the_name_and_kept_unique() {
        assert_eq!(slug("My provider", &[]), "my-provider");
        assert_eq!(slug("  Télé Nord  (HD)! ", &[]), "tele-nord-hd");
        assert_eq!(slug("Northwind_TV.2", &[]), "northwind-tv-2");
        assert_eq!(slug("★★★", &[]), "provider");
        assert_eq!(slug("", &["provider"]), "provider-2");
        let taken = ["my-provider", "my-provider-2"];
        assert_eq!(slug("My Provider", &taken), "my-provider-3");
        assert_eq!(slug("My provider 2", &taken), "my-provider-2-2");
    }

    #[test]
    fn a_url_is_cut_to_its_origin() {
        let ok = |s: &str, cut: &str, dropped: bool| {
            assert_eq!(origin(s), Ok((cut.into(), dropped)), "{s}");
        };
        ok("http://example.com:8080", "http://example.com:8080", false);
        ok(
            " http://example.com:8080/ ",
            "http://example.com:8080",
            false,
        );
        ok("example.com:8080", "http://example.com:8080", false);
        ok("HTTPS://Example.com:443", "https://example.com", false);
        ok(
            "http://example.com:8080/get.php?username=u&password=hunter2&type=m3u",
            "http://example.com:8080",
            true,
        );
        ok("http://u:hunter2@example.com/", "http://example.com", true);
        ok(
            "http://example.com/player_api.php",
            "http://example.com",
            true,
        );
        for bad in [
            "",
            "ftp://example.com",
            "http://",
            "http://exa mple.com/?password=hunter2",
        ] {
            let err = origin(bad).unwrap_err();
            assert!(!err.contains("hunter2") && !err.contains("exa"), "{err}");
        }
    }

    fn added(id: &str) -> Provider {
        Provider {
            id: id.into(),
            name: format!("P {id}"),
            url: "http://h".into(),
            username: "u".into(),
            password: format!("secret-{id}"),
            epg: Vec::new(),
            source: crate::config::Source::Xtream,
            user_agent: None,
        }
    }

    #[test]
    fn adding_keeps_comments_and_sets_the_modes() {
        let (_root, file) = setup(0o755, 0o644);
        let text = format!(
            "# Two accounts\n\n{}\n# spare below\n",
            provider_toml("main")
        );
        fs::write(&file, &text).unwrap();
        let providers = add_provider(&file, &added("spare")).unwrap();
        let ids: Vec<&str> = providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["main", "spare"]);
        let saved = fs::read_to_string(&file).unwrap();
        assert_eq!(saved, format!("{text}\n{}", provider_toml("spare")));
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(file.parent().unwrap()), 0o700);
        assert_eq!(load_credentials(&file).unwrap().len(), 2);
        // No temporary file left beside it.
        assert_eq!(fs::read_dir(file.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn adding_creates_the_file_and_its_directory() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("config/vidicon/credentials.toml");
        add_provider(&file, &added("main")).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(file.parent().unwrap()), 0o700);
        let providers = load_credentials(&file).unwrap();
        assert_eq!(providers[0].password, "secret-main");
    }

    #[test]
    fn adding_to_a_managed_or_broken_file_writes_nothing() {
        let (_root, file) = setup(0o700, 0o400);
        let err = add_provider(&file, &added("main")).unwrap_err();
        assert_eq!(
            err,
            "credentials.toml is read-only to you, so nothing here saves"
        );

        let (_root, file) = setup(0o700, 0o600);
        let broken = "[[provider]]\nid = \"main\"\npassword = \"hunter2\nname = 1\n";
        fs::write(&file, broken).unwrap();
        let err = add_provider(&file, &added("spare")).unwrap_err();
        assert!(!err.contains("hunter2"), "{err}");
        assert_eq!(fs::read_to_string(&file).unwrap(), broken);

        // A duplicate id is refused by the same check as at start.
        fs::write(&file, provider_toml("main")).unwrap();
        let err = add_provider(&file, &added("main")).unwrap_err();
        assert!(err.contains("unique") && !err.contains("secret"), "{err}");
        assert_eq!(fs::read_to_string(&file).unwrap(), provider_toml("main"));
    }

    #[test]
    fn editing_keeps_the_id_comments_and_other_providers() {
        let (_root, file) = setup(0o700, 0o644);
        let text = format!(
            "{}\n# the spare\n[[provider]]\nid = \"spare\"\nname = \"P spare\"  # shown\nurl = \"http://h\"\nusername = \"u\"\npassword = \"old\"\n",
            provider_toml("main")
        );
        fs::write(&file, &text).unwrap();
        let edited = Provider {
            name: "Spare TV".into(),
            url: "http://other:8080".into(),
            password: "new".into(),
            ..added("spare")
        };
        let providers = edit_provider(&file, &edited).unwrap();
        assert_eq!(providers[1].name, "Spare TV");
        let saved = fs::read_to_string(&file).unwrap();
        let want = text
            .replace("\"P spare\"  # shown", "\"Spare TV\"  # shown")
            .replace(
                "url = \"http://h\"\nusername = \"u\"\npassword = \"old\"",
                "url = \"http://other:8080\"\nusername = \"u\"\npassword = \"new\"",
            );
        assert_eq!(saved, want);
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file), 0o600);

        let err = edit_provider(&file, &added("gone")).unwrap_err();
        assert!(err.contains("no provider gone"), "{err}");
        assert_eq!(fs::read_to_string(&file).unwrap(), want);
    }

    #[test]
    fn removing_moves_its_comments_on_down_to_none() {
        let (_root, file) = setup(0o700, 0o600);
        let text = format!(
            "# Accounts\n{}\n{}\n{}",
            provider_toml("main"),
            provider_toml("spare"),
            provider_toml("third")
        );
        fs::write(&file, &text).unwrap();
        let providers = remove_provider(&file, "main").unwrap();
        let ids: Vec<&str> = providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["spare", "third"]);
        let saved = fs::read_to_string(&file).unwrap();
        assert_eq!(
            saved,
            format!(
                "# Accounts\n\n{}\n{}",
                provider_toml("spare"),
                provider_toml("third")
            )
        );
        remove_provider(&file, "third").unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            format!("# Accounts\n\n{}", provider_toml("spare"))
        );
        // The last one goes too, leaving the setup screen to follow.
        assert!(remove_provider(&file, "spare").unwrap().is_empty());
        assert_eq!(fs::read_to_string(&file).unwrap(), "# Accounts\n\n");
        assert_eq!(load_credentials(&file).err(), Some(Setup::Empty));
        add_provider(&file, &added("main")).unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            format!("# Accounts\n\n{}", provider_toml("main"))
        );
    }

    /// The setup screen's states, in the order they are checked: nothing
    /// is read from a file open to others.
    #[test]
    fn each_setup_state_is_chosen_in_order() {
        let (_root, file) = setup(0o700, 0o600);
        assert_eq!(load_credentials(&file).err(), Some(Setup::Empty));
        fs::write(&file, "# none yet\n").unwrap();
        assert_eq!(load_credentials(&file).err(), Some(Setup::Empty));
        fs::write(&file, "[[provider]]\nid = \"main\"\npassword = \"hunter2\n").unwrap();
        let Err(Setup::Invalid(why)) = load_credentials(&file) else {
            panic!("not invalid");
        };
        assert!(
            why.contains("(line 3)") && !why.contains("hunter2"),
            "{why}"
        );

        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        let shown = file.display();
        assert_eq!(
            load_credentials(&file).err(),
            Some(Setup::Loose {
                why: format!(
                    "{shown} has mode 0644; it must not be readable by others.\nFix: chmod 600 {shown}"
                ),
                fix: true
            })
        );
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        let dir = file.parent().unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        let Err(Setup::Loose { why, fix: true }) = load_credentials(&file) else {
            panic!("not loose");
        };
        assert!(
            why.contains(&format!("chmod 700 {}", dir.display())),
            "{why}"
        );

        fs::remove_file(&file).unwrap();
        assert_eq!(load_credentials(&file).err(), Some(Setup::Missing));
        fs::write(&file, provider_toml("main")).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(load_credentials(&file).unwrap()[0].id, "main");
    }

    #[test]
    fn fixing_permissions_sets_the_modes() {
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let (_root, file) = setup(0o755, 0o644);
        fs::write(&file, provider_toml("main")).unwrap();
        fix_permissions(&file).unwrap();
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(file.parent().unwrap()), 0o700);
        assert_eq!(load_credentials(&file).unwrap().len(), 1);
        // A strict one is left as it is.
        let (_root, file) = setup(0o750, 0o400);
        fix_permissions(&file).unwrap();
        assert_eq!(mode(&file), 0o400);
        assert_eq!(mode(file.parent().unwrap()), 0o700);
    }

    #[test]
    fn no_fix_for_a_store_path_or_another_owners_file() {
        use std::os::unix::fs::MetadataExt;
        let (root, file) = setup(0o755, 0o644);
        let me = fs::metadata(&file).unwrap().uid();
        let store = root.path().join("store");
        assert!(fixable_in(&file, &store, me));
        assert!(!fixable_in(&file, &store, me + 1));
        fs::create_dir(&store).unwrap();
        let stored = store.join("credentials.toml");
        fs::rename(&file, &stored).unwrap();
        std::os::unix::fs::symlink(&stored, &file).unwrap();
        assert!(!fixable_in(&file, &store, me));
    }

    #[test]
    fn parse_error_does_not_echo_values() {
        let (_root, file) = setup(0o700, 0o600);
        fs::write(
            &file,
            "[[provider]]\nid = \"main\"\npassword = \"hunter2\nname = 1\n",
        )
        .unwrap();
        let Err(Setup::Invalid(err)) = load_credentials(&file) else {
            panic!("loaded");
        };
        assert!(!err.contains("hunter2"), "{err}");
    }
}
