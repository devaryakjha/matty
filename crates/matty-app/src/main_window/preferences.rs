//! Preferences (#155, PLAN §7): a sheet from the window's right edge with a
//! section list on its left (General, Appearance, Messages, Notifications,
//! Advanced) and the chosen section's controls beside it. Every control is
//! a gpuicn one and every change applies straight away; settings that live
//! on the server say so with a "Synced with your Mattermost account" line.
//!
//! The account's display and advanced settings (#156, #157) are mirrored
//! into the local settings file: [`mirror`] takes what the server sent, and
//! the setters here write both the file and the server's preference.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    AnyElement, App, Context, Div, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use matty_api::models::Preference;

use super::{MainWindow, TEXT_SIZES};
use crate::actions::Platform;
use crate::dnd;
use crate::mattermost_theme::ServerThemes;
use crate::settings::{
    ClockFormat, MessageDisplay, NameFormat, RemoteImages, SendKey, Settings, ThemeChoice,
    ThemeSource, UpdateChannel,
};
use crate::sidebar::Presence;
use crate::style::{self, TypeStyleExt as _, metrics, scaled};
use crate::t;
use crate::ui::{
    button::{Button, ButtonSize, ButtonVariant},
    drawer::{Drawer, DrawerContent, DrawerSide, drawer_description, drawer_title},
    field::Field,
    scroll_area::ScrollArea,
    select::Select,
    sidebar::{Sidebar, SidebarItem},
    switch::Switch,
    tag::{Tag, TagVariant},
    theme::UiTheme,
    toggle_group::{ToggleGroup, ToggleGroupItem},
};

/// The Mattermost preference categories Preferences mirrors.
const DISPLAY: &str = "display_settings";
const ADVANCED: &str = "advanced_settings";

/// One page of Preferences, picked from the section list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Section {
    /// The window, start-up and downloads.
    #[default]
    General,
    /// Theme, text size, message display, clock and names.
    Appearance,
    /// The send key and collapsed reply threads.
    Messages,
    /// Desktop notifications and what counts as a mention.
    Notifications,
    /// Logs and signing out.
    Advanced,
}

impl Section {
    /// The sections in list order.
    pub(crate) const ALL: [Self; 5] = [
        Self::General,
        Self::Appearance,
        Self::Messages,
        Self::Notifications,
        Self::Advanced,
    ];

    pub(crate) fn label(self) -> SharedString {
        match self {
            Self::General => t!("prefs-section-general"),
            Self::Appearance => t!("prefs-section-appearance"),
            Self::Messages => t!("prefs-section-messages"),
            Self::Notifications => t!("prefs-section-notifications"),
            Self::Advanced => t!("prefs-section-advanced"),
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::General => "preferences.section.general",
            Self::Appearance => "preferences.section.appearance",
            Self::Messages => "preferences.section.messages",
            Self::Notifications => "preferences.section.notifications",
            Self::Advanced => "preferences.section.advanced",
        }
    }

    fn icon(self) -> LucideIcon {
        match self {
            Self::General => LucideIcon::Settings,
            Self::Appearance => LucideIcon::Palette,
            Self::Messages => LucideIcon::MessageSquare,
            Self::Notifications => LucideIcon::Bell,
            Self::Advanced => LucideIcon::Wrench,
        }
    }
}

/// The section list: a gpuicn `Sidebar` whose rows are one tab stop, Up
/// and Down move between them and Enter or a click opens one.
pub(crate) fn section_list(
    selected: Section,
    on_select: impl Fn(Section, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Sidebar {
    let theme = UiTheme::read(cx);
    let on_select = std::rc::Rc::new(on_select);
    let mut list = Sidebar::new("preferences.sections", t!("prefs-sections")).roving_focus(true);
    for section in Section::ALL {
        let on_select = on_select.clone();
        list = list.child(
            SidebarItem::new(section.id(), section.label())
                .icon(
                    lucide(section.icon())
                        .size(scaled(theme, metrics::ICON))
                        .text_color(theme.colors.muted_foreground),
                )
                .selected(section == selected)
                .on_activate(move |_, window, cx| on_select(section, window, cx)),
        );
    }
    list
}

// Mirroring the account's settings (#156, #157).

fn find<'a>(preferences: &'a [Preference], category: &str, name: &str) -> Option<&'a str> {
    preferences
        .iter()
        .find(|p| p.category == category && p.name == name)
        .map(|p| p.value.as_str())
}

/// Takes the account's message display, clock, name format and send key
/// from `preferences` into `settings`. A preference the user never set on
/// the server leaves the local choice as it is.
pub(crate) fn mirror(preferences: &[Preference], settings: &mut Settings) {
    if let Some(value) = find(preferences, DISPLAY, "message_display") {
        settings.display = match value {
            "compact" => MessageDisplay::Compact,
            _ => MessageDisplay::Standard,
        };
    }
    if let Some(value) = find(preferences, DISPLAY, "use_military_time") {
        settings.clock = match value {
            "true" => ClockFormat::TwentyFourHour,
            _ => ClockFormat::TwelveHour,
        };
    }
    if let Some(value) = find(preferences, DISPLAY, "collapse_previews") {
        settings.collapse_previews = value == "true";
    }
    if let Some(value) = find(preferences, DISPLAY, "name_format") {
        settings.name_format = match value {
            "username" => NameFormat::Username,
            "full_name" => NameFormat::FullName,
            _ => NameFormat::NicknameFullName,
        };
    }
    let ctrl = find(preferences, ADVANCED, "send_on_ctrl_enter");
    let code = find(preferences, ADVANCED, "code_block_ctrl_enter");
    if ctrl.is_some() || code.is_some() {
        // The web app's defaults: off for every message, on for code blocks.
        settings.send_on = match (ctrl.unwrap_or("false"), code.unwrap_or("true")) {
            ("true", _) => SendKey::ModEnter,
            (_, "true") => SendKey::CodeBlocks,
            _ => SendKey::Enter,
        };
    }
}

fn preference(category: &str, name: &str, value: &str) -> Preference {
    Preference {
        category: category.into(),
        name: name.into(),
        value: value.into(),
        ..Preference::default()
    }
}

/// The server preference for a message display.
pub(crate) fn display_preference(display: MessageDisplay) -> Vec<Preference> {
    let value = match display {
        MessageDisplay::Standard => "clean",
        MessageDisplay::Compact => "compact",
    };
    vec![preference(DISPLAY, "message_display", value)]
}

/// The server preference for whether link previews start collapsed.
pub(crate) fn collapse_previews_preference(collapsed: bool) -> Vec<Preference> {
    vec![preference(
        DISPLAY,
        "collapse_previews",
        if collapsed { "true" } else { "false" },
    )]
}

/// The server preference for a clock.
pub(crate) fn clock_preference(clock: ClockFormat) -> Vec<Preference> {
    let value = match clock {
        ClockFormat::TwelveHour => "false",
        ClockFormat::TwentyFourHour => "true",
    };
    vec![preference(DISPLAY, "use_military_time", value)]
}

