//! Dev tooling (`devtools` feature only): window snapshots, key replay and
//! the frame probe. Snapshots capture the window's own contents, never the
//! desktop or mpv.

use crate::config;
use crate::ui::AppWindow;
use slint::ComponentHandle;
use slint::platform::Key;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SETTLE: Duration = Duration::from_millis(500);

/// Gap between replayed keys, so each is handled in its own event-loop turn
/// with layout and scrolling settled in between, as with real typing.
const KEY_GAP: Duration = Duration::from_millis(30);

/// `VIDICON_FPS=1` (SPEC 10): frame stats for each second of scrolling.
#[derive(Default)]
struct FpsProbe {
    last: Option<Instant>,
    before: Option<Instant>,
    window_start: Option<Instant>,
    intervals: Vec<f32>,
    render_ms: Vec<f32>,
}

impl FpsProbe {
    fn frame(&mut self, at: Instant) {
        if let Some(b) = self.before.take() {
            self.render_ms
                .push(at.duration_since(b).as_secs_f32() * 1000.0);
        }
        if let Some(last) = self.last {
            self.intervals
                .push(at.duration_since(last).as_secs_f32() * 1000.0);
        }
        self.last = Some(at);
        let secs = at
            .duration_since(*self.window_start.get_or_insert(at))
            .as_secs_f32();
        if secs < 1.0 || self.intervals.is_empty() {
            return;
        }
        let mut v = std::mem::take(&mut self.intervals);
        let mut r = std::mem::take(&mut self.render_ms);
        v.sort_by(f32::total_cmp);
        r.sort_by(f32::total_cmp);
        eprintln!(
            "fps {:6.1} | interval p50 {:5.2} ms  p99 {:5.2} ms  max {:5.2} ms | render p50 {:5.2} ms  max {:5.2} ms",
            v.len() as f32 / secs,
            v[v.len() / 2],
            v[(v.len() * 99 / 100).min(v.len() - 1)],
            v[v.len() - 1],
            r.get(r.len() / 2).copied().unwrap_or(0.0),
            r.last().copied().unwrap_or(0.0),
        );
        self.window_start = Some(at);
    }
}

/// After the first frame, feeds `keys` to `press` one per [`KEY_GAP`]; then,
/// with `--screenshot`, saves the window once it settles and exits. With
/// `step` (`VIDICON_FPS=1`), calls it once per frame from then on, to scroll
/// as fast as frames are drawn, and prints frame stats every second.
pub fn install(
    window: &AppWindow,
    started: Instant,
    screenshot: Option<PathBuf>,
    keys: Vec<(String, bool)>,
    press: impl Fn(&str, bool) + 'static,
    step: Option<Rc<dyn Fn()>>,
) {
    let weak = window.as_weak();
    let mut first = Some((screenshot, keys, press));
    let mut probe = FpsProbe::default();
    let scrolling = Rc::new(Cell::new(false));
    let res = window.window().set_rendering_notifier(move |state, _| {
        match state {
            slint::RenderingState::BeforeRendering if scrolling.get() => {
                probe.before = Some(Instant::now());
                return;
            }
            slint::RenderingState::AfterRendering => {}
            _ => return,
        }
        if let (true, Some(step)) = (scrolling.get(), &step) {
            probe.frame(Instant::now());
            let step = step.clone();
            slint::Timer::single_shot(Duration::ZERO, move || step());
        }
        let Some((screenshot, keys, press)) = first.take() else {
            return;
        };
        log::debug!(
            "first frame {} ms after launch",
            started.elapsed().as_millis()
        );
        let weak = weak.clone();
        let (scrolling, step) = (scrolling.clone(), step.clone());
        replay(keys.into_iter(), std::rc::Rc::new(press), move || {
            if let Some(step) = step {
                scrolling.set(true);
                step();
            }
            let Some(path) = screenshot else { return };
            slint::Timer::single_shot(SETTLE, move || {
                if let Err(e) = save(weak.unwrap().window(), &path) {
                    eprintln!("vidicon: screenshot failed: {e}");
                    std::process::exit(1);
                }
                eprintln!("screenshot saved to {}", path.display());
                let _ = slint::quit_event_loop();
            });
        });
    });
    if let Err(e) = res {
        eprintln!("vidicon: --screenshot and --keys unsupported by this renderer: {e:?}");
        std::process::exit(1);
    }
}

