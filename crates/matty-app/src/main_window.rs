//! The main window frame from PLAN §4.1: team rail, sidebar, channel view and
//! right pane. In M0 the rail and right pane are placeholders that stay hidden
//! (the rail appears with a second team, the right pane with threads and
//! search), and the channel view shows an empty state until sync lands.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    Context, Entity, IntoElement, ParentElement as _, Pixels, Render, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::settings::{SIDEBAR_MAX, SIDEBAR_MIN, SettingsChanged, SettingsStore, WindowGeometry};
use crate::ui::{
    empty::{Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle},
    resizable::{PaneLimits, Resizable},
    separator::Separator,
    sidebar::Sidebar,
    theme::UiTheme,
};

/// The channel view's minimum width beside the sidebar.
const CONTENT_MIN: f32 = 260.;

pub struct MainWindow {
    settings: Entity<SettingsStore>,
    /// Shown when the user belongs to more than one team (M1).
    rail_visible: bool,
    /// Holds threads, search results and the other right-pane views (M1).
    right_pane_open: bool,
}

impl MainWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = SettingsStore::global(cx);
        cx.observe_window_appearance(window, |_, window, cx| crate::apply_theme(window, cx))
            .detach();
        cx.subscribe_in(
            &settings,
            window,
            |_, _, _: &SettingsChanged, window, cx| {
                crate::apply_theme(window, cx);
                cx.notify();
            },
        )
        .detach();
        cx.observe_window_bounds(window, |this, window, cx| this.remember_bounds(window, cx))
            .detach();
        Self {
            settings,
            rail_visible: false,
            right_pane_open: false,
        }
    }

    fn remember_bounds(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let maximized = window.is_maximized();
        let bounds = window.window_bounds().get_bounds();
        if window.is_fullscreen() {
            return;
        }
        let geometry = WindowGeometry {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
            maximized,
        };
        self.settings.update(cx, |store, cx| {
            store.update(cx, |s| s.window = Some(geometry))
        });
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx);
        Sidebar::new("sidebar", "Channels").header(
            div()
                .flex()
                .items_center()
                .h(theme.space(8.))
                .px(theme.space(2.))
                .text_size(theme.text(15.))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child("Matty"),
        )
    }

    fn channel_view(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx);
        div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(theme.colors.background)
            .child(
                div()
                    .flex_shrink_0()
                    .h(theme.space(12.))
                    .px(theme.space(4.))
                    .flex()
                    .items_center()
                    .text_size(theme.text(13.))
                    .text_color(theme.colors.muted_foreground)
                    .child("No channel open"),
            )
            .child(Separator::new("channel.header-separator").horizontal())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(theme.space(6.))
                    .child(
                        Empty::new().header(
                            EmptyHeader::new()
                                .media(
                                    EmptyMedia::new().child(
                                        lucide(LucideIcon::MessagesSquare)
                                            .size(theme.space(6.))
                                            .text_color(theme.colors.foreground),
                                    ),
                                )
                                .title(EmptyTitle::new().child("Not connected yet"))
                                .description(EmptyDescription::new().child(
                                    "Matty will show your channels here once it is connected to a Mattermost server.",
                                )),
                        ),
                    ),
            )
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let sidebar_width = px(self.settings.read(cx).get().sidebar_width);
        let settings = self.settings.clone();
        let _ = window;
        let frame = Resizable::new("main.sidebar-split", "Resize sidebar", sidebar_width)
            .first(self.sidebar(cx))
            .second(self.channel_view(cx))
            .first_limits(PaneLimits::new(px(SIDEBAR_MIN), px(SIDEBAR_MAX)))
            .second_limits(PaneLimits::new(px(CONTENT_MIN), px(100_000.)))
            .on_resize(move |width: &Pixels, _, cx| {
                let width = f32::from(*width);
                settings.update(cx, |store, cx| {
                    store.update(cx, |s| s.sidebar_width = width)
                });
            });
        div()
            .size_full()
            .flex()
            .bg(theme.colors.background)
            .text_color(theme.colors.foreground)
            .font_family(theme.fonts.body.clone())
            .text_size(theme.text(13.))
            .when(self.rail_visible, |root| {
                root.child(div().w(px(64.)).h_full().bg(theme.colors.sidebar))
            })
            .child(div().flex_1().min_w_0().h_full().child(frame))
            .when(self.right_pane_open, |root| {
                root.child(div().w(px(400.)).h_full())
            })
    }
}
