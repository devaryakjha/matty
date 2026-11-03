//! The channel view: header, transcript and composer (DESIGN.md §4). The
//! view draws a [`ChannelModel`] that sync will fill; until then it shows
//! the empty state, or the preview fixtures with `MATTY_PREVIEW=1`.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, prelude::FluentBuilder as _, px,
};

use crate::actions::{EditLastMessage, FocusComposer, ReplyToLastMessage, UploadFiles};
use crate::sidebar::ChannelKind;
use crate::style::{self, Palette, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    alert_dialog::{alert_dialog, alert_dialog_action, alert_dialog_cancel},
    avatar::{Avatar, AvatarSize},
    button::{Button, ButtonSize, ButtonVariant},
    composer::{Composer, ComposerEvent, ComposerState},
    dialog::{DialogHandle, dialog_description, dialog_popup, dialog_title},
    empty::{Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle},
    markdown::markdown,
    menu::{Menu, MenuItem, MenuState},
    message::{
        Message, MessageHighlight, MessageStatus, Reaction, day_divider, message_action,
        message_actions, new_messages_divider,
    },
    message_list::{MessageList, MessageListState},
    skeleton::Skeleton,
    theme::UiTheme,
};

/// One reaction chip under a post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostReaction {
    pub emoji: SharedString,
    pub count: usize,
    /// Whether the current user is among the reactors.
    pub mine: bool,
}

/// Where a post is on its way to the server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PostStatus {
    #[default]
    Sent,
    /// In the outbox; drawn at reduced opacity.
    Pending,
    /// The send failed; the row offers Retry and Delete.
    Failed,
}

/// Mattermost's default `MaxPostSize`, until the server's config arrives.
pub const DEFAULT_MAX_POST_SIZE: usize = 16383;

/// One post, already formatted for display: sync does the time zone and
/// Markdown rewriting, the view only draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Post {
    pub id: SharedString,
    pub author: SharedString,
    /// `HH:MM` in the user's zone.
    pub time: SharedString,
    /// The day divider label this post falls under ("Today", "Yesterday", a date).
    pub day: SharedString,
    /// Markdown body.
    pub body: SharedString,
    pub reactions: Vec<PostReaction>,
    pub mentions_me: bool,
    pub edited: bool,
    /// True when the previous post is from the same author within five
    /// minutes; the row then hides its avatar and header.
    pub continues: bool,
    /// Written by the current user: Edit and Delete appear in its menu.
    pub mine: bool,
    pub saved: bool,
    pub pinned: bool,
    pub status: PostStatus,
    /// The thread under this post, when it has one, for the reply footer
    /// (Collapsed Reply Threads, PLAN §4.4).
    pub replies: Option<ReplySummary>,
}

/// What the channel shows about a post's thread without loading it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplySummary {
    pub count: usize,
    /// `HH:MM` of the newest reply.
    pub last_time: SharedString,
    /// Display names of the people who replied, newest first, at most three.
    pub participants: Vec<SharedString>,
    pub following: bool,
    /// Replies the user has not read yet.
    pub unread: bool,
}

/// A channel with the posts loaded for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelModel {
    pub id: SharedString,
    pub name: SharedString,
    pub kind: ChannelKind,
    /// The channel header text, shown after the name when present.
    pub header: Option<SharedString>,
    pub members: usize,
    pub pinned: usize,
    pub favorite: bool,
    /// The channel purpose, shown in the empty state.
    pub purpose: Option<SharedString>,
    /// True while the first page of posts is on its way: skeleton rows
    /// instead of the empty state (PLAN §4.5).
    pub loading: bool,
    pub posts: Vec<Post>,
    /// Index of the first post after the user's last view, for the
    /// "New messages" line.
    pub first_unread: Option<usize>,
    /// The server's `MaxPostSize`; the composer counts down to it.
    pub max_post_size: usize,
}

/// A row in the transcript: posts interleaved with dividers.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    Day(SharedString),
    NewMessages,
    Post(usize),
}

impl ChannelModel {
    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::with_capacity(self.posts.len() + 4);
        let mut day: Option<&SharedString> = None;
        for (index, post) in self.posts.iter().enumerate() {
            if day != Some(&post.day) {
                rows.push(Row::Day(post.day.clone()));
                day = Some(&post.day);
            }
            if self.first_unread == Some(index) {
                rows.push(Row::NewMessages);
            }
            rows.push(Row::Post(index));
        }
        rows
    }
}

/// What the channel view asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelEvent {
    /// The user submitted the composer with this text.
    Send(SharedString),
    /// The user asked to reply to this post in a thread.
    OpenThread(SharedString),
    /// The user added or removed their reaction; the view already shows it.
    ToggleReaction {
        post: SharedString,
        emoji: SharedString,
    },
    /// Saved or unsaved a post; the view already shows it.
    ToggleSave(SharedString),
    /// Pinned or unpinned a post to the channel; the view already shows it.
    TogglePin(SharedString),
    /// The user wants to edit their post.
    Edit(SharedString),
    /// The user confirmed deleting their post; the view already removed it.
    Delete(SharedString),
    /// Retry a failed send; the view shows it pending again.
    Retry(SharedString),
    /// The user toggled following the thread under `post`.
    ToggleFollow { post: SharedString, following: bool },
}

