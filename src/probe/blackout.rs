//! `--probe-blackout <stream_id>` (`devtools` only, stream recovery ticket
//! 05): plays a live channel in an mpv window as the app runs it, and prints
//! the black, frozen and silent spans ffmpeg's `blackdetect`, `freezedetect`
//! and `silencedetect` find, with the video bitrate and what the filters
//! cost mpv in CPU. Runs until mpv is closed, so it can be watched for an
//! evening; a stream that ends is played again with the app's retries.
//! Prints clock times, seconds and rates only.

use super::pause::{Mpv, live_url};
use crate::config::Provider;
use crate::player::RETRIES;
use serde_json::json;
use std::time::{Duration, Instant};

/// Labelled so their metadata can be observed. `pix_th` and `n` are
/// ffmpeg's defaults; black from 1 s, to see what fades do, frozen from 5 s.
const FILTERS: [(&str, &str); 3] = [
    ("vf", "@black:lavfi=[blackdetect=d=1:pix_th=0.1]"),
    ("vf", "@freeze:lavfi=[freezedetect=n=-60dB:d=5]"),
    ("af", "@silence:lavfi=[silencedetect=n=-60dB:d=2]"),
];
/// Each span's name, the property its filter's metadata is in, and the
/// keys of its start and end there (stream seconds).
const SPANS: [(&str, &str, &str, &str); 3] = [
    (
        "black",
        "vf-metadata/black",
        "lavfi.black_start",
        "lavfi.black_end",
    ),
    (
        "frozen",
        "vf-metadata/freeze",
        "lavfi.freezedetect.freeze_start",
        "lavfi.freezedetect.freeze_end",
    ),
    (
        "silent",
        "af-metadata/silence",
        "lavfi.silence_start",
        "lavfi.silence_end",
    ),
];
/// Played without the filters, then with them, to compare mpv's CPU.
const COST: Duration = Duration::from_secs(60);
/// How often the video bitrate is read.
const SAMPLE: Duration = Duration::from_secs(5);
/// How often the bitrate and CPU so far are printed.
const SUMMARY: Duration = Duration::from_secs(600);
/// Before a span, the bitrate it is compared with.
const BEFORE: Duration = Duration::from_secs(60);
/// Upper bounds of the span lengths counted at the end, in seconds.
const BUCKETS: [f64; 4] = [2.0, 10.0, 60.0, f64::INFINITY];

#[derive(Default)]
struct Span {
    start: Option<String>,
    end: Option<String>,
    /// The running span's start: when it was seen, in stream seconds.
    open: Option<(Instant, f64)>,
    /// Finished spans, as when they were seen to start and end.
    done: Vec<(Instant, Instant)>,
}

