//! Local app settings: the "Local to Matty" column of PLAN §7.
//!
//! Stored as JSON in the OS config directory. Writes are atomic (temporary
//! file, then rename) and run on the background executor. Keys this version
//! does not know are kept, so an older Matty never drops a newer one's
//! settings. Server-synced preferences live in the store, not here.

use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, Task};
use serde::{Deserialize, Serialize};

/// Sidebar width limits from PLAN §4.1.
pub const SIDEBAR_MIN: f32 = 200.;
pub const SIDEBAR_MAX: f32 = 400.;
pub const SIDEBAR_DEFAULT: f32 = 240.;
/// Right pane width limits from PLAN §4.1.
pub const RIGHT_PANE_MIN: f32 = 320.;
pub const RIGHT_PANE_MAX: f32 = 640.;
pub const RIGHT_PANE_DEFAULT: f32 = 400.;
/// Zoom limits and step.
pub const ZOOM_MIN: f32 = 0.5;
pub const ZOOM_MAX: f32 = 2.;
pub const ZOOM_STEP: f32 = 0.1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteImages {
    /// Load remote Markdown images directly, as the webapp does.
    #[default]
    Always,
    /// Show a click-to-load placeholder.
    Ask,
}

/// How post times read: "8:02 PM" or "20:02".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockFormat {
    TwelveHour,
    #[default]
    TwentyFourHour,
}

impl ClockFormat {
    /// Formats minutes since local midnight.
    pub fn format(self, minutes: u16) -> String {
        let (hour, minute) = ((minutes / 60) % 24, minutes % 60);
        match self {
            Self::TwentyFourHour => format!("{hour:02}:{minute:02}"),
            Self::TwelveHour => {
                let suffix = if hour < 12 { "AM" } else { "PM" };
                let hour = match hour % 12 {
                    0 => 12,
                    hour => hour,
                };
                format!("{hour}:{minute:02} {suffix}")
            }
        }
    }

    /// The format the user chose, for views that print a time.
    pub fn current(cx: &App) -> Self {
        SettingsStore::global(cx).read(cx).get().clock
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

/// The window's last position and size, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub window: Option<WindowGeometry>,
    pub sidebar_width: f32,
    pub right_pane_width: f32,
    pub theme: ThemeChoice,
    pub text_scale: f32,
    /// Window zoom from Cmd+= / Cmd+- / Cmd+0, scaling text and spacing.
    pub zoom: f32,
    pub launch_at_login: bool,
    pub close_to_tray: bool,
    pub cache_size_mb: u32,
    pub download_folder: Option<PathBuf>,
    pub update_channel: UpdateChannel,
    pub log_level: LogLevel,
    pub load_remote_images: RemoteImages,
    pub clock: ClockFormat,
    /// Keys written by another Matty version, kept as they were.
    #[serde(flatten)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            window: None,
            sidebar_width: SIDEBAR_DEFAULT,
            right_pane_width: RIGHT_PANE_DEFAULT,
            theme: ThemeChoice::System,
            text_scale: 1.,
            zoom: 1.,
            launch_at_login: false,
            close_to_tray: false,
            cache_size_mb: 500,
            download_folder: None,
            update_channel: UpdateChannel::Stable,
            log_level: LogLevel::Info,
            load_remote_images: RemoteImages::Always,
            clock: ClockFormat::TwentyFourHour,
            unknown: serde_json::Map::new(),
        }
    }
}

impl Settings {
    /// Clamps values a hand-edited file could put out of range.
    fn sanitize(mut self) -> Self {
        let clamp = |value: f32, min: f32, max: f32, default: f32| {
            if value.is_finite() {
                value.clamp(min, max)
            } else {
                default
            }
        };
        self.sidebar_width = clamp(
            self.sidebar_width,
            SIDEBAR_MIN,
            SIDEBAR_MAX,
            SIDEBAR_DEFAULT,
        );
        self.right_pane_width = clamp(
            self.right_pane_width,
            RIGHT_PANE_MIN,
            RIGHT_PANE_MAX,
            RIGHT_PANE_DEFAULT,
        );
        self.text_scale = clamp(self.text_scale, 0.8, 1.5, 1.);
        self.zoom = clamp(self.zoom, ZOOM_MIN, ZOOM_MAX, 1.);
        self
    }
}

/// What [`load`] found on disk.
#[derive(Debug, PartialEq)]
pub enum Loaded {
    /// No file yet: defaults.
    Missing,
    /// The file parsed.
    Read,
    /// The file did not parse; it was moved aside to this path and defaults are used.
    Corrupt(PathBuf),
}

/// The default settings file: `<config dir>/Matty/settings.json`.
pub fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("Matty").join("settings.json"))
}

/// Reads settings from `path`. A file that does not parse is renamed to
/// `settings.json.corrupt-<unix seconds>` so the user's data is not lost.
pub fn load(path: &Path) -> (Settings, Loaded) {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return (Settings::default(), Loaded::Missing);
        }
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "could not read settings");
            return (Settings::default(), Loaded::Missing);
        }
    };
    match serde_json::from_slice::<Settings>(&bytes) {
        Ok(settings) => (settings.sanitize(), Loaded::Read),
        Err(error) => {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs());
            let mut aside = path.as_os_str().to_owned();
            aside.push(format!(".corrupt-{stamp}"));
            let aside = PathBuf::from(aside);
            tracing::warn!(
                path = %path.display(),
                kept_as = %aside.display(),
                %error,
                "settings file is not valid; using defaults"
            );
            if let Err(error) = std::fs::rename(path, &aside) {
                tracing::warn!(%error, "could not move the corrupt settings aside");
            }
            (Settings::default(), Loaded::Corrupt(aside))
        }
    }
}