/// What a More-menu item does to the view.
type MenuAction = Box<dyn Fn(&mut ChannelView, &mut Window, &mut Context<ChannelView>)>;

/// The reactions one click away, as Mattermost's recent-emoji row. The full
/// picker arrives with the emoji data from sync.
const QUICK_REACTIONS: [(&str, &str); 6] = [
    ("👍", "Thumbs up"),
    ("❤️", "Heart"),
    ("😂", "Joy"),
    ("🎉", "Tada"),
    ("👀", "Eyes"),
    ("🚀", "Rocket"),
];

pub struct ChannelView {
    model: Option<ChannelModel>,
    rows: Vec<Row>,
    list: Entity<MessageListState>,
    composer: Entity<ComposerState>,
    /// One menu for every post: its items are set when it opens.
    menu: Entity<MenuState>,
    delete: DialogHandle,
    /// The post the delete dialog asks about.
    deleting: Option<SharedString>,
    /// The current user's display name, for optimistic rows.
    me: SharedString,
    /// Characters in the composer, for the Send button and the counter.
    draft_len: usize,
    /// Numbers the optimistic rows until sync assigns ids.
    pending_seq: usize,
    _subscriptions: Vec<gpui_kit::Subscription>,
}

impl EventEmitter<ChannelEvent> for ChannelView {}

impl ChannelView {
    pub fn new(
        model: Option<ChannelModel>,
        me: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let rows = model.as_ref().map(ChannelModel::rows).unwrap_or_default();
        let list = cx.new(|cx| MessageListState::new(rows.len(), cx));
        let composer = cx.new(|cx| ComposerState::new(8, window, cx));
        let menu = cx.new(|cx| MenuState::new([], cx));
        let subscriptions = vec![cx.subscribe_in(
            &composer,
            window,
            |this, composer, event: &ComposerEvent, window, cx| match event {
                ComposerEvent::Change => {
                    this.draft_len = composer.read(cx).value(cx).chars().count();
                    cx.notify();
                }
                ComposerEvent::Submit(text) => this.send(text.clone(), window, cx),
                _ => {}
            },
        )];
        let mut this = Self {
            model: None,
            rows,
            list,
            composer,
            menu,
            delete: DialogHandle::new(false),
            deleting: None,
            me: me.into(),
            draft_len: 0,
            pending_seq: 0,
            _subscriptions: subscriptions,
        };
        this.set_model(model, window, cx);
        this
    }

    pub fn model(&self) -> Option<&ChannelModel> {
        self.model.as_ref()
    }

    /// Replaces the channel and resets the transcript to its end.
    pub fn set_model(
        &mut self,
        model: Option<ChannelModel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rows = model.as_ref().map(ChannelModel::rows).unwrap_or_default();
        let count = self.rows.len();
        self.list = cx.new(|cx| MessageListState::new(count, cx));
        let placeholder: SharedString = match &model {
            Some(channel) => match channel.kind {
                ChannelKind::Direct { .. } | ChannelKind::Group { .. } => {
                    format!("Message {}", channel.name).into()
                }
                _ => format!("Message #{}", channel.name).into(),
            },
            None => "Write a message…".into(),
        };
        self.composer.update(cx, |composer, cx| {
            composer.set_placeholder(placeholder, window, cx);
        });
        self.model = model;
        cx.notify();
    }

    fn max_post_size(&self) -> usize {
        self.model
            .as_ref()
            .map_or(DEFAULT_MAX_POST_SIZE, |channel| channel.max_post_size)
    }

    fn can_send(&self) -> bool {
        self.model.is_some() && self.draft_len > 0 && self.draft_len <= self.max_post_size()
    }

    /// Appends the text as a pending post by the current user, clears the
    /// composer and tells sync (PLAN §4.3: optimistic send through the
    /// outbox). Blank text and text over the limit stay in the composer.
    fn send(&mut self, text: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let body: SharedString = text.trim().to_string().into();
        if body.is_empty() || body.chars().count() > self.max_post_size() {
            return;
        }
        let Some(mut model) = self.model.clone() else {
            return;
        };
        self.pending_seq += 1;
        let last = model.posts.last();
        let post = Post {
            id: format!("pending-{}", self.pending_seq).into(),
            author: self.me.clone(),
            time: "Sending…".into(),
            day: last.map_or_else(|| "Today".into(), |post| post.day.clone()),
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
        model.posts.push(post);
        self.set_model(Some(model), window, cx);
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.draft_len = 0;
        cx.emit(ChannelEvent::Send(body));
    }

    fn retry(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let changed = self.mutate_post(
            &id,
            |post| {
                post.status = PostStatus::Pending;
                post.time = "Sending…".into();
            },
            cx,
        );
        if changed {
            cx.emit(ChannelEvent::Retry(id));
        }
    }

    /// Wraps the selection in Markdown from a toolbar button or shortcut.
    fn wrap(&mut self, before: &str, after: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |composer, cx| {
            composer.wrap_selection(before, after, window, cx);
            composer.focus(window, cx);
        });
    }