pub async fn run(provider: Provider, mpv: &str, stream_id: i64) -> Result<(), String> {
    let url = live_url(provider, stream_id)?;
    let mut args = vec!["--idle=yes", "--force-window=yes"];
    args.extend(crate::player::RECONNECT);
    let mut mpv = Mpv::spawn(mpv, &args).await?;
    let pid = mpv.pid().ok_or("mpv has no pid")?;
    for (n, (_, prop, ..)) in (1..).zip(SPANS) {
        mpv.cmd(json!(["observe_property", n, prop])).await?;
    }
    mpv.cmd(json!(["observe_property", 9, "hwdec-current"]))
        .await?;
    // A filter that fails is only logged, as a warning.
    mpv.cmd(json!(["request_log_messages", "warn"])).await?;
    println!(
        "plays the channel until mpv is closed; {} s without the filters, then with them",
        COST.as_secs()
    );
    let began = Instant::now();
    let line = |at: Instant, what: &str| {
        let s = at.saturating_duration_since(began).as_secs();
        println!(
            "{}  +{}:{:02}:{:02}  {what}",
            clock(),
            s / 3600,
            s / 60 % 60,
            s % 60
        );
    };

    let mut spans: [Span; 3] = Default::default();
    let mut rates: Vec<(Instant, f64)> = Vec::new();
    // When the latest load was, whether it played, and the retries since
    // one last did.
    let (mut loaded, mut played, mut tries) = (Instant::now(), false, 0);
    let mut cost: Vec<(Instant, f64)> = Vec::new();
    let mut hwdec = String::from("no");
    let mut window = (Instant::now(), 0.0, 0usize);
    let mut next = Instant::now() + SAMPLE;
    mpv.cmd(json!(["loadfile", url.as_str(), "replace"]))
        .await?;
    loop {
        let wait = tokio::time::timeout_at(next.into(), mpv.events.recv()).await;
        let (at, v) = match wait {
            Ok(Some(ev)) => ev,
            Ok(None) => break,
            Err(_) => {
                next += SAMPLE;
                let now = Instant::now();
                if let Some(r) = mpv.number("video-bitrate").await {
                    rates.push((now, r));
                }
                // Without the filters for COST, then with them for COST.
                if played && cost.len() < 3 {
                    let since = cost.last().map_or(loaded, |c| c.0);
                    if cost.is_empty() || now - since >= COST {
                        cost.push((now, cpu(pid).unwrap_or(0.0)));
                        match cost[..] {
                            [a, b] => {
                                line(now, &format!("without the filters: {}", share(a, b)));
                                // Software filters cannot take frames left on
                                // the GPU, so mpv would disable them.
                                if hwdec != "no" && !hwdec.ends_with("-copy") {
                                    line(now, "hwdec auto-copy, for the filters");
                                    mpv.cmd(json!(["set_property", "hwdec", "auto-copy"]))
                                        .await?;
                                }
                                for (kind, f) in FILTERS {
                                    if let Err(e) = mpv.cmd(json!([kind, "add", f])).await {
                                        line(now, &format!("{kind} add failed: {e}"));
                                    }
                                }
                                // Measured again from here, with the filters on.
                                cost[1] = (Instant::now(), cpu(pid).unwrap_or(0.0));
                            }
                            [_, b, c] => {
                                line(now, &format!("with them: {}", share(b, c)));
                                window = (c.0, c.1, rates.len());
                            }
                            _ => {}
                        }
                    }
                }
                if cost.len() == 3 && now - window.0 >= SUMMARY {
                    let cpu_now = cpu(pid).unwrap_or(0.0);
                    let r = mean(rates[window.2..].iter().map(|r| r.1));
                    line(
                        now,
                        &format!(
                            "last {} min: video {}, {}",
                            SUMMARY.as_secs() / 60,
                            mbit(r),
                            share((window.0, window.1), (now, cpu_now))
                        ),
                    );
                    window = (now, cpu_now, rates.len());
                }
                continue;
            }
        };
        match v["event"].as_str().unwrap_or("") {
            "playback-restart" if !played => {
                played = true;
                line(
                    at,
                    &format!("playing after {:.1} s", (at - loaded).as_secs_f64()),
                );
            }
            "playback-restart" => line(at, "playback-restart"),
            "end-file" => {
                let reason = v["reason"].as_str().unwrap_or("?");
                line(
                    at,
                    &format!(
                        "end-file {reason} ({}) {} s after the load",
                        v["file_error"].as_str().unwrap_or("no error"),
                        (at - loaded).as_secs()
                    ),
                );
                if !matches!(reason, "eof" | "error") {
                    break;
                }
                tries = if played { 0 } else { tries + 1 };
                let Some(wait) = RETRIES.get(tries) else {
                    println!("the retries failed, so not again");
                    break;
                };
                tokio::time::sleep(*wait).await;
                mpv.cmd(json!(["loadfile", url.as_str(), "replace"]))
                    .await?;
                // The filters stay; the cost is measured once.
                (loaded, played) = (Instant::now(), false);
            }
            "mpv-exited" => break,
            "log-message" => {
                let text = v["text"].as_str().unwrap_or("");
                if text.starts_with("Disabling filter") {
                    line(at, text.trim_end());
                }
            }
            "property-change" if v["name"] == "hwdec-current" => {
                hwdec = v["data"].as_str().unwrap_or("no").to_owned();
                line(at, &format!("hwdec {hwdec}"));
            }
            "property-change" => {
                let Some(k) = SPANS.iter().position(|s| v["name"] == s.1) else {
                    continue;
                };
                let (name, _, start_key, end_key) = SPANS[k];
                let meta = &v["data"];
                let start = meta[start_key].as_str().map(str::to_owned);
                let end = meta[end_key].as_str().map(str::to_owned);
                if !meta.is_object() {
                    // The file ended: a running span ends with it.
                    let s = &mut spans[k];
                    if let Some((from, _)) = s.open.take() {
                        s.done.push((from, at));
                        let secs = (at - from).as_secs_f64();
                        let mut what = format!("{name} {secs:.1} s, until the stream ended");
                        if name != "silent" {
                            what += &detail(&spans[2], &rates, from, at);
                        }
                        line(at, &what);
                    }
                    let s = &mut spans[k];
                    (s.start, s.end) = (None, None);
                    continue;
                }
                if start.is_some() && start != spans[k].start {
                    let pts = start.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                    spans[k].open = Some((at, pts));
                    line(at, &format!("{name} from here"));
                }
                if end.is_some()
                    && end != spans[k].end
                    && let Some((from, pts)) = spans[k].open.take()
                {
                    let stop = end.as_deref().and_then(|s| s.parse().ok());
                    let secs = stop.map_or((at - from).as_secs_f64(), |e: f64| e - pts);
                    let from = at
                        .checked_sub(Duration::from_secs_f64(secs))
                        .unwrap_or(from);
                    spans[k].done.push((from, at));
                    let mut what = format!("{name} {secs:.1} s");
                    if name != "silent" {
                        what += &detail(&spans[2], &rates, from, at);
                    }
                    line(at, &what);
                }
                (spans[k].start, spans[k].end) = (start, end);
            }
            _ => {}
        }
    }

    let s = began.elapsed().as_secs();
    println!(
        "watched {}:{:02}:{:02}; spans by length (under 2 s, 10 s, 60 s, longer):",
        s / 3600,
        s / 60 % 60,
        s % 60
    );
    for ((name, ..), span) in SPANS.iter().zip(&spans) {
        let mut counts = [0; BUCKETS.len()];
        for (from, to) in &span.done {
            let secs = (*to - *from).as_secs_f64();
            counts[BUCKETS.iter().position(|b| secs < *b).unwrap_or(0)] += 1;
        }
        println!("  {name}: {counts:?}");
    }
    Ok(())
}

