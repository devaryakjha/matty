//! The account menu behind the title bar avatar (PLAN §3 M1 "Presence and
//! typing": set own status; §4.5: signing out confirms). Presence, a custom
//! status, Preferences and Sign out, nothing else.

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::actions::OpenPreferences;
use crate::dnd::{self, DURATIONS, DndDuration};
use crate::settings::ClockFormat;
use crate::sidebar::Presence;
use crate::style::{Palette, metrics, scaled, tipped_when};
use crate::ui::{
    alert_dialog::{alert_dialog, alert_dialog_action, alert_dialog_cancel},
    avatar::{Avatar, AvatarSize},
    button::{Button, ButtonSize, ButtonVariant, style_button},
    dialog::{DialogHandle, dialog, dialog_description, dialog_popup, dialog_title},
    field::Field,
    input::{InputEvent, InputState},
    menu::{Menu, MenuItem, MenuState},
    select::{Select, SelectEvent, SelectItem, SelectState},
    theme::UiTheme,
};

/// The signed-in user as the menu shows them; sync fills it from the
/// session and keeps presence current.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    /// The user's id, for their profile picture.
    pub user_id: SharedString,
    pub display_name: SharedString,
    pub username: SharedString,
    pub presence: Presence,
    pub status_text: Option<SharedString>,
    /// When Do not disturb ends, in seconds since the epoch; `None` while
    /// it has no end or another presence is set.
    pub dnd_end: Option<i64>,
}

/// What the menu asks of sync; the view already shows the change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountEvent {
    /// A presence, and for Do not disturb when it ends.
    SetPresence(Presence, Option<i64>),
    SetStatus(Option<SharedString>),
    SignOut,
}

const PRESENCES: [(Presence, &str, &str); 4] = [
    (Presence::Online, "online", "Online"),
    (Presence::Away, "away", "Away"),
    (Presence::Dnd, "dnd", "Do not disturb"),
    (Presence::Offline, "offline", "Offline"),
];

pub fn presence_label(presence: Presence) -> &'static str {
    PRESENCES
        .iter()
        .find(|(candidate, _, _)| *candidate == presence)
        .map_or("Online", |(_, _, label)| label)
}

/// The end Do not disturb has after the server reports `presence`: the
/// reported end, or the one already shown when the report has none.
fn server_dnd_end(shown: &Account, presence: Presence, reported: Option<i64>) -> Option<i64> {
    match presence {
        Presence::Dnd if reported.is_some() => reported,
        Presence::Dnd if shown.presence == Presence::Dnd => shown.dnd_end,
        _ => None,
    }
}