/// Writes settings atomically: a temporary file in the same directory, then a rename.
pub fn save(path: &Path, settings: &Settings) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "settings path has no parent")
    })?;
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(settings).map_err(io::Error::other)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, json)?;
    std::fs::rename(&temporary, path)
}

/// Emitted once per change, after the in-memory settings are updated.
#[derive(Clone, Debug)]
pub struct SettingsChanged;

/// The app's settings, shared through a GPUI global.
pub struct SettingsStore {
    settings: Settings,
    path: Option<PathBuf>,
    /// The pending write. Replacing it cancels the previous one, so a window
    /// drag produces one write when it settles, not one per frame.
    save: Option<Task<()>>,
}

/// How long settings must stay unchanged before they are written.
const SAVE_DELAY: Duration = Duration::from_millis(400);

impl EventEmitter<SettingsChanged> for SettingsStore {}

struct GlobalSettings(Entity<SettingsStore>);
impl Global for GlobalSettings {}

impl SettingsStore {
    /// Loads settings from `path` (or keeps them in memory only when `None`)
    /// and installs the store as a global.
    pub fn init(path: Option<PathBuf>, cx: &mut App) -> Entity<Self> {
        let settings = path
            .as_deref()
            .map_or_else(Settings::default, |path| load(path).0);
        let store = cx.new(|_| Self {
            settings,
            path,
            save: None,
        });
        cx.set_global(GlobalSettings(store.clone()));
        store
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalSettings>().0.clone()
    }

    pub fn get(&self) -> &Settings {
        &self.settings
    }

    /// Applies `change`; if anything changed, emits one [`SettingsChanged`] and
    /// writes the file in the background.
    pub fn update(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Settings)) {
        let mut next = self.settings.clone();
        change(&mut next);
        let next = next.sanitize();
        if next == self.settings {
            return;
        }
        self.settings = next;
        cx.emit(SettingsChanged);
        cx.notify();
        if let Some(path) = self.path.clone() {
            let settings = self.settings.clone();
            self.save = Some(cx.spawn(async move |_, cx| {
                cx.background_executor().timer(SAVE_DELAY).await;
                let written = cx
                    .background_spawn(async move { save(&path, &settings).map_err(|e| (path, e)) })
                    .await;
                if let Err((path, error)) = written {
                    tracing::warn!(path = %path.display(), %error, "could not save settings");
                }
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_keeps_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"sidebar_width": 320, "theme": "dark", "from_the_future": {"x": 1}}"#,
        )
        .unwrap();
        let (mut settings, loaded) = load(&path);
        assert_eq!(loaded, Loaded::Read);
        assert_eq!(settings.sidebar_width, 320.);
        assert_eq!(settings.theme, ThemeChoice::Dark);
        assert_eq!(settings.right_pane_width, RIGHT_PANE_DEFAULT);

        settings.right_pane_width = 500.;
        save(&path, &settings).unwrap();
        let (again, _) = load(&path);
        assert_eq!(again, settings);
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["from_the_future"]["x"], 1);
    }

    #[test]
    fn clock_formats_both_ways() {
        assert_eq!(ClockFormat::TwentyFourHour.format(8 * 60 + 2), "08:02");
        assert_eq!(ClockFormat::TwentyFourHour.format(0), "00:00");
        assert_eq!(ClockFormat::TwelveHour.format(8 * 60 + 2), "8:02 AM");
        assert_eq!(ClockFormat::TwelveHour.format(0), "12:00 AM");
        assert_eq!(ClockFormat::TwelveHour.format(12 * 60), "12:00 PM");
        assert_eq!(ClockFormat::TwelveHour.format(20 * 60 + 2), "8:02 PM");
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let (settings, loaded) = load(&dir.path().join("settings.json"));
        assert_eq!(loaded, Loaded::Missing);
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn corrupt_file_is_moved_aside_and_defaults_are_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let (settings, loaded) = load(&path);
        assert_eq!(settings, Settings::default());
        let Loaded::Corrupt(aside) = loaded else {
            panic!("expected the corrupt file to be moved aside");
        };
        assert!(!path.exists());
        assert_eq!(std::fs::read(aside).unwrap(), b"{ not json");
    }

    #[test]
    fn out_of_range_widths_are_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"sidebar_width": 9000, "right_pane_width": 10}"#).unwrap();
        let (settings, _) = load(&path);
        assert_eq!(settings.sidebar_width, SIDEBAR_MAX);
        assert_eq!(settings.right_pane_width, RIGHT_PANE_MIN);
    }

    #[gpui_kit::test]
    fn one_change_emits_one_event(cx: &mut gpui_kit::TestAppContext) {
        let store = cx.update(|cx| SettingsStore::init(None, cx));
        let events = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = events.clone();
        cx.update(|cx| {
            cx.subscribe(&store, move |_, _: &SettingsChanged, _| {
                seen.set(seen.get() + 1)
            })
            .detach();
        });
        store.update(cx, |store, cx| store.update(cx, |s| s.sidebar_width = 300.));
        store.update(cx, |store, cx| store.update(cx, |s| s.sidebar_width = 300.));
        cx.run_until_parked();
        assert_eq!(events.get(), 1);
        store.read_with(cx, |store, _| assert_eq!(store.get().sidebar_width, 300.));
    }
}
