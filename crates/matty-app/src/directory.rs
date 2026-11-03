//! Browse channels (PLAN §3 M1 "Finding people and channels"): the team's
//! public channels in a dialog, with search and Join.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _,
};

use crate::profile::UserProfile;
use crate::sidebar::Presence;
use crate::style::{self, Palette, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    avatar::{Avatar, AvatarSize},
    button::{Button, ButtonSize, ButtonVariant},
    checkbox::Checkbox,
    dialog::{DialogHandle, dialog_description, dialog_title},
    field::Field,
    input::{Input, InputEvent, InputState},
    radio_group::{RadioGroup, RadioItem},
    theme::UiTheme,
};

/// A public channel the user may browse to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryChannel {
    pub id: SharedString,
    pub name: SharedString,
    pub purpose: Option<SharedString>,
    pub members: usize,
    /// Already in the user's sidebar: the row opens it instead of joining.
    pub joined: bool,
}

/// What the browse dialog asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirectoryEvent {
    /// Join the channel and open it; the dialog already shows it joined.
    Join(SharedString),
    /// Open a channel the user is already in.
    Open(SharedString),
    /// Start or open a direct message with this person.
    Message(UserProfile),
    /// Create a channel; sync owns the server side and the window adds
    /// the row meanwhile.
    Create {
        name: SharedString,
        purpose: Option<SharedString>,
        private: bool,
    },
}

/// The browse-channels dialog body; the window wraps it in `dialog`.
pub struct BrowseView {
    channels: Vec<DirectoryChannel>,
    team_name: SharedString,
    search: Entity<InputState>,
    handle: DialogHandle,
    focus_search: bool,
    _subscription: gpui_kit::Subscription,
}

impl EventEmitter<DirectoryEvent> for BrowseView {}

impl BrowseView {
    pub fn new(
        team_name: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search channels"));
        let subscription = cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify());
        Self {
            channels: Vec::new(),
            team_name: team_name.into(),
            search,
            handle: DialogHandle::new(false),
            focus_search: false,
            _subscription: subscription,
        }
    }

    /// The dialog's open state, for the window's `dialog` wrapper.
    pub fn handle(&self) -> &DialogHandle {
        &self.handle
    }

    pub fn set_channels(&mut self, channels: Vec<DirectoryChannel>, cx: &mut Context<Self>) {
        self.channels = channels;
        cx.notify();
    }

    pub fn set_team_name(&mut self, name: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.team_name = name.into();
        cx.notify();
    }

    /// Opens the dialog with an empty search and the field focused.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search
            .update(cx, |search, cx| search.set_value("", window, cx));
        self.handle.open(window, cx);
        self.focus_search = true;
        cx.notify();
    }

    /// Sync confirmed or reverted a join.
    pub fn set_joined(&mut self, id: &str, joined: bool, cx: &mut Context<Self>) {
        if let Some(channel) = self.channels.iter_mut().find(|c| c.id.as_ref() == id) {
            channel.joined = joined;
            cx.notify();
        }
    }

    fn join(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.set_joined(&id, true, cx);
        self.handle.close(window, cx);
        cx.emit(DirectoryEvent::Join(id));
    }

    fn open_channel(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.handle.close(window, cx);
        cx.emit(DirectoryEvent::Open(id));
    }

    fn row(&self, channel: &DirectoryChannel, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let muted = theme.colors.muted_foreground;
        let id = channel.id.clone();
        let (row_id, button_id) = (id.clone(), id.clone());
        let joined = channel.joined;
        let members = match channel.members {
            1 => "1 member".to_string(),
            n => format!("{n} members"),
        };
        let button = if joined {
            Button::new(gpui_kit::ElementId::Name(
                format!("browse.{id}.view").into(),
            ))
            .variant(ButtonVariant::Outline)
            .size(ButtonSize::Sm)
            .label("View")
            .w(theme.space(16.))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_channel(button_id.clone(), window, cx)
            }))
        } else {
            Button::new(gpui_kit::ElementId::Name(
                format!("browse.{id}.join").into(),
            ))
            .variant(ButtonVariant::Default)
            .size(ButtonSize::Sm)
            .label("Join")
            .w(theme.space(16.))
            .on_click(
                cx.listener(move |this, _, window, cx| this.join(button_id.clone(), window, cx)),
            )
        };
        div()
            .id(gpui_kit::ElementId::Name(format!("browse.{id}").into()))
            .px(theme.space(2.))
            .py(theme.space(1.5))
            .mx(theme.space(-2.))
            .rounded(theme.radius.md)
            .flex()
            .items_center()
            .gap(theme.space(3.))
            .cursor_pointer()
            .hover(|style| style.bg(theme.colors.accent))
            .on_click(cx.listener(move |this, _, window, cx| {
                if joined {
                    this.open_channel(row_id.clone(), window, cx)
                } else {
                    this.join(row_id.clone(), window, cx)
                }
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(theme.space(0.5))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space(1.5))
                            .min_w_0()
                            .child(
                                lucide(LucideIcon::Hash)
                                    .size(scaled(&theme, 14.))
                                    .text_color(muted),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .type_style(style::UI_STRONG, &theme)
                                    .text_color(theme.colors.foreground)
                                    .child(channel.name.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .type_style(style::META, &theme)
                                    .text_color(muted)
                                    .child(members),
                            ),
                    )
                    .children(channel.purpose.clone().map(|purpose| {
                        div()
                            .truncate()
                            .type_style(style::META, &theme)
                            .text_color(muted)
                            .child(purpose)
                    })),
            )
            .child(button)
    }
}

