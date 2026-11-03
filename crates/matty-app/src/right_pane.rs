//! The right pane (DESIGN.md §4): threads first, later search results,
//! saved, pinned, mentions and channel info. It shares the headers' 48px
//! height, separates from the channel with a hairline, and closes on
//! Escape.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};

use crate::actions::{self, CloseRightPane};
use crate::channel::{ChannelModel, Post, PostStatus};
use crate::style::{self, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    avatar::{Avatar, AvatarSize},
    button::{Button, ButtonSize, ButtonVariant},
    composer::{Composer, ComposerEvent, ComposerState},
    empty::{Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle},
    markdown::markdown,
    message::{Message, MessageStatus, Reaction},
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

/// A post listed outside its channel: saved, pinned or a mention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedPost {
    pub channel_id: SharedString,
    pub channel_name: SharedString,
    pub post: Post,
}

/// What the pane shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RightPaneContent {
    Thread(ThreadModel),
    ChannelInfo(ChannelModel),
    /// The current channel's pinned posts, newest first.
    Pinned {
        channel_name: SharedString,
        posts: Vec<ListedPost>,
    },
    /// The user's saved posts across channels, newest first.
    Saved(Vec<ListedPost>),
    /// Posts that mention the user, newest first.
    Mentions(Vec<ListedPost>),
}

/// What the pane asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RightPaneEvent {
    Close,
    /// The user replied under `root`; the pane already shows the reply
    /// pending (PLAN §4.3).
    Reply {
        root: SharedString,
        text: SharedString,
    },
    /// Retry a reply that failed; the pane shows it pending again.
    Retry(SharedString),
    /// The user toggled following the thread under `root`.
    ToggleFollow {
        root: SharedString,
        following: bool,
    },
    ToggleReaction {
        post: SharedString,
        emoji: SharedString,
    },
    /// Jump to `post` in `channel` from a listing.
    Jump {
        channel: SharedString,
        post: SharedString,
    },
}

pub struct RightPaneView {
    content: Option<RightPaneContent>,
    list: Entity<MessageListState>,
    composer: Entity<ComposerState>,
    focus: FocusHandle,
    /// The current user's display name, for optimistic rows.
    me: SharedString,
    draft_len: usize,
    pending_seq: usize,
    _subscriptions: Vec<gpui_kit::Subscription>,
}

impl EventEmitter<RightPaneEvent> for RightPaneView {}

impl Focusable for RightPaneView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl RightPaneView {
    pub fn new(me: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let list = cx.new(|cx| MessageListState::new(0, cx));
        let composer = cx.new(|cx| {
            let mut state = ComposerState::new(6, window, cx);
            state.set_placeholder("Reply…", window, cx);
            state
        });
        let subscriptions = vec![cx.subscribe_in(
            &composer,
            window,
            |this, composer, event: &ComposerEvent, window, cx| match event {
                ComposerEvent::Change => {
                    this.draft_len = composer.read(cx).value(cx).chars().count();
                    cx.notify();
                }
                ComposerEvent::Submit(text) => this.reply(text.clone(), window, cx),
                _ => {}
            },
        )];
        Self {
            content: None,
            list,
            composer,
            focus: cx.focus_handle(),
            me: me.into(),
            draft_len: 0,
            pending_seq: 0,
            _subscriptions: subscriptions,
        }
    }

    fn thread_mut(&mut self) -> Option<&mut ThreadModel> {
        match self.content.as_mut()? {
            RightPaneContent::Thread(thread) => Some(thread),
            _ => None,
        }
    }

    /// Appends the text as a pending reply by the current user, clears the
    /// composer and tells sync. Blank text stays in the composer.
    fn reply(&mut self, text: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let body: SharedString = text.trim().to_string().into();
        if body.is_empty() {
            return;
        }
        self.pending_seq += 1;
        let (me, seq) = (self.me.clone(), self.pending_seq);
        let Some(thread) = self.thread_mut() else {
            return;
        };
        let root = thread.root.id.clone();
        let last = thread.replies.last().unwrap_or(&thread.root);
        let reply = Post {
            id: format!("{root}-pending-{seq}").into(),
            author: me.clone(),
            time: "Sending…".into(),
            day: last.day.clone(),
            body: body.clone(),
            reactions: vec![],
            mentions_me: false,
            edited: false,
            // Never a continuation: the row's own "Sending…" must show.
            continues: false,
            mine: true,
            saved: false,
            pinned: false,
            status: PostStatus::Pending,
            replies: None,
        };
        thread.replies.push(reply);
        let content = self.content.clone();
        self.set_content(content, cx);
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.draft_len = 0;
        cx.emit(RightPaneEvent::Reply { root, text: body });
    }