/// The server preference for a name format.
pub(crate) fn name_format_preference(format: NameFormat) -> Vec<Preference> {
    let value = match format {
        NameFormat::Username => "username",
        NameFormat::NicknameFullName => "nickname_full_name",
        NameFormat::FullName => "full_name",
    };
    vec![preference(DISPLAY, "name_format", value)]
}

/// The server preferences for a send key, both written as the web app
/// writes them.
pub(crate) fn send_key_preferences(send_on: SendKey) -> Vec<Preference> {
    let (ctrl, code) = match send_on {
        SendKey::Enter => ("false", "false"),
        SendKey::ModEnter => ("true", "true"),
        SendKey::CodeBlocks => ("false", "true"),
    };
    vec![
        preference(ADVANCED, "send_on_ctrl_enter", ctrl),
        preference(ADVANCED, "code_block_ctrl_enter", code),
    ]
}

impl MainWindow {
    /// Changes a setting the account keeps too: the local file now, the
    /// server's preference behind it.
    pub(super) fn set_synced(
        &mut self,
        change: impl FnOnce(&mut Settings),
        preferences: impl FnOnce(&Settings) -> Vec<Preference>,
        cx: &mut Context<Self>,
    ) {
        let before = self.settings.read(cx).get().clone();
        self.settings
            .update(cx, |store, cx| store.update(cx, change));
        let after = self.settings.read(cx).get().clone();
        if after == before {
            return;
        }
        if let Some(live) = &self.live {
            live.send(matty_sync::Command::SetPreferences {
                preferences: preferences(&after),
            });
        }
    }

    /// Takes the account's settings the server sent into the local file.
    pub(super) fn mirror_account_preferences(
        &mut self,
        preferences: &[Preference],
        cx: &mut Context<Self>,
    ) {
        self.settings
            .update(cx, |store, cx| store.update(cx, |s| mirror(preferences, s)));
    }

    /// Keeps the account's Mattermost theme for the team on screen current
    /// (#214): `preferences`, when sync sent them, hold every account
    /// preference, themes among them. Reinstalls the theme when the one to
    /// show changed and Preferences says to use it.
    pub(super) fn sync_server_theme(
        &mut self,
        preferences: Option<&[Preference]>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let signed_in = self.live.is_some();
        let changed = match &self.live {
            Some(live) => {
                let team = live.team().map(str::to_owned);
                ServerThemes::update(cx, preferences, team.as_deref())
            }
            None => ServerThemes::clear(cx),
        };
        // The theme is cached for the next start, and dropped on signing
        // out; writing settings reapplies the theme.
        if changed || !signed_in {
            let value = ServerThemes::current_value(cx).map(str::to_owned);
            if value != self.settings.read(cx).get().mattermost_theme {
                self.settings.update(cx, |store, cx| {
                    store.update(cx, |s| s.mattermost_theme = value)
                });
            }
        }
        if changed {
            crate::apply_theme(window, cx);
            cx.notify();
        }
    }