    fn last_own_post(&self) -> Option<SharedString> {
        self.model.as_ref()?.posts.iter().rev().find_map(|post| {
            (post.mine && post.status == PostStatus::Sent).then(|| post.id.clone())
        })
    }

    /// Applies `change` to the post with `id`, if it is loaded, and redraws.
    /// Rows index posts, so nothing else moves.
    fn mutate_post(
        &mut self,
        id: &SharedString,
        change: impl FnOnce(&mut Post),
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(post) = self
            .model
            .as_mut()
            .and_then(|channel| channel.posts.iter_mut().find(|post| &post.id == id))
        else {
            return false;
        };
        change(post);
        cx.notify();
        true
    }

    /// Flips the user's reaction locally and tells sync. A chip that drops
    /// to zero disappears, as on the server.
    fn toggle_reaction(&mut self, id: SharedString, emoji: SharedString, cx: &mut Context<Self>) {
        let changed = self.mutate_post(
            &id,
            |post| {
                if let Some(at) = post.reactions.iter().position(|r| r.emoji == emoji) {
                    let reaction = &mut post.reactions[at];
                    if reaction.mine {
                        reaction.mine = false;
                        reaction.count = reaction.count.saturating_sub(1);
                        if reaction.count == 0 {
                            post.reactions.remove(at);
                        }
                    } else {
                        reaction.mine = true;
                        reaction.count += 1;
                    }
                } else {
                    post.reactions.push(PostReaction {
                        emoji: emoji.clone(),
                        count: 1,
                        mine: true,
                    });
                }
            },
            cx,
        );
        if changed {
            cx.emit(ChannelEvent::ToggleReaction { post: id, emoji });
        }
    }

    fn toggle_save(&mut self, id: SharedString, cx: &mut Context<Self>) {
        if self.mutate_post(&id, |post| post.saved = !post.saved, cx) {
            cx.emit(ChannelEvent::ToggleSave(id));
        }
    }

    fn toggle_pin(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let mut now_pinned = false;
        let changed = self.mutate_post(
            &id,
            |post| {
                post.pinned = !post.pinned;
                now_pinned = post.pinned;
            },
            cx,
        );
        if changed {
            if let Some(channel) = self.model.as_mut() {
                channel.pinned = if now_pinned {
                    channel.pinned + 1
                } else {
                    channel.pinned.saturating_sub(1)
                };
            }
            cx.emit(ChannelEvent::TogglePin(id));
        }
    }