    /// Applies `change` to the reply with `id`, if it is loaded, and redraws.
    fn mutate_reply(
        &mut self,
        id: &SharedString,
        change: impl FnOnce(&mut Post),
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(thread) = self.thread_mut() else {
            return false;
        };
        let Some(post) = std::iter::once(&mut thread.root)
            .chain(thread.replies.iter_mut())
            .find(|post| &post.id == id)
        else {
            return false;
        };
        change(post);
        cx.notify();
        true
    }

    fn retry(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let changed = self.mutate_reply(
            &id,
            |post| {
                post.status = PostStatus::Pending;
                post.time = "Sending…".into();
            },
            cx,
        );
        if changed {
            cx.emit(RightPaneEvent::Retry(id));
        }
    }

    /// Removes a reply that never reached the server.
    fn discard(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let Some(thread) = self.thread_mut() else {
            return;
        };
        thread.replies.retain(|post| post.id != id);
        let content = self.content.clone();
        self.set_content(content, cx);
    }

    fn toggle_reaction(&mut self, id: SharedString, emoji: SharedString, cx: &mut Context<Self>) {
        let changed = self.mutate_reply(
            &id,
            |post| {
                if let Some(at) = post.reactions.iter().position(|r| r.emoji == emoji) {
                    let reaction = &mut post.reactions[at];
                    if reaction.mine {
                        reaction.count -= 1;
                        reaction.mine = false;
                        if reaction.count == 0 {
                            post.reactions.remove(at);
                        }
                    } else {
                        reaction.count += 1;
                        reaction.mine = true;
                    }
                }
            },
            cx,
        );
        if changed {
            cx.emit(RightPaneEvent::ToggleReaction { post: id, emoji });
        }
    }