    /// Opens a section of Preferences.
    fn show_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.preferences_section != section {
            self.preferences_section = section;
            cx.notify();
        }
    }

    /// Adds the keywords typed in `list`'s field, then empties it (#192).
    pub(super) fn add_keywords(
        &mut self,
        list: matty_notify::KeywordList,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = match list {
            matty_notify::KeywordList::Mention => self.keywords.clone(),
            matty_notify::KeywordList::Highlight => self.highlight_keywords.clone(),
        };
        let typed = input.read(cx).value().to_string();
        if typed.trim().is_empty() {
            return;
        }
        let Some(props) = &self.notify_props else {
            return;
        };
        let next = matty_notify::mention::add_keywords(&self.username(cx), props, list, &typed);
        input.update(cx, |input, cx| input.set_value("", window, cx));
        self.write_notify_props(next, cx);
    }

    /// Takes `keyword` out of `list` (#192).
    fn remove_keyword(
        &mut self,
        list: matty_notify::KeywordList,
        keyword: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(props) = &self.notify_props else {
            return;
        };
        let next = matty_notify::mention::remove_keyword(props, list, keyword);
        self.write_notify_props(next, cx);
    }

    /// Writes one of the user's `notify_props` to the server (#158). The
    /// server sends the user back, which updates the policy's preferences.
    pub(super) fn set_notify_prop(&mut self, key: &str, value: &str, cx: &mut Context<Self>) {
        let Some(props) = &self.notify_props else {
            return;
        };
        if props.get(key).map(String::as_str) == Some(value) {
            return;
        }
        let mut next = props.clone();
        next.insert(key.to_owned(), value.to_owned());
        self.write_notify_props(next, cx);
    }

    /// Sends `next` as the user's `notify_props` when it changes them.
    fn write_notify_props(
        &mut self,
        next: std::collections::HashMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        if self.notify_props.as_ref() == Some(&next) {
            return;
        }
        if let Some(live) = &self.live {
            live.send(matty_sync::Command::SetNotifyProps {
                notify_props: next.clone(),
            });
        }
        self.notify_props = Some(next);
        cx.notify();
    }

    /// Turns collapsed reply threads on or off for the user (#157). The
    /// server's preferences come back and every count follows.
    fn set_collapsed_threads(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some((choice, current)) = self.collapsed_threads else {
            return;
        };
        if current == on {
            return;
        }
        if let Some(live) = &self.live {
            live.send(matty_sync::Command::SetCollapsedThreads { on });
        }
        self.collapsed_threads = Some((choice, on));
        cx.notify();
    }

    /// Asks for a folder and saves downloads there from now on (#160).
    fn choose_download_folder(&mut self, cx: &mut Context<Self>) {
        let answer = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("prefs-download-folder-choose")),
        });
        let settings = self.settings.clone();
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(paths))) = answer.await else {
                return;
            };
            let Some(folder) = paths.into_iter().next() else {
                return;
            };
            settings.update(cx, |store, cx| {
                store.update(cx, |s| s.download_folder = Some(folder))
            });
        })
        .detach();
    }

    /// Preferences as a sheet from the right edge: the section list, then
    /// the open section's controls.
    pub(super) fn preferences(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let view = cx.entity().downgrade();
        let list = section_list(
            self.preferences_section,
            move |section, _, cx| {
                view.update(cx, |this, cx| this.show_section(section, cx))
                    .ok();
            },
            cx,
        );
        let section = self.preferences_section;
        let controls = match section {
            Section::General => self.general(cx),
            Section::Appearance => self.appearance(cx),
            Section::Messages => self.messages(cx),
            Section::Notifications => self.notifications(cx),
            Section::Advanced => self.advanced(cx),
        };
        let handle = self.preferences.clone();
        let header = div()
            .flex()
            .items_start()
            .justify_between()
            .gap(theme.space(4.))
            .p(theme.space(4.))
            .border_b_1()
            .border_color(theme.colors.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space(1.5))
                    .child(drawer_title("preferences.title", cx).child(t!("prefs-title")))
                    .child(
                        drawer_description("preferences.description", cx)
                            .child(t!("prefs-description")),
                    ),
            )
            .child(
                Button::new("preferences.close")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Sm)
                    .label(t!("prefs-done"))
                    .on_click(move |_, window, cx| handle.close(window, cx)),
            );
        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(
                div()
                    .flex_shrink_0()
                    .w(scaled(&theme, SECTION_LIST_WIDTH))
                    .h_full()
                    .border_r_1()
                    .border_color(theme.colors.border)
                    .child(list),
            )
            .child(
                div().flex_1().min_w_0().h_full().child(
                    ScrollArea::new("preferences.body")
                        .aria_label(section.label())
                        .size_full()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(theme.space(5.))
                                .p(theme.space(6.))
                                .child(
                                    div()
                                        .id("preferences.section-title")
                                        .role(gpui_kit::Role::Heading)
                                        .aria_level(3)
                                        .type_style(style::TITLE, &theme)
                                        .text_color(theme.colors.foreground)
                                        .child(section.label()),
                                )
                                .child(controls),
                        ),
                ),
            );
        let width = window
            .viewport_size()
            .width
            .min(scaled(&theme, SHEET_WIDTH));
        Drawer::new("preferences", &self.preferences)
            .direction(DrawerSide::Right)
            .content(
                DrawerContent::new("preferences.panel", t!("prefs-title"))
                    .w(width)
                    .child(header)
                    .child(body),
            )
    }

    fn general(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = UiTheme::read(cx).clone();
        let settings = self.settings.read(cx).get().clone();
        let download_folder = settings
            .download_folder
            .clone()
            .or_else(dirs::download_dir)
            .map(|path| SharedString::from(path.display().to_string()))
            .unwrap_or_else(|| t!("prefs-download-folder-default"));
        let folder = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(theme.space(4.))
            .child(
                // A path can be long: one truncated line, the whole path
                // in a tooltip.
                div()
                    .id("preferences.download-folder.path")
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(theme.space(0.5))
                    .tooltip({
                        let path = download_folder.clone();
                        move |_, cx| crate::ui::tooltip::text_tooltip(path.clone(), cx)
                    })
                    .child(
                        div()
                            .type_style(style::UI, &theme)
                            .text_color(theme.colors.foreground)
                            .child(t!("prefs-download-folder")),
                    )
                    .child(
                        div()
                            .type_style(style::META, &theme)
                            .text_color(theme.colors.muted_foreground)
                            .truncate()
                            .child(download_folder),
                    ),
            )
            .child(
                Button::new("preferences.download-folder")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Sm)
                    .aria_label(t!("prefs-download-folder-change-label"))
                    .label(t!("prefs-download-folder-change"))
                    .on_click(cx.listener(|this, _, _, cx| this.choose_download_folder(cx))),
            );
        page(&theme)
            .child(group(
                t!("prefs-window"),
                stack(&theme)
                    // macOS keeps Matty in the dock when its window closes,
                    // so only Linux and Windows have a tray to keep it in.
                    .when(!cfg!(target_os = "macos"), |stack| {
                        stack.child(self.switch_row(
                            "preferences.close-to-tray",
                            t!("prefs-close-to-tray"),
                            t!("prefs-close-to-tray-detail"),
                            settings.close_to_tray,
                            |s, v| s.close_to_tray = v,
                            cx,
                        ))
                    })
                    .child(self.switch_row(
                        "preferences.launch-at-login",
                        t!("prefs-launch-at-login"),
                        t!("prefs-launch-at-login-detail"),
                        settings.launch_at_login,
                        |s, v| s.launch_at_login = v,
                        cx,
                    )),
                false,
                &theme,
            ))
            .child(group(t!("prefs-files"), folder, false, &theme))
            .into_any_element()
    }

    fn appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = UiTheme::read(cx).clone();
        let settings = self.settings.read(cx).get().clone();
        let store = self.settings.clone();
        let theme_value: SharedString = match settings.theme {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
        }
        .into();
        let theme_item =
            |id: &'static str, value: &'static str, glyph: LucideIcon, text: SharedString| {
                ToggleGroupItem::new(id, value)
                    .aria_label(text.clone())
                    .flex_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space(1.5))
                            .child(lucide(glyph).size(scaled(&theme, metrics::ICON_SMALL)))
                            .child(text),
                    )
            };
        let theme_store = store.clone();
        let theme_group = ToggleGroup::new("preferences.theme")
            .aria_label(t!("prefs-theme"))
            .joined(true)
            .w_full()
            .value([theme_value])
            .item(theme_item(
                "preferences.theme.system",
                "system",
                LucideIcon::Monitor,
                t!("prefs-theme-system"),
            ))
            .item(theme_item(
                "preferences.theme.light",
                "light",
                LucideIcon::Sun,
                t!("prefs-theme-light"),
            ))
            .item(theme_item(
                "preferences.theme.dark",
                "dark",
                LucideIcon::Moon,
                t!("prefs-theme-dark"),
            ))
            .on_change(move |next, _, cx| {
                let choice = match next.first().map(|v| v.as_ref()) {
                    Some("light") => ThemeChoice::Light,
                    Some("dark") => ThemeChoice::Dark,
                    _ => ThemeChoice::System,
                };
                theme_store.update(cx, |store, cx| store.update(cx, |s| s.theme = choice));
            });
        let source_store = store.clone();
        let colors_group = choices(
            "preferences.colors",
            t!("prefs-colors"),
            match settings.theme_source {
                ThemeSource::Matty => "matty",
                ThemeSource::Mattermost => "mattermost",
            },
            &[
                ("matty", t!("prefs-colors-matty")),
                ("mattermost", t!("prefs-colors-mattermost")),
            ],
            move |value, cx| {
                let source = match value {
                    "mattermost" => ThemeSource::Mattermost,
                    _ => ThemeSource::Matty,
                };
                source_store.update(cx, |store, cx| {
                    store.update(cx, |s| s.theme_source = source)
                });
            },
        );
        let imported = crate::mattermost_theme::shown(&settings, cx);
        let use_imported = settings.theme_source == ThemeSource::Mattermost;
        let colors_hint: SharedString = match (&imported, use_imported) {
            (_, false) => t!("prefs-colors-matty-hint"),
            (Some(imported), true) => {
                t!("prefs-colors-imported-hint", name = imported.name())
            }
            (None, true) => t!("prefs-colors-waiting-hint"),
        };
        let failing = imported
            .filter(|_| use_imported)
            .map(|imported| imported.warnings())
            .unwrap_or_default();
        let colors = hinted(colors_group, colors_hint, &theme)
            .when(!failing.is_empty(), |colors| {
                colors.child(contrast_warning(&failing, &theme))
            });
        let theme_group = if use_imported {
            hinted(theme_group, t!("prefs-theme-from-mattermost"), &theme)
        } else {
            div().child(theme_group)
        };
        let size_value: SharedString = TEXT_SIZES
            .iter()
            .min_by(|a, b| {
                (a.1 - settings.text_scale)
                    .abs()
                    .total_cmp(&(b.1 - settings.text_scale).abs())
            })
            .map(|(name, _)| *name)
            .unwrap_or("default")
            .into();
        let size_store = store.clone();
        let mut size_group = ToggleGroup::new("preferences.text-size")
            .aria_label(t!("prefs-text-size"))
            .joined(true)
            .w_full()
            .value([size_value]);
        for (name, scale) in TEXT_SIZES {
            let text = format!("{}%", (scale * 100.).round());
            size_group = size_group.item(
                ToggleGroupItem::new(
                    gpui_kit::ElementId::Name(format!("preferences.text-size.{name}").into()),
                    *name,
                )
                .aria_label(t!("prefs-text-size-option", size = text.clone()))
                .flex_1()
                .child(text),
            );
        }
        let size_group = size_group.on_change(move |next, _, cx| {
            let scale = TEXT_SIZES
                .iter()
                .find(|(name, _)| next.first().map(|v| v.as_ref()) == Some(*name))
                .map(|(_, scale)| *scale)
                .unwrap_or(1.);
            size_store.update(cx, |store, cx| store.update(cx, |s| s.text_scale = scale));
        });
        let view = cx.entity().downgrade();
        let display_group = choices(
            "preferences.display",
            t!("prefs-display"),
            match settings.display {
                MessageDisplay::Standard => "standard",
                MessageDisplay::Compact => "compact",
            },
            &[
                ("standard", t!("prefs-display-standard")),
                ("compact", t!("prefs-display-compact")),
            ],
            {
                let view = view.clone();
                move |value, cx| {
                    let display = match value {
                        "compact" => MessageDisplay::Compact,
                        _ => MessageDisplay::Standard,
                    };
                    view.update(cx, |this, cx| {
                        this.set_synced(
                            |s| s.display = display,
                            |s| display_preference(s.display),
                            cx,
                        )
                    })
                    .ok();
                }
            },
        );
        let display_hint = match settings.display {
            MessageDisplay::Standard => t!("prefs-display-standard-hint"),
            MessageDisplay::Compact => t!("prefs-display-compact-hint"),
        };
        let clock_group = choices(
            "preferences.clock",
            t!("prefs-clock"),
            match settings.clock {
                ClockFormat::TwelveHour => "12",
                ClockFormat::TwentyFourHour => "24",
            },
            &[("12", t!("prefs-clock-12")), ("24", t!("prefs-clock-24"))],
            {
                let view = view.clone();
                move |value, cx| {
                    let clock = match value {
                        "12" => ClockFormat::TwelveHour,
                        _ => ClockFormat::TwentyFourHour,
                    };
                    view.update(cx, |this, cx| {
                        this.set_synced(|s| s.clock = clock, |s| clock_preference(s.clock), cx)
                    })
                    .ok();
                }
            },
        );
        let names_group = choices(
            "preferences.names",
            t!("prefs-names-label"),
            match settings.name_format {
                NameFormat::NicknameFullName => "nickname",
                NameFormat::FullName => "full",
                NameFormat::Username => "username",
            },
            &[
                ("nickname", t!("prefs-names-nickname")),
                ("full", t!("prefs-names-full")),
                ("username", t!("prefs-names-username")),
            ],
            move |value, cx| {
                let format = match value {
                    "full" => NameFormat::FullName,
                    "username" => NameFormat::Username,
                    _ => NameFormat::NicknameFullName,
                };
                view.update(cx, |this, cx| {
                    this.set_synced(
                        |s| s.name_format = format,
                        |s| name_format_preference(s.name_format),
                        cx,
                    )
                })
                .ok();
            },
        );
        let names_hint = match settings.name_format {
            NameFormat::NicknameFullName => t!("prefs-names-nickname-hint"),
            NameFormat::FullName => t!("prefs-names-full-hint"),
            NameFormat::Username => t!("prefs-names-username-hint"),
        };
        page(&theme)
            .child(group(t!("prefs-theme"), theme_group, false, &theme))
            .child(group(t!("prefs-colors"), colors, false, &theme))
            .child(group(t!("prefs-text-size"), size_group, false, &theme))
            .child(group(
                t!("prefs-display"),
                hinted(display_group, display_hint, &theme),
                true,
                &theme,
            ))
            .child(group(t!("prefs-clock"), clock_group, true, &theme))
            .child(group(
                t!("prefs-names"),
                hinted(names_group, names_hint, &theme),
                true,
                &theme,
            ))
            .child(group(
                t!("prefs-motion"),
                self.switch_row(
                    "preferences.reduce-motion",
                    t!("prefs-reduce-motion"),
                    t!("prefs-reduce-motion-detail"),
                    settings.reduce_motion,
                    |s, v| s.reduce_motion = v,
                    cx,
                ),
                false,
                &theme,
            ))
            .into_any_element()
    }

    fn messages(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = UiTheme::read(cx).clone();
        let settings = self.settings.read(cx).get().clone();
        let mod_enter = match Platform::current() {
            Platform::MacOs => t!("prefs-send-cmd-enter"),
            Platform::Other => t!("prefs-send-ctrl-enter"),
        };
        let view = cx.entity().downgrade();
        let send_group = choices(
            "preferences.send",
            t!("prefs-send"),
            match settings.send_on {
                SendKey::Enter => "enter",
                SendKey::ModEnter => "mod-enter",
                SendKey::CodeBlocks => "code-blocks",
            },
            &[
                ("enter", t!("prefs-send-enter")),
                ("mod-enter", mod_enter.clone()),
                ("code-blocks", t!("prefs-send-code-blocks")),
            ],
            {
                let view = view.clone();
                move |value, cx| {
                    let send_on = match value {
                        "mod-enter" => SendKey::ModEnter,
                        "code-blocks" => SendKey::CodeBlocks,
                        _ => SendKey::Enter,
                    };
                    view.update(cx, |this, cx| {
                        this.set_synced(
                            |s| s.send_on = send_on,
                            |s| send_key_preferences(s.send_on),
                            cx,
                        )
                    })
                    .ok();
                }
            },
        );
        let send_hint: SharedString = match settings.send_on {
            SendKey::Enter => t!("prefs-send-enter-hint"),
            SendKey::ModEnter => t!("prefs-send-mod-enter-hint"),
            SendKey::CodeBlocks => t!("prefs-send-code-blocks-hint", key = mod_enter.to_string()),
        };
        let previews = {
            let view = view.clone();
            group(
                t!("prefs-link-previews"),
                labelled_row(
                    t!("prefs-collapse-previews"),
                    t!("prefs-collapse-previews-detail"),
                    Switch::new("preferences.collapse-previews")
                        .checked(settings.collapse_previews)
                        .aria_label(t!("prefs-collapse-previews"))
                        .on_change(move |next, _, _, cx| {
                            view.update(cx, |this, cx| {
                                this.set_synced(
                                    |s| s.collapse_previews = next,
                                    |s| collapse_previews_preference(s.collapse_previews),
                                    cx,
                                )
                            })
                            .ok();
                        })
                        .into_any_element(),
                    &theme,
                ),
                true,
                &theme,
            )
        };
        // Local to this device, unlike the synced settings around it (#159).
        let images_group = {
            let store = self.settings.clone();
            choices(
                "preferences.remote-images",
                t!("prefs-remote-images"),
                match settings.load_remote_images {
                    RemoteImages::Always => "always",
                    RemoteImages::Ask => "ask",
                },
                &[
                    ("always", t!("prefs-remote-images-always")),
                    ("ask", t!("prefs-remote-images-ask")),
                ],
                move |value, cx| {
                    let choice = match value {
                        "ask" => RemoteImages::Ask,
                        _ => RemoteImages::Always,
                    };
                    store.update(cx, |store, cx| {
                        store.update(cx, |s| s.load_remote_images = choice)
                    });
                },
            )
        };
        let images_hint = match settings.load_remote_images {
            RemoteImages::Always => t!("prefs-remote-images-always-hint"),
            RemoteImages::Ask => t!("prefs-remote-images-ask-hint"),
        };
        let threads = self
            .collapsed_threads
            .filter(|(choice, _)| *choice)
            .map(|(_, on)| {
                group(
                    t!("prefs-threads"),
                    labelled_row(
                        t!("prefs-collapsed-threads"),
                        t!("prefs-collapsed-threads-detail"),
                        Switch::new("preferences.collapsed-threads")
                            .checked(on)
                            .aria_label(t!("prefs-collapsed-threads"))
                            .on_change(move |next, _, _, cx| {
                                view.update(cx, |this, cx| this.set_collapsed_threads(next, cx))
                                    .ok();
                            })
                            .into_any_element(),
                        &theme,
                    ),
                    true,
                    &theme,
                )
            });
        page(&theme)
            .child(group(
                t!("prefs-send"),
                hinted(send_group, send_hint, &theme),
                true,
                &theme,
            ))
            .child(previews)
            .child(group(
                t!("prefs-remote-images"),
                hinted(images_group, images_hint, &theme),
                false,
                &theme,
            ))
            .children(threads)
            .into_any_element()
    }

    /// The Notifications section, from the user's `notify_props` with
    /// Mattermost's defaults (#158).
    fn notifications(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = UiTheme::read(cx).clone();
        let Some(props) = &self.notify_props else {
            return div()
                .type_style(style::META, &theme)
                .text_color(theme.colors.muted_foreground)
                .child(t!("prefs-notifications-offline"))
                .into_any_element();
        };
        let user = matty_notify::UserPrefs::from_props(&self.username(cx), "", props);
        let level: SharedString = match user.desktop {
            matty_notify::Level::All => "all",
            matty_notify::Level::Mention => "mention",
            matty_notify::Level::None => "none",
        }
        .into();
        let view = cx.entity().downgrade();
        let item = |id: &'static str, value: &'static str, text: SharedString| {
            ToggleGroupItem::new(id, value)
                .aria_label(text.clone())
                .flex_1()
                .child(text)
        };
        let desktop = ToggleGroup::new("preferences.desktop")
            .aria_label(t!("prefs-desktop-label"))
            .joined(true)
            .w_full()
            .value([level])
            .item(item(
                "preferences.desktop.all",
                "all",
                t!("prefs-desktop-all"),
            ))
            .item(item(
                "preferences.desktop.mention",
                "mention",
                t!("prefs-desktop-mention"),
            ))
            .item(item(
                "preferences.desktop.none",
                "none",
                t!("prefs-desktop-none"),
            ))
            .on_change({
                let view = view.clone();
                move |next, _, cx| {
                    let value = next.first().map(|v| v.to_string());
                    view.update(cx, |this, cx| {
                        this.set_notify_prop("desktop", value.as_deref().unwrap_or("mention"), cx)
                    })
                    .ok();
                }
            });
        let on = |key: &str, default: bool| props.get(key).map_or(default, |v| v == "true");
        let switch = |id: &'static str,
                      title: SharedString,
                      detail: SharedString,
                      checked: bool,
                      key: &'static str,
                      values: (&'static str, &'static str)| {
            let view = view.clone();
            labelled_row(
                title.clone(),
                detail,
                Switch::new(id)
                    .checked(checked)
                    .aria_label(title)
                    .on_change(move |next, _, _, cx| {
                        let value = if next { values.0 } else { values.1 };
                        view.update(cx, |this, cx| this.set_notify_prop(key, value, cx))
                            .ok();
                    })
                    .into_any_element(),
                &theme,
            )
        };
        let first_name = self.account.read(cx).account().display_name.clone();
        let first_name = first_name
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_owned();
        let first_name_detail: SharedString = if first_name.is_empty() {
            t!("prefs-first-name-detail")
        } else {
            t!("prefs-first-name-named", name = first_name)
        };
        page(&theme)
            .child(group(t!("prefs-status"), self.dnd_row(cx), true, &theme))
            .child(group(
                t!("prefs-desktop"),
                stack(&theme)
                    .child(desktop)
                    .child(switch(
                        "preferences.desktop-threads",
                        t!("prefs-desktop-threads"),
                        t!("prefs-desktop-threads-detail"),
                        user.desktop_threads == matty_notify::Level::All,
                        "desktop_threads",
                        ("all", "mention"),
                    ))
                    .child(switch(
                        "preferences.desktop-sound",
                        t!("prefs-sound"),
                        t!("prefs-sound-detail"),
                        user.desktop_sound,
                        "desktop_sound",
                        ("true", "false"),
                    ))
                    .child(labelled_row(
                        t!("prefs-notification-sound"),
                        t!("prefs-notification-sound-detail"),
                        div()
                            .flex_shrink_0()
                            .w(scaled(&theme, metrics::FIELD_NARROW_WIDTH))
                            .child(
                                Select::new(&self.sound)
                                    .aria_label(t!("prefs-notification-sound"))
                                    .disabled(!user.desktop_sound),
                            )
                            .into_any_element(),
                        &theme,
                    )),
                true,
                &theme,
            ))
            .child(group(
                t!("prefs-mentions"),
                stack(&theme)
                    .child(switch(
                        "preferences.first-name",
                        t!("prefs-first-name"),
                        first_name_detail,
                        on("first_name", false),
                        "first_name",
                        ("true", "false"),
                    ))
                    .child(switch(
                        "preferences.channel-wide",
                        t!("prefs-channel-wide"),
                        t!("prefs-channel-wide-detail"),
                        on("channel", true),
                        "channel",
                        ("true", "false"),
                    ))
                    .child(self.keyword_editor(matty_notify::KeywordList::Mention, props, cx)),
                true,
                &theme,
            ))
            .when(self.highlight_keys, |page| {
                page.child(group(
                    t!("prefs-highlights"),
                    stack(&theme).child(self.keyword_editor(
                        matty_notify::KeywordList::Highlight,
                        props,
                        cx,
                    )),
                    true,
                    &theme,
                ))
            })
            .into_any_element()
    }

    /// Do not disturb as the status menu set it, with a switch (#158). On
    /// turns it on until the user turns it off; a set time is the status
    /// menu's job, with its durations and custom picker (#190).
    fn dnd_row(&self, cx: &mut Context<Self>) -> Div {
        let theme = UiTheme::read(cx).clone();
        let account = self.account.read(cx).account();
        let dnd = account.presence == Presence::Dnd;
        let now = jiff::Zoned::now();
        let on = dnd::is_on(dnd, account.dnd_end, &now);
        let detail = dnd::preferences_detail(dnd, account.dnd_end, &now, ClockFormat::current(cx));
        let (view, account) = (cx.entity().downgrade(), self.account.clone());
        labelled_row(
            t!("prefs-dnd"),
            detail,
            Switch::new("preferences.dnd")
                .checked(on)
                .aria_label(t!("prefs-dnd"))
                .on_change(move |next, _, _, cx| {
                    let presence = if next {
                        Presence::Dnd
                    } else {
                        Presence::Online
                    };
                    account.update(cx, |account, cx| account.set_presence(presence, None, cx));
                    view.update(cx, |_, cx| cx.notify()).ok();
                })
                .into_any_element(),
            &theme,
        )
    }

    /// A keyword list in Preferences: its field and its tags (#192).
    fn keyword_editor(
        &self,
        list: matty_notify::KeywordList,
        props: &std::collections::HashMap<String, String>,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = UiTheme::read(cx).clone();
        let keywords = matty_notify::mention::keywords(&self.username(cx), props, list);
        let (id, label, description, input) = match list {
            matty_notify::KeywordList::Mention => (
                "preferences.keywords",
                t!("prefs-keywords"),
                t!("prefs-keywords-detail"),
                &self.keywords,
            ),
            matty_notify::KeywordList::Highlight => (
                "preferences.highlight-keywords",
                t!("prefs-highlight-keywords"),
                t!("prefs-highlight-keywords-detail"),
                &self.highlight_keywords,
            ),
        };
        let view = cx.entity().downgrade();
        keyword_editor(
            KeywordEditor {
                id,
                label,
                description,
            },
            input,
            keywords,
            move |keyword, cx| {
                view.update(cx, |this, cx| this.remove_keyword(list, keyword, cx))
                    .ok();
            },
            &theme,
        )
    }

    fn advanced(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = UiTheme::read(cx).clone();
        let store = self.settings.clone();
        // The updater reads the channel at each check (#180).
        let channel_group = choices(
            "preferences.update-channel",
            t!("prefs-update-channel"),
            match self.settings.read(cx).get().update_channel {
                UpdateChannel::Stable => "stable",
                UpdateChannel::Beta => "beta",
            },
            &[
                ("stable", t!("prefs-update-stable")),
                ("beta", t!("prefs-update-beta")),
            ],
            move |value, cx| {
                let channel = match value {
                    "beta" => UpdateChannel::Beta,
                    _ => UpdateChannel::Stable,
                };
                store.update(cx, |store, cx| {
                    store.update(cx, |s| s.update_channel = channel)
                });
            },
        );
        let channel_hint = match self.settings.read(cx).get().update_channel {
            UpdateChannel::Stable => t!("prefs-update-stable-hint"),
            UpdateChannel::Beta => t!("prefs-update-beta-hint"),
        };
        let logs = group(
            t!("prefs-logs"),
            stack(&theme)
                .child(labelled_row(
                    t!("prefs-log-detail"),
                    t!("prefs-log-detail-hint"),
                    div()
                        .flex_shrink_0()
                        .w(scaled(&theme, metrics::FIELD_NARROW_WIDTH))
                        .child(Select::new(&self.log_level).aria_label(t!("prefs-log-detail")))
                        .into_any_element(),
                    &theme,
                ))
                .child(labelled_row(
                    t!("prefs-logs"),
                    t!("prefs-logs-detail"),
                    Button::new("preferences.open-logs")
                        .variant(ButtonVariant::Outline)
                        .size(ButtonSize::Sm)
                        .aria_label(t!("prefs-open-logs-label"))
                        .label(t!("prefs-open-logs"))
                        .on_click(|_, _, cx| {
                            if let Some(dir) = crate::logging::default_dir() {
                                let _ = std::fs::create_dir_all(&dir);
                                cx.open_with_system(&dir);
                            }
                        })
                        .into_any_element(),
                    &theme,
                )),
            false,
            &theme,
        );
        let sign_out = labelled_row(
            t!("prefs-sign-out"),
            t!("prefs-sign-out-detail"),
            Button::new("preferences.sign-out")
                .variant(ButtonVariant::Destructive)
                .size(ButtonSize::Sm)
                .aria_label(t!("prefs-sign-out"))
                .label(t!("prefs-sign-out-button"))
                .on_click(cx.listener(|this, _, window, cx| {
                    // The confirmation is the account menu's own, over the
                    // window rather than over this sheet.
                    this.preferences.close(window, cx);
                    let dialog = this.account.read(cx).sign_out_dialog_handle();
                    dialog.open(window, cx);
                }))
                .into_any_element(),
            &theme,
        );
        page(&theme)
            .child(group(
                t!("prefs-updates"),
                hinted(channel_group, channel_hint, &theme),
                false,
                &theme,
            ))
            .child(logs.into_any_element())
            .child(group(t!("prefs-account"), sign_out, false, &theme))
            .into_any_element()
    }

    /// A switch for a local setting, with its title and one line on it.
    fn switch_row(
        &self,
        id: &'static str,
        title: SharedString,
        detail: SharedString,
        checked: bool,
        write: fn(&mut Settings, bool),
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = UiTheme::read(cx).clone();
        let store = self.settings.clone();
        labelled_row(
            title.clone(),
            detail,
            Switch::new(id)
                .checked(checked)
                .aria_label(title)
                .on_change(move |next, _, _, cx| {
                    store.update(cx, |store, cx| store.update(cx, |s| write(s, next)))
                })
                .into_any_element(),
            &theme,
        )
    }
}

