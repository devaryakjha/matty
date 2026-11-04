//! Local app settings: the "Local to Matty" column of PLAN §7.
//!
//! Stored as JSON in the OS config directory. Writes are atomic (temporary
//! file, then rename) and run on the background executor. Keys this version
//! does not know are kept, so an older Matty never drops a newer one's
//! settings. Server-synced preferences live in the store; the few that
//! Preferences shows (message display, clock, name format, send key) are
//! mirrored here too, so they apply before sync connects (#156, #157).

use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
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

/// Whose colors Matty shows (#214): its own, or the Mattermost theme the
/// account keeps on the server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeSource {
    /// Matty's theme, light or dark as [`ThemeChoice`] says.
    #[default]
    Matty,
    /// The account's Mattermost theme for the team on screen, which is
    /// light or dark by itself.
    Mattermost,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

impl UpdateChannel {
    /// The channel a fresh install follows (#180): beta for a build with a
    /// pre-release suffix (an alpha or beta), else stable.
    pub fn for_this_build() -> Self {
        match matty_update::Channel::default_for(matty_update::CURRENT_VERSION) {
            matty_update::Channel::Stable => Self::Stable,
            matty_update::Channel::Beta => Self::Beta,
        }
    }

    /// The feed this channel reads.
    pub fn feed(self) -> matty_update::Channel {
        match self {
            Self::Stable => matty_update::Channel::Stable,
            Self::Beta => matty_update::Channel::Beta,
        }
    }
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

/// Which key sends a message from the composer; the other inserts a
/// newline. Mirrors the account's `advanced_settings/send_on_ctrl_enter`
/// and `code_block_ctrl_enter` (#157).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendKey {
    #[default]
    Enter,
    /// Ctrl+Enter, or Cmd+Enter on macOS.
    ModEnter,
    /// Enter, except in a code block that is still open (an odd number of
    /// ``` fences), where Enter starts a new line and Ctrl+Enter sends.
    CodeBlocks,
}

impl SendKey {
    /// Whether Enter sends `text` as typed so far.
    pub fn submits_on_enter(self, text: &str) -> bool {
        match self {
            Self::Enter => true,
            Self::ModEnter => false,
            Self::CodeBlocks => text.matches("```").count().is_multiple_of(2),
        }
    }
}

/// How people's names read: Mattermost's "Teammate name display", the
/// account's `display_settings/name_format` (#156).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameFormat {
    /// The username.
    Username,
    /// The nickname, else the full name, else the username.
    #[default]
    NicknameFullName,
    /// The full name, else the username.
    FullName,
}

/// How much room a message row takes (PLAN §4.2 display).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDisplay {
    /// Avatar, a name and time line, then the body.
    #[default]
    Standard,
    /// One line per post with the time in the gutter, as Mattermost's
    /// compact mode.
    Compact,
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

/// A server the user signed in to, so its cache can open at launch. Holds
/// no secret: the session token belongs in the keychain (#77). The fields
/// after `display_name` came with multiple servers (#206); a file without
/// them gets them from [`crate::servers`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedAccount {
    pub server: String,
    pub user_id: String,
    pub username: String,
    pub display_name: String,
    /// The server's name on the rail: its `SiteName`, or its host.
    #[serde(default)]
    pub name: String,
    /// Names this account's database and media folder.
    #[serde(default)]
    pub data_key: String,
    /// Names this account's item in the system's credential store.
    #[serde(default)]
    pub keychain_key: String,
}

/// A server whose check went through, for the sign-in card's server step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentServer {
    /// The normalized URL Matty checked, like `https://chat.example.com`.
    pub url: String,
    /// The server's `SiteName`, or its host when it has none.
    pub name: String,
}

/// How many servers the server step remembers.
pub const RECENT_SERVERS: usize = 3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub window: Option<WindowGeometry>,
    pub sidebar_width: f32,
    pub right_pane_width: f32,
    pub theme: ThemeChoice,
    /// Matty's colors or the account's Mattermost theme (#214).
    pub theme_source: ThemeSource,
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
    /// Enter sends by default; Ctrl+Enter for people who write in paragraphs.
    pub send_on: SendKey,
    pub display: MessageDisplay,
    /// How people's names read in messages, threads and profiles.
    pub name_format: NameFormat,
    /// Turns off the eases on panes, banners and disclosures (DESIGN.md §1);
    /// the OS reduced-motion setting does the same on its own.
    pub reduce_motion: bool,
    /// Link previews start collapsed: the account's `collapse_previews`
    /// display setting (#187).
    pub collapse_previews: bool,
    /// Posts whose previews the user opened or closed against
    /// [`collapse_previews`](Self::collapse_previews), oldest first, at most
    /// [`PREVIEW_CHOICES`].
    pub preview_choices: Vec<PreviewChoice>,
    /// The server shown now, one of [`servers`](Self::servers).
    pub account: Option<SavedAccount>,
    /// Every server signed in to, in rail order (#206).
    pub servers: Vec<SavedAccount>,
    /// Servers whose check went through, newest first.
    pub recent_servers: Vec<RecentServer>,
    /// The last searches, newest first, at most [`RECENT_SEARCHES`] (#129).
    pub recent_searches: Vec<String>,
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
            theme_source: ThemeSource::Matty,
            text_scale: 1.,
            zoom: 1.,
            launch_at_login: false,
            close_to_tray: false,
            cache_size_mb: 500,
            download_folder: None,
            update_channel: UpdateChannel::for_this_build(),
            log_level: LogLevel::Info,
            load_remote_images: RemoteImages::Always,
            clock: ClockFormat::TwentyFourHour,
            send_on: SendKey::Enter,
            display: MessageDisplay::Standard,
            name_format: NameFormat::NicknameFullName,
            reduce_motion: false,
            collapse_previews: false,
            preview_choices: Vec::new(),
            account: None,
            servers: Vec::new(),
            recent_servers: Vec::new(),
            recent_searches: Vec::new(),
            unknown: serde_json::Map::new(),
        }
    }
}

