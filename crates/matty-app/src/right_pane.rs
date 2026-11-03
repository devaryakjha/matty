//! The right pane (DESIGN.md §4): threads first, later search results,
//! saved, pinned, mentions and channel info. It shares the headers' 48px
//! height, separates from the channel with a hairline, and closes on
//! Escape.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Window, div,
};

use crate::actions::{self, CloseRightPane};
use crate::channel::{ChannelModel, Post};
use crate::style::{self, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    avatar::{Avatar, AvatarSize},
    badge::{Badge, BadgeVariant},
    button::{Button, ButtonSize, ButtonVariant},
    composer::{Composer, ComposerState},
    markdown::markdown,
    message::{Message, Reaction},
    message_list::{MessageList, MessageListState},
    theme::UiTheme,
};

/// A thread: its root post and the replies loaded for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadModel {
    pub channel_name: SharedString,
    pub root: Post,
    pub replies: Vec<Post>,
    pub following: bool,
}

/// What the pane shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RightPaneContent {
    Thread(ThreadModel),
    ChannelInfo(ChannelModel),
}

/// What the pane asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RightPaneEvent {
    Close,
}

pub struct RightPaneView {
    content: Option<RightPaneContent>,
    list: Entity<MessageListState>,
    composer: Entity<ComposerState>,
    focus: FocusHandle,
}

impl EventEmitter<RightPaneEvent> for RightPaneView {}

