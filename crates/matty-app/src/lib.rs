//! The Matty GPUI app: windows, views, actions, keymap and settings.
//!
//! Every reusable component comes from gpuicn through [`ui`]; Mattermost
//! models, API calls and app state live in the plain-Rust crates below this
//! one (PLAN §2, §6.1).

#![allow(missing_docs)]

mod ui;

use std::borrow::Cow;

use gpui_icons::LucideAssetSource;
use gpui_kit::{
    App, AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled, Window,
    WindowAppearance, WindowOptions, div, px,
};
use ui::theme::{ThemeMode, UiTheme};

/// Starts the GPUI application and opens the main window. Blocks until the
/// app quits.
pub fn run() {
    gpui_kit::platform::application()
        .with_assets(LucideAssetSource)
        .run(|cx: &mut App| {
            ui::theme::init(cx);
            load_fonts(cx);
            cx.open_window(WindowOptions::default(), |window, cx| {
                follow_system_appearance(window, cx);
                cx.new(|cx| {
                    cx.observe_window_appearance(window, |_, window, cx| {
                        follow_system_appearance(window, cx);
                    })
                    .detach();
                    MainWindow
                })
            })
            .expect("open the main window");
            cx.activate(true);
        });
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

fn follow_system_appearance(window: &Window, cx: &mut App) {
    let mode = match window.appearance() {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
        WindowAppearance::Light | WindowAppearance::VibrantLight => ThemeMode::Light,
    };
    if UiTheme::read(cx).mode != mode {
        UiTheme::switch(cx, mode);
    }
}

struct MainWindow;

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx);
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.colors.background)
            .text_color(theme.colors.foreground)
            .font_family(theme.fonts.body.clone())
            .text_size(theme.text(13.))
            .child("Matty")
            .p(px(24.))
    }
}