impl Render for BrowseView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        if self.focus_search {
            self.focus_search = false;
            let focus = self.search.read(cx).focus_handle(cx);
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        let query = self.search.read(cx).value().trim().to_lowercase();
        let matching: Vec<&DirectoryChannel> = self
            .channels
            .iter()
            .filter(|channel| {
                query.is_empty()
                    || channel.name.to_lowercase().contains(&query)
                    || channel
                        .purpose
                        .as_ref()
                        .is_some_and(|purpose| purpose.to_lowercase().contains(&query))
            })
            .collect();
        let rows: Vec<_> = matching
            .iter()
            .map(|channel| self.row(channel, cx).into_any_element())
            .collect();
        let public = self.channels.len();
        div()
            .flex()
            .flex_col()
            .gap(theme.space(4.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space(1.))
                    .child(dialog_title("browse.title", cx).child("Browse channels"))
                    .child(
                        dialog_description("browse.description", cx)
                            .child(format!("{public} public channels in {}", self.team_name)),
                    ),
            )
            .child(Input::new(&self.search).aria_label("Search channels"))
            .child(
                div()
                    .id("browse.list")
                    .overflow_y_scroll()
                    .max_h(scaled(&theme, metrics::ROW_HEIGHT * 8.))
                    .flex()
                    .flex_col()
                    .gap(theme.space(0.5))
                    .when(rows.is_empty(), |list| {
                        list.child(
                            div()
                                .py(theme.space(6.))
                                .flex()
                                .justify_center()
                                .type_style(style::UI, &theme)
                                .text_color(theme.colors.muted_foreground)
                                .child(format!("No channels match “{query}”")),
                        )
                    })
                    .children(rows),
            )
    }
}

/// The "New message" dialog: pick a person to message.
pub struct NewMessageView {
    profiles: Vec<UserProfile>,
    search: Entity<InputState>,
    handle: DialogHandle,
    focus_search: bool,
    _subscription: gpui_kit::Subscription,
}

impl EventEmitter<DirectoryEvent> for NewMessageView {}