/// The section list's width, in px at zoom 1.
const SECTION_LIST_WIDTH: f32 = 184.;
/// The sheet's width, in px at zoom 1, within the window.
const SHEET_WIDTH: f32 = 760.;

/// What a keyword editor says about its list.
struct KeywordEditor {
    id: &'static str,
    label: SharedString,
    description: SharedString,
}

/// A keyword list as removable tags under the field that adds more
/// (#192). Each tag's remove button calls `on_remove` with its keyword.
fn keyword_editor(
    editor: KeywordEditor,
    input: &gpui_kit::Entity<crate::ui::input::InputState>,
    keywords: Vec<String>,
    on_remove: impl Fn(&str, &mut App) + 'static,
    theme: &UiTheme,
) -> Div {
    let on_remove = std::rc::Rc::new(on_remove);
    let tags = keywords.into_iter().map(|keyword| {
        let on_remove = on_remove.clone();
        // Keywords are unique ignoring case, so the keyword names its tag.
        let tag_id: SharedString = format!("{}.{}", editor.id, keyword.to_lowercase()).into();
        let removed = keyword.clone();
        div()
            .debug_selector({
                let tag_id = tag_id.clone();
                move || tag_id.to_string()
            })
            .child(Tag::new(tag_id, keyword).on_remove(move |_, _, cx| on_remove(&removed, cx)))
    });
    stack(theme)
        .child(
            Field::new(editor.id, input)
                .label(editor.label)
                .description(editor.description),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(theme.space(1.5))
                .children(tags),
        )
}