impl Focusable for RightPaneView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl RightPaneView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let list = cx.new(|cx| MessageListState::new(0, cx));
        let composer = cx.new(|cx| {
            let mut state = ComposerState::new(6, window, cx);
            state.set_placeholder("Reply…", window, cx);
            state
        });
        Self {
            content: None,
            list,
            composer,
            focus: cx.focus_handle(),
        }
    }

    pub fn content(&self) -> Option<&RightPaneContent> {
        self.content.as_ref()
    }

    /// Replaces what the pane shows and resets the transcript to its end.
    pub fn set_content(&mut self, content: Option<RightPaneContent>, cx: &mut Context<Self>) {
        let rows = match &content {
            // Root, the replies divider, then replies.
            Some(RightPaneContent::Thread(thread)) => thread.replies.len() + 2,
            _ => 0,
        };
        self.list = cx.new(|cx| MessageListState::new(rows, cx));
        self.content = content;
        cx.notify();
    }

    fn header(
        &self,
        title: SharedString,
        subtitle: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let muted = theme.colors.muted_foreground;
        let action = |id: &'static str, label: &'static str, glyph: LucideIcon| {
            Button::new(id)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::IconSm)
                .aria_label(label)
                .child(
                    lucide(glyph)
                        .size(scaled(&theme, metrics::ICON))
                        .text_color(muted),
                )
        };
        div()
            .flex_shrink_0()
            .h(scaled(&theme, metrics::HEADER_HEIGHT))
            .pl(theme.space(4.))
            .pr(theme.space(2.))
            .flex()
            .items_center()
            .justify_between()
            .gap(theme.space(2.))
            .border_b(style::hairline())
            .border_color(theme.colors.border)
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(theme.space(2.))
                    .min_w_0()
                    .child(
                        div()
                            .type_style(style::TITLE, &theme)
                            .text_color(theme.colors.foreground)
                            .child(title),
                    )
                    .children(subtitle.map(|text| {
                        div()
                            .min_w_0()
                            .truncate()
                            .type_style(style::META, &theme)
                            .text_color(muted)
                            .child(text)
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap(theme.space(0.5))
                    .child(action("right-pane.expand", "Expand", LucideIcon::Maximize2))
                    .child(
                        action("right-pane.close", "Close", LucideIcon::X)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(RightPaneEvent::Close))),
                    ),
            )
    }

    fn post_row(post: &Post, root: bool, cx: &mut App) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let id = post.id.clone();
        let initial: String = post.author.chars().take(1).collect();
        Message::new(gpui_kit::ElementId::Name(format!("thread.{id}").into()))
            .author(post.author.clone())
            .timestamp(post.time.clone())
            .gutter_time(post.time.clone())
            .continuation(post.continues && !root)
            .avatar(
                Avatar::new(gpui_kit::ElementId::Name(
                    format!("thread.{id}.avatar").into(),
                ))
                .size(AvatarSize::Default)
                .fallback(initial)
                .aria_label(post.author.clone()),
            )
            .reactions(post.reactions.iter().map(|reaction| {
                Reaction::new(reaction.emoji.clone(), reaction.count, reaction.mine)
            }))
            .px(theme.space(4.))
            .child(markdown(
                gpui_kit::ElementId::Name(format!("thread.{id}.body").into()),
                post.body.clone(),
                cx,
            ))
    }

    fn replies_divider(thread: &ThreadModel, cx: &App) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let count = thread.replies.len();
        let label = match count {
            1 => "1 reply".to_string(),
            n => format!("{n} replies"),
        };
        div()
            .flex()
            .items_center()
            .gap(theme.space(3.))
            .px(theme.space(4.))
            .py(theme.space(2.))
            .child(
                div()
                    .type_style(style::META, &theme)
                    .text_color(theme.colors.muted_foreground)
                    .child(label),
            )
            .child(div().flex_1().h(style::hairline()).bg(theme.colors.border))
            .child(
                Badge::new(if thread.following {
                    "Following"
                } else {
                    "Follow"
                })
                .variant(if thread.following {
                    BadgeVariant::Secondary
                } else {
                    BadgeVariant::Outline
                }),
            )
    }

    fn thread(&self, thread: &ThreadModel, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let model = thread.clone();
        let transcript =
            MessageList::new(
                "right-pane.thread",
                &self.list,
                move |index, _, cx| match index {
                    0 => Self::post_row(&model.root, true, cx).into_any_element(),
                    1 => Self::replies_divider(&model, cx).into_any_element(),
                    index => match model.replies.get(index - 2) {
                        Some(reply) => Self::post_row(reply, false, cx).into_any_element(),
                        None => div().into_any_element(),
                    },
                },
            )
            .size_full()
            .pt(theme.space(2.));
        let send = Button::new("right-pane.send")
            .variant(ButtonVariant::Default)
            .size(ButtonSize::IconSm)
            .aria_label("Reply")
            .child(
                lucide(LucideIcon::SendHorizontal)
                    .size(scaled(&theme, metrics::ICON))
                    .text_color(theme.colors.primary_foreground),
            );
        let tool = |id: &'static str, label: &'static str, glyph: LucideIcon| {
            Button::new(id)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::IconSm)
                .aria_label(label)
                .child(
                    lucide(glyph)
                        .size(scaled(&theme, metrics::ICON))
                        .text_color(theme.colors.muted_foreground),
                )
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex_1().min_h_0().child(transcript))
            .child(
                div()
                    .flex_shrink_0()
                    .px(theme.space(4.))
                    .pt(theme.space(1.))
                    .pb(theme.space(4.))
                    .child(
                        Composer::new(&self.composer)
                            .rounded(theme.radius.lg)
                            .child(tool("right-pane.attach", "Attach files", LucideIcon::Plus))
                            .child(tool(
                                "right-pane.emoji",
                                "Emoji",
                                LucideIcon::FaceSlightlySmiling,
                            ))
                            .child(div().flex_1())
                            .child(send),
                    ),
            )
    }

    fn channel_info(channel: &ChannelModel, cx: &App) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let row = |label: &'static str, value: String| {
            div()
                .flex()
                .flex_col()
                .gap(theme.space(1.))
                .child(
                    div()
                        .type_style(style::LABEL, &theme)
                        .text_color(theme.colors.muted_foreground)
                        .child(label.to_uppercase()),
                )
                .child(
                    div()
                        .type_style(style::BODY, &theme)
                        .text_color(theme.colors.foreground)
                        .child(value),
                )
        };
        div()
            .flex()
            .flex_col()
            .gap(theme.space(5.))
            .p(theme.space(4.))
            .child(row(
                "Header",
                channel
                    .header
                    .as_deref()
                    .unwrap_or("No header yet")
                    .to_string(),
            ))
            .child(row("Members", channel.members.to_string()))
            .child(row("Pinned messages", channel.pinned.to_string()))
    }
}

impl Render for RightPaneView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let root = div()
            .key_context(actions::RIGHT_PANE)
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &CloseRightPane, _, cx| cx.emit(RightPaneEvent::Close)))
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(theme.colors.background)
            .border_l(style::hairline())
            .border_color(theme.colors.border);
        match self.content.clone() {
            Some(RightPaneContent::Thread(thread)) => root
                .child(self.header(
                    "Thread".into(),
                    Some(match thread.channel_name.as_ref() {
                        name if name.contains(' ') => name.to_string().into(),
                        name => format!("#{name}").into(),
                    }),
                    cx,
                ))
                .child(div().flex_1().min_h_0().child(self.thread(&thread, cx))),
            Some(RightPaneContent::ChannelInfo(channel)) => root
                .child(self.header("Channel info".into(), Some(channel.name.clone()), cx))
                .child(Self::channel_info(&channel, cx)),
            None => root.child(self.header("Thread".into(), None, cx)),
        }
    }
}