impl NewMessageView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search people"));
        let subscription = cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify());
        Self {
            profiles: Vec::new(),
            search,
            handle: DialogHandle::new(false),
            focus_search: false,
            _subscription: subscription,
        }
    }

    pub fn handle(&self) -> &DialogHandle {
        &self.handle
    }

    pub fn set_profiles(&mut self, profiles: Vec<UserProfile>, cx: &mut Context<Self>) {
        self.profiles = profiles;
        cx.notify();
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search
            .update(cx, |search, cx| search.set_value("", window, cx));
        self.handle.open(window, cx);
        self.focus_search = true;
        cx.notify();
    }

    fn message(&mut self, profile: UserProfile, window: &mut Window, cx: &mut Context<Self>) {
        self.handle.close(window, cx);
        cx.emit(DirectoryEvent::Message(profile));
    }

    fn row(&self, profile: &UserProfile, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let palette = Palette::read(cx);
        let dot = match profile.presence {
            Presence::Online => palette.online,
            Presence::Away => palette.away,
            Presence::Dnd => palette.dnd,
            Presence::Offline => theme.colors.muted_foreground,
        };
        let initial: String = profile.display_name.chars().take(1).collect();
        let username = profile.username.clone();
        let chosen = profile.clone();
        div()
            .id(gpui_kit::ElementId::Name(
                format!("new-message.{username}").into(),
            ))
            .h(scaled(&theme, metrics::ROW_HEIGHT + 8.))
            .px(theme.space(2.))
            .mx(theme.space(-2.))
            .rounded(theme.radius.md)
            .flex()
            .items_center()
            .gap(theme.space(3.))
            .cursor_pointer()
            .hover(|style| style.bg(theme.colors.accent))
            .on_click(
                cx.listener(move |this, _, window, cx| this.message(chosen.clone(), window, cx)),
            )
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .child(
                        Avatar::new(gpui_kit::ElementId::Name(
                            format!("new-message.{username}.avatar").into(),
                        ))
                        .size(AvatarSize::Sm)
                        .fallback(initial)
                        .aria_label(profile.display_name.clone()),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom(gpui_kit::px(-1.))
                            .right(gpui_kit::px(-1.))
                            .size(gpui_kit::px(9.))
                            .rounded_full()
                            .bg(dot)
                            .border_2()
                            .border_color(theme.colors.popover),
                    ),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .type_style(style::UI, &theme)
                    .text_color(theme.colors.foreground)
                    .child(profile.display_name.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .type_style(style::META, &theme)
                    .text_color(theme.colors.muted_foreground)
                    .child(format!("@{username}")),
            )
            .children(profile.position.clone().map(|position| {
                div()
                    .ml_auto()
                    .flex_shrink_0()
                    .type_style(style::META, &theme)
                    .text_color(theme.colors.muted_foreground)
                    .child(position)
            }))
    }
}

impl Render for NewMessageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        if self.focus_search {
            self.focus_search = false;
            let focus = self.search.read(cx).focus_handle(cx);
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        let query = self.search.read(cx).value().trim().to_lowercase();
        let rows: Vec<_> = self
            .profiles
            .iter()
            .filter(|profile| {
                query.is_empty()
                    || profile.display_name.to_lowercase().contains(&query)
                    || profile.username.to_lowercase().contains(&query)
            })
            .map(|profile| self.row(profile, cx).into_any_element())
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(theme.space(4.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space(1.))
                    .child(dialog_title("new-message.title", cx).child("New message"))
                    .child(
                        dialog_description("new-message.description", cx).child(
                            "Pick someone to message. Existing conversations open in place.",
                        ),
                    ),
            )
            .child(Input::new(&self.search).aria_label("Search people"))
            .child(
                div()
                    .id("new-message.list")
                    .overflow_y_scroll()
                    .max_h(scaled(&theme, metrics::ROW_HEIGHT * 8.))
                    .flex()
                    .flex_col()
                    .gap(theme.space(0.5))
                    .when(rows.is_empty(), |list| {
                        list.child(
                            div()
                                .py(theme.space(6.))
                                .flex()
                                .justify_center()
                                .type_style(style::UI, &theme)
                                .text_color(theme.colors.muted_foreground)
                                .child(format!("Nobody matches “{query}”")),
                        )
                    })
                    .children(rows),
            )
    }
}

/// Mattermost's channel name rules: 2 to 64 lowercase letters, digits,
/// dashes or underscores, derived from the display name.
pub fn channel_handle(name: &str) -> String {
    let mut handle = String::new();
    for c in name.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || c == '_' {
            handle.push(c);
        } else if (c == ' ' || c == '-') && !handle.ends_with('-') && !handle.is_empty() {
            handle.push('-');
        }
    }
    while handle.ends_with('-') {
        handle.pop();
    }
    handle.truncate(64);
    handle
}

/// The "Create channel" dialog: name, purpose, public or private.
pub struct CreateChannelView {
    name: Entity<InputState>,
    purpose: Entity<InputState>,
    private: bool,
    handle: DialogHandle,
    focus_name: bool,
    _subscriptions: Vec<gpui_kit::Subscription>,
}

impl EventEmitter<DirectoryEvent> for CreateChannelView {}