/// How much of a span was silent, and the video bitrate in it and in the
/// minute before.
fn detail(silence: &Span, rates: &[(Instant, f64)], from: Instant, to: Instant) -> String {
    let rate =
        |a: Instant, b: Instant| mean(rates.iter().filter(|r| r.0 >= a && r.0 <= b).map(|r| r.1));
    let before = from.checked_sub(BEFORE).unwrap_or(from);
    format!(
        ", silent {:.0} s of it, video {} ({} the minute before)",
        overlap(silence, from, to),
        mbit(rate(from, to)),
        mbit(rate(before, from)),
    )
}

/// Seconds of `silence` within `from..to`, a span still running included.
fn overlap(silence: &Span, from: Instant, to: Instant) -> f64 {
    let running = silence.open.map(|(s, _)| (s, to));
    silence
        .done
        .iter()
        .copied()
        .chain(running)
        .map(|(a, b)| {
            b.min(to)
                .saturating_duration_since(a.max(from))
                .as_secs_f64()
        })
        // Not `sum`, which gives -0 for none.
        .fold(0.0, |sum, s| sum + s)
}

fn mean(xs: impl Iterator<Item = f64>) -> Option<f64> {
    let (n, sum) = xs.fold((0, 0.0), |(n, s), x| (n + 1, s + x));
    (n > 0).then(|| sum / n as f64)
}

fn mbit(r: Option<f64>) -> String {
    r.map_or("-".into(), |r| format!("{:.2} Mbit/s", r / 1e6))
}

/// mpv's CPU between two readings, as a share of one core.
fn share(a: (Instant, f64), b: (Instant, f64)) -> String {
    let wall = (b.0 - a.0).as_secs_f64();
    format!("mpv {:.0} % of a core", (b.1 - a.1) / wall * 100.0)
}

/// CPU seconds the process has used, from `/proc/<pid>/stat`.
fn cpu(pid: u32) -> Option<f64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Fields 14 and 15, utime and stime, counted after the `(comm)`.
    let rest: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let ticks: f64 = rest.get(11)?.parse::<f64>().ok()? + rest.get(12)?.parse::<f64>().ok()?;
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    Some(ticks / hz as f64)
}

/// The local time of day.
fn clock() -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}
