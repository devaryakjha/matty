//! The account menu behind the title bar avatar (PLAN §3 M1 "Presence and
//! typing": set own status; §4.5: signing out confirms). Presence, a custom
//! status, Preferences and Sign out, nothing else.

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::actions::OpenPreferences;
use crate::sidebar::Presence;
use crate::style::{Palette, scaled};
use crate::ui::{
    alert_dialog::{alert_dialog, alert_dialog_action, alert_dialog_cancel},
    avatar::{Avatar, AvatarSize},
    button::{Button, ButtonSize, ButtonVariant, style_button},
    dialog::{DialogHandle, dialog, dialog_description, dialog_popup, dialog_title},
    field::Field,
    input::{InputEvent, InputState},
    menu::{Menu, MenuItem, MenuState},
    theme::UiTheme,
};

/// The signed-in user as the menu shows them; sync fills it from the
/// session and keeps presence current.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub display_name: SharedString,
    pub username: SharedString,
    pub presence: Presence,
    pub status_text: Option<SharedString>,
}

/// What the menu asks of sync; the view already shows the change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountEvent {
    SetPresence(Presence),
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

pub struct AccountView {
    account: Account,
    menu: Entity<MenuState>,
    status: DialogHandle,
    status_input: Entity<InputState>,
    sign_out: DialogHandle,
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
        let subscriptions = vec![cx.subscribe_in(
            &status_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.save_status(window, cx);
                }
            },
        )];
        let mut this = Self {
            account,
            menu,
            status: DialogHandle::new(false),
            status_input,
            sign_out: DialogHandle::new(false),
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

    fn set_presence(&mut self, presence: Presence, cx: &mut Context<Self>) {
        if self.account.presence != presence {
            self.account.presence = presence;
            self.refresh_items(cx);
            cx.emit(AccountEvent::SetPresence(presence));
            cx.notify();
        }
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
            let view = view.clone();
            items.push(
                MenuItem::new(
                    gpui_kit::ElementId::Name(format!("account.presence.{name}").into()),
                    label,
                )
                .radio(account.presence == presence)
                .on_click(move |_, _, cx| {
                    view.update(cx, |this, cx| this.set_presence(presence, cx))
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
        div()
            .relative()
            .flex_shrink_0()
            .child(
                Avatar::new("account.avatar")
                    .size(AvatarSize::Sm)
                    .fallback(initial),
            )
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
            )
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
                                .child("Clear")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.set_status(None, cx);
                                    this.status.close(window, cx);
                                })),
                        )
                    })
                    .child(
                        Button::new("account.status.cancel")
                            .variant(ButtonVariant::Outline)
                            .child("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.status.close(window, cx);
                            })),
                    )
                    .child(
                        Button::new("account.status.save")
                            .variant(ButtonVariant::Default)
                            .child("Done")
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
                    "Your drafts stay on this computer. You will need your password or \
                     single sign-on to come back.",
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
        let label = format!(
            "{}, {}",
            self.account.display_name,
            presence_label(self.account.presence).to_lowercase()
        );
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
            .child(self.sign_out_dialog(window, cx))
    }
}