    /// Removes the post from the transcript and tells sync. The rows are
    /// rebuilt because the dividers around it may go too.
    fn delete_post(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut model) = self.model.clone() else {
            return;
        };
        let Some(at) = model.posts.iter().position(|post| post.id == id) else {
            return;
        };
        let reached_server = model.posts[at].status == PostStatus::Sent;
        model.posts.remove(at);
        // The "New messages" line stays above the same post.
        model.first_unread = model.first_unread.and_then(|first| {
            let first = if at < first { first - 1 } else { first };
            (first < model.posts.len()).then_some(first)
        });
        self.set_model(Some(model), window, cx);
        if reached_server {
            cx.emit(ChannelEvent::Delete(id));
        }
    }

    /// Opens the shared menu at the pointer with `items`.
    fn open_menu(
        &mut self,
        items: Vec<MenuItem>,
        at: gpui_kit::Point<gpui_kit::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu.update(cx, |menu, cx| {
            menu.set_items(items, cx);
            menu.open_at(Some(at), window, cx);
        });
    }

    fn reaction_items_for(post: &Post, cx: &Context<Self>) -> Vec<MenuItem> {
        let view = cx.entity().downgrade();
        let id = post.id.clone();
        QUICK_REACTIONS
            .iter()
            .map(|(emoji, name)| {
                let view = view.clone();
                let id = id.clone();
                let emoji: SharedString = (*emoji).into();
                let label: SharedString = format!("{emoji}  {name}").into();
                MenuItem::new(
                    gpui_kit::ElementId::Name(format!("react.{name}").into()),
                    label,
                )
                .checked(post.reactions.iter().any(|r| r.emoji == emoji && r.mine))
                .on_click(move |_, _, cx| {
                    let (view, id, emoji) = (view.clone(), id.clone(), emoji.clone());
                    view.update(cx, |this, cx| this.toggle_reaction(id, emoji, cx))
                        .ok();
                })
            })
            .collect()
    }

    fn more_items_for(post: &Post, cx: &Context<Self>) -> Vec<MenuItem> {
        let view = cx.entity().downgrade();
        let id = post.id.clone();
        let act = |name: &'static str, label: SharedString, run: MenuAction| {
            let view = view.clone();
            MenuItem::new(
                gpui_kit::ElementId::Name(format!("post.menu.{name}").into()),
                label,
            )
            .on_click(move |_, window, cx| {
                view.update(cx, |this, cx| run(this, window, cx)).ok();
            })
        };
        let body = post.body.clone();
        let mut items = vec![
            act(
                "copy",
                "Copy text".into(),
                Box::new(move |_, _, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(body.to_string()))
                }),
            ),
            {
                let id = id.clone();
                act(
                    "pin",
                    if post.pinned {
                        "Unpin from channel".into()
                    } else {
                        "Pin to channel".into()
                    },
                    Box::new(move |this, _, cx| this.toggle_pin(id.clone(), cx)),
                )
            },
            {
                let id = id.clone();
                act(
                    "save",
                    if post.saved {
                        "Unsave".into()
                    } else {
                        "Save".into()
                    },
                    Box::new(move |this, _, cx| this.toggle_save(id.clone(), cx)),
                )
            },
        ];
        if post.mine {
            let edit_id = id.clone();
            let delete_id = id.clone();
            items.push(MenuItem::separator());
            items.push(act(
                "edit",
                "Edit".into(),
                Box::new(move |_, _, cx| cx.emit(ChannelEvent::Edit(edit_id.clone()))),
            ));
            items.push(act(
                "delete",
                "Delete…".into(),
                Box::new(move |this, window, cx| {
                    this.deleting = Some(delete_id.clone());
                    this.delete.open(window, cx);
                    cx.notify();
                }),
            ));
        }
        items
    }

    /// "Delete message?" with the action named in the button (PLAN §4.5).
    fn delete_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let channel: SharedString = match self.model.as_ref() {
            Some(channel) => match channel.kind {
                ChannelKind::Direct { .. } | ChannelKind::Group { .. } => {
                    "this conversation".into()
                }
                _ => format!("#{}", channel.name).into(),
            },
            None => "the channel".into(),
        };
        let popup = dialog_popup("post.delete.popup", "Delete message", cx)
            .child(dialog_title("post.delete.title", cx).child("Delete this message?"))
            .child(
                dialog_description("post.delete.description", cx).child(format!(
                    "It is removed for everyone in {channel}. This cannot be undone."
                )),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        alert_dialog_cancel("post.delete.cancel")
                            .variant(ButtonVariant::Outline)
                            .label("Cancel"),
                    )
                    .child(
                        alert_dialog_action("post.delete.confirm")
                            .variant(ButtonVariant::Destructive)
                            .label("Delete"),
                    ),
            );
        let view = cx.entity().downgrade();
        alert_dialog("post.delete", &self.delete, popup, window, cx).on_ok(move |_, window, cx| {
            view.update(cx, |this, cx| {
                if let Some(id) = this.deleting.take() {
                    this.delete_post(id, window, cx);
                }
            })
            .ok();
            true
        })
    }

    fn header(&self, channel: &ChannelModel, cx: &App) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let muted = theme.colors.muted_foreground;
        let icon = |glyph: LucideIcon| {
            lucide(glyph)
                .size(scaled(&theme, metrics::ICON))
                .text_color(muted)
        };
        let action = |id: &'static str, label: &'static str, glyph: LucideIcon| {
            Button::new(id)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::IconSm)
                .aria_label(label)
                .child(icon(glyph))
        };
        let kind_icon = match channel.kind {
            ChannelKind::Public => Some(LucideIcon::Hash),
            ChannelKind::Private => Some(LucideIcon::Lock),
            ChannelKind::Direct { .. } | ChannelKind::Group { .. } => None,
        };
        let count = |id: &'static str, label: String, glyph: LucideIcon, n: usize| {
            Button::new(id)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::Sm)
                .aria_label(label)
                .px(theme.space(2.))
                .type_style(style::UI, &theme)
                .text_color(muted)
                .child(icon(glyph))
                .child(n.to_string())
        };
        div()
            .flex_shrink_0()
            .h(scaled(&theme, metrics::HEADER_HEIGHT))
            .px(scaled(&theme, metrics::GUTTER) - theme.space(2.))
            .flex()
            .items_center()
            .justify_between()
            .gap(theme.space(2.))
            .border_b(style::hairline())
            .border_color(theme.colors.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .gap(theme.space(1.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space(1.5))
                            .px(theme.space(2.))
                            .type_style(style::TITLE, &theme)
                            .text_color(theme.colors.foreground)
                            .children(kind_icon.map(|glyph| {
                                lucide(glyph)
                                    .size(scaled(&theme, metrics::ICON))
                                    .text_color(muted)
                            }))
                            .child(channel.name.clone()),
                    )
                    .child(
                        Button::new("channel.star")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::IconSm)
                            .aria_label(if channel.favorite {
                                "Remove from favorites"
                            } else {
                                "Add to favorites"
                            })
                            .child(
                                lucide(LucideIcon::Star)
                                    .size(scaled(&theme, 14.))
                                    .text_color(if channel.favorite {
                                        Palette::read(cx).away
                                    } else {
                                        muted
                                    }),
                            ),
                    )
                    .children(channel.header.clone().map(|text| {
                        div()
                            .min_w_0()
                            .truncate()
                            .pl(theme.space(2.))
                            .border_l(style::hairline())
                            .border_color(theme.colors.border)
                            .type_style(style::UI, &theme)
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
                    .child(count(
                        "channel.members",
                        format!("{} members", channel.members),
                        LucideIcon::Users,
                        channel.members,
                    ))
                    .child(count(
                        "channel.pinned",
                        format!("{} pinned messages", channel.pinned),
                        LucideIcon::Pin,
                        channel.pinned,
                    ))
                    .child(action("channel.info", "Channel info", LucideIcon::Info)),
            )
    }

    fn post_row(post: &Post, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let id = post.id.clone();
        let initial: String = post.author.chars().take(1).collect();
        let thread_id = id.clone();
        let react_items = Self::reaction_items_for(post, cx);
        let more_items = Self::more_items_for(post, cx);
        let save_id = id.clone();
        let actions = message_actions(cx)
            .child(
                message_action(
                    gpui_kit::ElementId::Name(format!("post.{id}.react").into()),
                    LucideIcon::FaceSlightlySmilingPlus,
                    "Add reaction",
                    cx,
                )
                .on_click(cx.listener(
                    move |this, event: &gpui_kit::ClickEvent, window, cx| {
                        this.open_menu(react_items.clone(), event.position(), window, cx)
                    },
                )),
            )
            .child(
                message_action(
                    gpui_kit::ElementId::Name(format!("post.{id}.reply").into()),
                    LucideIcon::MessageSquareReply,
                    "Reply in thread",
                    cx,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(ChannelEvent::OpenThread(thread_id.clone()))
                })),
            )
            .child(
                message_action(
                    gpui_kit::ElementId::Name(format!("post.{id}.save").into()),
                    if post.saved {
                        LucideIcon::BookmarkCheck
                    } else {
                        LucideIcon::Bookmark
                    },
                    if post.saved { "Unsave" } else { "Save" },
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_save(save_id.clone(), cx))),
            )
            .child(
                message_action(
                    gpui_kit::ElementId::Name(format!("post.{id}.more").into()),
                    LucideIcon::Ellipsis,
                    "More actions",
                    cx,
                )
                .on_click(cx.listener(
                    move |this, event: &gpui_kit::ClickEvent, window, cx| {
                        this.open_menu(more_items.clone(), event.position(), window, cx)
                    },
                )),
            );
        let reaction_post = id.clone();
        let view = cx.entity().downgrade();
        let retry_view = view.clone();
        let retry_id = id.clone();
        let mut row = Message::new(gpui_kit::ElementId::Name(format!("post.{id}").into()))
            .author(post.author.clone())
            .timestamp(post.time.clone())
            .gutter_time(post.time.clone())
            .continuation(post.continues)
            .highlight(if post.mentions_me {
                MessageHighlight::Mention
            } else {
                MessageHighlight::None
            })
            .avatar(
                Avatar::new(gpui_kit::ElementId::Name(
                    format!("post.{id}.avatar").into(),
                ))
                .size(AvatarSize::Default)
                .fallback(initial)
                .aria_label(post.author.clone()),
            )
            .reactions(post.reactions.iter().map(|reaction| {
                Reaction::new(reaction.emoji.clone(), reaction.count, reaction.mine)
            }))
            .on_reaction(move |emoji, _, cx| {
                let (id, emoji) = (reaction_post.clone(), emoji.clone());
                view.update(cx, |this, cx| this.toggle_reaction(id, emoji, cx))
                    .ok();
            })
            .status(match post.status {
                PostStatus::Sent => MessageStatus::Sent,
                PostStatus::Pending => MessageStatus::Pending,
                PostStatus::Failed => MessageStatus::Failed,
            })
            .on_retry({
                let (view, id) = (retry_view.clone(), retry_id.clone());
                move |_, _, cx| {
                    let id = id.clone();
                    view.update(cx, |this, cx| this.retry(id, cx)).ok();
                }
            })
            .on_delete({
                let (view, id) = (retry_view, retry_id);
                move |_, window, cx| {
                    let id = id.clone();
                    view.update(cx, |this, cx| this.delete_post(id, window, cx))
                        .ok();
                }
            })
            .actions(actions)
            .px(scaled(&theme, metrics::GUTTER))
            .child(markdown(
                gpui_kit::ElementId::Name(format!("post.{id}.body").into()),
                post.body.clone(),
                cx,
            ));
        if post.edited {
            row = row.header_child(
                div()
                    .type_style(style::META, &theme)
                    .text_color(theme.colors.muted_foreground)
                    .child("(edited)"),
            );
        }
        if !post.reactions.is_empty() {
            // The chip row gets an add button; posts without reactions use
            // the hover toolbar, so the transcript stays quiet.
            row = row.on_add_reaction(|_, _, _| {});
        }
        if let Some(summary) = &post.replies {
            row = row.child(Self::reply_footer(&id, summary, cx));
        }
        row
    }

    /// The thread line under a post with replies: who replied, how many,
    /// when last, and Follow. Clicking it opens the thread.
    fn reply_footer(
        id: &SharedString,
        summary: &ReplySummary,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let label = match summary.count {
            1 => "1 reply".to_string(),
            n => format!("{n} replies"),
        };
        let open_id = id.clone();
        let follow_id = id.clone();
        div()
            .id(gpui_kit::ElementId::Name(
                format!("post.{id}.thread").into(),
            ))
            .mt(theme.space(1.))
            .ml(px(-6.))
            .px(theme.space(1.5))
            .py(theme.space(1.))
            .rounded(theme.radius.md)
            .w_auto()
            .flex()
            .items_center()
            .gap(theme.space(2.))
            .cursor_pointer()
            .hover(|style| style.bg(theme.colors.accent))
            .on_click(
                cx.listener(move |_, _, _, cx| cx.emit(ChannelEvent::OpenThread(open_id.clone()))),
            )
            .child(
                div().flex().items_center().children(
                    summary
                        .participants
                        .iter()
                        .take(3)
                        .enumerate()
                        .map(|(index, name)| {
                            let initial: String = name.chars().take(1).collect();
                            div().when(index > 0, |this| this.ml(px(-4.))).child(
                                Avatar::new(gpui_kit::ElementId::Name(
                                    format!("post.{id}.thread.{index}").into(),
                                ))
                                .size(AvatarSize::Sm)
                                .fallback(initial)
                                .aria_label(name.clone()),
                            )
                        }),
                ),
            )
            .child(
                div()
                    .type_style(style::UI_STRONG, &theme)
                    .text_color(theme.colors.primary)
                    .child(label),
            )
            .when(summary.unread, |this| {
                this.child(
                    div()
                        .size(theme.space(2.))
                        .rounded_full()
                        .bg(theme.colors.primary),
                )
            })
            .child(
                div()
                    .type_style(style::META, &theme)
                    .text_color(theme.colors.muted_foreground)
                    .child(format!("Last reply {}", summary.last_time)),
            )
            .child(
                Button::new(gpui_kit::ElementId::Name(
                    format!("post.{id}.thread.follow").into(),
                ))
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::Xs)
                .child(if summary.following {
                    "Following"
                } else {
                    "Follow"
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_follow(follow_id.clone(), cx);
                    cx.stop_propagation();
                })),
            )
    }

    fn toggle_follow(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let mut following = None;
        self.mutate_post(
            &id,
            |post| {
                if let Some(summary) = post.replies.as_mut() {
                    summary.following = !summary.following;
                    following = Some(summary.following);
                }
            },
            cx,
        );
        if let Some(following) = following {
            cx.emit(ChannelEvent::ToggleFollow {
                post: id,
                following,
            });
        }
    }

    fn transcript(
        &self,
        channel: &ChannelModel,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let rows = self.rows.clone();
        let posts = channel.posts.clone();
        let view = cx.entity();
        MessageList::new(
            "channel.transcript",
            &self.list,
            move |index, _, cx| match rows.get(index) {
                Some(Row::Day(label)) => day_divider(
                    gpui_kit::ElementId::Name(format!("day.{label}").into()),
                    label.clone(),
                    cx,
                )
                .into_any_element(),
                Some(Row::NewMessages) => {
                    new_messages_divider("new-messages", "New messages", cx).into_any_element()
                }
                Some(Row::Post(post)) => {
                    let post = posts[*post].clone();
                    view.update(cx, |_, cx| Self::post_row(&post, cx).into_any_element())
                }
                None => div().into_any_element(),
            },
        )
        .size_full()
        .pt(theme.space(2.))
        .pb(theme.space(1.))
    }

    fn composer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
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
        let max = self.max_post_size();
        let over = self.draft_len > max;
        // The counter appears in the last tenth and stays while over.
        let counter = (over || self.draft_len + max / 10 >= max).then(|| {
            div()
                .type_style(style::META, &theme)
                .text_color(if over {
                    theme.colors.destructive
                } else {
                    theme.colors.muted_foreground
                })
                .child(format!("{} / {}", self.draft_len, max))
        });
        let emoji_items: Vec<MenuItem> = QUICK_REACTIONS
            .iter()
            .map(|(emoji, name)| {
                let view = cx.entity().downgrade();
                let glyph: SharedString = (*emoji).into();
                MenuItem::new(
                    gpui_kit::ElementId::Name(format!("composer.emoji.{name}").into()),
                    format!("{emoji}  {name}"),
                )
                .on_click(move |_, window, cx| {
                    let glyph = glyph.clone();
                    view.update(cx, |this, cx| {
                        this.composer.update(cx, |composer, cx| {
                            composer.insert(format!("{glyph} "), window, cx);
                            composer.focus(window, cx);
                        })
                    })
                    .ok();
                })
            })
            .collect();
        let mut key = gpui_kit::KeyContext::new_with_defaults(cx.platform_info().os);
        key.add("Composer");
        if self.draft_len == 0 {
            key.add("empty");
        }
        div()
            .key_context(key)
            .on_action(cx.listener(|this, _: &EditLastMessage, _, cx| {
                if let Some(id) = this.last_own_post() {
                    cx.emit(ChannelEvent::Edit(id));
                }
            }))
            .on_action(cx.listener(|this, _: &ReplyToLastMessage, _, cx| {
                if let Some(post) = this.model.as_ref().and_then(|m| m.posts.last()) {
                    cx.emit(ChannelEvent::OpenThread(post.id.clone()));
                }
            }))
            .flex_shrink_0()
            .px(scaled(&theme, metrics::GUTTER))
            .pt(theme.space(1.))
            .pb(theme.space(4.))
            .child(
                Composer::new(&self.composer)
                    .rounded(theme.radius.lg)
                    .child(
                        tool("composer.attach", "Attach files", LucideIcon::Plus).on_click(
                            |_, window, cx| window.dispatch_action(Box::new(UploadFiles), cx),
                        ),
                    )
                    .child(tool("composer.bold", "Bold", LucideIcon::Bold).on_click(
                        cx.listener(|this, _, window, cx| this.wrap("**", "**", window, cx)),
                    ))
                    .child(
                        tool("composer.italic", "Italic", LucideIcon::Italic).on_click(
                            cx.listener(|this, _, window, cx| this.wrap("_", "_", window, cx)),
                        ),
                    )
                    .child(tool("composer.code", "Code", LucideIcon::Code).on_click(
                        cx.listener(|this, _, window, cx| this.wrap("`", "`", window, cx)),
                    ))
                    .child(
                        tool("composer.emoji", "Emoji", LucideIcon::FaceSlightlySmiling).on_click(
                            cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                                this.open_menu(emoji_items.clone(), event.position(), window, cx)
                            }),
                        ),
                    )
                    .child(
                        tool("composer.mention", "Mention someone", LucideIcon::AtSign).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.composer.update(cx, |composer, cx| {
                                    composer.insert("@", window, cx);
                                    composer.focus(window, cx);
                                })
                            }),
                        ),
                    )
                    .child(div().flex_1())
                    .children(counter)
                    .child(
                        Button::new("composer.send")
                            .variant(ButtonVariant::Default)
                            .size(ButtonSize::IconSm)
                            .aria_label("Send")
                            .disabled(!self.can_send())
                            .on_click(cx.listener(|this, _, window, cx| {
                                let text = this.composer.read(cx).value(cx);
                                this.send(text, window, cx)
                            }))
                            .child(
                                lucide(LucideIcon::SendHorizontal)
                                    .size(scaled(&theme, metrics::ICON))
                                    .text_color(theme.colors.primary_foreground),
                            ),
                    ),
            )
    }

    /// Skeleton rows shaped like grouped posts, in place of a spinner.
    fn loading(cx: &App) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let gutter = scaled(&theme, metrics::GUTTER);
        let row = |index: usize, lines: &[f32]| {
            let avatar = scaled(&theme, metrics::AVATAR);
            div()
                .flex()
                .gap(theme.space(3.))
                .px(gutter)
                .py(theme.space(2.))
                .child(
                    Skeleton::new(gpui_kit::ElementId::Name(
                        format!("channel.loading.{index}.avatar").into(),
                    ))
                    .size(avatar)
                    .rounded_full(),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(theme.space(2.))
                        .pt(theme.space(1.))
                        .child(
                            Skeleton::new(gpui_kit::ElementId::Name(
                                format!("channel.loading.{index}.author").into(),
                            ))
                            .w(theme.space(28.))
                            .h(theme.space(3.)),
                        )
                        .children(lines.iter().enumerate().map(|(line, fraction)| {
                            Skeleton::new(gpui_kit::ElementId::Name(
                                format!("channel.loading.{index}.{line}").into(),
                            ))
                            .w(gpui_kit::relative(*fraction))
                            .h(theme.space(3.))
                        })),
                )
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_end()
            .pb(theme.space(2.))
            .child(row(0, &[0.72, 0.4]))
            .child(row(1, &[0.55]))
            .child(row(2, &[0.8, 0.64, 0.3]))
            .child(row(3, &[0.46]))
    }

    /// A channel with no posts yet: its purpose and a way to bring people in.
    fn empty_channel(channel: &ChannelModel, cx: &App) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let direct = matches!(channel.kind, ChannelKind::Direct { .. });
        let media = div()
            .size(theme.space(10.))
            .rounded_full()
            .bg(theme.colors.muted)
            .flex()
            .items_center()
            .justify_center()
            .child(
                lucide(if direct {
                    LucideIcon::MessageSquare
                } else {
                    LucideIcon::Hash
                })
                .size(scaled(&theme, metrics::ICON_LARGE))
                .text_color(theme.colors.muted_foreground),
            );
        let title: SharedString = if direct {
            format!(
                "This is the start of your conversation with {}",
                channel.name
            )
            .into()
        } else {
            format!("Welcome to #{}", channel.name).into()
        };
        let description: SharedString = channel
            .purpose
            .clone()
            .unwrap_or_else(|| {
                if direct {
                    "Messages here stay between the two of you.".into()
                } else {
                    "Nothing has been posted yet. Say hello, or set a purpose so people know what belongs here.".into()
                }
            });
        let mut header = EmptyHeader::new()
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
            );
        let _ = &mut header;
        let mut empty = Empty::new().header(header);
        if !direct {
            empty = empty.content(
                EmptyContent::new().child(
                    Button::new("channel.empty.add-members")
                        .variant(ButtonVariant::Outline)
                        .size(ButtonSize::Sm)
                        .aria_label("Add members")
                        .child(
                            lucide(LucideIcon::UserPlus)
                                .size(scaled(&theme, 14.))
                                .text_color(theme.colors.muted_foreground),
                        )
                        .child("Add members"),
                ),
            );
        }
        div()
            .size_full()
            .flex()
            .items_end()
            .justify_center()
            .p(theme.space(6.))
            .pb(theme.space(8.))
            .child(div().max_w(scaled(&theme, 420.)).child(empty))
    }

    fn empty(cx: &App) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let header = div()
            .flex_shrink_0()
            .h(scaled(&theme, metrics::HEADER_HEIGHT))
            .px(scaled(&theme, metrics::GUTTER))
            .flex()
            .items_center()
            .gap(theme.space(1.5))
            .border_b(style::hairline())
            .border_color(theme.colors.border)
            .type_style(style::TITLE, &theme)
            .text_color(theme.colors.muted_foreground)
            .child(
                lucide(LucideIcon::Hash)
                    .size(scaled(&theme, metrics::ICON))
                    .text_color(theme.colors.muted_foreground),
            )
            .child("No channel open");
        let media = div()
            .size(theme.space(10.))
            .rounded_full()
            .bg(theme.colors.muted)
            .flex()
            .items_center()
            .justify_center()
            .child(
                lucide(LucideIcon::MessagesSquare)
                    .size(scaled(&theme, metrics::ICON_LARGE))
                    .text_color(theme.colors.muted_foreground),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
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
                                .media(EmptyMedia::new().child(media))
                                .title(
                                    EmptyTitle::new()
                                        .type_style(style::TITLE, &theme)
                                        .child("Not connected yet"),
                                )
                                .description(
                                    EmptyDescription::new()
                                        .type_style(style::META, &theme)
                                        .child("Your channels appear here once Matty is connected to a Mattermost server."),
                                ),
                        ),
                    ),
            )
    }
}