fn replay<P: Fn(&str, bool) + 'static>(
    mut keys: impl Iterator<Item = (String, bool)> + 'static,
    press: std::rc::Rc<P>,
    done: impl FnOnce() + 'static,
) {
    slint::Timer::single_shot(KEY_GAP, move || match keys.next() {
        Some((text, ctrl)) => {
            press(&text, ctrl);
            replay(keys, press, done);
        }
        None => done(),
    });
}

/// F12: saves to `$XDG_CACHE_HOME/vidicon/snapshots/<unix millis>.png`.
pub fn snapshot(window: &AppWindow) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = config::snapshots_dir().join(format!("{millis}.png"));
    match save(window.window(), &path) {
        Ok(()) => log::info!("snapshot saved to {}", path.display()),
        Err(e) => log::error!("snapshot failed: {e}"),
    }
}

/// `--keys` syntax: each character is a key and whitespace separates nothing;
/// named keys go in angle brackets (`<Enter>`, `<Down>`, `<C-d>`, `<C-Enter>`,
/// `<Space>`), `<lt>` and `<gt>` for the brackets themselves.
/// Returns Slint key text plus whether Ctrl is held.
pub fn parse_keys(seq: &str) -> Result<Vec<(String, bool)>, String> {
    let mut out = Vec::new();
    let mut chars = seq.chars();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if c != '<' {
            out.push((c.to_string(), false));
            continue;
        }
        let name: String = chars.by_ref().take_while(|&c| c != '>').collect();
        let (named, ctrl) = match name.strip_prefix("C-") {
            Some(rest) => (rest, true),
            None => (name.as_str(), false),
        };
        let mut l = named.chars();
        if let (true, Some(c), None) = (ctrl, l.next(), l.next()) {
            out.push((c.to_string(), true));
            continue;
        }
        if let (false, "lt" | "gt") = (ctrl, named) {
            out.push((if named == "lt" { "<" } else { ">" }.to_string(), false));
            continue;
        }
        let key = match named {
            "Enter" => Key::Return,
            "Esc" => Key::Escape,
            "Space" => Key::Space,
            "Tab" => Key::Tab,
            "Backspace" => Key::Backspace,
            "Up" => Key::UpArrow,
            "Down" => Key::DownArrow,
            "Left" => Key::LeftArrow,
            "Right" => Key::RightArrow,
            "PageUp" => Key::PageUp,
            "PageDown" => Key::PageDown,
            "Home" => Key::Home,
            "End" => Key::End,
            _ => {
                return Err(format!(
                    "--keys: unknown key <{name}>; use Enter, Esc, Space, Tab, Backspace, Up, Down, Left, Right, PageUp, PageDown, Home, End, lt, gt, C-<letter> or C-<name>"
                ));
            }
        };
        out.push((char::from(key).to_string(), ctrl));
    }
    Ok(out)
}

fn save(window: &slint::Window, path: &Path) -> Result<(), String> {
    let buf = window.take_snapshot().map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), buf.width(), buf.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut w| w.write_image_data(buf.as_bytes()))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(key: Key) -> (String, bool) {
        (char::from(key).to_string(), false)
    }

    #[test]
    fn plain_characters_map_to_themselves() {
        let keys = parse_keys("l j j /kxn").unwrap();
        let texts: Vec<&str> = keys.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(texts, ["l", "j", "j", "/", "k", "x", "n"]);
        assert!(keys.iter().all(|(_, ctrl)| !ctrl));
    }

    #[test]
    fn named_keys() {
        assert_eq!(
            parse_keys("<Down><Enter> <Esc>G<C-d><C-Enter>").unwrap(),
            [
                k(Key::DownArrow),
                k(Key::Return),
                k(Key::Escape),
                ("G".into(), false),
                ("d".into(), true),
                (char::from(Key::Return).to_string(), true)
            ]
        );
        let texts: Vec<String> = parse_keys("<lt>P<gt>")
            .unwrap()
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(texts, ["<", "P", ">"]);
    }

    #[test]
    fn unknown_names_are_errors() {
        assert!(parse_keys("<Nope>").is_err());
        assert!(parse_keys("<C-dd>").is_err());
    }
}