fn stack(theme: &UiTheme) -> Div {
    div().flex().flex_col().gap(theme.space(3.))
}

/// A section's groups, one under the other.
fn page(theme: &UiTheme) -> Div {
    div().flex().flex_col().gap(theme.space(6.))
}

/// A titled group of controls; `synced` adds the line saying the account
/// keeps the setting.
fn group(title: SharedString, control: impl IntoElement, synced: bool, theme: &UiTheme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(theme.space(2.))
        .child(
            div()
                .type_style(style::LABEL, theme)
                .text_color(theme.colors.muted_foreground)
                .child(title.to_uppercase()),
        )
        .child(control)
        .when(synced, |group| group.child(synced_hint(theme)))
}

/// "Synced with your Mattermost account", under a server-backed setting.
fn synced_hint(theme: &UiTheme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(theme.space(1.))
        .type_style(style::META, theme)
        .text_color(theme.colors.muted_foreground)
        .child(
            lucide(LucideIcon::Cloud)
                .size(scaled(theme, metrics::ICON_TINY))
                .text_color(theme.colors.muted_foreground),
        )
        .child(t!("prefs-synced"))
}

/// A control with one muted line under it.
fn hinted(control: impl IntoElement, hint: impl Into<SharedString>, theme: &UiTheme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(theme.space(2.))
        .child(control)
        .child(
            div()
                .type_style(style::META, theme)
                .text_color(theme.colors.muted_foreground)
                .child(hint.into()),
        )
}

