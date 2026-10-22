mod config;
#[cfg(feature = "devtools")]
mod devtools;
mod download;
mod epg_sources;
#[cfg(test)]
mod fake_xtream;
mod import;
mod info;
mod logos;
mod m3u;
mod player;
#[cfg(feature = "devtools")]
mod probe;
mod recorder;
mod refresh;
mod search;
mod store;
mod tables;
mod trash;
mod ts;
mod ui;
mod xtream;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: vidicon [--credentials <path>]";
#[cfg(feature = "devtools")]
const DEV_USAGE: &str = " [--screenshot <path>] [--keys <sequence>] [--play] [--probe-archive <stream_id>] [--probe-tables <id,...>] [--probe-vod] [--probe-alternates] [--probe-pause <stream_id>] [--probe-blackout <stream_id>] [--probe-epg <url-or-path>] [--probe-m3u <url-or-path>]";
#[cfg(not(feature = "devtools"))]
const DEV_USAGE: &str = "";

fn main() -> ExitCode {
    init_logging();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("vidicon: {}", xtream::redact(&e));
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    #[cfg(feature = "devtools")]
    let started = std::time::Instant::now();
    let mut credentials = None;
    #[cfg(feature = "devtools")]
    let (
        mut screenshot,
        mut keys,
        mut play,
        mut probe,
        mut tables,
        mut vod,
        mut alternates,
        mut pause,
        mut blackout,
        mut epg,
        mut m3u,
    ) = (
        None, None, false, None, None, false, false, None, None, None, None,
    );
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .map(PathBuf::from)
                .ok_or_else(|| format!("{arg:?} needs a value\n{USAGE}{DEV_USAGE}"))
        };
        match arg.to_str() {
            Some("--credentials") => credentials = Some(value()?),
            #[cfg(feature = "devtools")]
            Some("--screenshot") => screenshot = Some(value()?),
            #[cfg(feature = "devtools")]
            Some("--keys") => keys = Some(value()?.to_string_lossy().into_owned()),
            #[cfg(feature = "devtools")]
            Some("--play") => play = true,
            #[cfg(feature = "devtools")]
            Some("--probe-archive") => {
                let id = value()?;
                probe = Some(
                    id.to_str()
                        .and_then(|s| s.parse::<i64>().ok())
                        .ok_or_else(|| format!("--probe-archive needs a stream id, not {id:?}"))?,
                );
            }
            #[cfg(feature = "devtools")]
            Some("--probe-tables") => {
                let ids = value()?;
                tables = Some(
                    ids.to_str()
                        .and_then(|s| s.split(',').map(|i| i.parse::<i64>().ok()).collect())
                        .ok_or_else(|| format!("--probe-tables needs stream ids, not {ids:?}"))?,
                );
            }
            #[cfg(feature = "devtools")]
            Some("--probe-vod") => vod = true,
            #[cfg(feature = "devtools")]
            Some("--probe-alternates") => alternates = true,
            #[cfg(feature = "devtools")]
            Some("--probe-pause") => {
                let id = value()?;
                pause = Some(
                    id.to_str()
                        .and_then(|s| s.parse::<i64>().ok())
                        .ok_or_else(|| format!("--probe-pause needs a stream id, not {id:?}"))?,
                );
            }
            #[cfg(feature = "devtools")]
            Some("--probe-blackout") => {
                let id = value()?;
                blackout = Some(
                    id.to_str()
                        .and_then(|s| s.parse::<i64>().ok())
                        .ok_or_else(|| format!("--probe-blackout needs a stream id, not {id:?}"))?,
                );
            }
            #[cfg(feature = "devtools")]
            Some("--probe-epg") => {
                let source = value()?;
                epg = Some(
                    source
                        .to_str()
                        .ok_or_else(|| format!("--probe-epg needs a URL or path, not {source:?}"))?
                        .to_owned(),
                );
            }
            #[cfg(feature = "devtools")]
            Some("--probe-m3u") => {
                let source = value()?;
                m3u = Some(
                    source
                        .to_str()
                        .ok_or_else(|| format!("--probe-m3u needs a URL or path, not {source:?}"))?
                        .to_owned(),
                );
            }
            Some("-h" | "--help") => {
                println!("{USAGE}{DEV_USAGE}");
                return Ok(());
            }
            _ => return Err(format!("unknown argument {arg:?}\n{USAGE}{DEV_USAGE}")),
        }
    }

    let opts = ui::Options {
        #[cfg(feature = "devtools")]
        started,
        #[cfg(feature = "devtools")]
        screenshot,
        #[cfg(feature = "devtools")]
        keys,
        #[cfg(feature = "devtools")]
        play,
    };
    let settings = config::load_settings(&config::settings_path())?;
    let path = config::credentials_path(credentials);
    let credentials = config::load_credentials(&path);
    #[cfg(feature = "devtools")]
    let first = || match &credentials {
        Ok(providers) => Ok(providers[0].clone()),
        Err(setup) => Err(setup.message(&path)),
    };

    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    #[cfg(feature = "devtools")]
    if let Some(id) = probe {
        let provider = first()?;
        return rt.block_on(probe::run(provider, &settings.mpv_path, id));
    }
    #[cfg(feature = "devtools")]
    if let Some(ids) = tables {
        let provider = first()?;
        return rt.block_on(probe::tables(provider, ids));
    }
    #[cfg(feature = "devtools")]
    if vod {
        let provider = first()?;
        return rt.block_on(probe::vod(provider, &settings.mpv_path));
    }
    #[cfg(feature = "devtools")]
    if let Some(id) = pause {
        let provider = first()?;
        return rt.block_on(probe::pause(provider, &settings.mpv_path, id));
    }
    #[cfg(feature = "devtools")]
    if let Some(id) = blackout {
        let provider = first()?;
        return rt.block_on(probe::blackout(provider, &settings.mpv_path, id));
    }
    #[cfg(feature = "devtools")]
    if let Some(source) = epg {
        let provider = first()?;
        return rt.block_on(probe::epg(provider, source));
    }
    #[cfg(feature = "devtools")]
    if let Some(source) = m3u {
        return rt.block_on(probe::m3u(source));
    }
    #[cfg(feature = "devtools")]
    if alternates {
        let provider = first()?;
        return probe::alternates(&provider.id);
    }
    let result = ui::run(rt.handle(), credentials, path, &settings, opts);
    // Dropping the runtime drops the player task, which kills mpv.
    rt.shutdown_timeout(std::time::Duration::from_secs(1));
    result
}

/// stderr only (SPEC 3). `RUST_LOG` works as usual, but HTTP crates stay at
/// `warn` because their debug output can include request URLs.
fn init_logging() {
    let filters = std::env::var("RUST_LOG").unwrap_or_else(|_| "vidicon=info".into());
    env_logger::Builder::new()
        .parse_filters(&filters)
        .filter_module("reqwest", log::LevelFilter::Warn)
        .filter_module("hyper", log::LevelFilter::Warn)
        .filter_module("hyper_util", log::LevelFilter::Warn)
        .init();
}