/// How many searches the Search pane offers again.
pub const RECENT_SEARCHES: usize = 10;

impl Settings {
    /// Puts `query` first among the recent searches, once, ignoring case.
    pub fn remember_search(&mut self, query: &str) {
        let query = query.trim();
        if query.is_empty() {
            return;
        }
        self.recent_searches
            .retain(|seen| !seen.eq_ignore_ascii_case(query));
        self.recent_searches.insert(0, query.to_owned());
        self.recent_searches.truncate(RECENT_SEARCHES);
    }

    /// Puts `url` first among the recent servers under `name`, dropping
    /// its older entry and any past [`RECENT_SERVERS`].
    pub fn remember_server(&mut self, url: &str, name: &str) {
        self.recent_servers
            .retain(|server| !server.url.eq_ignore_ascii_case(url));
        self.recent_servers.insert(
            0,
            RecentServer {
                url: url.to_string(),
                name: name.to_string(),
            },
        );
        self.recent_servers.truncate(RECENT_SERVERS);
    }

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
        self.recent_servers.truncate(RECENT_SERVERS);
        self.migrate_servers();
        let extra = self.preview_choices.len().saturating_sub(PREVIEW_CHOICES);
        self.preview_choices.drain(..extra);
        self
    }

    /// Whether `post_id`'s link previews show collapsed.
    pub fn previews_collapsed(&self, post_id: &str) -> bool {
        self.preview_choices
            .iter()
            .find(|choice| choice.post_id == post_id)
            .map_or(self.collapse_previews, |choice| choice.collapsed)
    }

    /// Remembers that the user collapsed or expanded `post_id`'s previews.
    /// A choice that matches the default is forgotten.
    pub fn choose_previews(&mut self, post_id: &str, collapsed: bool) {
        self.preview_choices
            .retain(|choice| choice.post_id != post_id);
        if collapsed != self.collapse_previews {
            self.preview_choices.push(PreviewChoice {
                post_id: post_id.to_string(),
                collapsed,
            });
        }
        let extra = self.preview_choices.len().saturating_sub(PREVIEW_CHOICES);
        self.preview_choices.drain(..extra);
    }
}

/// How many posts' preview choices are kept; the oldest go first.
pub const PREVIEW_CHOICES: usize = 500;

/// One post's previews, opened or closed by the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewChoice {
    pub post_id: String,
    pub collapsed: bool,
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
    // On the web this names the `localStorage` entry instead.
    #[cfg(target_family = "wasm")]
    return Some(PathBuf::from("Matty/settings.json"));
    #[cfg(not(target_family = "wasm"))]
    dirs::config_dir().map(|dir| dir.join("Matty").join("settings.json"))
}

/// Reads settings from `path`. A file that does not parse is renamed to
/// `settings.json.corrupt-<unix seconds>` so the user's data is not lost.
pub fn load(path: &Path) -> (Settings, Loaded) {
    #[cfg(target_family = "wasm")]
    let bytes = crate::web_storage::get(&path.to_string_lossy()).map(String::into_bytes);
    #[cfg(not(target_family = "wasm"))]
    let bytes = std::fs::read(path);
    let bytes = match bytes {
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
            let stamp = web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
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
            #[cfg(target_family = "wasm")]
            let moved =
                crate::web_storage::set(&aside.to_string_lossy(), &String::from_utf8_lossy(&bytes))
                    .and_then(|()| crate::web_storage::remove(&path.to_string_lossy()));
            #[cfg(not(target_family = "wasm"))]
            let moved = std::fs::rename(path, &aside);
            if let Err(error) = moved {
                tracing::warn!(%error, "could not move the corrupt settings aside");
            }
            (Settings::default(), Loaded::Corrupt(aside))
        }
    }
}

/// Writes settings atomically: a temporary file in the same directory, then a rename.
pub fn save(path: &Path, settings: &Settings) -> io::Result<()> {
    #[cfg(target_family = "wasm")]
    {
        let json = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
        crate::web_storage::set(&path.to_string_lossy(), &json)
    }
    #[cfg(not(target_family = "wasm"))]
    save_file(path, settings)
}

