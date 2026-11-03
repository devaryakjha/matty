//! The main window frame from PLAN §4.1 and DESIGN.md §4: a title bar that
//! holds navigation and the quick switcher, then the team rail, sidebar,
//! channel view and right pane. In M0 the rail and right pane stay hidden
//! (the rail appears with a second team, the right pane with threads and
//! search), the sidebar shows its loading rows and the channel view shows an
//! empty state until sync lands.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    Action, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, prelude::FluentBuilder as _, px,
};

use crate::actions::{
    self, HistoryBack, HistoryForward, OpenPreferences, Platform, QuickSwitcher, RecentMentions,
    ResetZoom, ShowShortcuts, ToggleSidebar, ZoomIn, ZoomOut,
};
use crate::channel::ChannelView;
use crate::settings::{
    SIDEBAR_MAX, SIDEBAR_MIN, SettingsChanged, SettingsStore, WindowGeometry, ZOOM_STEP,
};
use crate::sidebar::{SidebarEvent, SidebarModel, SidebarView};
use crate::style::{self, Palette, TypeStyleExt as _, metrics, scaled};
use crate::switcher::{Switcher, SwitcherEvent};
use crate::ui::{
    avatar::{Avatar, AvatarSize},
    button::{Button, ButtonSize, ButtonVariant},
    dialog::{DialogHandle, dialog, dialog_close, dialog_description, dialog_popup, dialog_title},
    kbd::{Kbd, format_shortcut},
    resizable::{PaneLimits, Resizable},
    theme::UiTheme,
    title_bar::TitleBar,
    tooltip::text_tooltip,
};

/// The channel view's minimum width beside the sidebar.
const CONTENT_MIN: f32 = 260.;

