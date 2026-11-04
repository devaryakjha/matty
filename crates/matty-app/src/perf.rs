//! The perf log (PLAN §9, #174): frame times, channel switch to first paint,
//! keypress to glyph in the composer and frames drawn while idle, written as
//! JSON lines for `scripts/perf-run` to summarise.
//!
//! It is off unless `MATTY_PERF_LOG` names a file. Off, every probe is one
//! check of a `OnceLock` and the window root is the main window itself; on,
//! the root is [`PerfRoot`], which times each draw and notes the last input.
//!
//! Every line carries `t`, milliseconds since the log opened; the first line
//! (`"ev":"start"`) also carries `epoch_ms`, so a driver can line its own
//! clock up with the app's. The events:
//!
//! - `frame`: one per drawn frame. `dur` is the CPU time from the root's
//!   render to the end of its paint (presenting is not included);
//!   `input` is the most recent input (`key`, `pointer`, `scroll` or `none`)
//!   and `since_input` how long before the frame it came; `idle` is true when
//!   that is over [`IDLE_AFTER_MS`], and `idle_frames` counts those so far.
//! - `first_paint`: the end of the first frame, from the log opening.
//! - `switch`: a channel shown in place of another, from the input that
//!   asked for it (or the view taking the channel, without one) to the end of
//!   the first frame that draws its messages. `cached` is true when the view
//!   got the messages with the channel (the preview) or the channel was shown
//!   before in this run (sync has its posts in the store); otherwise they
//!   came from the server. A channel with no messages logs no switch.
//! - `key_to_glyph`: from a key press that changed the composer's text to the
//!   end of the frame that draws the change.

use std::collections::HashSet;
use std::fs::File;
use std::io::{LineWriter, Write};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use gpui_kit::{
    AnyView, App, Context, DispatchPhase, IntoElement, MouseDownEvent, MouseMoveEvent,
    ParentElement as _, Render, ScrollEvent, Styled as _, Subscription, Window, canvas, div,
};

/// The variable that turns the log on: the path of the file to write.
pub const ENV: &str = "MATTY_PERF_LOG";

/// A frame this long after the last input counts as idle: nothing the user
/// did asked for it.
pub const IDLE_AFTER_MS: f64 = 1000.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Input {
    Key,
    Pointer,
    Scroll,
}

impl Input {
    fn name(self) -> &'static str {
        match self {
            Input::Key => "key",
            Input::Pointer => "pointer",
            Input::Scroll => "scroll",
        }
    }
}

struct Switch {
    channel: String,
    started: Instant,
    cached: bool,
    /// The messages are in the view; the next frame ends the switch.
    ready: bool,
}

struct State<W: Write> {
    out: W,
    opened: Instant,
    frame_start: Option<Instant>,
    frames: u64,
    idle_frames: u64,
    last_input: Option<(Input, Instant)>,
    /// A key press not yet matched to a composer change.
    key: Option<Instant>,
    /// A key press whose composer change waits for its frame.
    glyph: Option<Instant>,
    channel: Option<String>,
    /// Channels shown so far, whose posts sync has stored.
    seen: HashSet<String>,
    switch: Option<Switch>,
}

fn ms(from: Instant, to: Instant) -> f64 {
    to.saturating_duration_since(from).as_secs_f64() * 1000.
}

impl<W: Write> State<W> {
    fn new(out: W, opened: Instant) -> Self {
        Self {
            out,
            opened,
            frame_start: None,
            frames: 0,
            idle_frames: 0,
            last_input: None,
            key: None,
            glyph: None,
            channel: None,
            seen: HashSet::new(),
            switch: None,
        }
    }

    fn write(&mut self, line: serde_json::Value) {
        // A failed write loses a sample, never the app.
        let _ = writeln!(self.out, "{line}");
    }

    fn input(&mut self, kind: Input, now: Instant) {
        self.last_input = Some((kind, now));
        if kind == Input::Key {
            self.key = Some(now);
        }
    }

    fn composer_changed(&mut self) {
        if let Some(key) = self.key.take() {
            self.glyph = Some(key);
        }
    }

    fn channel_shown(&mut self, channel: Option<&str>, loading: bool, posts: usize, now: Instant) {
        let Some(channel) = channel else {
            self.channel = None;
            self.switch = None;
            return;
        };
        if self.channel.as_deref() == Some(channel) {
            if let Some(switch) = self.switch.as_mut()
                && switch.channel == channel
                && !loading
                && posts > 0
            {
                switch.ready = true;
            }
            return;
        }
        let first = self.channel.is_none();
        let seen = !self.seen.insert(channel.to_owned());
        self.channel = Some(channel.to_owned());
        if first {
            // The channel the window opens on is start-up, not a switch.
            return;
        }
        // The switch began with the click or key that asked for it, when
        // that came just now; otherwise (sync moved the view) here.
        let started = self
            .last_input
            .map(|(_, at)| at)
            .filter(|at| ms(*at, now) < 250.)
            .unwrap_or(now);
        self.switch = Some(Switch {
            channel: channel.to_owned(),
            started,
            cached: seen || !loading,
            ready: !loading,
        });
    }