/// Says that some of a Mattermost theme's color pairs fall below WCAG AA
/// (#214, PLAN §8): a destructive tag, then how many and the three that
/// miss by most, with their ratios.
fn contrast_warning(failing: &[crate::contrast::Pair], theme: &UiTheme) -> Div {
    let mut worst: Vec<_> = failing.iter().collect();
    worst.sort_by(|a, b| (a.ratio() / a.kind.minimum()).total_cmp(&(b.ratio() / b.kind.minimum())));
    let named: Vec<String> = worst
        .iter()
        .take(3)
        .map(|pair| {
            t!(
                "prefs-contrast-pair",
                name = pair.label().to_string(),
                ratio = format!("{:.1}", pair.ratio()),
            )
            .to_string()
        })
        .collect();
    let examples = match named.as_slice() {
        [one] => one.clone(),
        [rest @ .., last] => t!(
            "prefs-list-and",
            rest = rest.join(&*t!("prefs-list-separator")),
            last = last.clone(),
        )
        .to_string(),
        [] => String::new(),
    };
    div()
        .flex()
        .flex_col()
        .items_start()
        .gap(theme.space(1.5))
        .child(
            Tag::new("preferences.colors.contrast", t!("prefs-contrast-tag"))
                .variant(TagVariant::Destructive)
                .icon(LucideIcon::TriangleAlert),
        )
        .child(
            div()
                .type_style(style::META, theme)
                .text_color(theme.colors.muted_foreground)
                .child(t!(
                    "prefs-contrast-warning",
                    count = failing.len(),
                    examples = examples,
                )),
        )
}

