//! Browse channels (PLAN §3 M1 "Finding people and channels"): the team's
//! public channels in a dialog, with search and Join.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _,
};

use crate::style::{self, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    button::{Button, ButtonSize, ButtonVariant},
    dialog::{DialogHandle, dialog_description, dialog_title},
    input::{Input, InputEvent, InputState},
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