    fn frame_end(&mut self, now: Instant) {
        let Some(start) = self.frame_start.take() else {
            return;
        };
        self.frames += 1;
        let t = ms(self.opened, now);
        let (input, since_input) = match self.last_input {
            Some((kind, at)) => (kind.name(), ms(at, now)),
            None => ("none", t),
        };
        let idle = since_input > IDLE_AFTER_MS;
        if idle {
            self.idle_frames += 1;
        }
        let line = serde_json::json!({
            "ev": "frame",
            "t": t,
            "dur": ms(start, now),
            "input": input,
            "since_input": since_input,
            "idle": idle,
            "idle_frames": self.idle_frames,
        });
        self.write(line);
        if self.frames == 1 {
            self.write(serde_json::json!({ "ev": "first_paint", "t": t, "ms": t }));
        }
        if let Some(key) = self.glyph.take() {
            let ms = ms(key, now);
            self.write(serde_json::json!({ "ev": "key_to_glyph", "t": t, "ms": ms }));
        }
        if self.switch.as_ref().is_some_and(|switch| switch.ready)
            && let Some(switch) = self.switch.take()
        {
            self.write(serde_json::json!({
                "ev": "switch",
                "t": t,
                "channel": switch.channel,
                "cached": switch.cached,
                "ms": ms(switch.started, now),
            }));
        }
    }
}

static STATE: OnceLock<Option<Mutex<State<LineWriter<File>>>>> = OnceLock::new();

fn with(f: impl FnOnce(&mut State<LineWriter<File>>)) {
    let state = STATE.get_or_init(|| {
        let path = std::env::var_os(ENV).filter(|path| !path.is_empty())?;
        let file = File::create(&path)
            .inspect_err(|error| tracing::warn!(%error, "could not open the perf log"))
            .ok()?;
        let mut state = State::new(LineWriter::new(file), Instant::now());
        let epoch_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(0));
        state.write(serde_json::json!({
            "ev": "start",
            "t": 0.0,
            "epoch_ms": epoch_ms,
            "version": env!("CARGO_PKG_VERSION"),
            "debug": cfg!(debug_assertions),
        }));
        Some(Mutex::new(state))
    });
    if let Some(state) = state
        && let Ok(mut state) = state.lock()
    {
        f(&mut state);
    }
}

/// Whether `MATTY_PERF_LOG` is set and its file opened.
pub fn enabled() -> bool {
    let mut on = false;
    with(|_| on = true);
    on
}

fn input(kind: Input) {
    with(|state| state.input(kind, Instant::now()));
}

/// The composer's text changed. If a key press did it, the next frame ends
/// a `key_to_glyph` sample.
pub fn composer_changed() {
    with(State::composer_changed);
}

/// The channel view took `channel`: still `loading`, or loaded with `posts`
/// messages. A different channel than before starts a switch, complete at
/// once when it came loaded (the preview). The same channel again, loaded
/// with messages, completes it: live, the view first gets a loading channel,
/// then sync's snapshot of the store, which is empty for a channel never
/// fetched, then the server's posts, appended to it.
pub fn channel_shown(channel: Option<&str>, loading: bool, posts: usize) {
    with(|state| state.channel_shown(channel, loading, posts, Instant::now()));
}

/// The window root while the log is on: the main window, timed. Its render
/// starts each frame and a last, empty child's paint ends it; a first,
/// empty child and a keystroke interceptor note input before any view sees
/// it.
pub struct PerfRoot {
    main: AnyView,
    _keys: Subscription,
}

impl PerfRoot {
    pub fn new(main: impl Into<AnyView>, cx: &mut App) -> Self {
        // Before any binding or view sees the key, so keys bound to actions
        // (Alt-Down to the next channel) count too.
        let keys = cx.intercept_keystrokes(|_, _, _| input(Input::Key));
        Self {
            main: main.into(),
            _keys: keys,
        }
    }
}