pub struct AccountView {
    account: Account,
    menu: Entity<MenuState>,
    status: DialogHandle,
    status_input: Entity<InputState>,
    sign_out: DialogHandle,
    /// The custom Do not disturb picker: a day and a time.
    custom: DialogHandle,
    custom_day: Entity<SelectState>,
    custom_time: Entity<SelectState>,
    /// The days `custom_day` offers, from when the picker last opened.
    custom_days: Vec<jiff::civil::Date>,
    /// Set when the status dialog opens: the next frame moves the cursor
    /// into the field, after the dialog has taken focus for itself.
    focus_status_input: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AccountEvent> for AccountView {}

impl AccountView {
    pub fn new(account: Account, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let menu = cx.new(|cx| MenuState::new([], cx));
        let status_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("What are you up to?"));
        let custom_day = cx.new(|cx| SelectState::new([], window, cx));
        let clock = ClockFormat::current(cx);
        let times = (0..24 * 60)
            .step_by(dnd::STEP_MINUTES as usize)
            .map(|minutes: u16| SelectItem::new(minutes.to_string(), clock.format(minutes)));
        let custom_time = cx.new(|cx| SelectState::new(times, window, cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &status_input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.save_status(window, cx);
                    }
                },
            ),
            // The Done button follows whether the pick is still ahead.
            cx.subscribe(&custom_time, |_, _, _: &SelectEvent, cx| cx.notify()),
        ];
        let mut this = Self {
            account,
            menu,
            status: DialogHandle::new(false),
            status_input,
            sign_out: DialogHandle::new(false),
            custom: DialogHandle::new(false),
            custom_day,
            custom_time,
            custom_days: Vec::new(),
            focus_status_input: false,
            _subscriptions: subscriptions,
        };
        this.refresh_items(cx);
        this
    }

    pub fn account(&self) -> &Account {
        &self.account
    }

    /// Replaces the account; sync calls this when presence or the status
    /// changes on the server.
    pub fn set_account(&mut self, account: Account, cx: &mut Context<Self>) {
        if self.account != account {
            self.account = account;
            self.refresh_items(cx);
            cx.notify();
        }
    }

    /// Takes the user's presence from the server. A `status_change` event
    /// carries no end time, so Do not disturb keeps the end it had; the
    /// server's own reply to setting it does carry one.
    pub fn set_server_presence(
        &mut self,
        presence: Presence,
        dnd_end: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        let mut account = self.account.clone();
        account.dnd_end = server_dnd_end(&account, presence, dnd_end);
        account.presence = presence;
        if self.account != account {
            self.account = account;
            self.refresh_items(cx);
            cx.notify();
        }
    }

    fn set_presence(&mut self, presence: Presence, dnd_end: Option<i64>, cx: &mut Context<Self>) {
        let dnd_end = dnd_end.filter(|_| presence == Presence::Dnd);
        if self.account.presence != presence || self.account.dnd_end != dnd_end {
            self.account.presence = presence;
            self.account.dnd_end = dnd_end;
            self.refresh_items(cx);
            cx.emit(AccountEvent::SetPresence(presence, dnd_end));
            cx.notify();
        }
    }

    fn set_dnd(&mut self, duration: DndDuration, cx: &mut Context<Self>) {
        let end = dnd::end_time(duration, &jiff::Zoned::now());
        self.set_presence(Presence::Dnd, end, cx);
    }

    /// "Until 14:30" while Do not disturb has an end still ahead.
    fn until(&self, cx: &App) -> Option<String> {
        let end = self
            .account
            .dnd_end
            .filter(|_| self.account.presence == Presence::Dnd)?;
        let now = jiff::Zoned::now();
        if end <= now.timestamp().as_second() {
            return None;
        }
        dnd::until_label(end, &now, ClockFormat::current(cx))
    }

    fn open_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = jiff::Zoned::now();
        let days = dnd::custom_days(&now);
        let items = days
            .iter()
            .enumerate()
            .map(|(index, (_, label))| SelectItem::new(index.to_string(), label.clone()));
        // The days are relative to today, so the list is made afresh.
        self.custom_day = cx.new(|cx| SelectState::new(items, window, cx));
        cx.subscribe(&self.custom_day, |_, _, _: &SelectEvent, cx| cx.notify())
            .detach();
        self.custom_days = days.into_iter().map(|(date, _)| date).collect();
        // The first half hour still ahead, which may be tomorrow's first.
        let (day, minutes) = match dnd::custom_times(now.date(), &now).first() {
            Some(minutes) => (0, *minutes),
            None => (1, 0),
        };
        self.custom_day.update(cx, |select, cx| {
            select.set_value(Some(day.to_string().into()), window, cx)
        });
        self.custom_time.update(cx, |select, cx| {
            select.set_value(Some(minutes.to_string().into()), window, cx)
        });
        self.custom.open(window, cx);
        cx.notify();
    }

    /// The custom pick in seconds since the epoch, if it is still ahead.
    fn custom_end(&self, cx: &App) -> Option<i64> {
        let day: usize = self.custom_day.read(cx).value()?.parse().ok()?;
        let minutes: u16 = self.custom_time.read(cx).value()?.parse().ok()?;
        let date = *self.custom_days.get(day)?;
        let now = jiff::Zoned::now();
        dnd::local_end(date, minutes, now.time_zone())
            .filter(|end| *end > now.timestamp().as_second())
    }

    fn set_status(&mut self, status: Option<SharedString>, cx: &mut Context<Self>) {
        if self.account.status_text != status {
            self.account.status_text = status.clone();
            self.refresh_items(cx);
            cx.emit(AccountEvent::SetStatus(status));
            cx.notify();
        }
    }

    fn open_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.account.status_text.clone().unwrap_or_default();
        self.status_input.update(cx, |input, cx| {
            input.set_value(current, window, cx);
        });
        self.status.open(window, cx);
        self.focus_status_input = true;
        cx.notify();
    }

    fn save_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.status.is_open() {
            return;
        }
        let typed = self.status_input.read(cx).value().trim().to_string();
        let status = (!typed.is_empty()).then(|| SharedString::from(typed));
        self.set_status(status, cx);
        self.status.close(window, cx);
    }

    /// The menu's items follow the account, so they are rebuilt when it
    /// changes rather than on every frame.
    fn refresh_items(&mut self, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        let account = self.account.clone();
        let mut items = vec![MenuItem::label(
            "account.who",
            format!("{} · @{}", account.display_name, account.username),
        )];
        for (presence, name, label) in PRESENCES {
            let id = gpui_kit::ElementId::Name(format!("account.presence.{name}").into());
            if presence == Presence::Dnd {
                items.push(self.dnd_submenu(id, label, cx));
                continue;
            }
            let view = view.clone();
            items.push(
                MenuItem::new(id, label)
                    .radio(account.presence == presence)
                    .on_click(move |_, _, cx| {
                        view.update(cx, |this, cx| this.set_presence(presence, None, cx))
                            .ok();
                    }),
            );
        }
        items.push(MenuItem::separator());
        let status_view = view.clone();
        items.push(
            MenuItem::new(
                "account.status",
                match &account.status_text {
                    Some(text) => format!("Status: {text}"),
                    None => "Set a status…".to_string(),
                },
            )
            .on_click(move |_, window, cx| {
                status_view
                    .update(cx, |this, cx| this.open_status(window, cx))
                    .ok();
            }),
        );
        items.push(MenuItem::separator());
        items.push(
            MenuItem::new("account.preferences", "Preferences").on_click(|_, window, cx| {
                window.dispatch_action(Box::new(OpenPreferences), cx);
            }),
        );
        items.push(MenuItem::separator());
        let sign_out = self.sign_out.clone();
        items.push(
            MenuItem::new("account.sign-out", "Sign out…")
                .on_click(move |_, window, cx| sign_out.open(window, cx)),
        );
        self.menu.update(cx, |menu, cx| menu.set_items(items, cx));
    }

    /// Do not disturb opens its durations; the label carries the end.
    fn dnd_submenu(
        &self,
        id: gpui_kit::ElementId,
        label: &str,
        cx: &mut Context<Self>,
    ) -> MenuItem {
        let view = cx.entity().downgrade();
        let mut durations: Vec<MenuItem> = DURATIONS
            .iter()
            .map(|(duration, name, label)| {
                let (view, duration) = (view.clone(), *duration);
                MenuItem::new(
                    gpui_kit::ElementId::Name(format!("account.dnd.{name}").into()),
                    *label,
                )
                // Submenu items run while the menu is being updated, so the
                // change that rebuilds its items waits until after.
                .on_click(move |_, _, cx| {
                    let view = view.clone();
                    cx.defer(move |cx| {
                        view.update(cx, |this, cx| this.set_dnd(duration, cx)).ok();
                    });
                })
            })
            .collect();
        durations.push(MenuItem::separator());
        durations.push(MenuItem::new("account.dnd.custom", "Custom…").on_click(
            move |_, window, cx| {
                let view = view.clone();
                window.defer(cx, move |window, cx| {
                    view.update(cx, |this, cx| this.open_custom(window, cx))
                        .ok();
                });
            },
        ));
        let label = match self.until(cx) {
            Some(until) => format!("{label} · {until}"),
            None => label.to_string(),
        };
        MenuItem::submenu(id, label, durations).radio(self.account.presence == Presence::Dnd)
    }

    fn custom_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let end = self.custom_end(cx);
        let popup = dialog_popup("account.dnd.custom.popup", "Do not disturb until", cx)
            .child(dialog_title("account.dnd.custom.title", cx).child("Do not disturb until"))
            .child(
                dialog_description("account.dnd.custom.description", cx).child(
                    "Notifications stay quiet until then. Your status goes back afterwards.",
                ),
            )
            .child(
                div()
                    .flex()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        div()
                            .flex_1()
                            .child(Select::new(&self.custom_day).aria_label("Day")),
                    )
                    .child(
                        div()
                            .w(scaled(&theme, metrics::FIELD_NARROW_WIDTH))
                            .child(Select::new(&self.custom_time).aria_label("Time")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        Button::new("account.dnd.custom.cancel")
                            .variant(ButtonVariant::Outline)
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.custom.close(window, cx);
                            })),
                    )
                    .child(
                        Button::new("account.dnd.custom.save")
                            .variant(ButtonVariant::Default)
                            .label("Done")
                            .disabled(end.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(end) = this.custom_end(cx) {
                                    this.set_presence(Presence::Dnd, Some(end), cx);
                                    this.custom.close(window, cx);
                                }
                            })),
                    ),
            );
        dialog("account.dnd.custom", &self.custom, popup, window, cx)
    }

    fn trigger(&self, cx: &App) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let palette = Palette::read(cx);
        let dot = match self.account.presence {
            Presence::Online => palette.online,
            Presence::Away => palette.away,
            Presence::Dnd => palette.dnd,
            Presence::Offline => theme.colors.muted_foreground,
        };
        let initial: String = self.account.display_name.chars().take(1).collect();
        let until = self.until(cx);
        let tip = format!(
            "{} · {}",
            presence_label(self.account.presence),
            until.clone().unwrap_or_default()
        );
        let avatar = div()
            .relative()
            .flex_shrink_0()
            .child(crate::avatars::with_picture(
                Avatar::new("account.avatar")
                    .size(AvatarSize::Sm)
                    .fallback(initial),
                &self.account.user_id,
                cx,
            ))
            .child(
                div()
                    .absolute()
                    .bottom(px(-1.))
                    .right(px(-1.))
                    .size(scaled(&theme, 10.))
                    .rounded_full()
                    .bg(dot)
                    .border_2()
                    .border_color(theme.colors.sidebar),
            );
        tipped_when(until.is_some(), "account.avatar.tip", tip, avatar)
    }

    fn status_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let has_status = self.account.status_text.is_some();
        let popup = dialog_popup("account.status.popup", "Set a status", cx)
            .child(dialog_title("account.status.title", cx).child("Set a status"))
            .child(
                dialog_description("account.status.description", cx)
                    .child("Shown next to your name until you clear it."),
            )
            .child(
                div()
                    .pt(theme.space(2.))
                    .child(Field::new("account.status.input", &self.status_input).label("Status")),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .when(has_status, |row| {
                        row.child(
                            Button::new("account.status.clear-button")
                                .variant(ButtonVariant::Ghost)
                                .label("Clear")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.set_status(None, cx);
                                    this.status.close(window, cx);
                                })),
                        )
                    })
                    .child(
                        Button::new("account.status.cancel")
                            .variant(ButtonVariant::Outline)
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.status.close(window, cx);
                            })),
                    )
                    .child(
                        Button::new("account.status.save")
                            .variant(ButtonVariant::Default)
                            .label("Done")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_status(window, cx);
                            })),
                    ),
            );
        let element = dialog("account.status", &self.status, popup, window, cx);
        if self.focus_status_input && self.status.is_open() {
            self.focus_status_input = false;
            let focus = self.status_input.read(cx).focus_handle(cx);
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        element
    }

    fn sign_out_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let popup = dialog_popup("account.sign-out.popup", "Sign out", cx)
            .child(dialog_title("account.sign-out.title", cx).child("Sign out of Matty?"))
            .child(
                dialog_description("account.sign-out.description", cx).child(
                    "This removes Matty's copy of your messages, files and drafts from \
                     this computer. You'll sign in again next time.",
                ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        alert_dialog_cancel("account.sign-out.cancel")
                            .variant(ButtonVariant::Outline)
                            .label("Cancel"),
                    )
                    .child(
                        alert_dialog_action("account.sign-out.confirm")
                            .variant(ButtonVariant::Destructive)
                            .label("Sign out"),
                    ),
            );
        let view = cx.entity().downgrade();
        alert_dialog("account.sign-out", &self.sign_out, popup, window, cx).on_ok(
            move |_, _, cx| {
                view.update(cx, |_, cx| cx.emit(AccountEvent::SignOut)).ok();
                true
            },
        )
    }
}