pub struct MainWindow {
    settings: Entity<SettingsStore>,
    sidebar: Entity<SidebarView>,
    channel: Entity<ChannelView>,
    switcher: Entity<Switcher>,
    focus: FocusHandle,
    /// The channel the sidebar selected; the channel view follows it in M1.
    selected_channel: Option<gpui_kit::SharedString>,
    /// Cmd+Shift+D hides the sidebar for this session.
    sidebar_visible: bool,
    shortcuts: DialogHandle,
    preferences: DialogHandle,
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
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let model = if crate::preview::enabled() {
            crate::preview::sidebar()
        } else {
            SidebarModel::loading()
        };
        let selected_channel = model.selected.clone();
        let switcher = cx.new(|cx| Switcher::new(&model, window, cx));
        cx.subscribe_in(
            &switcher,
            window,
            |this, _, event: &SwitcherEvent, window, cx| {
                let SwitcherEvent::Open(id) = event;
                this.open_channel(id.clone(), window, cx);
            },
        )
        .detach();
        let sidebar = cx.new(|_| SidebarView::new("Matty", model));
        let initial = selected_channel
            .as_deref()
            .filter(|_| crate::preview::enabled())
            .and_then(crate::preview::channel);
        let channel = cx.new(|cx| ChannelView::new(initial, window, cx));
        cx.subscribe_in(
            &sidebar,
            window,
            |this, _, event: &SidebarEvent, window, cx| {
                let SidebarEvent::Select(id) = event;
                this.selected_channel = Some(id.clone());
                if crate::preview::enabled() {
                    let model = crate::preview::channel(id);
                    this.channel
                        .update(cx, |view, cx| view.set_model(model, window, cx));
                }
                cx.notify();
            },
        )
        .detach();
        Self {
            settings,
            sidebar,
            channel,
            switcher,
            focus,
            selected_channel,
            sidebar_visible: true,
            shortcuts: DialogHandle::new(false),
            preferences: DialogHandle::new(false),
            rail_visible: false,
            right_pane_open: false,
        }
    }

    /// Shows `id` in the channel view and marks it in the sidebar. Until
    /// sync lands only the preview fixtures have content.
    fn open_channel(
        &mut self,
        id: gpui_kit::SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_channel = Some(id.clone());
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.select(Some(id.clone()), cx));
        if crate::preview::enabled() {
            let model = crate::preview::channel(&id);
            self.channel
                .update(cx, |view, cx| view.set_model(model, window, cx));
        }
        cx.notify();
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

    fn zoom(&mut self, step: Option<f32>, cx: &mut Context<Self>) {
        self.settings.update(cx, |store, cx| {
            store.update(cx, |s| {
                s.zoom = match step {
                    Some(step) => ((s.zoom + step) * 10.).round() / 10.,
                    None => 1.,
                }
            })
        });
    }

    /// The keystroke bound to the action with this label in the shortcut
    /// sheet, formatted for the current platform.
    fn shortcut_for(label: &str) -> Option<String> {
        let platform = Platform::current();
        actions::bindings()
            .into_iter()
            .find(|row| row.label == label)
            .map(|row| {
                let os = match platform {
                    Platform::MacOs => gpui_kit::OperatingSystem::Mac,
                    Platform::Other => gpui_kit::OperatingSystem::Linux,
                };
                format_shortcut(row.keys(platform), os).to_string()
            })
    }

    /// A ghost icon button that dispatches `action`, with a tooltip naming
    /// it and its shortcut when the keymap has one.
    fn icon_button(
        id: &'static str,
        label: &'static str,
        glyph: LucideIcon,
        action: Box<dyn Action>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = UiTheme::read(cx);
        let tip: SharedString = match Self::shortcut_for(label) {
            Some(keys) => format!("{label}  {keys}").into(),
            None => label.into(),
        };
        let button = Button::new(id)
            .variant(ButtonVariant::Ghost)
            .size(ButtonSize::IconSm)
            .aria_label(label)
            .disabled(!enabled)
            .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
            .child(
                lucide(glyph)
                    .size(scaled(theme, metrics::ICON))
                    .text_color(theme.colors.muted_foreground),
            );
        div()
            .id(SharedString::from(format!("{id}.tooltip")))
            .tooltip(move |_, cx| text_tooltip(tip.clone(), cx))
            .child(button)
    }

    /// DESIGN.md §4: navigation on the left, the quick switcher as a search
    /// pill in the middle, mentions, saved, help and the user on the right.
    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let palette = Palette::read(cx);
        let platform = Platform::current();
        let switcher_keys = Self::shortcut_for("Quick switcher").unwrap_or_default();
        let switcher = Button::new("titlebar.switcher")
            .variant(ButtonVariant::Outline)
            .size(ButtonSize::Sm)
            .aria_label(format!("Search Matty, {switcher_keys}"))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(QuickSwitcher), cx))
            .flex_1()
            .max_w(scaled(&theme, metrics::SWITCHER_MAX_WIDTH))
            .justify_start()
            .bg(theme.colors.background)
            .text_color(theme.colors.muted_foreground)
            .font_weight(gpui_kit::FontWeight::NORMAL)
            .child(
                lucide(LucideIcon::Search)
                    .size(scaled(&theme, 14.))
                    .text_color(theme.colors.muted_foreground),
            )
            .child(div().flex_1().text_left().child("Search Matty"))
            .child(Kbd::shortcut(match platform {
                Platform::MacOs => "cmd-k",
                Platform::Other => "ctrl-k",
            }));
        let me = div()
            .relative()
            .flex_shrink_0()
            .child(
                Avatar::new("titlebar.me")
                    .size(AvatarSize::Sm)
                    .fallback("M")
                    .aria_label("You, online"),
            )
            .child(
                div()
                    .absolute()
                    .bottom(px(-1.))
                    .right(px(-1.))
                    .size(scaled(&theme, 10.))
                    .rounded_full()
                    .bg(palette.online)
                    .border_2()
                    .border_color(theme.colors.sidebar),
            );
        TitleBar::new()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space(0.5))
                    .flex_shrink_0()
                    .child(Self::icon_button(
                        "titlebar.back",
                        "Back",
                        LucideIcon::ChevronLeft,
                        Box::new(HistoryBack),
                        false,
                        cx,
                    ))
                    .child(Self::icon_button(
                        "titlebar.forward",
                        "Forward",
                        LucideIcon::ChevronRight,
                        Box::new(HistoryForward),
                        false,
                        cx,
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .justify_center()
                    .px(theme.space(2.))
                    .child(switcher),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space(0.5))
                    .pr(theme.space(2.))
                    .flex_shrink_0()
                    .child(Self::icon_button(
                        "titlebar.mentions",
                        "Recent mentions",
                        LucideIcon::AtSign,
                        Box::new(RecentMentions),
                        false,
                        cx,
                    ))
                    .child(Self::icon_button(
                        "titlebar.help",
                        "Keyboard shortcuts",
                        LucideIcon::CircleQuestionMark,
                        Box::new(ShowShortcuts),
                        true,
                        cx,
                    ))
                    .child(div().pl(theme.space(1.)).child(me)),
            )
    }

    fn shortcut_sheet(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let platform = Platform::current();
        let rows = actions::bindings().into_iter().map(|row| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(theme.space(6.))
                .h(scaled(&theme, metrics::ROW_HEIGHT))
                .type_style(style::UI, &theme)
                .child(row.label)
                .child(Kbd::shortcut(row.keys(platform)))
        });
        let popup = dialog_popup("shortcuts.popup", "Keyboard shortcuts", cx)
            .child(dialog_title("shortcuts.title", cx).child("Keyboard shortcuts"))
            .child(
                dialog_description("shortcuts.description", cx)
                    .child("Every shortcut works anywhere in the window unless it names a view."),
            )
            .child(div().flex().flex_col().children(rows))
            .child(dialog_close("shortcuts.close", cx).label("Close"));
        dialog("shortcuts", &self.shortcuts, popup, window, cx)
    }

    fn preferences_stub(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let popup = dialog_popup("preferences.popup", "Preferences", cx)
            .child(dialog_title("preferences.title", cx).child("Preferences"))
            .child(
                dialog_description("preferences.description", cx)
                    .child("Theme, message display and notification settings arrive with M1."),
            )
            .child(dialog_close("preferences.close", cx).label("Close"));
        dialog("preferences", &self.preferences, popup, window, cx)
    }

    /// DESIGN.md §4: 56px rail of 32px team squares, the current one
    /// outlined in `primary`. Hidden until the user has a second team.
    fn team_rail(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let square = |id: &'static str, label: &'static str, current: bool| {
            Button::new(id)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::Icon)
                .aria_label(label)
                .rounded(theme.radius.lg)
                .bg(theme.colors.sidebar_accent)
                .type_style(style::UI_STRONG, &theme)
                .when(current, |b| b.border_2().border_color(theme.colors.primary))
                .child(label.chars().next().unwrap_or('?').to_string())
        };
        div()
            .w(scaled(&theme, metrics::RAIL_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(theme.space(2.))
            .py(theme.space(2.))
            .bg(theme.colors.sidebar)
            .border_r(style::hairline())
            .border_color(theme.colors.sidebar_border)
            .child(square("rail.team.current", "Matty", true))
            .child(
                Button::new("rail.add")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Icon)
                    .aria_label("Join another team")
                    .child(
                        lucide(LucideIcon::Plus)
                            .size(scaled(&theme, metrics::ICON))
                            .text_color(theme.colors.muted_foreground),
                    ),
            )
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let sidebar_width = px(self.settings.read(cx).get().sidebar_width);
        let settings = self.settings.clone();
        let frame = if self.sidebar_visible {
            Resizable::new("main.sidebar-split", "Resize sidebar", sidebar_width)
                .first(self.sidebar.clone())
                .second(self.channel.clone())
                .first_limits(PaneLimits::new(px(SIDEBAR_MIN), px(SIDEBAR_MAX)))
                .second_limits(PaneLimits::new(px(CONTENT_MIN), px(100_000.)))
                .on_resize(move |width: &Pixels, _, cx| {
                    let width = f32::from(*width);
                    settings.update(cx, |store, cx| {
                        store.update(cx, |s| s.sidebar_width = width)
                    });
                })
                .into_any_element()
        } else {
            self.channel.clone().into_any_element()
        };
        let shortcuts = self.shortcuts.clone();
        let preferences = self.preferences.clone();
        let switcher = self.switcher.clone();
        let switcher_element = self.switcher.update(cx, |switcher, cx| {
            switcher.element(window, cx).into_any_element()
        });
        let right_pane_width = px(self.settings.read(cx).get().right_pane_width);
        div()
            .key_context(actions::MAIN_WINDOW)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.sidebar_visible = !this.sidebar_visible;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(Some(ZOOM_STEP), cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(Some(-ZOOM_STEP), cx)))
            .on_action(cx.listener(|this, _: &ResetZoom, _, cx| this.zoom(None, cx)))
            .on_action(move |_: &ShowShortcuts, window, cx| shortcuts.open(window, cx))
            .on_action(move |_: &OpenPreferences, window, cx| preferences.open(window, cx))
            .on_action(move |_: &QuickSwitcher, window, cx| {
                switcher.update(cx, |switcher, cx| switcher.open(window, cx))
            })
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.colors.background)
            .text_color(theme.colors.foreground)
            .font_family(theme.fonts.body.clone())
            .type_style(style::UI, &theme)
            .child(self.title_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .when(self.rail_visible, |row| row.child(self.team_rail(cx)))
                    .child(div().flex_1().min_w_0().h_full().child(frame))
                    .when(self.right_pane_open, |row| {
                        row.child(
                            div()
                                .w(right_pane_width)
                                .h_full()
                                .border_l(style::hairline())
                                .border_color(theme.colors.border),
                        )
                    }),
            )
            .child(self.shortcut_sheet(window, cx))
            .child(self.preferences_stub(window, cx))
            .child(switcher_element)
    }
}