impl Render for ChannelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let root = div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(theme.colors.background)
            .on_action(cx.listener(|this, _: &FocusComposer, window, cx| {
                this.composer
                    .update(cx, |composer, cx| composer.focus(window, cx))
            }));
        let Some(channel) = self.model.clone() else {
            return root.child(Self::empty(cx));
        };
        let body = if channel.loading && channel.posts.is_empty() {
            Self::loading(cx).into_any_element()
        } else if channel.posts.is_empty() {
            Self::empty_channel(&channel, cx).into_any_element()
        } else {
            self.transcript(&channel, cx).into_any_element()
        };
        root.child(self.header(&channel, cx))
            .child(div().flex_1().min_h_0().child(body))
            .child(self.composer(cx))
            // Zero-size host for the shared post menu; it opens at the pointer.
            .child(Menu::new(&self.menu, "Message actions").context_area(div()))
            .child(self.delete_dialog(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(id: &str, day: &str) -> Post {
        Post {
            id: id.into(),
            author: "a".into(),
            time: "10:00".into(),
            day: day.into(),
            body: "hi".into(),
            reactions: vec![],
            mentions_me: false,
            edited: false,
            continues: false,
            mine: false,
            saved: false,
            pinned: false,
            status: PostStatus::Sent,
            replies: None,
        }
    }

    #[test]
    fn rows_interleave_day_and_new_message_dividers() {
        let channel = ChannelModel {
            purpose: None,
            loading: false,
            max_post_size: DEFAULT_MAX_POST_SIZE,
            id: "c".into(),
            name: "c".into(),
            kind: ChannelKind::Public,
            header: None,
            members: 1,
            pinned: 0,
            favorite: false,
            posts: vec![
                post("1", "Yesterday"),
                post("2", "Yesterday"),
                post("3", "Today"),
            ],
            first_unread: Some(2),
        };
        assert_eq!(
            channel.rows(),
            vec![
                Row::Day("Yesterday".into()),
                Row::Post(0),
                Row::Post(1),
                Row::Day("Today".into()),
                Row::NewMessages,
                Row::Post(2),
            ]
        );
    }
}