impl Render for PerfRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        with(|state| state.frame_start = Some(Instant::now()));
        // Capture listeners run in paint order and any of them may stop
        // the event, so these go in first, before the main window's.
        let inputs = canvas(
            |_, _, _| {},
            |_, _, window, _| {
                window.on_mouse_event(|_: &ScrollEvent, phase, _, _| {
                    if phase == DispatchPhase::Capture {
                        input(Input::Scroll);
                    }
                });
                window.on_mouse_event(|_: &MouseDownEvent, phase, _, _| {
                    if phase == DispatchPhase::Capture {
                        input(Input::Pointer);
                    }
                });
                window.on_mouse_event(|_: &MouseMoveEvent, phase, _, _| {
                    if phase == DispatchPhase::Capture {
                        input(Input::Pointer);
                    }
                });
            },
        );
        let end = canvas(
            |_, _, _| {},
            |_, _, _, _| with(|state| state.frame_end(Instant::now())),
        );
        div()
            .size_full()
            .child(inputs.absolute().size_0())
            .child(self.main.clone())
            .child(end.absolute().size_0())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn lines(state: &State<Vec<u8>>) -> Vec<serde_json::Value> {
        String::from_utf8_lossy(&state.out)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn events<'a>(lines: &'a [serde_json::Value], ev: &str) -> Vec<&'a serde_json::Value> {
        lines.iter().filter(|line| line["ev"] == ev).collect()
    }

    fn frame(state: &mut State<Vec<u8>>, at: Instant) {
        state.frame_start = Some(at);
        state.frame_end(at + Duration::from_millis(4));
    }

    #[test]
    fn a_cached_switch_ends_at_the_next_frame_and_counts_from_the_key() {
        let t0 = Instant::now();
        let mut state = State::new(Vec::new(), t0);
        state.channel_shown(Some("a"), false, 3, t0);
        let key = t0 + Duration::from_millis(100);
        state.input(Input::Key, key);
        state.channel_shown(Some("b"), false, 3, key + Duration::from_millis(2));
        frame(&mut state, key + Duration::from_millis(10));
        let lines = lines(&state);
        let switches = events(&lines, "switch");
        assert_eq!(switches.len(), 1);
        assert_eq!(switches[0]["channel"], "b");
        assert_eq!(switches[0]["cached"], true);
        assert_eq!(switches[0]["ms"].as_f64().unwrap().round(), 14.);
    }

    #[test]
    fn an_uncached_switch_waits_for_posts_and_a_revisit_is_cached() {
        let t0 = Instant::now();
        let mut state = State::new(Vec::new(), t0);
        state.channel_shown(Some("a"), true, 0, t0);
        state.channel_shown(Some("b"), true, 0, t0);
        // Sync's empty snapshot of a channel it never fetched.
        state.channel_shown(Some("b"), false, 0, t0);
        frame(&mut state, t0 + Duration::from_millis(5));
        assert!(events(&lines(&state), "switch").is_empty());
        state.channel_shown(Some("b"), false, 60, t0 + Duration::from_millis(80));
        frame(&mut state, t0 + Duration::from_millis(90));
        state.channel_shown(Some("a"), true, 0, t0 + Duration::from_millis(200));
        state.channel_shown(Some("a"), false, 60, t0 + Duration::from_millis(210));
        frame(&mut state, t0 + Duration::from_millis(220));
        let lines = lines(&state);
        let switches = events(&lines, "switch");
        assert_eq!(switches.len(), 2);
        assert_eq!(switches[0]["cached"], false);
        assert_eq!(switches[0]["ms"].as_f64().unwrap().round(), 94.);
        assert_eq!(switches[1]["channel"], "a");
        assert_eq!(switches[1]["cached"], true);
    }

    #[test]
    fn only_keys_that_change_the_composer_give_a_glyph_sample() {
        let t0 = Instant::now();
        let mut state = State::new(Vec::new(), t0);
        state.input(Input::Key, t0);
        frame(&mut state, t0 + Duration::from_millis(5));
        state.input(Input::Key, t0 + Duration::from_millis(20));
        state.composer_changed();
        frame(&mut state, t0 + Duration::from_millis(30));
        let lines = lines(&state);
        let glyphs = events(&lines, "key_to_glyph");
        assert_eq!(glyphs.len(), 1);
        assert_eq!(glyphs[0]["ms"].as_f64().unwrap().round(), 14.);
    }

    #[test]
    fn frames_long_after_input_count_as_idle() {
        let t0 = Instant::now();
        let mut state = State::new(Vec::new(), t0);
        state.input(Input::Scroll, t0);
        frame(&mut state, t0 + Duration::from_millis(10));
        frame(&mut state, t0 + Duration::from_millis(1500));
        let lines = lines(&state);
        let frames = events(&lines, "frame");
        assert_eq!(frames[0]["input"], "scroll");
        assert_eq!(frames[0]["idle"], false);
        assert_eq!(frames[1]["idle"], true);
        assert_eq!(frames[1]["idle_frames"], 1);
        assert_eq!(events(&lines, "first_paint").len(), 1);
    }
}