#[cfg(not(target_family = "wasm"))]
fn save_file(path: &Path, settings: &Settings) -> io::Result<()> {
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

    /// The store, where one is installed (views built in tests may have none).
    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalSettings>()
            .map(|global| global.0.clone())
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
                // localStorage lives on the page, not in a worker.
                #[cfg(target_family = "wasm")]
                let written = save(&path, &settings).map_err(|e| (path, e));
                #[cfg(not(target_family = "wasm"))]
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
    fn recent_searches_put_the_newest_first_once_and_keep_ten() {
        let mut settings = Settings::default();
        for query in ["a", "b", "  ", "A"] {
            settings.remember_search(query);
        }
        assert_eq!(settings.recent_searches, ["A", "b"]);
        for n in 0..12 {
            settings.remember_search(&format!("q{n}"));
        }
        assert_eq!(settings.recent_searches.len(), RECENT_SEARCHES);
        assert_eq!(settings.recent_searches[0], "q11");
    }

    #[test]
    fn recent_servers_keep_the_newest_three_once_each() {
        let mut settings = Settings::default();
        for (url, name) in [
            ("https://a.example.com", "A"),
            ("https://b.example.com", "B"),
            ("https://c.example.com", "C"),
            ("https://A.example.com", "A again"),
            ("https://d.example.com", "D"),
        ] {
            settings.remember_server(url, name);
        }
        let names: Vec<_> = settings
            .recent_servers
            .iter()
            .map(|server| server.name.as_str())
            .collect();
        assert_eq!(names, ["D", "A again", "C"]);
        // An older file without the key still loads.
        let old: Settings = serde_json::from_str(r#"{"zoom": 1.0}"#).unwrap();
        assert!(old.recent_servers.is_empty());
    }

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
    fn the_download_folder_and_log_level_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let settings = Settings {
            download_folder: Some(dir.path().join("Saved files")),
            log_level: LogLevel::Debug,
            ..Settings::default()
        };
        save(&path, &settings).unwrap();
        let (again, loaded) = load(&path);
        assert_eq!(loaded, Loaded::Read);
        assert_eq!(again.download_folder, settings.download_folder);
        assert_eq!(again.log_level, LogLevel::Debug);
        // Unset, downloads go to the system's folder and logs stay at Info.
        let (defaults, _) = load(&dir.path().join("missing.json"));
        assert_eq!(defaults.download_folder, None);
        assert_eq!(defaults.log_level, LogLevel::Info);
    }

    #[test]
    fn a_post_remembers_its_previews_against_the_default() {
        let mut settings = Settings::default();
        assert!(!settings.previews_collapsed("p1"));
        settings.choose_previews("p1", true);
        assert!(settings.previews_collapsed("p1"));
        assert!(!settings.previews_collapsed("p2"));
        // Collapsing everything by default keeps p1's choice and leaves
        // the rest closed; opening p2 is remembered.
        settings.collapse_previews = true;
        settings.choose_previews("p2", false);
        assert!(!settings.previews_collapsed("p2"));
        assert!(settings.previews_collapsed("p3"));
        // A choice that matches the default is forgotten.
        settings.choose_previews("p1", true);
        assert_eq!(settings.preview_choices.len(), 1);
        // The oldest choices go first.
        settings.collapse_previews = false;
        for n in 0..PREVIEW_CHOICES + 5 {
            settings.choose_previews(&format!("q{n}"), true);
        }
        assert_eq!(settings.preview_choices.len(), PREVIEW_CHOICES);
        assert!(!settings.previews_collapsed("q0"));
        assert!(settings.previews_collapsed(&format!("q{}", PREVIEW_CHOICES + 4)));
        // They survive a restart.
        let json = serde_json::to_string(&settings).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.preview_choices, settings.preview_choices);
    }

    #[test]
    fn the_update_channel_defaults_by_build_and_a_choice_sticks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{}").unwrap();
        let (settings, _) = load(&path);
        assert_eq!(settings.update_channel, UpdateChannel::for_this_build());
        let expected = if matty_update::CURRENT_VERSION.contains('-') {
            UpdateChannel::Beta
        } else {
            UpdateChannel::Stable
        };
        assert_eq!(UpdateChannel::for_this_build(), expected);
        std::fs::write(&path, r#"{"update_channel": "beta"}"#).unwrap();
        assert_eq!(load(&path).0.update_channel, UpdateChannel::Beta);
        std::fs::write(&path, r#"{"update_channel": "stable"}"#).unwrap();
        assert_eq!(load(&path).0.update_channel, UpdateChannel::Stable);
    }

    #[test]
    fn code_blocks_only_holds_enter_inside_an_open_fence() {
        let key = SendKey::CodeBlocks;
        assert!(key.submits_on_enter("plain text"));
        assert!(!key.submits_on_enter("```rust\nfn main() {}"));
        assert!(key.submits_on_enter("```rust\nfn main() {}\n```"));
        assert!(SendKey::Enter.submits_on_enter("```"));
        assert!(!SendKey::ModEnter.submits_on_enter("plain"));
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
