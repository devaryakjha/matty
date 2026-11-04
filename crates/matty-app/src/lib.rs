//! The Matty GPUI app: windows, views, actions, keymap and settings.
//!
//! Every reusable component comes from gpuicn through [`ui`]; Mattermost
//! models, API calls and app state live in the plain-Rust crates below this
//! one (PLAN §2, §6.1).

#![allow(missing_docs)]

pub mod account;
pub mod actions;
pub mod attachments;
pub mod avatars;
pub mod badge;
pub mod channel;
pub mod channel_settings;
pub mod deeplink;
pub mod directory;
pub mod dnd;
pub mod draft_link;
pub mod drafts;
pub mod embeds;
pub mod emoji;
pub mod live;
pub mod logging;
pub mod login;
mod main_window;
pub mod menu;
pub mod preview;
pub mod profile;
pub mod rail;
pub mod right_pane;
pub mod secrets;
pub mod settings;
pub mod sidebar;
pub mod style;
pub mod switcher;
pub mod threads;
pub mod transcript;
mod ui;

use std::borrow::Cow;

use gpui_icons::LucideAssetSource;
use gpui_kit::{
    App, AppContext as _, Bounds, Entity, Window, WindowAppearance, WindowBounds, WindowOptions,
    point, px, size,
};
use settings::{SettingsStore, ThemeChoice};
use ui::theme::{ThemeMode, UiTheme};
use ui::title_bar::title_bar_window_options;

/// The smallest window PLAN §4.1 supports.
pub const MIN_WINDOW: (f32, f32) = (480., 400.);
const DEFAULT_WINDOW: (f32, f32) = (1200., 800.);

static OFFLINE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether `matty --offline` started the app: the last account's cache
/// opens without signing in.
pub fn offline() -> bool {
    OFFLINE.load(std::sync::atomic::Ordering::Relaxed)
}

/// [`run`] with the cache of the last account and no network.
pub fn run_offline() {
    OFFLINE.store(true, std::sync::atomic::Ordering::Relaxed);
    run();
}

/// Starts the GPUI application and opens the main window. Blocks until the
/// app quits.
pub fn run() {
    run_with_links(Vec::new());
}

/// [`run`], then handles `links` (a `mattermost://` URL given on the command
/// line) as if the OS had opened them. Links the OS opens later arrive the
/// same way. A second launch on Linux and Windows does not forward its link
/// to the first yet (#84).
pub fn run_with_links(links: Vec<String>) {
    let (links_tx, links_rx) = async_channel::unbounded::<String>();
    for link in links {
        let _ = links_tx.try_send(link);
    }
    let app = gpui_kit::platform::application().with_assets(LucideAssetSource);
    // macOS calls this for a URL-scheme open, at launch or while running.
    app.on_open_urls(move |urls| {
        for url in urls {
            tracing::info!(link = %deeplink::redacted(&url), "the system opened a link");
            let _ = links_tx.try_send(url);
        }
    });
    app.run(move |cx: &mut App| {
        // Settings load after this, so their level applies just below.
        let logs = logging::init(logging::default_dir().as_deref(), Default::default());
        tracing::info!(version = env!("CARGO_PKG_VERSION"), "matty starting");
        ui::theme::init(cx);
        style::install(ThemeMode::Light, cx);
        load_fonts(cx);
        let settings = SettingsStore::init(settings::default_path(), cx);
        if let Some(logs) = logs {
            follow_log_level(&settings, logs, cx);
        }
        actions::bind(cx);
        menu::init(cx);
        cx.set_global(deeplink::Incoming(links_rx));
        open_main_window(cx);
        cx.activate(true);
    });
}

/// Keeps the logger's level in step with the `log_level` setting.
fn follow_log_level(settings: &Entity<SettingsStore>, logs: logging::LevelHandle, cx: &mut App) {
    logs.set(settings.read(cx).get().log_level);
    cx.subscribe(settings, move |store, _: &settings::SettingsChanged, cx| {
        logs.set(store.read(cx).get().log_level);
    })
    .detach();
}

fn load_fonts(cx: &mut App) {
    cx.text_system()
        .add_fonts(vec![
            Cow::Borrowed(include_bytes!("../assets/fonts/Geist-Regular.ttf")),
            Cow::Borrowed(include_bytes!("../assets/fonts/Geist-Medium.ttf")),
            Cow::Borrowed(include_bytes!("../assets/fonts/GeistMono-Regular.ttf")),
        ])
        .expect("load bundled Geist fonts");
}

fn window_options(cx: &App) -> WindowOptions {
    let saved = SettingsStore::global(cx).read(cx).get().window;
    let bounds = match saved {
        Some(geometry) => {
            let bounds = Bounds::new(
                point(px(geometry.x), px(geometry.y)),
                size(
                    px(geometry.width.max(MIN_WINDOW.0)),
                    px(geometry.height.max(MIN_WINDOW.1)),
                ),
            );
            if geometry.maximized {
                WindowBounds::Maximized(bounds)
            } else {
                WindowBounds::Windowed(bounds)
            }
        }
        None => WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(DEFAULT_WINDOW.0), px(DEFAULT_WINDOW.1)),
            cx,
        )),
    };
    WindowOptions {
        window_bounds: Some(bounds),
        window_min_size: Some(size(px(MIN_WINDOW.0), px(MIN_WINDOW.1))),
        app_id: Some("app.matty.Matty".into()),
        ..title_bar_window_options()
    }
}

fn open_main_window(cx: &mut App) {
    cx.open_window(window_options(cx), |window, cx| {
        apply_theme(window, cx);
        cx.new(|cx| main_window::MainWindow::new(window, cx))
    })
    .expect("open the main window");
}

/// Applies the theme choice from settings, following the OS when it is
/// `System`, and the text scale and zoom.
pub(crate) fn apply_theme(window: &Window, cx: &mut App) {
    let settings = SettingsStore::global(cx).read(cx).get().clone();
    let choice = settings.theme;
    let mode = match choice {
        ThemeChoice::Light => ThemeMode::Light,
        ThemeChoice::Dark => ThemeMode::Dark,
        ThemeChoice::System => match window.appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
            WindowAppearance::Light | WindowAppearance::VibrantLight => ThemeMode::Light,
        },
    };
    let text_scale = settings.text_scale * settings.zoom;
    let unit = px(4. * settings.zoom);
    let theme = UiTheme::read(cx);
    if theme.text_scale != text_scale || theme.spacing.unit != unit {
        let mut theme = theme.clone();
        theme.text_scale = text_scale;
        theme.spacing.unit = unit;
        UiTheme::set(cx, theme);
    }
    style::install(mode, cx);
}