impl CreateChannelView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. design-reviews"));
        let purpose =
            cx.new(|cx| InputState::new(window, cx).placeholder("What is this channel for?"));
        let subscriptions = vec![
            cx.subscribe(&name, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&purpose, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        Self {
            name,
            purpose,
            private: false,
            handle: DialogHandle::new(false),
            focus_name: false,
            _subscriptions: subscriptions,
        }
    }

    pub fn handle(&self) -> &DialogHandle {
        &self.handle
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.name
            .update(cx, |name, cx| name.set_value("", window, cx));
        self.purpose
            .update(cx, |purpose, cx| purpose.set_value("", window, cx));
        self.private = false;
        self.handle.open(window, cx);
        self.focus_name = true;
        cx.notify();
    }

    fn handle_text(&self, cx: &App) -> String {
        channel_handle(&self.name.read(cx).value())
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.handle_text(cx);
        if handle.len() < 2 {
            return;
        }
        let name: SharedString = self.name.read(cx).value().trim().to_string().into();
        let purpose = self.purpose.read(cx).value().trim().to_string();
        let purpose = (!purpose.is_empty()).then(|| purpose.into());
        self.handle.close(window, cx);
        cx.emit(DirectoryEvent::Create {
            name,
            purpose,
            private: self.private,
        });
    }
}

impl Render for CreateChannelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        if self.focus_name {
            self.focus_name = false;
            let focus = self.name.read(cx).focus_handle(cx);
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        let handle = self.handle_text(cx);
        let description = if handle.is_empty() {
            "Lowercase letters, numbers and dashes; the URL follows the name.".to_string()
        } else {
            format!("Address: ~{handle}")
        };
        let can_create = handle.len() >= 2;
        let view = cx.weak_entity();
        div()
            .flex()
            .flex_col()
            .gap(theme.space(4.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space(1.))
                    .child(dialog_title("create-channel.title", cx).child("Create channel"))
                    .child(
                        dialog_description("create-channel.description", cx)
                            .child("Channels are where the team talks about one thing."),
                    ),
            )
            .child(
                Field::new("create-channel.name", &self.name)
                    .label("Name")
                    .required(true)
                    .description(description),
            )
            .child(
                Field::new("create-channel.purpose", &self.purpose)
                    .label("Purpose")
                    .description("Optional. Shown in Browse channels and the empty state."),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space(2.))
                    .child(
                        div()
                            .type_style(style::UI_STRONG, &theme)
                            .text_color(theme.colors.foreground)
                            .child("Visibility"),
                    )
                    .child(
                        RadioGroup::new("create-channel.visibility")
                            .aria_label("Visibility")
                            .value(if self.private { "private" } else { "public" })
                            .item(
                                RadioItem::new("create-channel.public", "public")
                                    .label("Public: anyone in the team can find and join it"),
                            )
                            .item(
                                RadioItem::new("create-channel.private", "private")
                                    .label("Private: only people you add can see it"),
                            )
                            .on_change(move |value, _, cx| {
                                view.update(cx, |this, cx| {
                                    this.private = value.as_ref() == "private";
                                    cx.notify();
                                })
                                .ok();
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        Button::new("create-channel.cancel")
                            .variant(ButtonVariant::Outline)
                            .child("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.handle.close(window, cx);
                            })),
                    )
                    .child(
                        Button::new("create-channel.create")
                            .variant(ButtonVariant::Default)
                            .disabled(!can_create)
                            .child("Create")
                            .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::channel_handle;

    #[test]
    fn handles_follow_mattermost_rules() {
        assert_eq!(channel_handle("Design Reviews"), "design-reviews");
        assert_eq!(channel_handle("  release -- 2026  "), "release-2026");
        assert_eq!(channel_handle("ok_name!"), "ok_name");
        assert_eq!(channel_handle("-"), "");
    }
}

/// What the Add members dialog decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddMembersEvent {
    /// Add these people (by user id) to the channel the dialog was opened for.
    Add(Vec<UserProfile>),
}

/// The "Add members" dialog: a searchable, multi-select people list.
pub struct AddMembersView {
    channel: SharedString,
    profiles: Vec<UserProfile>,
    selected: Vec<SharedString>,
    search: Entity<InputState>,
    handle: DialogHandle,
    focus_search: bool,
    _subscription: gpui_kit::Subscription,
}

impl EventEmitter<AddMembersEvent> for AddMembersView {}