impl Render for AccountView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let mut label = format!(
            "{}, {}",
            self.account.display_name,
            presence_label(self.account.presence).to_lowercase()
        );
        if let Some(until) = self.until(cx) {
            label = format!("{label}, {}", until.to_lowercase());
        }
        div()
            .flex()
            .items_center()
            .child(
                Menu::new(&self.menu, label)
                    .trigger(self.trigger(cx))
                    .trigger_style_with(move |button| {
                        style_button(
                            button,
                            false,
                            ButtonVariant::Ghost,
                            ButtonSize::IconSm,
                            &theme,
                        )
                        .rounded_full()
                    }),
            )
            .child(self.status_dialog(window, cx))
            .child(self.custom_dialog(window, cx))
            .child(self.sign_out_dialog(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(presence: Presence, dnd_end: Option<i64>) -> Account {
        Account {
            user_id: "u-arya".into(),
            display_name: "Arya".into(),
            username: "arya".into(),
            presence,
            status_text: None,
            dnd_end,
        }
    }

    #[test]
    fn a_reported_end_wins() {
        let account = shown(Presence::Online, None);
        assert_eq!(server_dnd_end(&account, Presence::Dnd, Some(10)), Some(10));
    }

    #[test]
    fn a_status_change_keeps_the_end() {
        let account = shown(Presence::Dnd, Some(10));
        assert_eq!(server_dnd_end(&account, Presence::Dnd, None), Some(10));
    }

    #[test]
    fn leaving_dnd_drops_the_end() {
        let account = shown(Presence::Dnd, Some(10));
        assert_eq!(server_dnd_end(&account, Presence::Online, None), None);
        let account = shown(Presence::Away, None);
        assert_eq!(server_dnd_end(&account, Presence::Dnd, None), None);
    }
}