    fn toggle_follow(&mut self, cx: &mut Context<Self>) {
        let Some(thread) = self.thread_mut() else {
            return;
        };
        thread.following = !thread.following;
        let event = RightPaneEvent::ToggleFollow {
            root: thread.root.id.clone(),
            following: thread.following,
        };
        cx.notify();
        cx.emit(event);
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

    fn post_row(
        view: &gpui_kit::WeakEntity<Self>,
        post: &Post,
        root: bool,
        cx: &mut App,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let id = post.id.clone();
        let initial: String = post.author.chars().take(1).collect();
        let (retry_view, retry_id) = (view.clone(), id.clone());
        let (delete_view, delete_id) = (view.clone(), id.clone());
        let (react_view, react_id) = (view.clone(), id.clone());
        Message::new(gpui_kit::ElementId::Name(format!("thread.{id}").into()))
            .status(match post.status {
                PostStatus::Sent => MessageStatus::Sent,
                PostStatus::Pending => MessageStatus::Pending,
                PostStatus::Failed => MessageStatus::Failed,
            })
            .on_retry(move |_, _, cx| {
                let id = retry_id.clone();
                retry_view.update(cx, |this, cx| this.retry(id, cx)).ok();
            })
            .on_delete(move |_, _, cx| {
                let id = delete_id.clone();
                delete_view.update(cx, |this, cx| this.discard(id, cx)).ok();
            })
            .on_reaction(move |emoji, _, cx| {
                let (id, emoji) = (react_id.clone(), emoji.clone());
                react_view
                    .update(cx, |this, cx| this.toggle_reaction(id, emoji, cx))
                    .ok();
            })
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

    fn replies_divider(
        view: &gpui_kit::WeakEntity<Self>,
        thread: &ThreadModel,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let view = view.clone();
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
                Button::new("right-pane.follow")
                    .variant(if thread.following {
                        ButtonVariant::Secondary
                    } else {
                        ButtonVariant::Outline
                    })
                    .size(ButtonSize::Xs)
                    .child(if thread.following {
                        "Following"
                    } else {
                        "Follow"
                    })
                    .on_click(move |_, _, cx| {
                        view.update(cx, |this, cx| this.toggle_follow(cx)).ok();
                    }),
            )
    }

    fn thread(&self, thread: &ThreadModel, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let model = thread.clone();
        let view = cx.entity().downgrade();
        let transcript =
            MessageList::new(
                "right-pane.thread",
                &self.list,
                move |index, _, cx| match index {
                    0 => Self::post_row(&view, &model.root, true, cx).into_any_element(),
                    1 => Self::replies_divider(&view, &model, cx).into_any_element(),
                    index => match model.replies.get(index - 2) {
                        Some(reply) => Self::post_row(&view, reply, false, cx).into_any_element(),
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
            .disabled(self.draft_len == 0)
            .on_click(cx.listener(|this, _, window, cx| {
                let text = this.composer.read(cx).value(cx);
                this.reply(text, window, cx)
            }))
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
                            .child(
                                tool("right-pane.emoji", "Emoji", LucideIcon::FaceSlightlySmiling)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.composer.update(cx, |composer, cx| {
                                            composer.insert(":", window, cx);
                                            composer.focus(window, cx);
                                        })
                                    })),
                            )
                            .child(div().flex_1())
                            .child(send),
                    ),
            )
    }

    /// Nothing open: one line on how to get a thread here.
    fn empty(cx: &App) -> impl IntoElement + use<> {
        Self::empty_state(
            LucideIcon::MessageSquareReply,
            "No thread open",
            "Hover a message and choose Reply in thread to follow it here.",
            cx,
        )
    }

    fn empty_state(
        glyph: LucideIcon,
        title: &'static str,
        description: &'static str,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let media = div()
            .size(theme.space(10.))
            .rounded_full()
            .bg(theme.colors.muted)
            .flex()
            .items_center()
            .justify_center()
            .child(
                lucide(glyph)
                    .size(scaled(&theme, metrics::ICON_LARGE))
                    .text_color(theme.colors.muted_foreground),
            );
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
                        .media(EmptyMedia::new().child(media))
                        .title(
                            EmptyTitle::new()
                                .type_style(style::TITLE, &theme)
                                .text_center()
                                .child(title),
                        )
                        .description(
                            EmptyDescription::new()
                                .type_style(style::META, &theme)
                                .text_center()
                                .child(description),
                        ),
                ),
            )
    }

    /// Saved, pinned and mention listings: each post under a caption with
    /// its channel and a Jump link, separated by hairlines.
    fn listing(
        &self,
        id: &'static str,
        items: &[ListedPost],
        empty: (LucideIcon, &'static str, &'static str),
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        if items.is_empty() {
            return Self::empty_state(empty.0, empty.1, empty.2, cx).into_any_element();
        }
        let theme = UiTheme::read(cx).clone();
        let view = cx.entity().downgrade();
        div()
            .id(id)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(theme.space(2.))
            .children(items.iter().map(|item| {
                let channel = match item.channel_name.as_ref() {
                    name if name.contains(' ') => name.to_string(),
                    name => format!("#{name}"),
                };
                let (jump_channel, jump_post) = (item.channel_id.clone(), item.post.id.clone());
                let jump_view = view.clone();
                div()
                    .pb(theme.space(2.))
                    .mb(theme.space(2.))
                    .border_b(style::hairline())
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .px(theme.space(4.))
                            .pb(theme.space(1.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .type_style(style::META, &theme)
                                    .text_color(theme.colors.muted_foreground)
                                    .child(format!("{channel} · {}", item.post.day)),
                            )
                            .child(
                                Button::new(gpui_kit::ElementId::Name(
                                    format!("{id}.{}.jump", item.post.id).into(),
                                ))
                                .variant(ButtonVariant::Link)
                                .size(ButtonSize::Xs)
                                .child("Jump")
                                .on_click(move |_, _, cx| {
                                    let event = RightPaneEvent::Jump {
                                        channel: jump_channel.clone(),
                                        post: jump_post.clone(),
                                    };
                                    jump_view.update(cx, |_, cx| cx.emit(event)).ok();
                                }),
                            ),
                    )
                    .child(Self::post_row(
                        &view,
                        &Post {
                            continues: false,
                            ..item.post.clone()
                        },
                        true,
                        cx,
                    ))
            }))
            .into_any_element()
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
            Some(RightPaneContent::Pinned {
                channel_name,
                posts,
            }) => root
                .child(self.header(
                    "Pinned messages".into(),
                    Some(match channel_name.as_ref() {
                        name if name.contains(' ') => name.to_string().into(),
                        name => format!("#{name}").into(),
                    }),
                    cx,
                ))
                .child(self.listing(
                    "right-pane.pinned",
                    &posts,
                    (
                        LucideIcon::Pin,
                        "Nothing pinned yet",
                        "Pin a message from its More menu to keep it here for everyone.",
                    ),
                    cx,
                )),
            Some(RightPaneContent::Saved(posts)) => root
                .child(self.header("Saved messages".into(), None, cx))
                .child(self.listing(
                    "right-pane.saved",
                    &posts,
                    (
                        LucideIcon::Bookmark,
                        "Nothing saved yet",
                        "Save a message from its hover toolbar to find it here later.",
                    ),
                    cx,
                )),
            Some(RightPaneContent::Mentions(posts)) => root
                .child(self.header("Recent mentions".into(), None, cx))
                .child(self.listing(
                    "right-pane.mentions",
                    &posts,
                    (
                        LucideIcon::AtSign,
                        "No mentions yet",
                        "Messages that mention you or your keywords show up here.",
                    ),
                    cx,
                )),
            None => root
                .child(self.header("Thread".into(), None, cx))
                .child(Self::empty(cx)),
        }
    }
}