impl AddMembersView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search people"));
        let subscription = cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify());
        Self {
            channel: SharedString::default(),
            profiles: Vec::new(),
            selected: Vec::new(),
            search,
            handle: DialogHandle::new(false),
            focus_search: false,
            _subscription: subscription,
        }
    }

    pub fn handle(&self) -> &DialogHandle {
        &self.handle
    }

    /// Opens the dialog for `channel` with `profiles` to choose from (people
    /// not yet in it, as far as the caller knows).
    pub fn open(
        &mut self,
        channel: SharedString,
        profiles: Vec<UserProfile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.channel = channel;
        self.profiles = profiles;
        self.selected.clear();
        self.search
            .update(cx, |search, cx| search.set_value("", window, cx));
        self.handle.open(window, cx);
        self.focus_search = true;
        cx.notify();
    }

    fn toggle(&mut self, user_id: SharedString, cx: &mut Context<Self>) {
        match self.selected.iter().position(|id| *id == user_id) {
            Some(index) => {
                self.selected.remove(index);
            }
            None => self.selected.push(user_id),
        }
        cx.notify();
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            return;
        }
        let people: Vec<UserProfile> = self
            .profiles
            .iter()
            .filter(|profile| self.selected.contains(&profile.user_id))
            .cloned()
            .collect();
        self.handle.close(window, cx);
        cx.emit(AddMembersEvent::Add(people));
    }

    fn row(&self, profile: &UserProfile, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let checked = self.selected.contains(&profile.user_id);
        let initial: String = profile.display_name.chars().take(1).collect();
        let username = profile.username.clone();
        let (row_id, box_id) = (profile.user_id.clone(), profile.user_id.clone());
        let box_view = cx.weak_entity();
        div()
            .id(gpui_kit::ElementId::Name(
                format!("add-members.{username}").into(),
            ))
            .h(scaled(&theme, metrics::ROW_HEIGHT + 8.))
            .px(theme.space(2.))
            .mx(theme.space(-2.))
            .rounded(theme.radius.md)
            .flex()
            .items_center()
            .gap(theme.space(3.))
            .cursor_pointer()
            .hover(|style| style.bg(theme.colors.accent))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(row_id.clone(), cx)))
            .child(
                Checkbox::new(gpui_kit::ElementId::Name(
                    format!("add-members.{username}.checkbox").into(),
                ))
                .checked(checked)
                .aria_label(profile.display_name.clone())
                .on_change(move |_, _, _, cx| {
                    box_view
                        .update(cx, |this, cx| this.toggle(box_id.clone(), cx))
                        .ok();
                }),
            )
            .child(
                Avatar::new(gpui_kit::ElementId::Name(
                    format!("add-members.{username}.avatar").into(),
                ))
                .size(AvatarSize::Sm)
                .fallback(initial)
                .aria_label(profile.display_name.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .type_style(style::UI, &theme)
                    .text_color(theme.colors.foreground)
                    .child(profile.display_name.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .type_style(style::META, &theme)
                    .text_color(theme.colors.muted_foreground)
                    .child(format!("@{username}")),
            )
    }
}

impl Render for AddMembersView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        if self.focus_search {
            self.focus_search = false;
            let focus = self.search.read(cx).focus_handle(cx);
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        let query = self.search.read(cx).value().trim().to_lowercase();
        let rows: Vec<_> = self
            .profiles
            .iter()
            .filter(|profile| {
                query.is_empty()
                    || profile.display_name.to_lowercase().contains(&query)
                    || profile.username.to_lowercase().contains(&query)
            })
            .map(|profile| self.row(profile, cx).into_any_element())
            .collect();
        let count = self.selected.len();
        let add_label = match count {
            0 => "Add".to_string(),
            1 => "Add 1 person".to_string(),
            n => format!("Add {n} people"),
        };
        div()
            .flex()
            .flex_col()
            .gap(theme.space(4.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space(1.))
                    .child(dialog_title("add-members.title", cx).child("Add members"))
                    .child(
                        dialog_description("add-members.description", cx)
                            .child(format!("They join #{} and see its history.", self.channel)),
                    ),
            )
            .child(Input::new(&self.search).aria_label("Search people"))
            .child(
                div()
                    .id("add-members.list")
                    .overflow_y_scroll()
                    .max_h(scaled(&theme, metrics::ROW_HEIGHT * 8.))
                    .flex()
                    .flex_col()
                    .gap(theme.space(0.5))
                    .when(rows.is_empty(), |list| {
                        list.child(
                            div()
                                .py(theme.space(6.))
                                .flex()
                                .justify_center()
                                .type_style(style::UI, &theme)
                                .text_color(theme.colors.muted_foreground)
                                .child(format!("Nobody matches “{query}”")),
                        )
                    })
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .child(
                        Button::new("add-members.cancel")
                            .variant(ButtonVariant::Outline)
                            .child("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.handle.close(window, cx);
                            })),
                    )
                    .child(
                        Button::new("add-members.add")
                            .variant(ButtonVariant::Default)
                            .disabled(count == 0)
                            .child(add_label)
                            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                    ),
            )
    }
}
