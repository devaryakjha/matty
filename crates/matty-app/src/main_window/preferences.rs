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
use crate::settings::{ClockFormat, MessageDisplay, NameFormat, SendKey, Settings, ThemeChoice};
use crate::style::{self, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    button::{Button, ButtonSize, ButtonVariant},
    drawer::{Drawer, DrawerContent, DrawerSide, drawer_description, drawer_title},
    field::Field,
    scroll_area::ScrollArea,
    select::Select,
    sidebar::{Sidebar, SidebarItem},
    switch::Switch,
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
    /// Logs.
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

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Messages => "Messages",
            Self::Notifications => "Notifications",
            Self::Advanced => "Advanced",
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
    let mut list = Sidebar::new("preferences.sections", "Preferences sections").roving_focus(true);
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
    fn set_synced(
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

    /// Opens a section of Preferences.
    fn show_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.preferences_section != section {
            self.preferences_section = section;
            cx.notify();
        }
    }

    /// Sends the typed keywords when they differ from the server's.
    pub(super) fn save_keywords(&mut self, cx: &mut Context<Self>) {
        let (Some(live), Some(props)) = (&self.live, &self.notify_props) else {
            return;
        };
        let typed = self.keywords.read(cx).value().to_string();
        let next = matty_notify::mention::with_keywords(&self.username(cx), props, &typed);
        if next != *props {
            live.send(matty_sync::Command::SetNotifyProps {
                notify_props: next.clone(),
            });
            self.notify_props = Some(next);
        }
    }

    /// Writes one of the user's `notify_props` to the server (#158). The
    /// server sends the user back, which updates the policy's preferences.
    fn set_notify_prop(&mut self, key: &str, value: &str, cx: &mut Context<Self>) {
        let Some(props) = &self.notify_props else {
            return;
        };
        if props.get(key).map(String::as_str) == Some(value) {
            return;
        }
        let mut next = props.clone();
        next.insert(key.to_owned(), value.to_owned());
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
            prompt: Some("Choose".into()),
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
                    .child(drawer_title("preferences.title", cx).child("Preferences"))
                    .child(
                        drawer_description("preferences.description", cx)
                            .child("Changes apply right away and are saved for next time."),
                    ),
            )
            .child(
                Button::new("preferences.close")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Sm)
                    .label("Done")
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
                DrawerContent::new("preferences.panel", "Preferences")
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
            .unwrap_or_else(|| "Your Downloads folder".into());
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
                            .child("Download folder"),
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
                    .aria_label("Change download folder")
                    .label("Change…")
                    .on_click(cx.listener(|this, _, _, cx| this.choose_download_folder(cx))),
            );
        page(&theme)
            .child(group(
                "Window",
                stack(&theme)
                    .child(self.switch_row(
                        "preferences.close-to-tray",
                        "Keep running when closed",
                        "Closing the window leaves Matty in the tray.",
                        settings.close_to_tray,
                        |s, v| s.close_to_tray = v,
                        cx,
                    ))
                    .child(self.switch_row(
                        "preferences.launch-at-login",
                        "Open at login",
                        "Start Matty when you sign in.",
                        settings.launch_at_login,
                        |s, v| s.launch_at_login = v,
                        cx,
                    )),
                false,
                &theme,
            ))
            .child(group("Files", folder, false, &theme))
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
        let theme_item = |id: &'static str, value: &'static str, glyph: LucideIcon, text: &str| {
            ToggleGroupItem::new(id, value)
                .aria_label(text.to_string())
                .flex_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space(1.5))
                        .child(lucide(glyph).size(scaled(&theme, metrics::ICON_SMALL)))
                        .child(text.to_string()),
                )
        };
        let theme_store = store.clone();
        let theme_group = ToggleGroup::new("preferences.theme")
            .aria_label("Theme")
            .joined(true)
            .w_full()
            .value([theme_value])
            .item(theme_item(
                "preferences.theme.system",
                "system",
                LucideIcon::Monitor,
                "System",
            ))
            .item(theme_item(
                "preferences.theme.light",
                "light",
                LucideIcon::Sun,
                "Light",
            ))
            .item(theme_item(
                "preferences.theme.dark",
                "dark",
                LucideIcon::Moon,
                "Dark",
            ))
            .on_change(move |next, _, cx| {
                let choice = match next.first().map(|v| v.as_ref()) {
                    Some("light") => ThemeChoice::Light,
                    Some("dark") => ThemeChoice::Dark,
                    _ => ThemeChoice::System,
                };
                theme_store.update(cx, |store, cx| store.update(cx, |s| s.theme = choice));
            });
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
            .aria_label("Text size")
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
                .aria_label(format!("Text size {text}"))
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
            "Message display",
            match settings.display {
                MessageDisplay::Standard => "standard",
                MessageDisplay::Compact => "compact",
            },
            &[("standard", "Standard"), ("compact", "Compact")],
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
            MessageDisplay::Standard => "Avatars and a name line above each message.",
            MessageDisplay::Compact => "One line per message, with the time in the margin.",
        };
        let clock_group = choices(
            "preferences.clock",
            "Clock",
            match settings.clock {
                ClockFormat::TwelveHour => "12",
                ClockFormat::TwentyFourHour => "24",
            },
            &[("12", "12-hour"), ("24", "24-hour")],
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
            "Show names as",
            match settings.name_format {
                NameFormat::NicknameFullName => "nickname",
                NameFormat::FullName => "full",
                NameFormat::Username => "username",
            },
            &[
                ("nickname", "Nickname"),
                ("full", "Full name"),
                ("username", "Username"),
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
            NameFormat::NicknameFullName => "The nickname, else the full name, else the username.",
            NameFormat::FullName => "The full name, else the username.",
            NameFormat::Username => "Usernames only.",
        };
        page(&theme)
            .child(group("Theme", theme_group, false, &theme))
            .child(group("Text size", size_group, false, &theme))
            .child(group(
                "Message display",
                hinted(display_group, display_hint, &theme),
                true,
                &theme,
            ))
            .child(group("Clock", clock_group, true, &theme))
            .child(group(
                "Names",
                hinted(names_group, names_hint, &theme),
                true,
                &theme,
            ))
            .child(group(
                "Motion",
                self.switch_row(
                    "preferences.reduce-motion",
                    "Reduce motion",
                    "Panes and banners change without sliding.",
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
            Platform::MacOs => "Cmd+Enter",
            Platform::Other => "Ctrl+Enter",
        };
        let view = cx.entity().downgrade();
        let send_group = choices(
            "preferences.send",
            "Send messages with",
            match settings.send_on {
                SendKey::Enter => "enter",
                SendKey::ModEnter => "mod-enter",
                SendKey::CodeBlocks => "code-blocks",
            },
            &[
                ("enter", "Enter"),
                ("mod-enter", mod_enter),
                ("code-blocks", "Enter, but not in code"),
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
            SendKey::Enter => "Shift+Enter starts a new line.".into(),
            SendKey::ModEnter => "Enter starts a new line.".into(),
            SendKey::CodeBlocks => format!(
                "In a code block opened with ```, Enter starts a new line and {mod_enter} sends."
            )
            .into(),
        };
        let threads = self
            .collapsed_threads
            .filter(|(choice, _)| *choice)
            .map(|(_, on)| {
                group(
                    "Threads",
                    labelled_row(
                        "Collapsed reply threads".into(),
                        "Replies stay in their thread instead of the channel.".into(),
                        Switch::new("preferences.collapsed-threads")
                            .checked(on)
                            .aria_label("Collapsed reply threads")
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
                "Send messages with",
                hinted(send_group, send_hint, &theme),
                true,
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
                .child("Notification settings show once Matty is connected to your server.")
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
        let item = |id: &'static str, value: &'static str, text: &'static str| {
            ToggleGroupItem::new(id, value)
                .aria_label(text)
                .flex_1()
                .child(text)
        };
        let desktop = ToggleGroup::new("preferences.desktop")
            .aria_label("Notify me about")
            .joined(true)
            .w_full()
            .value([level])
            .item(item("preferences.desktop.all", "all", "All messages"))
            .item(item("preferences.desktop.mention", "mention", "Mentions"))
            .item(item("preferences.desktop.none", "none", "Nothing"))
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
                      title: &'static str,
                      detail: SharedString,
                      checked: bool,
                      key: &'static str,
                      values: (&'static str, &'static str)| {
            let view = view.clone();
            labelled_row(
                title.into(),
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
            "Your first name counts as a mention.".into()
        } else {
            format!("\u{201c}{first_name}\u{201d} counts as a mention.").into()
        };
        page(&theme)
            .child(group(
                "Desktop notifications",
                stack(&theme)
                    .child(desktop)
                    .child(switch(
                        "preferences.desktop-threads",
                        "Replies in threads I follow",
                        "Notify for every reply, not only mentions.".into(),
                        user.desktop_threads == matty_notify::Level::All,
                        "desktop_threads",
                        ("all", "mention"),
                    ))
                    .child(switch(
                        "preferences.desktop-sound",
                        "Sound",
                        "Play a sound with each notification.".into(),
                        user.desktop_sound,
                        "desktop_sound",
                        ("true", "false"),
                    )),
                true,
                &theme,
            ))
            .child(group(
                "Mentions",
                stack(&theme)
                    .child(switch(
                        "preferences.first-name",
                        "First name",
                        first_name_detail,
                        on("first_name", false),
                        "first_name",
                        ("true", "false"),
                    ))
                    .child(switch(
                        "preferences.channel-wide",
                        "@channel, @all and @here",
                        "Messages to the whole channel count as mentions.".into(),
                        on("channel", true),
                        "channel",
                        ("true", "false"),
                    ))
                    .child(
                        Field::new("preferences.keywords", &self.keywords)
                            .label("Keywords")
                            .description(
                                "Words that count as mentioning you, separated by commas. \
                                 @username always does.",
                            ),
                    ),
                true,
                &theme,
            ))
            .into_any_element()
    }

    fn advanced(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = UiTheme::read(cx).clone();
        group(
            "Logs",
            stack(&theme)
                .child(labelled_row(
                    "Log detail".into(),
                    "How much goes into the logs.".into(),
                    div()
                        .flex_shrink_0()
                        .w(scaled(&theme, metrics::FIELD_NARROW_WIDTH))
                        .child(Select::new(&self.log_level).aria_label("Log detail"))
                        .into_any_element(),
                    &theme,
                ))
                .child(labelled_row(
                    "Logs".into(),
                    "Attach them when you report a problem.".into(),
                    Button::new("preferences.open-logs")
                        .variant(ButtonVariant::Outline)
                        .size(ButtonSize::Sm)
                        .aria_label("Open logs folder")
                        .label("Open folder")
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
        )
        .into_any_element()
    }

    /// A switch for a local setting, with its title and one line on it.
    fn switch_row(
        &self,
        id: &'static str,
        title: &'static str,
        detail: &'static str,
        checked: bool,
        write: fn(&mut Settings, bool),
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = UiTheme::read(cx).clone();
        let store = self.settings.clone();
        labelled_row(
            title.into(),
            detail.into(),
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

fn stack(theme: &UiTheme) -> Div {
    div().flex().flex_col().gap(theme.space(3.))
}

/// A section's groups, one under the other.
fn page(theme: &UiTheme) -> Div {
    div().flex().flex_col().gap(theme.space(6.))
}

/// A titled group of controls; `synced` adds the line saying the account
/// keeps the setting.
fn group(title: &'static str, control: impl IntoElement, synced: bool, theme: &UiTheme) -> Div {
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
        .child("Synced with your Mattermost account")
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
    label: &'static str,
    value: &'static str,
    options: &[(&'static str, &'static str)],
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
            .aria_label(*text)
            .flex_1()
            .child(*text),
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
                (ADVANCED, "send_on_ctrl_enter", "true"),
            ]),
            &mut settings,
        );
        assert_eq!(settings.display, MessageDisplay::Compact);
        assert_eq!(settings.clock, ClockFormat::TwelveHour);
        assert_eq!(settings.name_format, NameFormat::Username);
        assert_eq!(settings.send_on, SendKey::ModEnter);

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
}