/// A title and one line on the left, its control on the right.
fn labelled_row(
    title: SharedString,
    detail: SharedString,
    control: AnyElement,
    theme: &UiTheme,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(theme.space(4.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(theme.space(0.5))
                .child(
                    div()
                        .type_style(style::UI, theme)
                        .text_color(theme.colors.foreground)
                        .child(title),
                )
                .child(
                    div()
                        .type_style(style::META, theme)
                        .text_color(theme.colors.muted_foreground)
                        .child(detail),
                ),
        )
        .child(control)
}

/// A joined toggle group of `options` (value, text) with `value` chosen;
/// `change` gets the value picked.
fn choices(
    id: &'static str,
    label: SharedString,
    value: &'static str,
    options: &[(&'static str, SharedString)],
    change: impl Fn(&str, &mut App) + 'static,
) -> ToggleGroup {
    let mut group = ToggleGroup::new(id)
        .aria_label(label)
        .joined(true)
        .w_full()
        .value([SharedString::from(value)]);
    for (option, text) in options {
        group = group.item(
            ToggleGroupItem::new(
                gpui_kit::ElementId::Name(format!("{id}.{option}").into()),
                *option,
            )
            .aria_label(text.clone())
            .flex_1()
            .child(text.clone()),
        );
    }
    group.on_change(move |next, _, cx| {
        if let Some(value) = next.first() {
            change(value, cx);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(pairs: &[(&str, &str, &str)]) -> Vec<Preference> {
        pairs
            .iter()
            .map(|(category, name, value)| preference(category, name, value))
            .collect()
    }

    #[test]
    fn the_accounts_settings_come_into_the_local_file() {
        let mut settings = Settings::default();
        mirror(
            &server(&[
                (DISPLAY, "message_display", "compact"),
                (DISPLAY, "use_military_time", "false"),
                (DISPLAY, "name_format", "username"),
                (DISPLAY, "collapse_previews", "true"),
                (ADVANCED, "send_on_ctrl_enter", "true"),
            ]),
            &mut settings,
        );
        assert_eq!(settings.display, MessageDisplay::Compact);
        assert_eq!(settings.clock, ClockFormat::TwelveHour);
        assert_eq!(settings.name_format, NameFormat::Username);
        assert_eq!(settings.send_on, SendKey::ModEnter);
        assert!(settings.collapse_previews);
        assert_eq!(
            collapse_previews_preference(false)[0].value,
            "false",
            "the switch writes the same preference back"
        );

        mirror(
            &server(&[
                (DISPLAY, "message_display", "clean"),
                (DISPLAY, "name_format", "full_name"),
                (ADVANCED, "send_on_ctrl_enter", "false"),
                (ADVANCED, "code_block_ctrl_enter", "true"),
            ]),
            &mut settings,
        );
        assert_eq!(settings.display, MessageDisplay::Standard);
        assert_eq!(settings.name_format, NameFormat::FullName);
        assert_eq!(settings.send_on, SendKey::CodeBlocks);
        assert_eq!(
            settings.clock,
            ClockFormat::TwelveHour,
            "a preference not sent keeps the last value"
        );
    }

    #[test]
    fn settings_never_set_on_the_server_stay_local() {
        let mut settings = Settings {
            display: MessageDisplay::Compact,
            send_on: SendKey::ModEnter,
            ..Settings::default()
        };
        let before = settings.clone();
        mirror(
            &server(&[("favorite_channel", "c1", "true")]),
            &mut settings,
        );
        assert_eq!(settings, before);
        // The web app turns Ctrl+Enter on for code blocks unless told not to.
        mirror(
            &server(&[(ADVANCED, "send_on_ctrl_enter", "false")]),
            &mut settings,
        );
        assert_eq!(settings.send_on, SendKey::CodeBlocks);
    }

    #[test]
    fn every_choice_round_trips_through_the_server() {
        for display in [MessageDisplay::Standard, MessageDisplay::Compact] {
            let mut settings = Settings::default();
            mirror(&display_preference(display), &mut settings);
            assert_eq!(settings.display, display);
        }
        for clock in [ClockFormat::TwelveHour, ClockFormat::TwentyFourHour] {
            let mut settings = Settings::default();
            mirror(&clock_preference(clock), &mut settings);
            assert_eq!(settings.clock, clock);
        }
        for format in [
            NameFormat::Username,
            NameFormat::NicknameFullName,
            NameFormat::FullName,
        ] {
            let mut settings = Settings::default();
            mirror(&name_format_preference(format), &mut settings);
            assert_eq!(settings.name_format, format);
        }
        for send_on in [SendKey::Enter, SendKey::ModEnter, SendKey::CodeBlocks] {
            let mut settings = Settings::default();
            mirror(&send_key_preferences(send_on), &mut settings);
            assert_eq!(settings.send_on, send_on);
        }
    }

    #[test]
    fn the_wire_values_are_mattermosts() {
        let values = |preferences: Vec<Preference>| -> Vec<(String, String, String)> {
            preferences
                .into_iter()
                .map(|p| (p.category, p.name, p.value))
                .collect()
        };
        assert_eq!(
            values(display_preference(MessageDisplay::Standard)),
            [(DISPLAY.into(), "message_display".into(), "clean".into())]
        );
        assert_eq!(
            values(name_format_preference(NameFormat::NicknameFullName)),
            [(
                DISPLAY.into(),
                "name_format".into(),
                "nickname_full_name".into()
            )]
        );
        assert_eq!(
            values(send_key_preferences(SendKey::CodeBlocks)),
            [
                (ADVANCED.into(), "send_on_ctrl_enter".into(), "false".into()),
                (
                    ADVANCED.into(),
                    "code_block_ctrl_enter".into(),
                    "true".into()
                ),
            ]
        );
    }

    #[test]
    fn the_sections_are_the_issues_five_in_order() {
        let labels: Vec<_> = Section::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(
            labels,
            [
                "General",
                "Appearance",
                "Messages",
                "Notifications",
                "Advanced"
            ]
        );
        assert_eq!(Section::default(), Section::General);
    }

    /// Hosts the section list the way Preferences does, after a control
    /// that has focus first.
    struct Probe {
        selected: Section,
        before: gpui_kit::FocusHandle,
    }

    impl gpui_kit::Render for Probe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let view = cx.entity().downgrade();
            div()
                .size_full()
                .flex()
                .child(
                    div()
                        .id("before")
                        .track_focus(&self.before.clone().tab_stop(true))
                        .child("Before"),
                )
                .child(
                    div()
                        .w(gpui_kit::px(200.))
                        .h(gpui_kit::px(300.))
                        .child(section_list(
                            self.selected,
                            move |section, _, cx| {
                                view.update(cx, |probe, cx| {
                                    probe.selected = section;
                                    cx.notify();
                                })
                                .ok();
                            },
                            cx,
                        )),
                )
        }
    }

    #[gpui_kit::test]
    fn the_keyboard_walks_the_section_list(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let window = cx.add_window(|window, cx| {
            window.activate_window();
            Probe {
                selected: Section::General,
                before: cx.focus_handle(),
            }
        });
        let probe = window.root(cx).unwrap();
        let mut cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| {
            let before = probe.read(cx).before.clone();
            before.focus(window, cx);
            window.draw(cx).clear(cx);
        });
        let mut press = |keys: &'static str| {
            for key in keys.split(' ') {
                let keystroke = gpui_kit::Keystroke::parse(key).unwrap();
                cx.simulate_event(gpui_kit::KeyDownEvent {
                    keystroke: keystroke.clone(),
                    is_held: false,
                    prefer_character_input: false,
                });
                cx.simulate_event(gpui_kit::KeyUpEvent { keystroke });
                cx.update(|window, cx| window.draw(cx).clear(cx));
            }
            cx.update(|_, cx| probe.read(cx).selected)
        };
        // Tab lands on the selected row; Down moves on and Enter opens it.
        assert_eq!(press("tab down enter"), Section::Appearance);
        assert_eq!(press("down down enter"), Section::Notifications);
        // Up from the first row wraps to the last.
        assert_eq!(press("home up enter"), Section::Advanced);
    }

    /// Hosts a keyword editor the way the Notifications section does.
    struct Keywords {
        input: gpui_kit::Entity<crate::ui::input::InputState>,
        keywords: Vec<String>,
    }

    impl gpui_kit::Render for Keywords {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = UiTheme::read(cx).clone();
            let view = cx.entity().downgrade();
            div().w(gpui_kit::px(600.)).child(keyword_editor(
                KeywordEditor {
                    id: "preferences.keywords",
                    label: "Keywords".into(),
                    description: "Words that count as mentioning you.".into(),
                },
                &self.input,
                self.keywords.clone(),
                move |keyword, cx| {
                    let keyword = keyword.to_owned();
                    view.update(cx, |this, cx| {
                        this.keywords.retain(|k| *k != keyword);
                        cx.notify();
                    })
                    .ok();
                },
                &theme,
            ))
        }
    }

    #[gpui_kit::test]
    fn a_keyword_tag_removes_its_keyword(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let window = cx.add_window(|window, cx| Keywords {
            input: gpui_kit::AppContext::new(cx, |cx| {
                crate::ui::input::InputState::new(window, cx)
            }),
            keywords: vec!["Deploy".into(), "on-call".into()],
        });
        let probe = window.root(cx).unwrap();
        let mut cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let deploy = cx
            .debug_bounds("preferences.keywords.deploy")
            .expect("a tag per keyword");
        let on_call = cx.debug_bounds("preferences.keywords.on-call").unwrap();
        assert!(deploy.origin.x < on_call.origin.x, "in the list's order");
        // The remove button ends the tag.
        let remove = gpui_kit::point(
            deploy.origin.x + deploy.size.width - gpui_kit::px(8.),
            deploy.center().y,
        );
        cx.simulate_click(remove, Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(
            cx.update(|_, cx| probe.read(cx).keywords.clone()),
            ["on-call"]
        );
        assert!(cx.debug_bounds("preferences.keywords.deploy").is_none());
    }
}
