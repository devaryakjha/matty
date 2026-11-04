//! The channel view: header, transcript and composer (DESIGN.md §4). The
//! view draws a [`ChannelModel`] that sync will fill; until then it shows
//! the empty state, or the preview fixtures with `MATTY_PREVIEW=1`.

use gpui_icons::{LucideIcon, lucide};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, ClipboardEntry, Context, Entity, EventEmitter, ExternalPaths,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Pixels, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::actions::{
    self, EditLastMessage, FocusComposer, MarkChannelRead, ReplyToLastMessage, UploadFiles,
};
use crate::attachments::{
    AttachmentStatus, PostFile, SendGate, Upload, UploadSource, UploadTray, post_files,
};
use crate::channel_settings::{
    ChannelEditEvent, ChannelEditView, ChannelField, NotificationsEvent, NotificationsView,
};
use crate::directory::{AddMembersEvent, AddMembersView};
use crate::profile::{ProfileActions, UserProfile, profile_card};
use crate::settings::{ClockFormat, MessageDisplay, SendKey, SettingsChanged, SettingsStore};
use crate::sidebar::{ChannelKind, ChannelRow};
use crate::style::{self, Palette, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    alert_dialog::{alert_dialog, alert_dialog_action, alert_dialog_cancel},
    attachment::Attachment,
    avatar::{Avatar, AvatarSize},
    button::{Button, ButtonSize, ButtonVariant, style_button},
    composer::{Composer, ComposerEvent, ComposerState, ComposerSuggestion},
    dialog::{DialogHandle, dialog, dialog_description, dialog_popup, dialog_title},
    embed::MessageEmbed,
    emoji_picker::{EmojiPicker, EmojiPickerEvent, EmojiPickerState},
    empty::EmptyContent,
    lightbox::{LightboxImage, LightboxState, lightbox},
    menu::{Menu, MenuItem, MenuState},
    message::{
        Message, MessageDensity, MessageHighlight, MessageStatus, Reaction, day_divider,
        message_action, message_actions, new_messages_divider,
    },
    message_list::{MessageList, MessageListState},
    popover::{hover_popover, popover},
    skeleton::Skeleton,
    spinner::Spinner,
    theme::UiTheme,
    tooltip::text_tooltip,
};

/// One reaction chip under a post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostReaction {
    pub emoji: SharedString,
    pub count: usize,
    /// Whether the current user is among the reactors.
    pub mine: bool,
    /// Display names of the other people who reacted, for the tooltip;
    /// may be shorter than `count` while sync is still loading them.
    pub users: Vec<SharedString>,
}

impl PostReaction {
    /// "You, Priya Nair and Sam Okafor reacted with 👍", shortening past
    /// three names to "and 2 others".
    pub fn summary(&self) -> String {
        let mut names: Vec<String> = Vec::new();
        if self.mine {
            names.push("You".to_string());
        }
        names.extend(self.users.iter().map(|user| user.to_string()));
        let unnamed = self.count.saturating_sub(names.len());
        let shown = names.len().min(3);
        let others = names.len() - shown + unnamed;
        let mut parts: Vec<String> = names.into_iter().take(shown).collect();
        if others > 0 {
            parts.push(match others {
                1 => "1 other".to_string(),
                n => format!("{n} others"),
            });
        }
        let who = match parts.len() {
            0 => "Nobody".to_string(),
            1 => parts.remove(0),
            _ => {
                let last = parts.pop().unwrap_or_default();
                format!("{} and {last}", parts.join(", "))
            }
        };
        format!("{who} reacted with {}", self.emoji)
    }
}

/// The header markers after a post's time: "Pinned", "Saved" and "(edited)",
/// in that order, as Mattermost shows them. Pinned and Saved carry their
/// icon so they read at a glance without competing with the author. The
/// icon is its own header child because the header aligns on baselines and
/// a nested row would sit low.
pub fn post_markers(post: &Post, theme: &UiTheme) -> Vec<gpui_kit::AnyElement> {
    let muted = theme.colors.muted_foreground;
    let label = |text: &'static str| {
        div()
            .type_style(style::META, theme)
            .text_color(muted)
            .child(text)
            .into_any_element()
    };
    let icon = |icon: LucideIcon| {
        lucide(icon)
            .size(scaled(theme, metrics::ICON_TINY))
            .text_color(muted)
            .into_any_element()
    };
    let mut markers = Vec::new();
    if post.status == PostStatus::Ephemeral {
        markers.extend([icon(LucideIcon::EyeOff), label("Only visible to you")]);
    }
    if post.pinned {
        markers.extend([icon(LucideIcon::Pin), label("Pinned")]);
    }
    if post.saved {
        markers.extend([icon(LucideIcon::Bookmark), label("Saved")]);
    }
    if post.edited {
        markers.push(label("(edited)"));
    }
    markers
}

/// Whether the post is the user's own and in the outbox but not on its
/// way: queued while offline, or refused. It can be edited in place or
/// discarded before it is sent (#195).
pub fn waiting_to_send(post: &Post, away: bool) -> bool {
    post.mine
        && post.kind == PostKind::Regular
        && match post.status {
            PostStatus::Sent | PostStatus::Ephemeral => false,
            PostStatus::Pending => away,
            PostStatus::Failed => true,
        }
}

/// Whether the post waits for the connection to come back (#195). It
/// never folds under the post before it, so its clock and "Will send
/// when back online" show even in a run of the user's own posts.
pub fn queued(post: &Post, away: bool) -> bool {
    post.status == PostStatus::Pending && away
}

/// Whether the user may delete the post: their own, sent or waiting to
/// send, or anyone's sent post when `delete_others` (#106).
pub(crate) fn deletable(post: &Post, away: bool, delete_others: bool) -> bool {
    if post.mine {
        return post.status == PostStatus::Sent || waiting_to_send(post, away);
    }
    delete_others && post.status == PostStatus::Sent && post.kind == PostKind::Regular
}

/// The clock in a queued post's header (#195).
pub(crate) fn queued_marker(theme: &UiTheme) -> impl IntoElement + use<> {
    lucide(LucideIcon::Clock)
        .size(scaled(theme, metrics::ICON_TINY))
        .text_color(theme.colors.muted_foreground)
}

/// A post's header time, or where it is on its way to the server.
pub(crate) fn post_time(post: &Post, away: bool, cx: &App) -> SharedString {
    match post.status {
        PostStatus::Sent | PostStatus::Ephemeral => {
            ClockFormat::current(cx).format(post.time).into()
        }
        PostStatus::Pending if away => "Will send when back online".into(),
        PostStatus::Pending => "Sending…".into(),
        PostStatus::Failed => "Not sent".into(),
    }
}

/// Edit and Discard… for a post waiting to send (#195), after a
/// separator; nothing for any other post. `prefix` scopes the item ids.
pub(crate) fn waiting_items(
    prefix: &str,
    post: &Post,
    away: bool,
    edit: impl Fn(&mut Window, &mut App) + 'static,
    discard: impl Fn(&mut Window, &mut App) + 'static,
) -> Vec<MenuItem> {
    if !waiting_to_send(post, away) {
        return Vec::new();
    }
    vec![
        MenuItem::separator(),
        MenuItem::new(
            gpui_kit::ElementId::Name(format!("{prefix}.edit").into()),
            "Edit",
        )
        .on_click(move |_, window, cx| edit(window, cx)),
        MenuItem::new(
            gpui_kit::ElementId::Name(format!("{prefix}.discard").into()),
            "Discard…",
        )
        .on_click(move |_, window, cx| discard(window, cx)),
    ]
}

/// A composer holding `body` to edit a post in place: the format
/// shortcuts apply and Escape runs `cancel`.
pub(crate) fn edit_composer(
    body: SharedString,
    cancel: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ComposerState> {
    let composer = cx.new(|cx| {
        let this = cx.weak_entity();
        let mut state = ComposerState::new(12, window, cx);
        state.set_value(body, window, cx);
        state.set_on_key(move |keystroke, window, cx| {
            if keystroke.key == "escape" {
                cancel(window, cx);
                return true;
            }
            this.upgrade()
                .is_some_and(|editor| apply_format(&editor, keystroke, window, cx))
        });
        state
    });
    crate::updater::watch_composer(&composer, cx);
    let focus = composer.clone();
    window.on_next_frame(move |window, cx| {
        focus.update(cx, |composer, cx| {
            let end = composer.value(cx).len();
            composer.set_selection(end..end, cx);
            composer.focus(window, cx);
        });
    });
    composer
}

/// The in-place editor for a post: its composer, the key hint and the
/// Cancel and Save buttons. `prefix` scopes the button ids.
pub(crate) fn post_editor(
    prefix: &str,
    composer: &Entity<ComposerState>,
    cap: Pixels,
    cancel: impl Fn(&mut Window, &mut App) + 'static,
    save: impl Fn(SharedString, &mut Window, &mut App) + 'static,
    cx: &App,
) -> gpui_kit::Div {
    let theme = UiTheme::read(cx).clone();
    let text = composer.clone();
    div()
        .flex()
        .flex_col()
        .gap(theme.space(2.))
        .pt(theme.space(1.))
        .child(
            Composer::new(composer)
                .max_height(cap)
                .rounded(theme.radius.md),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space(2.))
                .child(
                    div()
                        .flex_1()
                        .type_style(style::META, &theme)
                        .text_color(theme.colors.muted_foreground)
                        .child("Enter to save · Escape to cancel"),
                )
                .child(
                    Button::new(gpui_kit::ElementId::Name(format!("{prefix}.cancel").into()))
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Sm)
                        .label("Cancel")
                        .on_click(move |_, window, cx| cancel(window, cx)),
                )
                .child(
                    Button::new(gpui_kit::ElementId::Name(format!("{prefix}.save").into()))
                        .variant(ButtonVariant::Default)
                        .size(ButtonSize::Sm)
                        .label("Save")
                        .on_click(move |_, window, cx| {
                            let value = text.read(cx).value(cx);
                            save(value, window, cx)
                        }),
                ),
        )
}

/// "Delete this message?", or "Discard this message?" for one the server
/// never got (#195), naming `place` it is removed from. `prefix` scopes
/// the ids.
/// What deleting a sent post does: gone for everyone in `place`, and a
/// root takes its `replies` with it, as on the server (#118).
fn delete_description(place: &str, replies: usize) -> String {
    match replies {
        0 => format!("It is removed for everyone in {place}. This cannot be undone."),
        1 => format!(
            "It is removed for everyone in {place}, along with its reply. This cannot be undone."
        ),
        n => format!(
            "It is removed for everyone in {place}, along with its {n} replies. This cannot be undone."
        ),
    }
}

pub(crate) fn delete_popup(
    prefix: &str,
    unsent: bool,
    place: &str,
    replies: usize,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = UiTheme::read(cx).clone();
    let id = |part: &str| gpui_kit::ElementId::Name(format!("{prefix}.{part}").into());
    let (label, title, description, action): (_, _, SharedString, _) = if unsent {
        (
            "Discard message",
            "Discard this message?",
            "It has not been sent, so nobody else has seen it. This cannot be undone.".into(),
            "Discard",
        )
    } else {
        (
            "Delete message",
            "Delete this message?",
            delete_description(place, replies).into(),
            "Delete",
        )
    };
    dialog_popup(id("popup"), label, cx)
        .child(dialog_title(id("title"), cx).child(title))
        .child(dialog_description(id("description"), cx).child(description))
        .child(
            div()
                .flex()
                .justify_end()
                .gap(theme.space(2.))
                .pt(theme.space(2.))
                .child(
                    alert_dialog_cancel(id("cancel"))
                        .variant(ButtonVariant::Outline)
                        .label("Cancel"),
                )
                .child(
                    alert_dialog_action(id("confirm"))
                        .variant(ButtonVariant::Destructive)
                        .label(action),
                ),
        )
}

/// The tooltip builder for a message's reaction chips, looking each chip
/// up in `reactions` by emoji.
pub fn reaction_tooltip(
    reactions: Vec<PostReaction>,
) -> impl Fn(&Reaction, &mut Window, &mut App) -> gpui_kit::AnyView + 'static {
    move |reaction, _, cx| {
        let text = reactions
            .iter()
            .find(|r| r.emoji == reaction.emoji)
            .map(PostReaction::summary)
            .unwrap_or_else(|| format!("{} reacted with {}", reaction.count, reaction.emoji));
        text_tooltip(text.into(), cx)
    }
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
    /// Only the viewer sees it and the server does not keep it (#211):
    /// marked "Only visible to you", it can only be copied or dismissed.
    Ephemeral,
}

/// Mattermost's default `MaxPostSize`, until the server's config arrives.
pub const DEFAULT_MAX_POST_SIZE: usize = 16383;

/// How a post draws (#104).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PostKind {
    /// A message someone wrote.
    #[default]
    Regular,
    /// A system message, already worded in `body`: a muted line with no
    /// author, actions or reactions that ends the run of grouped posts.
    /// `activity` marks joins and leaves, which arrive combined into one
    /// row ("Ada, Sam and 3 others joined the channel.").
    System { activity: bool },
    /// A type Matty does not draw yet, such as a plugin's `custom_*`
    /// post: a card that offers to open it in the browser. Holds the type.
    Unsupported(SharedString),
}

/// One post, already formatted for display: sync does the time zone and
/// Markdown rewriting, the view only draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Post {
    pub id: SharedString,
    pub author: SharedString,
    /// The author's user id, for their profile picture; empty when unknown.
    pub author_id: SharedString,
    pub kind: PostKind,
    /// Minutes since midnight in the user's zone; `ClockFormat` prints it.
    pub time: u16,
    /// The day divider label this post falls under ("Today", "Yesterday", a date).
    pub day: SharedString,
    /// The full date for the time's tooltip, such as "Tuesday, November 3,
    /// 2026"; the view adds " at " and the clock time. Empty while a send
    /// is pending.
    pub date: SharedString,
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
    /// The root post's id when this post is a reply; empty for a root.
    pub root_id: SharedString,
    /// What a reply answers, drawn above it in the channel while collapsed
    /// reply threads are off (#123); `None` when the row above is already
    /// in the same thread.
    pub reply_to: Option<ReplyContext>,
    /// Files on the post (PLAN §4.6).
    pub files: Vec<PostFile>,
    /// Link previews and message attachments under the body (#187).
    pub embeds: Vec<crate::embeds::Embed>,
}

/// When "Remind me" brings a post back, as the webapp offers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemindAt {
    In30Minutes,
    In1Hour,
    In2Hours,
    /// Tomorrow at 9:00 in the user's zone.
    Tomorrow,
}

impl RemindAt {
    pub const ALL: [Self; 4] = [
        Self::In30Minutes,
        Self::In1Hour,
        Self::In2Hours,
        Self::Tomorrow,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::In30Minutes => "In 30 minutes",
            Self::In1Hour => "In 1 hour",
            Self::In2Hours => "In 2 hours",
            Self::Tomorrow => "Tomorrow at 9:00",
        }
    }

    /// The moment it names, in Unix seconds, from `now` in `zone`.
    pub fn target(self, now: jiff::Timestamp, zone: &jiff::tz::TimeZone) -> i64 {
        let minutes = match self {
            Self::In30Minutes => 30,
            Self::In1Hour => 60,
            Self::In2Hours => 120,
            Self::Tomorrow => {
                let tomorrow = now
                    .to_zoned(zone.clone())
                    .date()
                    .tomorrow()
                    .ok()
                    .and_then(|day| day.at(9, 0, 0, 0).to_zoned(zone.clone()).ok());
                return tomorrow
                    .map_or(now.as_second() + 24 * 3600, |at| at.timestamp().as_second());
            }
        };
        now.as_second() + minutes * 60
    }
}

/// The root a reply answers, for "Commented on Ada's message: …" above the
/// reply while collapsed reply threads are off (#123).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyContext {
    /// The root author's display name.
    pub author: SharedString,
    /// The root's words as plain text, for one truncated line.
    pub excerpt: SharedString,
}

impl Post {
    /// The thread this post opens: its root's, or its own.
    pub fn thread_root(&self) -> SharedString {
        if self.root_id.is_empty() {
            self.id.clone()
        } else {
            self.root_id.clone()
        }
    }
}

/// What the channel shows about a post's thread without loading it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplySummary {
    pub count: usize,
    /// `HH:MM` of the newest reply.
    /// Minutes since midnight in the user's zone.
    pub last_time: u16,
    /// The day divider label of the last reply ("Today", "Yesterday", a
    /// date); only a reply from today shows its time.
    pub last_day: SharedString,
    /// Display names of the people who replied, newest first, at most three.
    pub participants: Vec<SharedString>,
    /// Their user ids, in the same order, for their pictures.
    pub participant_ids: Vec<SharedString>,
    pub following: bool,
    /// Replies the user has not read yet (#124).
    pub unread_replies: usize,
    /// Mentions of the user among them.
    pub unread_mentions: usize,
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
    /// Muted channels fade in the sidebar and skip notifications.
    pub muted: bool,
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
    /// The user may archive the channel or make it private: a channel or
    /// system admin (#208). The server has the last word.
    pub can_manage: bool,
    /// The user may delete other people's posts: a system admin, as
    /// Mattermost's default `delete_others_posts` allows (#106). Team
    /// admins may too, but their role is not kept. The server has the
    /// last word.
    pub delete_others: bool,
    /// Profiles of the people who posted, for the avatar card and `@`
    /// autocomplete.
    pub profiles: Vec<UserProfile>,
    /// Display names of the people typing right now, for the line under
    /// the transcript.
    pub typing: Vec<SharedString>,
    /// The user's notification settings for this channel.
    pub notifications: ChannelNotifications,
    /// Matty is offline, so no posts may only mean none were saved here
    /// yet (#163).
    pub offline: bool,
}

/// When a channel notifies, per surface; `Default` follows the account
/// setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotifyLevel {
    #[default]
    Default,
    All,
    Mentions,
    None,
}

impl NotifyLevel {
    pub const ALL: [NotifyLevel; 4] = [Self::Default, Self::All, Self::Mentions, Self::None];

    pub fn id(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::All => "all",
            Self::Mentions => "mentions",
            Self::None => "none",
        }
    }

    pub fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|level| level.id() == id)
            .unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Use my default",
            Self::All => "All new messages",
            Self::Mentions => "Mentions and direct messages",
            Self::None => "Nothing",
        }
    }
}

/// A channel's notification preferences, as Mattermost stores them on
/// the membership.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChannelNotifications {
    pub desktop: NotifyLevel,
    pub mobile: NotifyLevel,
    /// Skip @channel, @here and @all in this channel.
    pub ignore_channel_mentions: bool,
    /// Only mentions make the channel bold in the sidebar.
    pub mark_unread_mentions_only: bool,
}

/// A row in the transcript: posts interleaved with dividers.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    /// The top row: loading older messages, the start of the channel, or a
    /// failed page with Retry (PLAN §4.2 history).
    History,
    Day(SharedString),
    NewMessages,
    Post(usize),
    /// A system message (#104); never selectable.
    System(usize),
    /// The bottom row: loading newer messages after a jump into the past,
    /// or a failed page with Retry. Empty while the transcript runs to the
    /// newest post.
    Newer,
}

/// Where the transcript stands in the channel's history.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct History {
    /// The oldest post is loaded; nothing more to ask for.
    pub at_start: bool,
    /// An older page is on its way.
    pub loading: bool,
    /// The last older page failed; the top row offers Retry.
    pub failed: bool,
    /// The newest post is loaded. False after a jump into the past, until
    /// newer pages reach the end.
    pub at_end: bool,
    /// A newer page is on its way.
    pub loading_newer: bool,
    /// The last newer page failed; the bottom row offers Retry.
    pub failed_newer: bool,
}

/// The Markdown a composer formatting shortcut wraps the selection in
/// (PLAN §4.3): Cmd/Ctrl+B bold, +I italic, +Alt+K link (Mattermost's
/// bindings), +Shift+X strikethrough and +Shift+C inline code (Slack's).
/// Cmd/Ctrl+K alone stays the quick switcher.
pub(crate) fn format_shortcut(
    keystroke: &gpui_kit::Keystroke,
) -> Option<(&'static str, &'static str)> {
    let mods = &keystroke.modifiers;
    if !(mods.control || mods.platform) || mods.function {
        return None;
    }
    match (keystroke.key.as_str(), mods.shift, mods.alt) {
        ("b", false, false) => Some(("**", "**")),
        ("i", false, false) => Some(("_", "_")),
        ("k", false, true) => Some(("[", "](url)")),
        ("x", true, false) => Some(("~~", "~~")),
        ("c", true, false) => Some(("`", "`")),
        _ => None,
    }
}

/// When `selection` in `value` is already wrapped in `before` and `after`,
/// either just outside it or as its own ends: the wrapped range to replace
/// with its inside, and the selection after, so the shortcut toggles the
/// format off (#115). A one-character marker next to another of its kind
/// is part of a longer one, such as `` ` `` in a code fence, and stays.
fn unwrap_selection(
    value: &str,
    selection: std::ops::Range<usize>,
    before: &str,
    after: &str,
) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let lone = |at: usize, marker: &str| {
        marker.chars().count() > 1
            || (!value[..at].ends_with(marker) && !value[at + marker.len()..].starts_with(marker))
    };
    // Markers just outside the selection: **|bold|**
    if let (Some(start), Some(end)) = (
        selection.start.checked_sub(before.len()),
        selection.end.checked_add(after.len()),
    ) && value.get(start..selection.start) == Some(before)
        && value.get(selection.end..end) == Some(after)
        && lone(start, before)
        && lone(selection.end, after)
    {
        return Some((start..end, start..selection.end - before.len()));
    }
    // The selection takes the markers in: |**bold**|
    let selected = value.get(selection.clone())?;
    if selected.len() >= before.len() + after.len()
        && selected.starts_with(before)
        && selected.ends_with(after)
    {
        let inner = selected.len() - before.len() - after.len();
        return Some((selection.clone(), selection.start..selection.start + inner));
    }
    None
}

/// Applies a formatting shortcut to `composer` and reports whether the key
/// was one. A link leaves "url" selected so typing replaces it; a selection
/// already in that format loses it.
pub(crate) fn apply_format(
    composer: &Entity<ComposerState>,
    keystroke: &gpui_kit::Keystroke,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some((before, after)) = format_shortcut(keystroke) else {
        return false;
    };
    // The key arrives from inside the composer's own update, so the edit
    // waits until it has let go of its state.
    let composer = composer.clone();
    window.defer(cx, move |window, cx| {
        composer.update(cx, |composer, cx| {
            let value = composer.value(cx);
            if let Some((wrapped, selection)) =
                unwrap_selection(&value, composer.selection(cx), before, after)
            {
                let inside =
                    value[wrapped.start + before.len()..wrapped.end - after.len()].to_string();
                composer.input().update(cx, |input, cx| {
                    input.set_selected_range(wrapped, cx);
                    input.replace(inside, window, cx);
                    input.set_selected_range(selection, cx);
                });
                return;
            }
            composer.wrap_selection(before, after, window, cx);
            if after == "](url)" {
                let end = composer.selection(cx).end;
                composer.set_selection(end + 2..end + 5, cx);
            }
        });
    });
    true
}

/// The user's own id, from a post of theirs on screen, for a send that
/// has not come back from sync yet.
pub fn own_id(posts: &[Post]) -> SharedString {
    posts
        .iter()
        .rev()
        .find(|post| post.mine && !post.author_id.is_empty())
        .map(|post| post.author_id.clone())
        .unwrap_or_default()
}

impl ReplySummary {
    /// The thread has replies the user has not read.
    pub fn unread(&self) -> bool {
        self.unread_replies > 0 || self.unread_mentions > 0
    }

    /// Opening the thread reads it.
    pub fn mark_read(&mut self) {
        self.unread_replies = 0;
        self.unread_mentions = 0;
    }

    /// "08:13" for a reply from today, otherwise its day.
    pub fn last_reply_label(&self, clock: ClockFormat) -> SharedString {
        if self.last_day.as_ref() == "Today" {
            clock.format(self.last_time).into()
        } else {
            self.last_day.clone()
        }
    }
}

/// A row as it compares across edits: a post by its id, since its index
/// shifts when a post above it goes.
#[derive(Debug, PartialEq, Eq)]
enum RowKey {
    Post(SharedString),
    Other(Row),
}

/// The rows of `before` that differ from `after`, and how many rows of
/// `after` take their place: everything between the common head and tail.
fn changed_rows(before: &[RowKey], after: &[RowKey]) -> (std::ops::Range<usize>, usize) {
    let head = before
        .iter()
        .zip(after)
        .take_while(|(old, new)| old == new)
        .count();
    let tail = before[head..]
        .iter()
        .rev()
        .zip(after[head..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    (head..before.len() - tail, after.len() - head - tail)
}

impl ChannelModel {
    fn row_keys(&self, rows: &[Row]) -> Vec<RowKey> {
        rows.iter()
            .map(|row| match row {
                Row::Post(index) | Row::System(index) => {
                    RowKey::Post(self.posts[*index].id.clone())
                }
                other => RowKey::Other(other.clone()),
            })
            .collect()
    }

    /// The newest post someone wrote, for Shift+Up's reply: system rows
    /// have no thread.
    fn last_message(&self) -> Option<&Post> {
        self.posts
            .iter()
            .rev()
            .find(|post| !matches!(post.kind, PostKind::System { .. }))
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::with_capacity(self.posts.len() + 4);
        rows.push(Row::History);
        let mut day: Option<&SharedString> = None;
        for (index, post) in self.posts.iter().enumerate() {
            if day != Some(&post.day) {
                rows.push(Row::Day(post.day.clone()));
                day = Some(&post.day);
            }
            if self.first_unread == Some(index) {
                rows.push(Row::NewMessages);
            }
            rows.push(match post.kind {
                PostKind::System { .. } => Row::System(index),
                _ => Row::Post(index),
            });
        }
        rows.push(Row::Newer);
        rows
    }

    /// Puts an older page before the loaded posts, skipping ids already
    /// shown, and returns how many were added. The unread marker moves with
    /// the post it sits on.
    fn prepend_posts(&mut self, posts: Vec<Post>) -> usize {
        let older: Vec<Post> = posts
            .into_iter()
            .filter(|post| !self.posts.iter().any(|known| known.id == post.id))
            .collect();
        let added = older.len();
        if added > 0 {
            self.posts.splice(0..0, older);
            if let Some(first_unread) = self.first_unread.as_mut() {
                *first_unread += added;
            }
        }
        added
    }
}

/// A post's words without the Markdown, for "Copy text" (PLAN §4.2).
pub fn plain_text(body: &str) -> String {
    matty_markdown::to_plain(&matty_markdown::parse(body))
}

/// The row density from Preferences (PLAN §4.2 display).
pub fn message_density(cx: &App) -> MessageDensity {
    match SettingsStore::global(cx).read(cx).get().display {
        MessageDisplay::Standard => MessageDensity::Standard,
        MessageDisplay::Compact => MessageDensity::Compact,
    }
}

/// What the channel view asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelEvent {
    /// Offline, the user asked for this action (as sync names it), which
    /// needs the server; nothing changed, and the window says so (#163).
    Offline(&'static str),
    /// The user typed `@` and this much of a name; sync asks the server who
    /// matches and [`ChannelView::set_suggested`] takes the answer.
    SuggestPeople(SharedString),
    /// The user changed the composer's text; sync tells the channel at
    /// most every few seconds. `root` is the thread when replying.
    Typing { root: Option<SharedString> },
    /// The user submitted the composer with this text and these uploads
    /// (ids from `Upload`), all of which the view showed as ready.
    Send {
        text: SharedString,
        files: Vec<SharedString>,
        /// False when the user removed the link's preview while composing,
        /// so the post shows none either (#188).
        unfurl: bool,
    },
    /// The user picked, dropped or pasted a file; sync uploads it and
    /// reports back through `set_upload_status`.
    Upload {
        id: SharedString,
        name: SharedString,
        source: UploadSource,
    },
    /// The draft has this link; sync asks the server for its preview and
    /// the window hands it back through `set_link_preview`.
    PreviewLink(SharedString),
    /// The composer's draft changed, after a pause in typing, on leaving
    /// the channel or on sending; sync stores it (#114).
    SaveDraft(crate::composer_draft::DraftSave),
    /// The user removed a file from the tray before sending.
    CancelUpload(SharedString),
    /// The user asked to upload a failed file again.
    RetryUpload(SharedString),
    /// The user clicked a file on a post; sync opens or downloads it.
    OpenFile(SharedString),
    /// The lightbox opened on these image files; sync fetches their full
    /// size and the window hands them back through `set_full_image`.
    ViewImages(Vec<SharedString>),
    /// The user asked to reply to this post in a thread.
    OpenThread(SharedString),
    /// U on a selected post: unread from this post on (PLAN §4.2); sync
    /// tells the server.
    MarkUnread(SharedString),
    /// "Copy link" on a post: the window puts its permalink on the
    /// clipboard, since only sync knows the server and the team (#133).
    CopyLink(SharedString),
    /// "Open in browser" on a post Matty cannot draw: the window opens its
    /// permalink (#104).
    OpenInBrowser(SharedString),
    /// The transcript showed its top row and wants an older page; sync
    /// answers with `prepend_posts` and `set_history`. Fired once per page:
    /// the view marks itself loading until one of those arrives.
    NearTop,
    /// The transcript showed its bottom row while it stops short of the
    /// newest post (after a jump into the past) and wants a newer page;
    /// sync answers with `append_posts` and `set_history`. Fired once per
    /// page.
    NearBottom,
    /// A jump named a post the loaded channel does not hold; sync loads the
    /// posts around it and the jump lands when the new snapshot arrives.
    JumpMissing(SharedString),
    /// The jump pill was pressed while the transcript stops short of the
    /// newest post; sync drops the history window and sends the latest
    /// snapshot, which `set_model` opens at the end.
    JumpLatest,
    /// The user asked for the channel's pinned messages.
    OpenPinned,
    /// The user clicked a link or an entity chip in a message (#103).
    OpenLink(SharedString),
    /// The user asked for the channel info pane (header info button or the
    /// members count).
    OpenInfo,
    /// The user starred or unstarred the channel; the header shows it.
    ToggleFavorite(bool),
    /// The user chose View profile on someone's profile card (#212).
    ViewProfile(UserProfile),
    /// The user chose Message on someone's profile card.
    OpenDirect {
        user_id: SharedString,
        username: SharedString,
        display_name: SharedString,
    },
    /// The user added or removed their reaction; the view already shows it.
    ToggleReaction {
        post: SharedString,
        emoji: SharedString,
    },
    /// Saved or unsaved a post; the view already shows it.
    ToggleSave(SharedString),
    /// Pinned or unpinned a post to the channel; the view already shows it.
    TogglePin(SharedString),
    /// The user saved an edit; the view already shows the new text with
    /// the "(edited)" marker.
    SaveEdit {
        post: SharedString,
        text: SharedString,
    },
    /// The user confirmed deleting their post; the view already removed it.
    Delete(SharedString),
    /// Retry a failed send; the view shows it pending again.
    Retry(SharedString),
    /// Delete on a send that never reached the server: sync drops it
    /// from the outbox. The row is already gone.
    Discard(SharedString),
    /// The user toggled following the thread under `post`.
    ToggleFollow { post: SharedString, following: bool },
    /// The user opened a quoted post: show it in its channel.
    OpenPermalink {
        channel: SharedString,
        post: SharedString,
    },
    /// The user wants to forward `post` to another channel (#213).
    Forward(SharedString),
    /// The user pressed an interactive message's button or picked from its
    /// menu (#211).
    PostAction(crate::interactive::ActionChoice),
    /// The user dismissed an ephemeral message (#211); the row is gone.
    DismissEphemeral(SharedString),
    /// The user asked to be reminded of `post` (#213).
    Remind { post: SharedString, at: RemindAt },
    /// The user renamed the channel from its header menu; the view already
    /// shows the new name.
    Rename(SharedString),
    /// The user changed the header text; the view already shows it.
    SetHeader(SharedString),
    /// The user changed the purpose; the view already shows it.
    SetPurpose(SharedString),
    /// The user confirmed leaving the channel from its header menu.
    Leave,
    /// "Archive channel" was confirmed (#208).
    Archive,
    /// "Convert to private channel" was confirmed (#208).
    ConvertToPrivate,
    /// The user saved the channel's notification preferences; `muted`
    /// is the mute switch from the same dialog.
    SetNotifications {
        muted: bool,
        notifications: ChannelNotifications,
    },
    /// The user added these people from the Add members dialog; the view
    /// already counts them.
    AddMembers(Vec<UserProfile>),
}

/// A post being edited in place: its own composer sits where the body was.
struct Editing {
    post: SharedString,
    composer: Entity<ComposerState>,
    _subscription: gpui_kit::Subscription,
}

/// What a More-menu item does to the view.
type MenuAction = Box<dyn Fn(&mut ChannelView, &mut Window, &mut Context<ChannelView>)>;

/// The reactions one click away, as Mattermost's recent-emoji row. The full
/// picker arrives with the emoji data from sync.
pub(crate) const QUICK_REACTIONS: [(&str, &str); 6] = [
    ("👍", "Thumbs up"),
    ("❤️", "Heart"),
    ("😂", "Joy"),
    ("🎉", "Tada"),
    ("👀", "Eyes"),
    ("🚀", "Rocket"),
];

/// What is behind an image in the lightbox, for its actions.
#[derive(Clone, Debug, PartialEq)]
enum LightboxItem {
    /// A file on the post, by id.
    File(SharedString),
    /// An image the message links to, by URL.
    Link(SharedString),
}

pub struct ChannelView {
    model: Option<ChannelModel>,
    rows: Vec<Row>,
    list: Entity<MessageListState>,
    composer: Entity<ComposerState>,
    /// One menu for every post: its items are set when it opens.
    menu: Entity<MenuState>,
    /// The channel name's menu: rename, header, purpose, leave.
    header_menu: Entity<MenuState>,
    edit: Entity<ChannelEditView>,
    notifications: Entity<NotificationsView>,
    add_members: Entity<AddMembersView>,
    leave: DialogHandle,
    /// Confirmations for archiving the channel and making it private.
    archive: DialogHandle,
    convert: DialogHandle,
    /// The full emoji picker, in a popover on the composer's emoji button.
    picker: Entity<EmojiPickerState>,
    picker_open: bool,
    /// The post a pick reacts to; `None` inserts into the composer.
    picker_target: Option<SharedString>,
    /// Shortcodes picked this session, newest first, for the picker's
    /// Recent row; sync persists them later.
    recent_emoji: Vec<SharedString>,
    /// The sidebar's channels, for `~` autocomplete.
    directory: Vec<ChannelRow>,
    /// What the server said about typed `@` names.
    people: PeopleAsk,
    /// Files waiting in the composer tray, and a send waiting on them.
    tray: UploadTray,
    /// The image viewer over the clicked post's images.
    lightbox: Entity<LightboxState>,
    lightbox_handle: DialogHandle,
    /// What is behind each of the lightbox's images, for its actions.
    lightbox_items: Vec<LightboxItem>,
    /// The post the lightbox shows, and full-size images by file id as
    /// they come; until then the lightbox shows the thumbnail.
    lightbox_post: Option<SharedString>,
    full_images: HashMap<SharedString, Arc<gpui_kit::Image>>,
    /// The post whose body is a composer right now (PLAN §4.3 edit).
    editing: Option<Editing>,
    /// Posts that arrived while the reader was scrolled up; the jump pill
    /// counts them until the list is back at the end.
    unseen: usize,
    /// The post the reader jumped to from a listing or a link, tinted as
    /// a permalink target until the timer below clears it.
    permalink: Option<SharedString>,
    _permalink_task: Option<gpui_kit::Task<()>>,
    /// The preview above the composer (#188): the draft's first link and
    /// its card once the server has read it; links the user removed from
    /// this draft stay removed.
    link: crate::draft_link::DraftLink,
    _link_task: Option<gpui_kit::Task<()>>,
    /// A jump whose post is not in the transcript yet; applied when it
    /// arrives with the channel or a later page.
    pending_jump: Option<SharedString>,
    /// The post sync was asked to load for a jump, so a snapshot that still
    /// lacks it does not ask again.
    jump_asked: Option<SharedString>,
    /// The row chosen with the keyboard (PLAN §4.2); single keys act on it.
    selection: Option<usize>,
    /// The top and bottom rows' state: loading, at an edge, or failed.
    history: History,
    list_focus: gpui_kit::FocusHandle,
    delete: DialogHandle,
    /// The post the delete dialog asks about.
    deleting: Option<SharedString>,
    /// The current user's display name, for optimistic rows.
    me: SharedString,
    /// Characters in the composer, for the Send button and the counter.
    draft_len: usize,
    /// Which channel's draft the composer holds, and what was saved of it.
    draft: crate::composer_draft::ComposerDraft,
    _draft_task: Option<gpui_kit::Task<()>>,
    /// Numbers the optimistic rows until sync assigns ids.
    pending_seq: usize,
    /// Set by the jump pill so the latest snapshot opens at the end.
    open_at_end: bool,
    /// `style::composer_cap` as of the last render.
    composer_cap: Pixels,
    /// The connection is down: sends wait, and their rows say so.
    away: bool,
    /// The banner says the user is offline: actions that need the server
    /// are refused before the transcript changes (#163).
    offline: bool,
    /// Part of every profile card's id: bumping it after a card's action
    /// closes the card, since a new id starts closed.
    profile_cards: usize,
    /// Interactive messages' menus (#211).
    action_menus: crate::interactive::Menus,
    _subscriptions: Vec<gpui_kit::Subscription>,
}

impl EventEmitter<ChannelEvent> for ChannelView {}

/// Points Enter in `composer` at sending or at a new line, for the send
/// key the user chose and the text typed so far (#157).
pub(crate) fn follow_send_key(
    composer: &Entity<ComposerState>,
    send_on: SendKey,
    cx: &mut gpui_kit::App,
) {
    composer.update(cx, |composer, cx| {
        let submit = send_on.submits_on_enter(&composer.value(cx));
        composer.set_submit_on_enter(submit);
    });
}

impl ChannelView {
    pub fn new(
        model: Option<ChannelModel>,
        me: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.subscribe(
            &SettingsStore::global(cx),
            |this, store, _: &SettingsChanged, cx| {
                let send_on = store.read(cx).get().send_on;
                crate::channel::follow_send_key(&this.composer, send_on, cx);
                // Display density changes every row's height.
                this.list.update(cx, |list, cx| list.remeasure(cx));
                cx.notify();
            },
        )
        .detach();
        let send_on = SettingsStore::global(cx).read(cx).get().send_on;
        let rows = model.as_ref().map(ChannelModel::rows).unwrap_or_default();
        let list = cx.new(|cx| MessageListState::new(rows.len(), cx));
        let view = cx.weak_entity();
        let keys = view.clone();
        let people = view.clone();
        let composer = cx.new(|cx| {
            let mut state = ComposerState::new(8, window, cx);
            state.set_triggers(['@', '~', ':'], cx);
            state.set_submit_on_enter(send_on.submits_on_enter(""));
            // Up in an empty composer edits the last own post, Shift+Up
            // replies to the newest one; the editor sees arrows first, so
            // the keymap rows alone never fire. Ctrl+Enter (Cmd+Enter on
            // macOS) always sends, which is the only way when the
            // preference puts a newline on Enter.
            state.set_on_key(move |keystroke, window, cx| {
                if let Some(composer) = keys.upgrade().map(|view| view.read(cx).composer.clone())
                    && apply_format(&composer, keystroke, window, cx)
                {
                    return true;
                }
                if keystroke.key == "enter"
                    && (keystroke.modifiers.control || keystroke.modifiers.platform)
                {
                    return keys
                        .update(cx, |this, cx| {
                            let text = this.composer.read(cx).value(cx);
                            this.send(text, window, cx);
                            true
                        })
                        .unwrap_or(false);
                }
                if keystroke.key == "tab" && keystroke.modifiers.shift {
                    // Shift+Tab leaves the composer for keyboard message
                    // selection (PLAN §4.2).
                    return keys
                        .update(cx, |this, cx| {
                            this.select_newest(window, cx);
                            true
                        })
                        .unwrap_or(false);
                }
                // Up and Shift+Up only: Alt+Shift+Up is the unread jump.
                let mods = &keystroke.modifiers;
                if keystroke.key != "up" || mods.alt || mods.control || mods.platform {
                    return false;
                }
                keys.update(cx, |this, cx| {
                    if this.draft_len > 0 {
                        return false;
                    }
                    if keystroke.modifiers.shift {
                        if let Some(post) = this.model.as_ref().and_then(|m| m.last_message()) {
                            cx.emit(ChannelEvent::OpenThread(post.thread_root()));
                        }
                    } else if let Some(id) = this.last_own_post() {
                        this.start_edit(id, window, cx);
                    }
                    true
                })
                .unwrap_or(false)
            });
            // Images and files on the clipboard go to the tray; text pastes
            // as usual.
            state.set_on_paste(move |item, _, cx| {
                let sources: Vec<UploadSource> = item
                    .entries()
                    .iter()
                    .flat_map(|entry| match entry {
                        ClipboardEntry::Image(image) => {
                            vec![UploadSource::Image(Arc::new(image.clone()))]
                        }
                        ClipboardEntry::ExternalPaths(paths) => paths
                            .paths()
                            .iter()
                            .cloned()
                            .map(UploadSource::Path)
                            .collect(),
                        ClipboardEntry::String(_) => vec![],
                    })
                    .collect();
                if sources.is_empty() {
                    return false;
                }
                view.update(cx, |this, cx| this.add_sources(sources, cx))
                    .is_ok()
            });
            // People get their avatar, emoji their glyph; channels keep the
            // suggestion's icon.
            state.set_suggestion_leading(move |suggestion, _, cx| {
                if let Some(username) = suggestion.value.strip_prefix('@') {
                    let initial: String = suggestion.label.chars().take(1).collect();
                    let user_id = people
                        .upgrade()
                        .and_then(|view| {
                            view.read(cx)
                                .model
                                .iter()
                                .flat_map(|model| model.profiles.iter())
                                .find(|profile| profile.username.as_ref() == username)
                                .map(|profile| profile.user_id.clone())
                        })
                        .unwrap_or_default();
                    Some(
                        crate::avatars::with_picture(
                            Avatar::new(gpui_kit::ElementId::Name(
                                format!("suggest.{username}").into(),
                            ))
                            .size(AvatarSize::Sm)
                            .fallback(initial),
                            &user_id,
                            cx,
                        )
                        .into_any_element(),
                    )
                } else if suggestion.value.starts_with('~') {
                    None
                } else {
                    Some(div().child(suggestion.value.clone()).into_any_element())
                }
            });
            state
        });
        crate::updater::watch_composer(&composer, cx);
        let menu = cx.new(|cx| MenuState::new([], cx));
        let header_menu = cx.new(|cx| MenuState::new([], cx));
        let edit = cx.new(|cx| ChannelEditView::new(window, cx));
        let notifications = cx.new(|_| NotificationsView::new());
        let add_members = cx.new(|cx| AddMembersView::new(window, cx));
        let picker = cx.new(|cx| EmojiPickerState::new(crate::emoji::categories(), window, cx));
        let subscriptions = vec![
            cx.subscribe(&edit, |this, _, event: &ChannelEditEvent, cx| {
                let ChannelEditEvent::Save { field, value } = event;
                this.apply_edit(*field, value.clone(), cx);
            }),
            cx.subscribe(&add_members, |this, _, event: &AddMembersEvent, cx| {
                let AddMembersEvent::Add(people) = event;
                if let Some(model) = this.model.as_mut() {
                    model.members += people.len();
                }
                cx.notify();
                cx.emit(ChannelEvent::AddMembers(people.clone()));
            }),
            cx.subscribe(&notifications, |this, _, event: &NotificationsEvent, cx| {
                let NotificationsEvent::Save {
                    muted,
                    notifications,
                } = *event;
                if let Some(model) = this.model.as_mut() {
                    model.muted = muted;
                    model.notifications = notifications;
                }
                cx.notify();
                cx.emit(ChannelEvent::SetNotifications {
                    muted,
                    notifications,
                });
            }),
            cx.subscribe_in(
                &composer,
                window,
                |this, composer, event: &ComposerEvent, window, cx| match event {
                    ComposerEvent::Change => {
                        crate::perf::composer_changed();
                        let send_on = SettingsStore::global(cx).read(cx).get().send_on;
                        follow_send_key(composer, send_on, cx);
                        let text = composer.read(cx).value(cx);
                        this.draft_len = text.chars().count();
                        this.draft_changed(&text, cx);
                        // A restored draft is not the user typing.
                        if !this.draft.is_saved(&text) {
                            this.schedule_draft_save(cx);
                            if this.draft_len > 0 {
                                cx.emit(ChannelEvent::Typing { root: None });
                            }
                        }
                        cx.notify();
                    }
                    ComposerEvent::Submit(text) => this.send(text.clone(), window, cx),
                    ComposerEvent::Autocomplete { trigger, query } => {
                        this.ask_people(*trigger, query, cx);
                        let suggestions = this.suggestions(*trigger, query);
                        composer
                            .update(cx, |composer, cx| composer.set_suggestions(suggestions, cx));
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &picker,
                window,
                |this, _, event: &EmojiPickerEvent, window, cx| match event {
                    EmojiPickerEvent::Pick(name) => this.pick_emoji(name.clone(), window, cx),
                    EmojiPickerEvent::Cancel => this.close_picker(window, cx),
                    EmojiPickerEvent::SkinTone(_) => {}
                },
            ),
        ];
        let mut this = Self {
            model: None,
            rows,
            list,
            composer,
            menu,
            header_menu,
            edit,
            notifications,
            add_members,
            leave: DialogHandle::new(false),
            archive: DialogHandle::new(false),
            convert: DialogHandle::new(false),
            picker,
            picker_open: false,
            picker_target: None,
            recent_emoji: Vec::new(),
            directory: Vec::new(),
            people: PeopleAsk::default(),
            tray: UploadTray::new("upload"),
            lightbox: cx.new(|_| LightboxState::new([])),
            lightbox_handle: DialogHandle::new(false),
            lightbox_items: Vec::new(),
            lightbox_post: None,
            full_images: HashMap::new(),
            editing: None,
            unseen: 0,
            permalink: None,
            _permalink_task: None,
            link: Default::default(),
            _link_task: None,
            pending_jump: None,
            jump_asked: None,
            selection: None,
            history: History::default(),
            list_focus: cx.focus_handle(),
            delete: DialogHandle::new(false),
            deleting: None,
            me: me.into(),
            draft_len: 0,
            draft: Default::default(),
            _draft_task: None,
            pending_seq: 0,
            open_at_end: false,
            composer_cap: px(320.),
            away: crate::preview::connection() != crate::main_window::ConnectionState::Online,
            offline: crate::preview::connection() == crate::main_window::ConnectionState::Offline,
            profile_cards: 0,
            action_menus: Default::default(),
            _subscriptions: subscriptions,
        };
        this.set_model(model, window, cx);
        this
    }

    pub fn model(&self) -> Option<&ChannelModel> {
        self.model.as_ref()
    }

    /// Puts the caret in the composer (Ctrl+Shift+L).
    pub fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |composer, cx| composer.focus(window, cx))
    }

    /// Replaces the channel and resets the transcript to its end.
    pub fn set_model(
        &mut self,
        model: Option<ChannelModel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::perf::channel_shown(
            model.as_ref().map(|model| model.id.as_ref()),
            model.as_ref().is_some_and(|model| model.loading),
            model.as_ref().map_or(0, |model| model.posts.len()),
        );
        self.rows = model.as_ref().map(ChannelModel::rows).unwrap_or_default();
        self.refresh_header_menu(model.as_ref(), cx);
        let count = self.rows.len();
        self.list = cx.new(|cx| MessageListState::new(count, cx));
        self.unseen = 0;
        self.editing = None;
        // Open at the "New messages" line when there is one, as Mattermost
        // does, so nothing unread scrolls past; the pill leads to the end.
        let at_end = std::mem::take(&mut self.open_at_end);
        let divider = self.rows.iter().position(|row| *row == Row::NewMessages);
        if let Some(divider) = divider.filter(|_| !at_end) {
            self.list.update(cx, |list, cx| {
                list.scroll_to_item(divider, cx);
            });
        }
        self.permalink = None;
        self.selection = None;
        // The server ranked its matches for the channel that was open.
        self.people = PeopleAsk::default();
        // Preview has no server to page from, so it starts at the beginning.
        self.history = History {
            at_start: crate::preview::enabled(),
            at_end: true,
            ..History::default()
        };
        if crate::preview::enabled() {
            self.schedule_preview_arrivals(model.as_ref(), cx);
        }
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
        let key = model
            .as_ref()
            .map(|model| crate::composer_draft::DraftKey::channel(model.id.clone()));
        if self.draft.key() != key.as_ref() {
            self.switch_draft(key, window, cx);
        }
        self.model = model;
        self.apply_pending_jump(cx);
        cx.notify();
    }

    /// Adds posts that arrived from the server to the end of the transcript
    /// without disturbing the scroll position. While the reader is scrolled
    /// up, the jump pill counts them as "N new messages".
    /// Scrolls to `post` and tints it as a permalink target for a few
    /// seconds (PLAN §4.2). When the post is not in the transcript, the
    /// jump waits for the channel or page that carries it; once the channel
    /// is loaded without it, `ChannelEvent::JumpMissing` asks sync for the
    /// posts around it.
    pub fn jump_to(&mut self, post: SharedString, cx: &mut Context<Self>) {
        let row = self.rows.iter().position(|row| {
            matches!(row, Row::Post(index)
                if self.model.as_ref().is_some_and(|m| m.posts[*index].id == post))
        });
        let Some(row) = row else {
            let loaded = self.model.as_ref().is_some_and(|model| !model.loading);
            if loaded && self.jump_asked.as_ref() != Some(&post) {
                self.jump_asked = Some(post.clone());
                cx.emit(ChannelEvent::JumpMissing(post.clone()));
            }
            self.pending_jump = Some(post);
            return;
        };
        self.pending_jump = None;
        self.jump_asked = None;
        self.list
            .update(cx, |list, cx| list.scroll_to_item(row, cx));
        self.permalink = Some(post);
        self._permalink_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(4))
                .await;
            this.update(cx, |this, cx| {
                this.permalink = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn apply_pending_jump(&mut self, cx: &mut Context<Self>) {
        if let Some(post) = self.pending_jump.clone() {
            self.jump_to(post, cx);
        }
    }

    pub fn append_posts(&mut self, posts: Vec<Post>, cx: &mut Context<Self>) {
        if posts.is_empty() {
            return;
        }
        let Some(model) = self.model.as_mut() else {
            return;
        };
        // Joins and other system rows are not messages to catch up on.
        let arrived = posts
            .iter()
            .filter(|post| !post.mine && !matches!(post.kind, PostKind::System { .. }))
            .count();
        model.posts.extend(posts);
        // Sync's first posts for a channel it had none of come as a patch.
        crate::perf::channel_shown(Some(&model.id), model.loading, model.posts.len());
        let rows = model.rows();
        let before = self.rows.len();
        let added = rows.len().saturating_sub(before);
        self.rows = rows;
        self.history.loading_newer = false;
        let scrolled_up = self.list.read(cx).is_scrolled_up();
        self.list.update(cx, |list, cx| {
            list.append(added, cx);
            // The old bottom row was the Newer row; it is a post now.
            list.remeasure_items(before.saturating_sub(1)..before, cx);
        });
        if scrolled_up {
            self.unseen += arrived;
        }
        self.apply_pending_jump(cx);
        cx.notify();
    }

    /// Puts an older page above the loaded posts, keeping the reader's place:
    /// gpuicn's list anchors the scroll on the row that was at the top.
    pub fn prepend_posts(&mut self, posts: Vec<Post>, cx: &mut Context<Self>) {
        self.history.loading = false;
        let Some(model) = self.model.as_mut() else {
            return;
        };
        let before = self.rows.len();
        if model.prepend_posts(posts) == 0 {
            self.list
                .update(cx, |list, cx| list.remeasure_items(0..1, cx));
            cx.notify();
            return;
        }
        self.rows = model.rows();
        let added = self.rows.len().saturating_sub(before);
        self.selection = self.selection.map(|row| row + added);
        self.list.update(cx, |list, cx| {
            list.prepend(added, cx);
            // The old top row may have been the History row or a day divider
            // that merged into the new page.
            list.remeasure_items(0..added + 1, cx);
        });
        self.apply_pending_jump(cx);
        cx.notify();
    }

    /// Sync reports where the history stands after a page: at the start,
    /// at the end, or failed. A failure lands on the edge that was loading;
    /// either way both edges stop loading.
    pub fn set_history(
        &mut self,
        at_start: bool,
        at_end: bool,
        failed: bool,
        cx: &mut Context<Self>,
    ) {
        let newer = self.history.loading_newer && !self.history.loading;
        self.history = History {
            at_start,
            at_end,
            loading: false,
            loading_newer: false,
            failed: failed && !newer,
            failed_newer: failed && newer,
        };
        let last = self.rows.len();
        // After the jump pill, sync's latest posts arrive as a patch that
        // swaps every row, so the scroll goes to the end here.
        let to_end = at_end && std::mem::take(&mut self.open_at_end);
        self.list.update(cx, |list, cx| {
            list.remeasure_items(0..1, cx);
            list.remeasure_items(last.saturating_sub(1)..last, cx);
            if to_end {
                list.scroll_to_end(cx);
            }
        });
        cx.notify();
    }

    /// The connection went down or came back: waiting sends read "Will
    /// send when back online" meanwhile (#195).
    pub fn set_away(&mut self, away: bool, cx: &mut Context<Self>) {
        if self.away != away {
            self.away = away;
            self.list.update(cx, |list, cx| list.remeasure(cx));
            cx.notify();
        }
    }

    /// The banner went to or from "You're offline" (#163).
    pub fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
    }

    /// Offline, refuses an action that needs the server before anything
    /// changes here, and asks the window to say so; `true` when refused.
    fn refused_offline(&self, action: &'static str, cx: &mut Context<Self>) -> bool {
        if self.offline {
            cx.emit(ChannelEvent::Offline(action));
        }
        self.offline
    }

    /// The post `id` in the model.
    fn shown_post(&self, id: &str) -> Option<&Post> {
        self.model.as_ref()?.posts.iter().find(|post| post.id == id)
    }

    /// The jump pill while the newest posts are not loaded: asks sync for
    /// them and scrolls to the end once they are in (`set_history`, or
    /// `set_model` when they come as a new model).
    /// Whether the newest post is on screen: the list reaches the
    /// channel's end and is not scrolled up from it (#145).
    pub fn at_newest(&self, cx: &gpui_kit::App) -> bool {
        self.model.is_some() && self.history.at_end && !self.list.read(cx).is_scrolled_up()
    }

    fn jump_latest(&mut self, cx: &mut Context<Self>) {
        self.open_at_end = true;
        cx.emit(ChannelEvent::JumpLatest);
    }

    /// Asks for a newer page when the bottom row shows after a jump into
    /// the past and nothing stops it.
    fn request_newer(&mut self, cx: &mut Context<Self>) {
        if self.model.is_none()
            || self.history.at_end
            || self.history.loading_newer
            || self.history.failed_newer
        {
            return;
        }
        self.history.loading_newer = true;
        cx.emit(ChannelEvent::NearBottom);
        let last = self.rows.len();
        let view = cx.entity().downgrade();
        cx.defer(move |cx| {
            view.update(cx, |this, cx| {
                this.list.update(cx, |list, cx| {
                    list.remeasure_items(last.saturating_sub(1)..last, cx)
                });
                cx.notify();
            })
            .ok();
        });
    }

    /// The bottom row of the transcript: nothing while it runs to the newest
    /// post, otherwise the newer page's progress.
    fn newer_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        if self.history.at_end {
            return div().into_any_element();
        }
        let row = div()
            .flex()
            .items_center()
            .justify_center()
            .gap(theme.space(2.))
            .px(scaled(&theme, metrics::GUTTER))
            .py(theme.space(3.))
            .text_size(theme.text(12.))
            .text_color(theme.colors.muted_foreground);
        if self.history.loading_newer {
            row.child(
                Spinner::new("channel.newer.spinner")
                    .label("Loading newer messages")
                    .size(gpui_kit::px(14.)),
            )
            .child("Loading newer messages…")
            .into_any_element()
        } else if self.history.failed_newer {
            row.child("Couldn't load newer messages.")
                .child(
                    Button::new("channel.newer.retry")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Sm)
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.history.failed_newer = false;
                            this.request_newer(cx);
                        })),
                )
                .into_any_element()
        } else {
            // The request goes out as the row renders.
            div().h(theme.space(2.)).into_any_element()
        }
    }

    /// Asks for an older page when the top row shows and nothing stops it.
    fn request_older(&mut self, cx: &mut Context<Self>) {
        if self.model.is_none()
            || self.history.at_start
            || self.history.loading
            || self.history.failed
        {
            return;
        }
        self.history.loading = true;
        cx.emit(ChannelEvent::NearTop);
        // This runs while the list lays out its top row, so the remeasure
        // waits until the list has let go of its state.
        let view = cx.entity().downgrade();
        cx.defer(move |cx| {
            view.update(cx, |this, cx| {
                this.list
                    .update(cx, |list, cx| list.remeasure_items(0..1, cx));
                cx.notify();
            })
            .ok();
        });
    }

    /// The top row of the transcript.
    fn history_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let row = div()
            .flex()
            .items_center()
            .justify_center()
            .gap(theme.space(2.))
            .px(scaled(&theme, metrics::GUTTER))
            .py(theme.space(3.))
            .text_size(theme.text(12.))
            .text_color(theme.colors.muted_foreground);
        if self.history.loading {
            row.child(
                Spinner::new("channel.history.spinner")
                    .label("Loading older messages")
                    .size(gpui_kit::px(14.)),
            )
            .child("Loading older messages…")
            .into_any_element()
        } else if self.history.failed {
            row.child("Couldn't load older messages.")
                .child(
                    Button::new("channel.history.retry")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Sm)
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.history.failed = false;
                            this.request_older(cx);
                        })),
                )
                .into_any_element()
        } else if self.history.at_start {
            let name: SharedString = match self.model.as_ref() {
                Some(channel) => match channel.kind {
                    ChannelKind::Direct { .. } | ChannelKind::Group { .. } => {
                        "this conversation".into()
                    }
                    _ => format!("#{}", channel.name).into(),
                },
                None => "the channel".into(),
            };
            row.child(format!("This is the start of {name}."))
                .into_any_element()
        } else {
            // Nothing to show yet; the request goes out as the row renders.
            div().h(theme.space(2.)).into_any_element()
        }
    }

    /// Replaces a post the server changed (edit, reaction, pin) in place.
    pub fn update_post(&mut self, post: Post, cx: &mut Context<Self>) {
        let id = post.id.clone();
        self.mutate_post(&id, |current| *current = post, cx);
    }

    /// Removes a post the server deleted. Only its rows leave the list, so
    /// the scroll and the New messages line stay where they were (#164).
    pub fn remove_post(&mut self, id: &SharedString, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.model.as_mut() else {
            return;
        };
        let Some(at) = model.posts.iter().position(|post| &post.id == id) else {
            return;
        };
        let before = model.row_keys(&self.rows);
        model.posts.remove(at);
        model.first_unread = model.first_unread.and_then(|first| {
            let first = if at < first { first - 1 } else { first };
            (first < model.posts.len()).then_some(first)
        });
        self.rows = model.rows();
        let (range, count) = changed_rows(&before, &model.row_keys(&self.rows));
        let rows = self.rows.len();
        self.list.update(cx, |list, cx| {
            if !list.splice(range.clone(), count, cx) {
                list.reset(rows, cx);
            }
        });
        if self
            .editing
            .as_ref()
            .is_some_and(|editing| &editing.post == id)
        {
            self.editing = None;
        }
        if self.selection.is_some_and(|row| row >= range.start) {
            self.selection = None;
        }
        cx.notify();
    }

    /// In preview, a couple of posts land in #general a while after it
    /// opens, so the live path and the pill can be seen without a server.
    fn schedule_preview_arrivals(&self, model: Option<&ChannelModel>, cx: &mut Context<Self>) {
        let Some(id) = model.map(|model| model.id.clone()) else {
            return;
        };
        let incoming = crate::preview::incoming(&id);
        if incoming.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(12))
                .await;
            this.update(cx, |this, cx| {
                if this.model.as_ref().is_some_and(|model| model.id == id) {
                    this.append_posts(incoming, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Sync or the info pane changed the star; the header follows.
    pub fn set_favorite(&mut self, favorite: bool, cx: &mut Context<Self>) {
        if let Some(model) = self.model.as_mut() {
            model.favorite = favorite;
            cx.notify();
        }
    }

    /// Sync loaded the channel's header, purpose and counts; the scroll
    /// stays where it is.
    pub fn set_details(&mut self, details: crate::live::Details, cx: &mut Context<Self>) {
        if let Some(model) = self.model.as_mut() {
            model.header = details.header;
            model.purpose = details.purpose;
            model.members = details.members;
            model.pinned = details.pinned;
            let menu_changed = model.can_manage != details.can_manage;
            model.can_manage = details.can_manage;
            model.delete_others = details.delete_others;
            if menu_changed {
                let model = self.model.clone();
                self.refresh_header_menu(model.as_ref(), cx);
            }
            cx.notify();
        }
    }

    /// The user's notification preferences for the channel as the server
    /// has them, with its mute.
    pub fn set_notifications(
        &mut self,
        muted: bool,
        notifications: ChannelNotifications,
        cx: &mut Context<Self>,
    ) {
        if let Some(model) = self.model.as_mut()
            && (model.muted, model.notifications) != (muted, notifications)
        {
            model.muted = muted;
            model.notifications = notifications;
            cx.notify();
        }
    }

    /// The people typing in the channel now, for the line under the
    /// transcript.
    pub fn set_typing(&mut self, names: Vec<SharedString>, cx: &mut Context<Self>) {
        if let Some(model) = self.model.as_mut()
            && model.typing != names
        {
            model.typing = names;
            cx.notify();
        }
    }

    /// Profiles for the avatar card and `@` autocomplete, as sync learns
    /// of people.
    pub fn set_profiles(&mut self, profiles: Vec<UserProfile>, cx: &mut Context<Self>) {
        if let Some(model) = self.model.as_mut()
            && model.profiles != profiles
        {
            model.profiles = profiles;
            cx.notify();
        }
    }

    /// Sync or the info pane muted or unmuted the channel.
    pub fn set_muted(&mut self, muted: bool, cx: &mut Context<Self>) {
        if let Some(model) = self.model.as_mut() {
            model.muted = muted;
            cx.notify();
        }
    }

    /// Puts text at the caret and focuses the composer, for "Mention" on a
    /// profile card.
    pub fn insert_text(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = text.into();
        self.composer.update(cx, |composer, cx| {
            composer.insert(text, window, cx);
            composer.focus(window, cx);
        });
    }

    fn max_post_size(&self) -> usize {
        self.model
            .as_ref()
            .map_or(DEFAULT_MAX_POST_SIZE, |channel| channel.max_post_size)
    }

    fn can_send(&self) -> bool {
        self.model.is_some()
            && self.tray.gate(self.draft_len > 0) == SendGate::Now
            && self.draft_len <= self.max_post_size()
    }

    /// The server's `MaxFileSize`, for checking files before they upload.
    pub fn set_max_file_size(&mut self, bytes: u64) {
        self.tray.set_max_file_size(bytes);
    }

    /// Opens the file picker; in preview it fills the tray with fixtures
    /// instead, since no dialog can open there, and offers one file over
    /// the limit so the tray says why it stayed out.
    pub fn choose_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if crate::preview::enabled() {
            self.tray.extend(crate::preview::uploads());
            self.tray.admit(vec![crate::preview::oversize_upload()]);
            cx.notify();
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                this.update(cx, |this, cx| this.add_paths(paths, cx)).ok();
            }
        })
        .detach();
    }

    /// Adds files to the tray and asks sync to upload them.
    pub fn add_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>, cx: &mut Context<Self>) {
        self.add_sources(paths.into_iter().map(UploadSource::Path), cx);
    }

    /// Adds what passes the size and count checks to the tray and asks
    /// sync to upload it; the tray says why the rest stayed out.
    fn add_sources(
        &mut self,
        sources: impl IntoIterator<Item = UploadSource>,
        cx: &mut Context<Self>,
    ) {
        for upload in self.tray.add(sources) {
            cx.emit(ChannelEvent::Upload {
                id: upload.id,
                name: upload.name,
                source: upload.source,
            });
        }
        cx.notify();
    }

    /// Sync reports progress, completion or failure for a tray file; a
    /// send that waited on it goes once the last one is up.
    pub fn set_upload_status(
        &mut self,
        id: &str,
        status: AttachmentStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tray.uploads().iter().any(|upload| upload.id == id) {
            // A file in another channel's draft.
            if let Some(save) = crate::composer_draft::DraftBook::set_upload_status(id, status, cx)
            {
                cx.emit(ChannelEvent::SaveDraft(save));
            }
            return;
        }
        if self.tray.set_status(id, status) {
            self.send_queued(window, cx);
        } else if status == AttachmentStatus::Ready {
            self.schedule_draft_save(cx);
        }
        cx.notify();
    }

    /// Sync is offline: the file waits in the tray, queued (#194).
    pub fn set_upload_queued(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.tray.set_queued(id) {
            self.send_queued(window, cx);
        }
        cx.notify();
    }

    fn remove_upload(&mut self, id: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let due = self.tray.remove(id);
        self.schedule_draft_save(cx);
        cx.emit(ChannelEvent::CancelUpload(id.clone()));
        if due {
            self.send_queued(window, cx);
        }
        cx.notify();
    }

    fn retry_upload(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        self.tray.set_status(id, AttachmentStatus::Uploading(0.));
        cx.emit(ChannelEvent::RetryUpload(id.clone()));
        cx.notify();
    }

    /// Sends what the composer holds now, for a send that waited on uploads.
    fn send_queued(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value(cx);
        self.send(text, window, cx);
    }

    /// Opens the lightbox over the images of `post`, at image `index`:
    /// its image files first, then the images its message links to.
    fn open_lightbox(
        &mut self,
        post: SharedString,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((items, images)) = self.lightbox_images(&post, cx) else {
            return;
        };
        let files = items
            .iter()
            .filter_map(|item| match item {
                LightboxItem::File(id) => Some(id.clone()),
                LightboxItem::Link(_) => None,
            })
            .collect::<Vec<_>>();
        self.lightbox_items = items;
        self.lightbox_post = Some(post);
        self.lightbox
            .update(cx, |lightbox, cx| lightbox.set_images(images, index, cx));
        self.lightbox_handle.open(window, cx);
        if !files.is_empty() {
            cx.emit(ChannelEvent::ViewImages(files));
        }
        cx.notify();
    }

    /// Opens the lightbox at the image `url` that `post` links to, among
    /// the post's other images. One still loading opens in the browser.
    fn open_linked_image(
        &mut self,
        post: SharedString,
        url: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = self.lightbox_images(&post, cx).and_then(|(items, _)| {
            items
                .iter()
                .position(|item| *item == LightboxItem::Link(url.clone()))
        });
        match index {
            Some(index) => self.open_lightbox(post, index, window, cx),
            None => cx.open_url(&url),
        }
    }

    /// Opens the lightbox on the selected post's first image, from Enter
    /// (#152). Returns whether the post has one.
    fn open_selected_images(
        &mut self,
        post: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.lightbox_images(&post, cx).is_none() {
            return false;
        }
        self.open_lightbox(post, 0, window, cx);
        true
    }

    /// The images of `post` and their lightbox entries: its image files,
    /// full size where it has come, then the linked images sync has
    /// fetched.
    fn lightbox_images(
        &self,
        post: &SharedString,
        cx: &App,
    ) -> Option<(Vec<LightboxItem>, Vec<LightboxImage>)> {
        let post = self
            .model
            .as_ref()
            .and_then(|model| model.posts.iter().find(|p| &p.id == post))?;
        let caption: SharedString = format!(
            "{} · {}",
            post.author,
            ClockFormat::current(cx).format(post.time)
        )
        .into();
        let mut items = Vec::new();
        let mut images: Vec<LightboxImage> = post
            .files
            .iter()
            .filter_map(|file| {
                let image = match self.full_images.get(&file.id) {
                    Some(full) => gpui_kit::ImageSource::Image(full.clone()),
                    None => file.image()?,
                };
                items.push(LightboxItem::File(file.id.clone()));
                Some(LightboxImage::new(image, file.name.clone()).caption(caption.clone()))
            })
            .collect();
        for embed in &post.embeds {
            if let crate::embeds::Embed::Image(image) = embed
                && let Some(loaded) = &image.loaded
            {
                items.push(LightboxItem::Link(image.url.clone()));
                images.push(
                    LightboxImage::new(
                        gpui_kit::ImageSource::Image(loaded.clone()),
                        crate::embeds::image_name(&image.url),
                    )
                    .caption(caption.clone()),
                );
            }
        }
        (!images.is_empty()).then_some((items, images))
    }

    /// Sync fetched a file's full-size image; the lightbox swaps it in
    /// when it shows that file's post.
    pub fn set_full_image(
        &mut self,
        file_id: SharedString,
        image: Arc<gpui_kit::Image>,
        cx: &mut Context<Self>,
    ) {
        let showing = self
            .lightbox_items
            .contains(&LightboxItem::File(file_id.clone()));
        self.full_images.insert(file_id, image);
        if !showing {
            return;
        }
        let Some((_, images)) = self
            .lightbox_post
            .clone()
            .and_then(|post| self.lightbox_images(&post, cx))
        else {
            return;
        };
        self.lightbox.update(cx, |lightbox, cx| {
            let index = lightbox.index();
            lightbox.set_images(images, index, cx);
        });
        cx.notify();
    }

    /// The lightbox, with Download for a file and Copy link and Open in
    /// browser for an image the message links to.
    fn lightbox(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let icon = |icon: LucideIcon| {
            lucide(icon)
                .size(theme.space(4.5))
                .text_color(gpui_kit::white())
        };
        // Test hooks for the buttons, which carry no selector of their own.
        let hook = |selector: &'static str, button: Button| {
            div()
                .debug_selector(move || selector.into())
                .child(button)
                .into_any_element()
        };
        let index = self.lightbox.read(cx).index();
        let actions: Vec<gpui_kit::AnyElement> = match self.lightbox_items.get(index).cloned() {
            Some(LightboxItem::File(id)) => vec![hook(
                "lightbox-download",
                Button::new("channel.lightbox.open")
                    .size(ButtonSize::Icon)
                    .variant(ButtonVariant::Ghost)
                    .aria_label("Download")
                    .child(icon(LucideIcon::Download))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(ChannelEvent::OpenFile(id.clone()));
                    })),
            )],
            Some(LightboxItem::Link(url)) => {
                let copy = url.clone();
                vec![
                    hook(
                        "lightbox-copy-link",
                        Button::new("channel.lightbox.copy-link")
                            .size(ButtonSize::Icon)
                            .variant(ButtonVariant::Ghost)
                            .aria_label("Copy link")
                            .child(icon(LucideIcon::Link))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                    copy.to_string(),
                                ));
                            }),
                    ),
                    hook(
                        "lightbox-browser",
                        Button::new("channel.lightbox.browser")
                            .size(ButtonSize::Icon)
                            .variant(ButtonVariant::Ghost)
                            .aria_label("Open in browser")
                            .child(icon(LucideIcon::ExternalLink))
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    ),
                ]
            }
            None => Vec::new(),
        };
        lightbox(
            "channel.lightbox",
            &self.lightbox_handle,
            &self.lightbox,
            actions,
            window,
            cx,
        )
    }

    /// A tray chip with its remove and Retry wired to this view.
    fn upload_chip(&self, upload: &Upload, cx: &mut Context<Self>) -> Attachment {
        let (remove, retry) = (upload.id.clone(), upload.id.clone());
        upload.chip(
            "upload",
            cx.listener(move |this, _, window, cx| this.remove_upload(&remove, window, cx)),
            cx.listener(move |this, _, _, cx| this.retry_upload(&retry, cx)),
        )
    }

    /// Puts the composer on `key`'s draft, saving the one it leaves.
    fn switch_draft(
        &mut self,
        key: Option<crate::composer_draft::DraftKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._draft_task = None;
        let left = self
            .draft
            .switch(key, &self.composer, &mut self.tray, Some(window), cx);
        if let Some(left) = left {
            cx.emit(ChannelEvent::SaveDraft(left));
        }
        self.draft_len = self.composer.read(cx).value(cx).chars().count();
        self.link = Default::default();
        self._link_task = None;
    }

    /// Saves the draft once typing pauses (#114).
    fn schedule_draft_save(&mut self, cx: &mut Context<Self>) {
        self._draft_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(crate::composer_draft::SAVE_PAUSE)
                .await;
            this.update(cx, |this, cx| this.save_draft(cx)).ok();
        }));
    }

    fn save_draft(&mut self, cx: &mut Context<Self>) {
        self._draft_task = None;
        if let Some(save) = self.draft.save(&self.composer, &self.tray, cx) {
            cx.emit(ChannelEvent::SaveDraft(save));
        }
    }

    /// The channel's draft changed elsewhere; the composer follows unless
    /// the user has typed since it was saved.
    pub fn refresh_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .draft
            .refresh(&self.composer, &mut self.tray, window, cx)
        {
            self.draft_len = self.composer.read(cx).value(cx).chars().count();
            cx.notify();
        }
    }

    /// Follows the draft's first link: a new one is asked about after a
    /// pause in typing, so a link being typed is not asked about letter by
    /// letter.
    fn draft_changed(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(url) = self.link.follow(text) else {
            if !self.link.has_link() {
                self._link_task = None;
            }
            return;
        };
        self._link_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(crate::draft_link::PAUSE)
                .await;
            this.update(cx, |_, cx| cx.emit(ChannelEvent::PreviewLink(url)))
                .ok();
        }));
    }

    /// What the server read from `url`; ignored once the draft moved on.
    pub fn set_link_preview(
        &mut self,
        url: &str,
        card: Option<crate::embeds::LinkCard>,
        cx: &mut Context<Self>,
    ) {
        if self.link.answer(url, card) {
            cx.notify();
        }
    }

    /// Appends the text as a pending post by the current user, clears the
    /// composer and tells sync (PLAN §4.3: optimistic send through the
    /// outbox). Blank text and text over the limit stay in the composer.
    fn send(&mut self, text: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let body: SharedString = text.trim().to_string().into();
        if body.chars().count() > self.max_post_size() {
            return;
        }
        // Enter while files upload waits for them (#149).
        if self.tray.request_send(!body.is_empty()) != SendGate::Now {
            cx.notify();
            return;
        }
        let files: Vec<PostFile> = self
            .tray
            .take()
            .into_iter()
            .map(PostFile::from_upload)
            .collect();
        let file_ids: Vec<SharedString> = files.iter().map(|file| file.id.clone()).collect();
        let Some(mut model) = self.model.clone() else {
            return;
        };
        self.pending_seq += 1;
        let last = model.posts.last();
        let post = Post {
            id: format!("pending-{}", self.pending_seq).into(),
            author: self.me.clone(),
            author_id: own_id(&model.posts),
            kind: PostKind::Regular,
            time: last.map_or(0, |post| post.time),
            day: last.map_or_else(|| "Today".into(), |post| post.day.clone()),
            date: SharedString::default(),
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
            root_id: "".into(),
            reply_to: None,
            files,
            embeds: Vec::new(),
        };
        model.posts.push(post);
        self.set_model(Some(model), window, cx);
        self._draft_task = None;
        if let Some(cleared) = self.draft.sent(cx) {
            cx.emit(ChannelEvent::SaveDraft(cleared));
        }
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.draft_len = 0;
        let unfurl = self.link.sent(&body);
        self._link_task = None;
        cx.emit(ChannelEvent::Send {
            text: body,
            files: file_ids,
            unfurl,
        });
    }

    fn retry(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let changed = self.mutate_post(&id, |post| post.status = PostStatus::Pending, cx);
        if changed {
            cx.emit(ChannelEvent::Retry(id));
        }
    }

    /// Wraps the selection in Markdown from a toolbar button.
    fn wrap(&mut self, before: &str, after: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |composer, cx| {
            composer.wrap_selection(before, after, window, cx);
            composer.focus(window, cx);
        });
    }

    fn last_own_post(&self) -> Option<SharedString> {
        self.model.as_ref()?.posts.iter().rev().find_map(|post| {
            (post.mine && post.status == PostStatus::Sent && post.kind == PostKind::Regular)
                .then(|| post.id.clone())
        })
    }

    /// Turns the post's body into a composer holding its text. Enter saves,
    /// Escape cancels; only the user's own posts qualify, sent or waiting
    /// to send (#195).
    fn start_edit(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(post) = self.model.as_ref().and_then(|model| {
            model.posts.iter().find(|post| {
                post.id == id
                    && post.mine
                    && post.kind == PostKind::Regular
                    && (post.status == PostStatus::Sent || waiting_to_send(post, self.away))
            })
        }) else {
            return;
        };
        let body = post.body.clone();
        let view = cx.weak_entity();
        let composer = edit_composer(
            body,
            move |window, cx| {
                view.update(cx, |this, cx| this.cancel_edit(window, cx))
                    .ok();
            },
            window,
            cx,
        );
        let subscription = cx.subscribe_in(
            &composer,
            window,
            |this, _, event: &ComposerEvent, window, cx| {
                if let ComposerEvent::Submit(text) = event {
                    this.save_edit(text.clone(), window, cx);
                }
            },
        );
        let row = self.rows.iter().position(|row| match row {
            Row::Post(index) => self
                .model
                .as_ref()
                .is_some_and(|model| model.posts[*index].id == id),
            _ => false,
        });
        self.editing = Some(Editing {
            post: id,
            composer,
            _subscription: subscription,
        });
        self.list.update(cx, |list, cx| {
            list.remeasure(cx);
            if let Some(row) = row {
                list.scroll_to_item(row, cx);
            }
        });
        cx.notify();
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            self.composer
                .update(cx, |composer, cx| composer.focus(window, cx));
            self.list.update(cx, |list, cx| list.remeasure(cx));
            cx.notify();
        }
    }

    /// Keeps the new text and tells sync; an unchanged or emptied body just
    /// closes the editor, since deleting has its own confirmation.
    fn save_edit(&mut self, text: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = self.editing.as_ref() else {
            return;
        };
        let id = editing.post.clone();
        let body: SharedString = text.trim().to_string().into();
        let unchanged = self
            .model
            .as_ref()
            .and_then(|model| model.posts.iter().find(|post| post.id == id))
            .is_none_or(|post| post.body == body);
        if body.is_empty() {
            // Emptying a message asks to delete it, as the webapp does.
            self.cancel_edit(window, cx);
            self.ask_delete(id, window, cx);
            return;
        }
        if body.chars().count() > self.max_post_size() || unchanged {
            self.cancel_edit(window, cx);
            return;
        }
        // A waiting send takes the new text offline (#195); a sent one
        // keeps its editor open for another try.
        let sent = self
            .shown_post(&id)
            .is_some_and(|post| post.status == PostStatus::Sent);
        if sent && self.refused_offline("edit", cx) {
            return;
        }
        self.mutate_post(
            &id,
            |post| {
                post.body = body.clone();
                // A waiting send just goes with the new text.
                post.edited = post.status == PostStatus::Sent;
            },
            cx,
        );
        cx.emit(ChannelEvent::SaveEdit {
            post: id,
            text: body,
        });
        self.cancel_edit(window, cx);
    }

    /// The in-place editor for a post: its composer, the key hint and the
    /// Cancel and Save buttons.
    fn editor(
        composer: &Entity<ComposerState>,
        cap: Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let (cancel, save) = (cx.weak_entity(), cx.weak_entity());
        post_editor(
            "post.edit",
            composer,
            cap,
            move |window, cx| {
                cancel
                    .update(cx, |this, cx| this.cancel_edit(window, cx))
                    .ok();
            },
            move |text, window, cx| {
                save.update(cx, |this, cx| this.save_edit(text, window, cx))
                    .ok();
            },
            cx,
        )
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
    /// Asks the server who matches an `@` query the channel's own
    /// profiles may not cover, once per query.
    fn ask_people(&mut self, trigger: char, query: &str, cx: &mut Context<Self>) {
        if let Some(term) = self.people.ask(trigger, query) {
            cx.emit(ChannelEvent::SuggestPeople(term.into()));
        }
    }

    /// Takes the server's matches for `term` and, while the composer still
    /// asks for it, shows them after the channel's own people.
    pub fn set_suggested(
        &mut self,
        term: String,
        people: Vec<UserProfile>,
        cx: &mut Context<Self>,
    ) {
        if self.people.answer(&term, people) {
            let suggestions = self.suggestions('@', &term);
            self.composer
                .update(cx, |composer, cx| composer.set_suggestions(suggestions, cx));
        }
    }

    /// The channels `~` can complete; the window passes the sidebar's rows.
    pub fn set_directory(&mut self, rows: Vec<ChannelRow>, cx: &mut Context<Self>) {
        self.directory = rows;
        cx.notify();
    }

    /// Suggestions for a composer trigger (PLAN §4.3): `@` people from the
    /// channel's profiles, `~` channels from the sidebar, `:` emoji from the
    /// catalog once two characters narrow it.
    fn suggestions(&self, trigger: char, query: &str) -> Vec<ComposerSuggestion> {
        let in_channel = self
            .model
            .as_ref()
            .is_some_and(|model| matches!(model.kind, ChannelKind::Public | ChannelKind::Private));
        composer_suggestions(
            trigger,
            query,
            self.model.iter().flat_map(|model| model.profiles.iter()),
            &self.people,
            &self.directory,
            in_channel,
        )
    }

    fn typing_line(channel: &ChannelModel, cx: &App) -> impl IntoElement + use<> {
        typing_line(&channel.typing, cx)
    }

    /// Opens the picker for `target` (a post to react to, or the composer).
    fn open_picker(&mut self, target: Option<SharedString>, cx: &mut Context<Self>) {
        self.picker_target = target;
        self.picker_open = true;
        cx.notify();
    }

    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker_open {
            self.picker_open = false;
            self.picker_target = None;
            self.composer
                .update(cx, |composer, cx| composer.focus(window, cx));
            cx.notify();
        }
    }

    fn pick_emoji(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.recent_emoji.retain(|recent| recent != &name);
        self.recent_emoji.insert(0, name.clone());
        self.recent_emoji.truncate(crate::emoji::RECENT_LIMIT);
        let recents = self.recent_emoji.clone();
        self.picker
            .update(cx, |picker, cx| picker.set_recents(recents, cx));
        let glyph = crate::emoji::glyph(&name).unwrap_or_else(|| format!(":{name}:").into());
        match self.picker_target.take() {
            Some(post) => self.toggle_reaction(post, glyph, cx),
            None => self.composer.update(cx, |composer, cx| {
                composer.insert(format!("{glyph} "), window, cx);
            }),
        }
        self.close_picker(window, cx);
    }

    fn toggle_reaction(&mut self, id: SharedString, emoji: SharedString, cx: &mut Context<Self>) {
        let mine = self.shown_post(&id).is_some_and(|post| {
            post.reactions
                .iter()
                .any(|reaction| reaction.emoji == emoji && reaction.mine)
        });
        if self.refused_offline(
            if mine {
                "remove reaction"
            } else {
                "add reaction"
            },
            cx,
        ) {
            return;
        }
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
                        users: vec![],
                    });
                }
            },
            cx,
        );
        if changed {
            cx.emit(ChannelEvent::ToggleReaction { post: id, emoji });
        }
    }

    /// Collapses or expands the images in a post's previews; the row's
    /// height changes, so the list measures it again.
    fn collapse_preview(&mut self, id: SharedString, collapsed: bool, cx: &mut Context<Self>) {
        // Remembered across restarts, against the account's default (#187).
        let store = SettingsStore::global(cx);
        if store.read(cx).get().previews_collapsed(&id) == collapsed {
            return;
        }
        store.update(cx, |store, cx| {
            store.update(cx, |settings| settings.choose_previews(&id, collapsed))
        });
        let posts = self.model.as_ref().map(|model| &model.posts);
        let row = self.rows.iter().position(|row| {
            matches!(row, Row::Post(index)
                if posts.and_then(|posts| posts.get(*index)).is_some_and(|post| post.id == id))
        });
        if let Some(row) = row {
            self.list
                .update(cx, |list, cx| list.remeasure_items(row..row + 1, cx));
        }
        cx.notify();
    }

    fn toggle_save(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let saved = self.shown_post(&id).is_some_and(|post| post.saved);
        if self.refused_offline(
            if saved {
                "unsave message"
            } else {
                "save message"
            },
            cx,
        ) {
            return;
        }
        if self.mutate_post(&id, |post| post.saved = !post.saved, cx) {
            cx.emit(ChannelEvent::ToggleSave(id));
        }
    }

    fn toggle_pin(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let pinned = self.shown_post(&id).is_some_and(|post| post.pinned);
        if self.refused_offline(
            if pinned {
                "unpin message"
            } else {
                "pin message"
            },
            cx,
        ) {
            return;
        }
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
        let status = model.posts[at].status;
        model.posts.remove(at);
        // The "New messages" line stays above the same post.
        model.first_unread = model.first_unread.and_then(|first| {
            let first = if at < first { first - 1 } else { first };
            (first < model.posts.len()).then_some(first)
        });
        self.set_model(Some(model), window, cx);
        match status {
            PostStatus::Sent => cx.emit(ChannelEvent::Delete(id)),
            PostStatus::Pending | PostStatus::Failed => cx.emit(ChannelEvent::Discard(id)),
            PostStatus::Ephemeral => cx.emit(ChannelEvent::DismissEphemeral(id)),
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
        let more_view = view.clone();
        let more_id = id.clone();
        let more = MenuItem::new("react.more", "More emoji…").on_click(move |_, _, cx| {
            let id = more_id.clone();
            more_view
                .update(cx, |this, cx| this.open_picker(Some(id), cx))
                .ok();
        });
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
            .chain([MenuItem::separator(), more])
            .collect()
    }

    /// The "More" menu. A post the server does not have yet only offers
    /// its text, plus Edit and Discard while it waits to send (#195).
    fn more_items_for(
        post: &Post,
        away: bool,
        delete_others: bool,
        cx: &Context<Self>,
    ) -> Vec<MenuItem> {
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
        let markdown_body = post.body.clone();
        let mut items = vec![
            act(
                "copy",
                "Copy text".into(),
                Box::new(move |_, _, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(plain_text(&body)))
                }),
            ),
            act(
                "copy-markdown",
                "Copy as Markdown".into(),
                Box::new(move |_, _, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                        markdown_body.to_string(),
                    ))
                }),
            ),
        ];
        if post.status == PostStatus::Ephemeral {
            let dismiss_view = view.clone();
            let dismiss_id = id.clone();
            items.push(MenuItem::separator());
            items.push(
                MenuItem::new("post.menu.dismiss", "Dismiss message").on_click(
                    move |_, window, cx| {
                        let id = dismiss_id.clone();
                        dismiss_view
                            .update(cx, |this, cx| this.delete_post(id, window, cx))
                            .ok();
                    },
                ),
            );
            return items;
        }
        if post.status != PostStatus::Sent {
            let (edit_view, edit_id) = (view.clone(), id.clone());
            let (discard_view, discard_id) = (view.clone(), id.clone());
            items.extend(waiting_items(
                "post.menu",
                post,
                away,
                move |window, cx| {
                    edit_view
                        .update(cx, |this, cx| this.start_edit(edit_id.clone(), window, cx))
                        .ok();
                },
                move |window, cx| {
                    discard_view
                        .update(cx, |this, cx| {
                            this.ask_delete(discard_id.clone(), window, cx)
                        })
                        .ok();
                },
            ));
            return items;
        }
        items.extend([
            {
                let id = id.clone();
                act(
                    "copy-link",
                    "Copy link".into(),
                    Box::new(move |_, _, cx| cx.emit(ChannelEvent::CopyLink(id.clone()))),
                )
            },
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
        ]);
        if let Some(replies) = &post.replies {
            let follow_id = id.clone();
            items.push(act(
                "follow",
                if replies.following {
                    "Unfollow thread".into()
                } else {
                    "Follow thread".into()
                },
                Box::new(move |this, _, cx| this.toggle_follow(follow_id.clone(), cx)),
            ));
        }
        let forward_id = id.clone();
        items.push(act(
            "forward",
            "Forward\u{2026}".into(),
            Box::new(move |_, _, cx| cx.emit(ChannelEvent::Forward(forward_id.clone()))),
        ));
        items.push(MenuItem::submenu(
            gpui_kit::ElementId::Name("post.menu.remind".into()),
            "Remind me",
            RemindAt::ALL.map(|at| {
                let id = id.clone();
                let view = view.clone();
                MenuItem::new(
                    gpui_kit::ElementId::Name(format!("post.menu.remind.{at:?}").into()),
                    at.label(),
                )
                .on_click(move |_, _, cx| {
                    let event = ChannelEvent::Remind {
                        post: id.clone(),
                        at,
                    };
                    view.update(cx, |_, cx| cx.emit(event)).ok();
                })
            }),
        ));
        if post.mine || deletable(post, away, delete_others) {
            items.push(MenuItem::separator());
        }
        if post.mine {
            let edit_id = id.clone();
            items.push(act(
                "edit",
                "Edit".into(),
                Box::new(move |this, window, cx| this.start_edit(edit_id.clone(), window, cx)),
            ));
        }
        if deletable(post, away, delete_others) {
            let delete_id = id.clone();
            items.push(act(
                "delete",
                "Delete…".into(),
                Box::new(move |this, window, cx| this.ask_delete(delete_id.clone(), window, cx)),
            ));
        }
        items
    }

    /// The right-click menu: reply and react first, then everything the
    /// "More" button offers, with "Mark as unread" after Save.
    fn context_items_for(
        post: &Post,
        away: bool,
        delete_others: bool,
        cx: &Context<Self>,
    ) -> Vec<MenuItem> {
        // The server does not have it yet: nothing to reply to or react to.
        if post.status != PostStatus::Sent {
            return Self::more_items_for(post, away, delete_others, cx);
        }
        let view = cx.entity().downgrade();
        let id = post.id.clone();
        let act = |name: &'static str, label: &'static str, run: MenuAction| {
            let view = view.clone();
            MenuItem::new(
                gpui_kit::ElementId::Name(format!("post.context.{name}").into()),
                label,
            )
            .on_click(move |_, window, cx| {
                view.update(cx, |this, cx| run(this, window, cx)).ok();
            })
        };
        let reply_id = post.thread_root();
        let react_id = id.clone();
        let unread_id = id.clone();
        let mut items = vec![
            act(
                "reply",
                "Reply in thread",
                Box::new(move |_, _, cx| cx.emit(ChannelEvent::OpenThread(reply_id.clone()))),
            ),
            act(
                "react",
                "Add reaction…",
                Box::new(move |this, _, cx| this.open_picker(Some(react_id.clone()), cx)),
            ),
            MenuItem::separator(),
        ];
        let more = Self::more_items_for(post, away, delete_others, cx);
        // `more` is copy, copy as Markdown, copy link, pin, save, then the
        // owner's edit and delete.
        let (keep, own) = more.split_at(5.min(more.len()));
        items.extend(keep.iter().cloned());
        items.push(act(
            "unread",
            "Mark as unread",
            Box::new(move |_, _, cx| cx.emit(ChannelEvent::MarkUnread(unread_id.clone()))),
        ));
        items.extend(own.iter().cloned());
        items
    }

    /// Selects the row that shows `id`, if it is in the transcript.
    fn select_post(&mut self, id: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(posts) = self.model.as_ref().map(|model| &model.posts) else {
            return;
        };
        let row = self.rows.iter().position(|row| {
            matches!(row, Row::Post(index) if posts.get(*index).is_some_and(|post| &post.id == id))
        });
        if row.is_some() {
            self.selection = row;
            window.focus(&self.list_focus, cx);
            cx.notify();
        }
    }

    /// "Delete message?" with the action named in the button (PLAN §4.5).
    fn delete_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let channel: SharedString = match self.model.as_ref() {
            Some(channel) => match channel.kind {
                ChannelKind::Direct { .. } | ChannelKind::Group { .. } => {
                    "this conversation".into()
                }
                _ => format!("#{}", channel.name).into(),
            },
            None => "the channel".into(),
        };
        // A post the server never got is discarded, not deleted (#195).
        let unsent = self.deleting.as_ref().is_some_and(|id| {
            self.model.as_ref().is_some_and(|model| {
                model
                    .posts
                    .iter()
                    .any(|post| &post.id == id && post.status != PostStatus::Sent)
            })
        });
        let replies = self
            .deleting
            .as_ref()
            .and_then(|id| {
                self.model
                    .as_ref()?
                    .posts
                    .iter()
                    .find(|post| &post.id == id)?
                    .replies
                    .as_ref()
                    .map(|summary| summary.count)
            })
            .unwrap_or(0);
        let popup = delete_popup("post.delete", unsent, &channel, replies, cx);
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

    /// Rebuilds the channel name's menu for `model`: conversations with
    /// people cannot be renamed or left.
    fn refresh_header_menu(&mut self, model: Option<&ChannelModel>, cx: &mut Context<Self>) {
        let view = cx.weak_entity();
        let edit = |id: &'static str, label: &'static str, field: ChannelField| {
            let view = view.clone();
            MenuItem::new(id, label).on_click(move |_, window, cx| {
                view.update(cx, |this, cx| this.edit_channel(field, window, cx))
                    .ok();
            })
        };
        let is_channel =
            model.is_some_and(|m| matches!(m.kind, ChannelKind::Public | ChannelKind::Private));
        let notify_view = view.clone();
        let mut items = vec![
            MenuItem::new("channel.menu.notifications", "Notification preferences…").on_click(
                move |_, window, cx| {
                    notify_view
                        .update(cx, |this, cx| this.open_notifications(window, cx))
                        .ok();
                },
            ),
            MenuItem::separator(),
            edit("channel.menu.header", "Edit header…", ChannelField::Header),
        ];
        if is_channel {
            items.push(edit(
                "channel.menu.purpose",
                "Edit purpose…",
                ChannelField::Purpose,
            ));
            items.push(edit(
                "channel.menu.rename",
                "Rename channel…",
                ChannelField::Name,
            ));
            items.push(MenuItem::separator());
            let members_view = view.clone();
            items.push(
                MenuItem::new("channel.menu.add-members", "Add members…").on_click(
                    move |_, window, cx| {
                        members_view
                            .update(cx, |this, cx| this.open_add_members(window, cx))
                            .ok();
                    },
                ),
            );
            let leave = self.leave.clone();
            items.push(
                MenuItem::new("channel.menu.leave", "Leave channel…")
                    .on_click(move |_, window, cx| leave.open(window, cx)),
            );
            if model.is_some_and(|m| m.can_manage) {
                items.push(MenuItem::separator());
                if model.is_some_and(|m| m.kind == ChannelKind::Public) {
                    let convert = self.convert.clone();
                    items.push(
                        MenuItem::new("channel.menu.convert", "Convert to private channel…")
                            .on_click(move |_, window, cx| convert.open(window, cx)),
                    );
                }
                let archive = self.archive.clone();
                items.push(
                    MenuItem::new("channel.menu.archive", "Archive channel…")
                        .on_click(move |_, window, cx| archive.open(window, cx)),
                );
            }
        }
        self.header_menu
            .update(cx, |menu, cx| menu.set_items(items, cx));
    }

    fn edit_channel(&mut self, field: ChannelField, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let current = match field {
            ChannelField::Name => model.name.to_string(),
            ChannelField::Header => model.header.clone().unwrap_or_default().to_string(),
            ChannelField::Purpose => model.purpose.clone().unwrap_or_default().to_string(),
        };
        self.edit
            .update(cx, |edit, cx| edit.open(field, &current, window, cx));
    }

    /// Opens Add members with the team's profiles minus the user.
    fn open_add_members(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let me = self.me.clone();
        let people: Vec<UserProfile> = model
            .profiles
            .iter()
            .filter(|profile| profile.display_name != me)
            .cloned()
            .collect();
        let name = model.name.clone();
        self.add_members
            .update(cx, |view, cx| view.open(name, people, window, cx));
    }

    fn add_members_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let handle = self.add_members.read(cx).handle().clone();
        let popup = dialog_popup("channel.add-members.popup", "Add members", cx)
            .child(self.add_members.clone());
        dialog("channel.add-members", &handle, popup, window, cx)
    }

    fn open_notifications(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let (name, muted, prefs) = (model.name.clone(), model.muted, model.notifications);
        self.notifications
            .update(cx, |view, cx| view.open(name, muted, prefs, window, cx));
    }

    fn notifications_dialog(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let handle = self.notifications.read(cx).handle().clone();
        let popup = dialog_popup(
            "channel.notifications.popup",
            "Notification preferences",
            cx,
        )
        .max_h(UiTheme::read(cx).space(170.))
        .child(self.notifications.clone());
        dialog("channel.notifications", &handle, popup, window, cx)
    }

    /// Applies a saved edit to the model and tells sync.
    fn apply_edit(&mut self, field: ChannelField, value: SharedString, cx: &mut Context<Self>) {
        let Some(model) = self.model.as_mut() else {
            return;
        };
        let optional = (!value.is_empty()).then(|| value.clone());
        let event = match field {
            ChannelField::Name => {
                if model.name == value {
                    return;
                }
                model.name = value.clone();
                ChannelEvent::Rename(value)
            }
            ChannelField::Header => {
                if model.header == optional {
                    return;
                }
                model.header = optional;
                ChannelEvent::SetHeader(value)
            }
            ChannelField::Purpose => {
                if model.purpose == optional {
                    return;
                }
                model.purpose = optional;
                ChannelEvent::SetPurpose(value)
            }
        };
        cx.notify();
        cx.emit(event);
    }

    fn edit_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let handle = self.edit.read(cx).handle().clone();
        let title = self.edit.read(cx).field().title();
        let popup = dialog_popup("channel.edit.popup", title, cx).child(self.edit.clone());
        dialog("channel.edit", &handle, popup, window, cx)
    }

    /// A confirmation for a channel action: `title` over `description`,
    /// with Cancel and a destructive `action` that emits `event`.
    #[allow(clippy::too_many_arguments)]
    fn confirm_dialog(
        &self,
        id: &'static str,
        handle: &DialogHandle,
        title: String,
        description: &'static str,
        action: &'static str,
        event: ChannelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let element_id = |part: &str| gpui_kit::ElementId::Name(format!("{id}.{part}").into());
        let popup = dialog_popup(element_id("popup"), title.clone(), cx)
            .child(dialog_title(element_id("title"), cx).child(title))
            .child(dialog_description(element_id("description"), cx).child(description))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        alert_dialog_cancel(element_id("cancel"))
                            .variant(ButtonVariant::Outline)
                            .label("Cancel"),
                    )
                    .child(
                        alert_dialog_action(element_id("confirm"))
                            .variant(ButtonVariant::Destructive)
                            .label(action),
                    ),
            );
        let view = cx.entity().downgrade();
        alert_dialog(id, handle, popup, window, cx).on_ok(move |_, _, cx| {
            view.update(cx, |_, cx| cx.emit(event.clone())).ok();
            true
        })
    }

    fn archive_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.channel_label();
        self.confirm_dialog(
            "channel.archive",
            &self.archive.clone(),
            format!("Archive {name}?"),
            "Nobody can post in it any more and it leaves everyone's sidebar. \
             Its messages and files stay on the server.",
            "Archive",
            ChannelEvent::Archive,
            window,
            cx,
        )
    }

    fn convert_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.channel_label();
        self.confirm_dialog(
            "channel.convert",
            &self.convert.clone(),
            format!("Make {name} private?"),
            "Only its members will see it, and people join by invitation. \
             Its history stays. This can't be undone from Matty.",
            "Make private",
            ChannelEvent::ConvertToPrivate,
            window,
            cx,
        )
    }

    /// "#name", or "this channel" before the model arrives.
    fn channel_label(&self) -> SharedString {
        match self.model.as_ref() {
            Some(channel) => format!("#{}", channel.name).into(),
            None => "this channel".into(),
        }
    }

    fn leave_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let name: SharedString = match self.model.as_ref() {
            Some(channel) => format!("#{}", channel.name).into(),
            None => "this channel".into(),
        };
        let popup = dialog_popup("channel.leave.popup", "Leave channel", cx)
            .child(dialog_title("channel.leave.title", cx).child(format!("Leave {name}?")))
            .child(
                dialog_description("channel.leave.description", cx).child(
                    "You stop receiving its messages and it leaves your sidebar. You can join again later.",
                ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        alert_dialog_cancel("channel.leave.cancel")
                            .variant(ButtonVariant::Outline)
                            .label("Cancel"),
                    )
                    .child(
                        alert_dialog_action("channel.leave.confirm")
                            .variant(ButtonVariant::Destructive)
                            .label("Leave"),
                    ),
            );
        let view = cx.entity().downgrade();
        alert_dialog("channel.leave", &self.leave, popup, window, cx).on_ok(move |_, _, cx| {
            view.update(cx, |_, cx| cx.emit(ChannelEvent::Leave)).ok();
            true
        })
    }

    fn header(&self, channel: &ChannelModel, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
        let favorite_label = if channel.favorite {
            "Remove from favorites"
        } else {
            "Add to favorites"
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
                        Menu::new(&self.header_menu, "Channel menu")
                            .trigger(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme.space(1.5))
                                    .type_style(style::TITLE, &theme)
                                    .text_color(theme.colors.foreground)
                                    .children(kind_icon.map(|glyph| {
                                        lucide(glyph)
                                            .size(scaled(&theme, metrics::ICON))
                                            .text_color(muted)
                                    }))
                                    .child(channel.name.clone())
                                    .child(
                                        lucide(LucideIcon::ChevronDown)
                                            .size(scaled(&theme, metrics::ICON_SMALL))
                                            .text_color(muted),
                                    ),
                            )
                            .trigger_style_with({
                                let theme = theme.clone();
                                move |button| {
                                    style_button(
                                        button.aria_label("Channel menu").px(theme.space(2.)),
                                        false,
                                        ButtonVariant::Ghost,
                                        ButtonSize::Sm,
                                        &theme,
                                    )
                                }
                            }),
                    )
                    .child(style::tipped(
                        "channel.star.tip",
                        favorite_label,
                        Button::new("channel.star")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::IconSm)
                            .aria_label(favorite_label)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let favorite = this.model.as_ref().is_some_and(|m| !m.favorite);
                                this.set_favorite(favorite, cx);
                                cx.emit(ChannelEvent::ToggleFavorite(favorite));
                            }))
                            .child(
                                lucide(LucideIcon::Star)
                                    .size(scaled(&theme, metrics::ICON_SMALL))
                                    .text_color(if channel.favorite {
                                        Palette::read(cx).away
                                    } else {
                                        muted
                                    }),
                            ),
                    ))
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
                    .child(style::tipped(
                        "channel.members.tip",
                        actions::tooltip_text("Members", "Channel info"),
                        count(
                            "channel.members",
                            format!("{} members", channel.members),
                            LucideIcon::Users,
                            channel.members,
                        )
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ChannelEvent::OpenInfo))),
                    ))
                    .child(style::tipped(
                        "channel.pinned.tip",
                        "Pinned messages",
                        count(
                            "channel.pinned",
                            format!("{} pinned messages", channel.pinned),
                            LucideIcon::Pin,
                            channel.pinned,
                        )
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ChannelEvent::OpenPinned))),
                    ))
                    .child(style::tipped(
                        "channel.info.tip",
                        actions::tooltip_text("Channel info", "Channel info"),
                        action("channel.info", "Channel info", LucideIcon::Info)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ChannelEvent::OpenInfo))),
                    )),
            )
    }

    /// The avatar, wrapped in the profile popover when the author is known.
    fn avatar(
        id: SharedString,
        post: &Post,
        profile: Option<&UserProfile>,
        card: usize,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let initial: String = post.author.chars().take(1).collect();
        let avatar = crate::avatars::with_picture(
            Avatar::new(gpui_kit::ElementId::Name(
                format!("post.{id}.avatar").into(),
            ))
            .size(AvatarSize::Default)
            .fallback(initial),
            &post.author_id,
            cx,
        );
        let Some(profile) = profile.cloned() else {
            return avatar.aria_label(post.author.clone()).into_any_element();
        };
        // The author's presence on the avatar (#135).
        let theme = UiTheme::read(cx);
        let avatar = avatar
            .aria_label(post.author.clone())
            .status(crate::style::avatar_status(
                profile.presence,
                &crate::style::Palette::read(cx),
            ))
            .ring_color(theme.colors.background);
        let view = cx.entity().downgrade();
        let card_id = id.clone();
        let label = crate::style::presence_name(&post.author, profile.presence);
        // Resting on the avatar opens the card; a click or Enter pins it
        // (gpuicn's hover popover, #118).
        hover_popover(
            gpui_kit::ElementId::Name(format!("post.{id}.profile.{card}").into()),
            label,
        )
        .trigger(avatar)
        .content(move |_, cx| {
            let (message_view, mention_view, view_view) =
                (view.clone(), view.clone(), view.clone());
            let (message_profile, mention_profile, view_profile) =
                (profile.clone(), profile.clone(), profile.clone());
            // Each action closes the card by giving it a fresh id.
            let close = |this: &mut Self, cx: &mut Context<Self>| {
                this.profile_cards += 1;
                cx.notify();
            };
            profile_card(
                &card_id,
                &profile,
                ProfileActions {
                    on_message: Box::new(move |_, cx| {
                        let event = ChannelEvent::OpenDirect {
                            user_id: message_profile.user_id.clone(),
                            username: message_profile.username.clone(),
                            display_name: message_profile.display_name.clone(),
                        };
                        message_view
                            .update(cx, |this, cx| {
                                close(this, cx);
                                cx.emit(event);
                            })
                            .ok();
                    }),
                    on_mention: Box::new(move |window, cx| {
                        let text = format!("@{} ", mention_profile.username);
                        mention_view
                            .update(cx, |this, cx| {
                                close(this, cx);
                                this.composer.update(cx, |composer, cx| {
                                    composer.insert(text, window, cx);
                                    composer.focus(window, cx);
                                })
                            })
                            .ok();
                    }),
                    on_view: Some(Box::new(move |_, cx| {
                        let event = ChannelEvent::ViewProfile(view_profile.clone());
                        view_view
                            .update(cx, |this, cx| {
                                close(this, cx);
                                cx.emit(event);
                            })
                            .ok();
                    })),
                },
                cx,
            )
        })
        .into_any_element()
    }

    fn post_row(
        post: &Post,
        profile: Option<&UserProfile>,
        editor: Option<&(Entity<ComposerState>, Pixels)>,
        highlight: MessageHighlight,
        (menu_open, away, previews_collapsed, delete_others, profile_card): (
            bool,
            bool,
            bool,
            bool,
            usize,
        ),
        menus: &crate::interactive::Menus,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let id = post.id.clone();
        let avatar = Self::avatar(id.clone(), post, profile, profile_card, cx);
        let thread_id = post.thread_root();
        let react_items = Self::reaction_items_for(post, cx);
        let add_items = react_items.clone();
        let more_items = Self::more_items_for(post, away, delete_others, cx);
        let save_id = id.clone();
        // A post the server does not have yet can only be copied, edited
        // or discarded, all in More (#195).
        let sent = post.status == PostStatus::Sent;
        let dismiss_id = id.clone();
        let actions = message_actions(cx)
            .when(post.status == PostStatus::Ephemeral, |actions| {
                actions.child(style::tipped_when(
                    !menu_open,
                    gpui_kit::ElementId::Name(format!("post.{id}.dismiss.tip").into()),
                    "Dismiss message",
                    message_action(
                        gpui_kit::ElementId::Name(format!("post.{id}.dismiss").into()),
                        LucideIcon::X,
                        "Dismiss message",
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.delete_post(dismiss_id.clone(), window, cx)
                    })),
                ))
            })
            .when(sent, |actions| {
                actions
                    .child(style::tipped_when(
                        !menu_open,
                        gpui_kit::ElementId::Name(format!("post.{id}.react.tip").into()),
                        "Add reaction",
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
                    ))
                    .child(style::tipped_when(
                        !menu_open,
                        gpui_kit::ElementId::Name(format!("post.{id}.reply.tip").into()),
                        "Reply in thread",
                        message_action(
                            gpui_kit::ElementId::Name(format!("post.{id}.reply").into()),
                            LucideIcon::MessageSquareReply,
                            "Reply in thread",
                            cx,
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(ChannelEvent::OpenThread(thread_id.clone()))
                        })),
                    ))
                    .child(style::tipped_when(
                        !menu_open,
                        gpui_kit::ElementId::Name(format!("post.{id}.save.tip").into()),
                        if post.saved { "Unsave" } else { "Save" },
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
                        .on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.toggle_save(save_id.clone(), cx)
                            }),
                        ),
                    ))
            })
            .child(style::tipped_when(
                !menu_open,
                gpui_kit::ElementId::Name(format!("post.{id}.more.tip").into()),
                "More actions",
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
            ));
        let reaction_post = id.clone();
        let view = cx.entity().downgrade();
        let add_view = view.clone();
        let retry_view = view.clone();
        let retry_id = id.clone();
        let time = post_time(post, away, cx);
        let mut row = Message::new(gpui_kit::ElementId::Name(format!("post.{id}").into()))
            .author(post.author.clone())
            .timestamp(time.clone())
            .gutter_time(time.clone())
            .when(
                matches!(post.status, PostStatus::Sent | PostStatus::Ephemeral)
                    && !post.date.is_empty(),
                |row| row.timestamp_tooltip(format!("{} at {time}", post.date)),
            )
            .on_gutter_click({
                let (view, id) = (cx.entity().downgrade(), id.clone());
                move |_, window, cx| {
                    view.update(cx, |this, cx| this.select_post(&id, window, cx))
                        .ok();
                }
            })
            .continuation(post.continues && !queued(post, away))
            .highlight(highlight)
            .avatar(avatar)
            .reactions(post.reactions.iter().map(|reaction| {
                crate::emoji::reaction(&reaction.emoji, reaction.count, reaction.mine, cx)
            }))
            .reaction_tooltip(reaction_tooltip(post.reactions.clone()))
            .on_reaction(move |emoji, _, cx| {
                let (id, emoji) = (reaction_post.clone(), emoji.clone());
                view.update(cx, |this, cx| this.toggle_reaction(id, emoji, cx))
                    .ok();
            })
            .status(match post.status {
                PostStatus::Sent | PostStatus::Ephemeral => MessageStatus::Sent,
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
            .density(message_density(cx))
            .px(scaled(&theme, metrics::GUTTER))
            .when_some(post.reply_to.as_ref(), |row, context| {
                row.child(Self::reply_context(post, context, cx))
            })
            .child(match editor {
                Some((composer, cap)) => Self::editor(composer, *cap, cx).into_any_element(),
                None => match &post.kind {
                    PostKind::Unsupported(_) => Self::unsupported_card(&id, cx).into_any_element(),
                    _ => {
                        let link_view = cx.entity().downgrade();
                        crate::links::message_body(
                            gpui_kit::ElementId::Name(format!("post.{id}.body").into()),
                            &post.body,
                            move |url, _, cx| {
                                let event = ChannelEvent::OpenLink(url.clone());
                                link_view.update(cx, |_, cx| cx.emit(event)).ok();
                            },
                            cx,
                        )
                        .into_any_element()
                    }
                },
            });
        if queued(post, away) {
            // Queued: a clock marks "Will send when back online" (#195).
            row = row.header_child(queued_marker(&theme));
        }
        for marker in post_markers(post, &theme) {
            row = row.header_child(marker);
        }
        if !post.reactions.is_empty() {
            // The chip row gets an add button; posts without reactions use
            // the hover toolbar, so the transcript stays quiet.
            row = row.on_add_reaction(move |event, window, cx| {
                let items = add_items.clone();
                add_view
                    .update(cx, |this, cx| {
                        this.open_menu(items, event.position(), window, cx)
                    })
                    .ok();
            });
        }
        if !post.embeds.is_empty() {
            let jump_view = cx.entity().downgrade();
            let on_jump: crate::embeds::OnJump = std::rc::Rc::new(move |channel, post, _, cx| {
                let event = ChannelEvent::OpenPermalink {
                    channel: channel.clone(),
                    post: post.clone(),
                };
                jump_view.update(cx, |_, cx| cx.emit(event)).ok();
            });
            let (view, post_id) = (cx.entity().downgrade(), id.clone());
            let (image_view, image_post) = (view.clone(), id.clone());
            row = row.child(crate::embeds::post_embeds(
                &id,
                &post.embeds,
                previews_collapsed,
                std::rc::Rc::new(move |collapsed, _, cx| {
                    let post_id = post_id.clone();
                    view.update(cx, |this, cx| this.collapse_preview(post_id, collapsed, cx))
                        .ok();
                }),
                std::rc::Rc::new(move |url, window, cx| {
                    let post = image_post.clone();
                    image_view
                        .update(cx, |this, cx| this.open_linked_image(post, url, window, cx))
                        .ok();
                }),
                on_jump,
                (menus, {
                    let view = cx.entity().downgrade();
                    std::rc::Rc::new(move |choice, _, cx| {
                        view.update(cx, |_, cx| cx.emit(ChannelEvent::PostAction(choice)))
                            .ok();
                    })
                }),
                cx,
            ));
        }
        if !post.files.is_empty() {
            let (image_view, image_post) = (cx.entity().downgrade(), id.clone());
            let file_view = image_view.clone();
            row = row.child(post_files(
                &format!("post.{id}"),
                &post.files,
                move |index, window, cx| {
                    let post = image_post.clone();
                    image_view
                        .update(cx, |this, cx| this.open_lightbox(post, index, window, cx))
                        .ok();
                },
                move |file, _, cx| {
                    file_view
                        .update(cx, |_, cx| cx.emit(ChannelEvent::OpenFile(file)))
                        .ok();
                },
                cx,
            ));
        }
        if let Some(summary) = &post.replies {
            row = row.child(Self::reply_footer(&id, summary, cx));
        }
        // Right-click selects the row and opens the same actions as the
        // toolbar's "More" button, plus reply and react (PLAN §4.2).
        let context_items = Self::context_items_for(post, away, delete_others, cx);
        div()
            .on_mouse_down(
                gpui_kit::MouseButton::Right,
                cx.listener(move |this, event: &gpui_kit::MouseDownEvent, window, cx| {
                    this.select_post(&id, window, cx);
                    this.open_menu(context_items.clone(), event.position, window, cx);
                }),
            )
            .child(row)
    }

    /// A system message (#104): a muted line in the body column, with the
    /// time in the gutter on hover like a continuation, and no actions.
    fn system_row(post: &Post, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let time: SharedString = ClockFormat::current(cx).format(post.time).into();
        Message::new(gpui_kit::ElementId::Name(
            format!("post.{}", post.id).into(),
        ))
        .continuation(true)
        .timestamp(time.clone())
        .gutter_time(time.clone())
        .when(!post.date.is_empty(), |row| {
            row.timestamp_tooltip(format!("{} at {time}", post.date))
        })
        .density(message_density(cx))
        .px(scaled(&theme, metrics::GUTTER))
        .py(theme.space(1.))
        .child(
            div()
                .text_size(theme.text(13.))
                .line_height(theme.text(20.))
                .text_color(theme.colors.muted_foreground)
                .child(post.body.clone()),
        )
    }

    /// The body of a post whose type Matty does not draw yet (#104): a card
    /// that says so and opens the post in the web app.
    fn unsupported_card(id: &SharedString, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let open = id.clone();
        MessageEmbed::new(gpui_kit::ElementId::Name(
            format!("post.{id}.unsupported").into(),
        ))
        .density(message_density(cx))
        .mt(theme.space(1.))
        .body(
            div()
                .flex()
                .flex_col()
                .items_start()
                .gap(theme.space(2.))
                .child(
                    div()
                        .text_color(theme.colors.muted_foreground)
                        .child("This message type isn't supported yet."),
                )
                .child(
                    Button::new(gpui_kit::ElementId::Name(
                        format!("post.{id}.open-in-browser").into(),
                    ))
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Sm)
                    .label("Open in browser")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(ChannelEvent::OpenInBrowser(open.clone()))
                    })),
                ),
        )
    }

    /// The thread line under a post with replies: who replied, how many,
    /// when last, and Follow. Clicking it opens the thread.
    /// The line above a reply while collapsed reply threads are off
    /// (#123): "Commented on Ada's message: …", which opens the thread.
    fn reply_context(
        post: &Post,
        context: &ReplyContext,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let root = post.thread_root();
        let whose = format!("{}'s", context.author);
        div()
            .id(gpui_kit::ElementId::Name(
                format!("post.{}.replying", post.id).into(),
            ))
            .role(gpui_kit::Role::Button)
            .aria_label(format!(
                "Open thread: commented on {whose} message: {}",
                context.excerpt
            ))
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(theme.space(1.))
            .pb(theme.space(0.5))
            .cursor_pointer()
            .type_style(style::META, &theme)
            .text_color(theme.colors.muted_foreground)
            .hover(|style| style.text_color(theme.colors.foreground))
            .on_click(
                cx.listener(move |_, _, _, cx| cx.emit(ChannelEvent::OpenThread(root.clone()))),
            )
            .child(
                lucide(LucideIcon::MessageSquareReply)
                    .flex_shrink_0()
                    .size(scaled(&theme, metrics::ICON_TINY)),
            )
            .child(div().flex_shrink_0().child("Commented on"))
            .child(
                div()
                    .flex_shrink_0()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(whose),
            )
            .child(div().flex_shrink_0().child("message:"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(context.excerpt.clone()),
            )
    }

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
            .role(gpui_kit::Role::Button)
            .aria_label(format!("Open thread, {label}"))
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
                            let user_id = summary
                                .participant_ids
                                .get(index)
                                .map(SharedString::as_ref)
                                .unwrap_or_default();
                            div().when(index > 0, |this| this.ml(px(-4.))).child(
                                crate::avatars::with_picture(
                                    Avatar::new(gpui_kit::ElementId::Name(
                                        format!("post.{id}.thread.{index}").into(),
                                    ))
                                    .size(AvatarSize::Sm)
                                    .fallback(initial)
                                    .aria_label(name.clone()),
                                    user_id,
                                    cx,
                                ),
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
            .when(summary.unread(), |this| {
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
                    .child(format!(
                        "Last reply {}",
                        summary.last_reply_label(ClockFormat::current(cx))
                    )),
            )
            .child(
                Button::new(gpui_kit::ElementId::Name(
                    format!("post.{id}.thread.follow").into(),
                ))
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::Xs)
                .label(if summary.following {
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
        let following_now = self
            .shown_post(&id)
            .and_then(|post| post.replies.as_ref())
            .is_some_and(|summary| summary.following);
        if self.refused_offline(
            if following_now {
                "unfollow thread"
            } else {
                "follow thread"
            },
            cx,
        ) {
            return;
        }
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
        let profiles = channel.profiles.clone();
        let view = cx.entity();
        MessageList::new(
            "channel.transcript",
            &self.list,
            move |index, _, cx| match rows.get(index) {
                Some(Row::History) => view.update(cx, |this, cx| {
                    this.request_older(cx);
                    this.history_row(cx).into_any_element()
                }),
                Some(Row::Newer) => view.update(cx, |this, cx| {
                    this.request_newer(cx);
                    this.newer_row(cx).into_any_element()
                }),
                Some(Row::Day(label)) => day_divider(
                    gpui_kit::ElementId::Name(format!("day.{label}").into()),
                    label.clone(),
                    cx,
                )
                .into_any_element(),
                Some(Row::NewMessages) => {
                    new_messages_divider("new-messages", "New messages", cx).into_any_element()
                }
                Some(Row::System(post)) => {
                    let post = posts[*post].clone();
                    view.update(cx, |_, cx| Self::system_row(&post, cx).into_any_element())
                }
                Some(Row::Post(post)) => {
                    let post = posts[*post].clone();
                    let profile = profiles
                        .iter()
                        .find(|profile| profile.display_name == post.author);
                    view.update(cx, |this, cx| {
                        let editor = this
                            .editing
                            .as_ref()
                            .filter(|editing| editing.post == post.id)
                            .map(|editing| (editing.composer.clone(), this.composer_cap));
                        let highlight = this.row_highlight(index, &post);
                        let menu_open = this.menu.read(cx).is_open();
                        let away = this.away;
                        let delete_others =
                            this.model.as_ref().is_some_and(|model| model.delete_others);
                        let previews_collapsed = SettingsStore::global(cx)
                            .read(cx)
                            .get()
                            .previews_collapsed(&post.id);
                        Self::post_row(
                            &post,
                            profile,
                            editor.as_ref(),
                            highlight,
                            (
                                menu_open,
                                away,
                                previews_collapsed,
                                delete_others,
                                this.profile_cards,
                            ),
                            &this.action_menus,
                            cx,
                        )
                        .into_any_element()
                    })
                }
                None => div().into_any_element(),
            },
        )
        .unseen(self.unseen)
        .when(!self.history.at_end, |list| {
            let view = cx.entity().downgrade();
            list.show_jump(true).on_jump(move |_, cx| {
                view.update(cx, |this, cx| this.jump_latest(cx)).ok();
            })
        })
        .track_focus(&self.list_focus)
        .selected(self.selection)
        .selectable({
            let rows = self.rows.clone();
            move |index, _| matches!(rows.get(index), Some(Row::Post(_)))
        })
        .on_select({
            let view = cx.entity().downgrade();
            move |index, window, cx| {
                view.update(cx, |this, cx| this.select_row(index, window, cx))
                    .ok();
            }
        })
        .on_key({
            let view = cx.entity().downgrade();
            move |event, selected, window, cx| {
                view.update(cx, |this, cx| {
                    this.selection_key(&event.keystroke, selected, window, cx)
                })
                .unwrap_or(false)
            }
        })
        .size_full()
        .pt(theme.space(2.))
        .pb(theme.space(1.))
    }

    /// Up, Down or Escape moved the selection; Escape hands focus back to
    /// the composer (PLAN §4.2).
    fn select_row(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.selection = index;
        if index.is_none() {
            self.composer
                .update(cx, |composer, cx| composer.focus(window, cx));
        }
        cx.notify();
    }

    /// Enters selection mode on the newest post, from Shift+Tab in the
    /// composer or a gutter click.
    fn select_newest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let newest = self
            .rows
            .iter()
            .rposition(|row| matches!(row, Row::Post(_)));
        if newest.is_some() {
            self.selection = newest;
            window.focus(&self.list_focus, cx);
            cx.notify();
        }
    }

    /// Single keys on the selected post, as in Discord: R reply, E edit,
    /// Delete removes own, + react, P pin, S save, U mark unread, Enter
    /// views its images. Keys with
    /// a modifier are left to the keymap. Returns whether the key was used.
    fn selection_key(
        &mut self,
        keystroke: &gpui_kit::Keystroke,
        selected: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let modifiers = keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt {
            return false;
        }
        let Some(Row::Post(index)) = selected.and_then(|row| self.rows.get(row).cloned()) else {
            return false;
        };
        let Some(post) = self
            .model
            .as_ref()
            .and_then(|m| m.posts.get(index))
            .cloned()
        else {
            return false;
        };
        let id = post.id.clone();
        match keystroke.key.as_str() {
            "r" => cx.emit(ChannelEvent::OpenThread(post.thread_root())),
            "e" if post.mine => {
                self.selection = None;
                self.start_edit(id, window, cx);
            }
            "delete" | "backspace"
                if deletable(
                    &post,
                    self.away,
                    self.model.as_ref().is_some_and(|model| model.delete_others),
                ) =>
            {
                self.ask_delete(id, window, cx)
            }
            "+" | "=" if keystroke.key == "+" || modifiers.shift => self.open_picker(Some(id), cx),
            "p" => self.toggle_pin(id, cx),
            "s" => self.toggle_save(id, cx),
            "u" => cx.emit(ChannelEvent::MarkUnread(id)),
            "enter" => return self.open_selected_images(id, window, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Opens "Delete message?" for `id` (PLAN §4.5).
    fn ask_delete(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        // A waiting send is discarded here; a sent one needs the server.
        let sent = self
            .shown_post(&id)
            .is_some_and(|post| post.status == PostStatus::Sent);
        if sent && self.refused_offline("delete", cx) {
            return;
        }
        self.deleting = Some(id);
        self.delete.open(window, cx);
        cx.notify();
    }

    /// Selection wins over permalink over mention: the reader's own choice
    /// comes first, then the row they asked for.
    fn row_highlight(&self, row: usize, post: &Post) -> MessageHighlight {
        if self.selection == Some(row) {
            MessageHighlight::Selected
        } else if self.permalink.as_ref() == Some(&post.id) {
            MessageHighlight::Permalink
        } else if post.mentions_me {
            MessageHighlight::Mention
        } else {
            MessageHighlight::None
        }
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
        let send_on = SettingsStore::global(cx).read(cx).get().send_on;
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
        let picker = self.picker.clone();
        let picker_open = self.picker_open;
        let emoji_tool = tool("composer.emoji", "Emoji", LucideIcon::FaceSlightlySmiling);
        let emoji_popover = popover("composer.emoji.popover")
            .open(picker_open)
            .offset(px(6.))
            .trigger_with(move |_, _, _| {
                style::tipped_when(!picker_open, "composer.emoji.tip", "Emoji", emoji_tool)
                    .into_any_element()
            })
            .on_open_change(cx.listener(|this, open: &bool, window, cx| {
                if *open {
                    this.open_picker(None, cx);
                } else {
                    this.close_picker(window, cx);
                }
            }))
            .content(move |_, window, cx| {
                picker.update(cx, |picker, cx| picker.open(window, cx));
                emoji_popup(EmojiPicker::new(&picker).bordered(false), cx)
            });
        let link_preview = self.link.preview(
            "composer.link-preview",
            cx,
            cx.listener(|this, _, _, cx| {
                this.link.remove();
                cx.notify();
            }),
        );
        let chips: Vec<Attachment> = self
            .tray
            .uploads()
            .iter()
            .map(|upload| self.upload_chip(upload, cx))
            .collect();
        let tray = crate::attachments::upload_tray("composer", &self.tray, chips, cx);
        let mut key = gpui_kit::KeyContext::new_with_defaults(cx.platform_info().os);
        key.add("Composer");
        if self.draft_len == 0 {
            key.add("empty");
        }
        div()
            .key_context(key)
            .on_action(cx.listener(|this, _: &EditLastMessage, window, cx| {
                if let Some(id) = this.last_own_post() {
                    this.start_edit(id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ReplyToLastMessage, _, cx| {
                if let Some(post) = this.model.as_ref().and_then(|m| m.last_message()) {
                    cx.emit(ChannelEvent::OpenThread(post.thread_root()));
                }
            }))
            .on_action(
                cx.listener(|this, _: &UploadFiles, window, cx| this.choose_files(window, cx)),
            )
            .flex_shrink_0()
            .px(scaled(&theme, metrics::GUTTER))
            .pt(theme.space(1.))
            .pb(theme.space(4.))
            .children(link_preview)
            .children(tray)
            .child(
                Composer::new(&self.composer)
                    .max_height(self.composer_cap)
                    .rounded(theme.radius.lg)
                    .child(style::tipped(
                        "composer.attach.tip",
                        actions::tooltip_text("Attach files", "Upload files"),
                        tool("composer.attach", "Attach files", LucideIcon::Plus).on_click(
                            |_, window, cx| window.dispatch_action(Box::new(UploadFiles), cx),
                        ),
                    ))
                    .child(style::tipped(
                        "composer.bold.tip",
                        actions::tooltip_text("Bold", "Bold"),
                        tool("composer.bold", "Bold", LucideIcon::Bold).on_click(
                            cx.listener(|this, _, window, cx| this.wrap("**", "**", window, cx)),
                        ),
                    ))
                    .child(style::tipped(
                        "composer.italic.tip",
                        actions::tooltip_text("Italic", "Italic"),
                        tool("composer.italic", "Italic", LucideIcon::Italic).on_click(
                            cx.listener(|this, _, window, cx| this.wrap("_", "_", window, cx)),
                        ),
                    ))
                    .child(style::tipped(
                        "composer.code.tip",
                        actions::tooltip_text("Code", "Inline code"),
                        tool("composer.code", "Code", LucideIcon::Code).on_click(
                            cx.listener(|this, _, window, cx| this.wrap("`", "`", window, cx)),
                        ),
                    ))
                    .child(emoji_popover)
                    .child(style::tipped(
                        "composer.mention.tip",
                        "Mention someone",
                        tool("composer.mention", "Mention someone", LucideIcon::AtSign).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.composer.update(cx, |composer, cx| {
                                    composer.insert("@", window, cx);
                                    composer.focus(window, cx);
                                })
                            }),
                        ),
                    ))
                    .child(div().flex_1())
                    .children(counter)
                    .child(style::tipped(
                        "composer.send.tip",
                        actions::send_tooltip("Send", send_on),
                        Button::new("composer.send")
                            .variant(ButtonVariant::Default)
                            .size(ButtonSize::IconSm)
                            .aria_label("Send")
                            .disabled(!self.can_send())
                            .on_click(cx.listener(|this, _, window, cx| {
                                let text = this.composer.read(cx).value(cx);
                                this.send(text, window, cx)
                            }))
                            .child(if self.tray.is_queued() {
                                Spinner::new("composer.send.spinner")
                                    .color(theme.colors.primary_foreground)
                                    .into_any_element()
                            } else {
                                lucide(LucideIcon::SendHorizontal)
                                    .size(scaled(&theme, metrics::ICON))
                                    .text_color(theme.colors.primary_foreground)
                                    .into_any_element()
                            }),
                    )),
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

    /// A channel with nothing saved on this computer while offline: saying
    /// it is empty would be wrong, so it says why and that it will load.
    fn offline_channel(channel: &ChannelModel, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let place: SharedString = match channel.kind {
            ChannelKind::Direct { .. } => format!("your conversation with {}", channel.name).into(),
            _ => format!("#{}", channel.name).into(),
        };
        let empty = style::empty_state(
            LucideIcon::WifiOff,
            "You're offline",
            format!(
                "Matty hasn't saved {place} on this computer yet. Its messages load when you're back online."
            ),
            cx,
        );
        div()
            .size_full()
            .flex()
            .items_end()
            .justify_center()
            .p(theme.space(6.))
            .pb(theme.space(8.))
            .child(
                div()
                    .max_w(scaled(&theme, metrics::EMPTY_MAX_WIDTH))
                    .child(empty),
            )
    }

    /// A channel with no posts yet: its purpose and a way to bring people in.
    fn empty_channel(channel: &ChannelModel, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let direct = matches!(channel.kind, ChannelKind::Direct { .. });
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
        let glyph = if direct {
            LucideIcon::MessageSquare
        } else {
            LucideIcon::Hash
        };
        let mut empty = style::empty_state(glyph, title, description, cx);
        if !direct {
            empty = empty.content(
                EmptyContent::new().child(
                    Button::new("channel.empty.add-members")
                        .variant(ButtonVariant::Outline)
                        .size(ButtonSize::Sm)
                        .aria_label("Add members")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_add_members(window, cx)),
                        )
                        .child(
                            lucide(LucideIcon::UserPlus)
                                .size(scaled(&theme, metrics::ICON_SMALL))
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
            .child(
                div()
                    .max_w(scaled(&theme, metrics::EMPTY_MAX_WIDTH))
                    .child(empty),
            )
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
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(style::centered_empty(
                LucideIcon::MessagesSquare,
                "Not connected yet",
                "Your channels appear here once Matty is connected to a Mattermost server.",
                cx,
            ))
    }
}

impl Render for ChannelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.composer_cap = style::composer_cap(window);
        if let Some(model) = &self.model {
            self.action_menus
                .sync(&model.posts, window, cx, |_, choice, cx| {
                    cx.emit(ChannelEvent::PostAction(choice))
                });
        }
        let theme = UiTheme::read(cx).clone();
        let root = div()
            .key_context(actions::CHANNEL_VIEW)
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(theme.colors.background)
            // Escape ends an edit or a selection first; only then does it
            // reach the window, which marks the channel read (#147).
            .on_action(cx.listener(|this, _: &MarkChannelRead, window, cx| {
                if this.editing.is_some() {
                    this.cancel_edit(window, cx);
                } else if this.selection.is_some() {
                    this.select_row(None, window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(
                cx.listener(|this, _: &FocusComposer, window, cx| this.focus_composer(window, cx)),
            );
        let Some(channel) = self.model.clone() else {
            return root.child(Self::empty(cx));
        };
        // Back at the end, everything has been seen.
        if self.unseen > 0 && !self.list.read(cx).is_scrolled_up() {
            self.unseen = 0;
        }
        let body = if channel.loading && channel.posts.is_empty() {
            Self::loading(cx).into_any_element()
        } else if channel.posts.is_empty() && channel.offline {
            Self::offline_channel(&channel, cx).into_any_element()
        } else if channel.posts.is_empty() {
            Self::empty_channel(&channel, cx).into_any_element()
        } else {
            self.transcript(&channel, cx).into_any_element()
        };
        // Files dropped anywhere on the view go to its composer (#150).
        let overlay = crate::attachments::drop_overlay(
            "channel.drop",
            crate::attachments::drop_title(&channel.name),
            "Files go into the message you are writing.",
            crate::attachments::preview_drop(),
            cx,
        );
        root.relative()
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.add_paths(paths.paths().iter().cloned(), cx)
            }))
            .child(self.header(&channel, cx))
            .child(div().flex_1().min_h_0().child(body))
            .child(Self::typing_line(&channel, cx))
            .child(self.composer(cx))
            .children(overlay)
            // Zero-size host for the shared post menu; it opens at the pointer.
            // Disabled so the empty host is not a tab stop.
            .child(
                Menu::new(&self.menu, "Message actions")
                    .context_area(div())
                    .disabled(true),
            )
            .child(self.delete_dialog(window, cx))
            .child(self.edit_dialog(window, cx))
            .child(self.notifications_dialog(window, cx))
            .child(self.add_members_dialog(window, cx))
            .child(self.leave_dialog(window, cx))
            .child(self.archive_dialog(window, cx))
            .child(self.convert_dialog(window, cx))
            .child(self.lightbox(window, cx))
    }
}

/// The picker's popover surface: gpuicn's popover colors and shadow around
/// the borderless picker.
fn emoji_popup(picker: EmojiPicker, cx: &App) -> impl IntoElement + use<> {
    let theme = UiTheme::read(cx);
    div()
        .rounded(theme.radius.xl)
        .border_1()
        .border_color(theme.colors.foreground.opacity(0.1))
        .bg(theme.colors.popover)
        .shadow(theme.shadows.md.to_vec())
        .overflow_hidden()
        .child(picker)
}

/// What the typing line says for `names`: "Ana is typing…", "Ana and Ben
/// are typing…", "Several people are typing…", or nothing.
pub(crate) fn typing_text(names: &[SharedString]) -> String {
    let first = |name: &SharedString| name.split(' ').next().unwrap_or(name).to_string();
    match names {
        [] => String::new(),
        [one] => format!("{} is typing\u{2026}", first(one)),
        [one, two] => format!("{} and {} are typing\u{2026}", first(one), first(two)),
        _ => "Several people are typing\u{2026}".to_string(),
    }
}

/// The typing line under a transcript (#137); it keeps its height when
/// nobody is typing, so the transcript does not jump.
pub(crate) fn typing_line(names: &[SharedString], cx: &App) -> gpui_kit::Div {
    let theme = UiTheme::read(cx);
    div()
        .flex_shrink_0()
        .h(scaled(theme, metrics::TYPING_LINE))
        .px(theme.space(4.))
        .type_style(style::META, theme)
        .text_color(theme.colors.muted_foreground)
        .child(typing_text(names))
}

/// `@` suggestions from `profiles` for `query` (lowercase): a person
/// matches by username prefix or anywhere in the name the user's format
/// shows (#156). The row reads that name, with the username under it
/// when it says something more.
/// `@here`, `@channel` and `@all`, after the people, as the webapp offers
/// them in a channel.
fn group_mentions(query: &str, limit: usize) -> Vec<ComposerSuggestion> {
    const GROUPS: [(&str, &str); 3] = [
        ("here", "Everyone online here"),
        ("channel", "Everyone in the channel"),
        ("all", "Everyone in the channel"),
    ];
    GROUPS
        .iter()
        .filter(|(name, _)| name.starts_with(query))
        .take(limit)
        .map(|(name, what)| {
            ComposerSuggestion::new(format!("@{name}"), format!("@{name}"))
                .description(*what)
                .icon(LucideIcon::Users)
        })
        .collect()
}

/// The `@` name a composer last asked the server about, and the answer.
/// Shared by the channel and thread composers.
#[derive(Default)]
pub(crate) struct PeopleAsk {
    /// The `@` query the composer shows suggestions for, if any.
    asking: Option<String>,
    /// The server's matches for a typed `@name`, by what was typed.
    answer: Option<(String, Vec<UserProfile>)>,
}

impl PeopleAsk {
    /// Notes what the composer is completing; the term to ask the server
    /// about, once per `@` query.
    pub(crate) fn ask(&mut self, trigger: char, query: &str) -> Option<String> {
        if trigger != '@' {
            self.asking = None;
            return None;
        }
        let query = query.to_lowercase();
        if self.asking.as_deref() == Some(query.as_str()) {
            return None;
        }
        self.asking = Some(query.clone());
        self.answer
            .as_ref()
            .is_none_or(|(term, _)| *term != query)
            .then_some(query)
    }

    /// Keeps the server's matches for `term`; whether the composer still
    /// shows that query.
    pub(crate) fn answer(&mut self, term: &str, people: Vec<UserProfile>) -> bool {
        self.answer = Some((term.to_owned(), people));
        self.asking.as_deref() == Some(term)
    }

    /// The server's matches that still fit `query`.
    fn matches(&self, query: &str) -> impl Iterator<Item = &UserProfile> + use<'_> {
        let query = query.to_owned();
        self.answer
            .iter()
            .filter(move |(term, _)| query.starts_with(term.as_str()))
            .flat_map(|(_, people)| people.iter())
    }
}

/// Suggestions for a composer trigger (PLAN §4.3): `@` people from `local`
/// then the server's matches, plus the group mentions in a channel; `~`
/// channels from the sidebar; `:` emoji from the catalog once two
/// characters narrow it.
pub(crate) fn composer_suggestions<'a>(
    trigger: char,
    query: &str,
    local: impl Iterator<Item = &'a UserProfile>,
    people: &'a PeopleAsk,
    directory: &[ChannelRow],
    groups: bool,
) -> Vec<ComposerSuggestion> {
    const LIMIT: usize = 8;
    let query = query.to_lowercase();
    match trigger {
        '@' => {
            // The groups keep their rows even when many people match.
            let groups = if groups {
                group_mentions(&query, LIMIT)
            } else {
                Vec::new()
            };
            // Local people first, then whoever the server found, each once.
            let mut seen = std::collections::HashSet::new();
            let mut found = people_suggestions(
                local
                    .chain(people.matches(&query))
                    .filter(|profile| seen.insert(profile.username.clone())),
                &query,
                LIMIT - groups.len(),
            );
            found.extend(groups);
            found
        }
        '~' => directory
            .iter()
            .filter(|row| matches!(row.kind, ChannelKind::Public | ChannelKind::Private))
            .filter(|row| {
                row.name.to_lowercase().contains(&query)
                    || row
                        .handle
                        .as_ref()
                        .is_some_and(|handle| handle.to_lowercase().contains(&query))
            })
            .take(LIMIT)
            .map(|row| {
                // Mattermost links `~url-name`, not the display name.
                let handle = row.handle.as_ref().unwrap_or(&row.name);
                ComposerSuggestion::new(format!("~{handle}"), row.name.clone())
                    .description(format!("~{handle}"))
                    .icon(match row.kind {
                        ChannelKind::Private => LucideIcon::Lock,
                        _ => LucideIcon::Hash,
                    })
            })
            .collect(),
        ':' => crate::emoji::suggest(&query, LIMIT)
            .into_iter()
            .map(|(name, glyph)| ComposerSuggestion::new(glyph, format!(":{name}:")))
            .collect(),
        _ => Vec::new(),
    }
}

fn people_suggestions<'a>(
    profiles: impl Iterator<Item = &'a UserProfile>,
    query: &str,
    limit: usize,
) -> Vec<ComposerSuggestion> {
    profiles
        .filter(|profile| {
            profile.username.to_lowercase().starts_with(query)
                || profile.display_name.to_lowercase().contains(query)
        })
        .take(limit)
        .map(|profile| {
            let suggestion = ComposerSuggestion::new(
                format!("@{}", profile.username),
                profile.display_name.clone(),
            );
            if profile.display_name == profile.username {
                suggestion
            } else {
                suggestion.description(format!("@{}", profile.username))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminders_land_at_the_offered_times() {
        let zone = jiff::tz::TimeZone::get("Asia/Kolkata").unwrap();
        // 2026-11-04 12:00 UTC is 17:30 in Kolkata.
        let now = jiff::Timestamp::from_second(1_793_793_600).unwrap();
        assert_eq!(
            RemindAt::In30Minutes.target(now, &zone),
            1_793_793_600 + 1800
        );
        assert_eq!(RemindAt::In2Hours.target(now, &zone), 1_793_793_600 + 7200);
        // 2026-11-05 09:00 in Kolkata is 03:30 UTC.
        assert_eq!(
            RemindAt::Tomorrow.target(now, &zone),
            jiff::Timestamp::from_second(1_793_793_600 + 15 * 3600 + 1800)
                .unwrap()
                .as_second()
        );
    }

    #[test]
    fn tilde_inserts_the_channels_url_name() {
        let rows = [
            ChannelRow::new("c1", "Town Square", ChannelKind::Public).handle("town-square"),
            ChannelRow::new("c2", "Ops", ChannelKind::Private),
            ChannelRow::new(
                "d",
                "Ada",
                ChannelKind::Direct {
                    presence: crate::sidebar::Presence::Offline,
                },
            )
            .handle("ada"),
        ];
        let values = |query: &str| {
            composer_suggestions(
                '~',
                query,
                std::iter::empty(),
                &PeopleAsk::default(),
                &rows,
                true,
            )
            .into_iter()
            .map(|s| (s.value.to_string(), s.label.to_string()))
            .collect::<Vec<_>>()
        };
        assert_eq!(
            values("town-"),
            [("~town-square".to_owned(), "Town Square".to_owned())]
        );
        assert_eq!(
            values("o"),
            [
                ("~town-square".to_owned(), "Town Square".to_owned()),
                ("~Ops".to_owned(), "Ops".to_owned()),
            ]
        );
        assert!(values("ada").is_empty());
    }

    #[test]
    fn a_format_shortcut_on_formatted_text_takes_the_format_off() {
        // Markers just outside the selection.
        assert_eq!(
            unwrap_selection("a **bold** b", 4..8, "**", "**"),
            Some((2..10, 2..6))
        );
        // Markers inside it.
        assert_eq!(
            unwrap_selection("a **bold** b", 2..10, "**", "**"),
            Some((2..10, 2..6))
        );
        assert_eq!(
            unwrap_selection("a `x` b", 3..4, "`", "`"),
            Some((2..5, 2..3))
        );
        // Not wrapped, or a lone marker that belongs to a longer one.
        assert_eq!(unwrap_selection("a bold b", 2..6, "**", "**"), None);
        assert_eq!(unwrap_selection("``x``", 2..3, "`", "`"), None);
        // A second Cmd+B on an empty pair takes it back out.
        assert_eq!(
            unwrap_selection("****", 2..2, "**", "**"),
            Some((0..4, 0..0))
        );
        assert_eq!(unwrap_selection("**", 0..2, "**", "**"), None);
    }

    #[test]
    fn deleting_a_root_names_the_replies_it_takes() {
        assert_eq!(
            delete_description("#town", 0),
            "It is removed for everyone in #town. This cannot be undone."
        );
        assert_eq!(
            delete_description("#town", 1),
            "It is removed for everyone in #town, along with its reply. This cannot be undone."
        );
        assert!(delete_description("#town", 3).contains("along with its 3 replies"));
    }

    #[test]
    fn group_mentions_follow_what_is_typed() {
        let values = |query: &str, limit| {
            group_mentions(query, limit)
                .into_iter()
                .map(|s| s.value.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(values("", 8), ["@here", "@channel", "@all"]);
        assert_eq!(values("ch", 8), ["@channel"]);
        assert_eq!(values("a", 8), ["@all"]);
        assert_eq!(values("", 1), ["@here"]);
        assert!(values("ada", 8).is_empty());
    }

    #[test]
    fn mention_suggestions_name_people_in_the_users_format() {
        use crate::settings::NameFormat;
        let user = matty_api::models::User {
            id: "u1".into(),
            username: "ada".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            nickname: "Countess".into(),
            ..Default::default()
        };
        let profile = |format| UserProfile {
            user_id: "u1".into(),
            username: "ada".into(),
            display_name: crate::transcript::display_name_as(&user, format).into(),
            position: None,
            presence: crate::sidebar::Presence::Offline,
            status_text: None,
            time_zone: None,
        };
        let shown = |format, query: &str| {
            people_suggestions(std::iter::once(&profile(format)), query, 8)
                .into_iter()
                .map(|s| (s.value, s.label, s.description))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            shown(NameFormat::FullName, "love"),
            vec![("@ada".into(), "Ada Lovelace".into(), Some("@ada".into()))]
        );
        assert_eq!(
            shown(NameFormat::NicknameFullName, "ad"),
            vec![("@ada".into(), "Countess".into(), Some("@ada".into()))]
        );
        // The username alone needs no second line saying it again.
        assert_eq!(
            shown(NameFormat::Username, "ad"),
            vec![("@ada".into(), "ada".into(), None)]
        );
        // A name the format hides no longer matches.
        assert!(shown(NameFormat::Username, "love").is_empty());
    }

    #[test]
    fn reaction_summaries_read_like_a_sentence() {
        let reaction = |count, mine, users: &[&str]| PostReaction {
            emoji: "👍".into(),
            count,
            mine,
            users: users
                .iter()
                .map(|u| SharedString::from(u.to_string()))
                .collect(),
        };
        assert_eq!(reaction(1, true, &[]).summary(), "You reacted with 👍");
        assert_eq!(
            reaction(2, false, &["Priya Nair", "Sam Okafor"]).summary(),
            "Priya Nair and Sam Okafor reacted with 👍"
        );
        assert_eq!(
            reaction(4, true, &["Priya Nair", "Sam Okafor", "Lena Fischer"]).summary(),
            "You, Priya Nair, Sam Okafor and 1 other reacted with 👍"
        );
        // Sync has the count before the names.
        assert_eq!(
            reaction(3, false, &[]).summary(),
            "3 others reacted with 👍"
        );
    }

    fn post(id: &str, day: &str) -> Post {
        Post {
            id: id.into(),
            author: "a".into(),
            author_id: SharedString::default(),
            kind: PostKind::Regular,
            time: 600,
            day: day.into(),
            date: SharedString::default(),
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
            root_id: "".into(),
            reply_to: None,
            files: vec![],
            embeds: vec![],
        }
    }

    #[test]
    fn admins_may_delete_other_peoples_posts() {
        let theirs = post("p1", "Today");
        let mine = Post {
            mine: true,
            ..post("p2", "Today")
        };
        // A member deletes only their own.
        assert!(deletable(&mine, false, false));
        assert!(!deletable(&theirs, false, false));
        // A system admin deletes anyone's sent post.
        assert!(deletable(&theirs, false, true));
        // Not a system row, and not someone else's post still sending.
        let system = Post {
            kind: PostKind::System { activity: false },
            ..post("p3", "Today")
        };
        assert!(!deletable(&system, false, true));
        let sending = Post {
            status: PostStatus::Pending,
            ..post("p4", "Today")
        };
        assert!(!deletable(&sending, false, true));
    }

    #[test]
    fn only_a_waiting_send_of_ones_own_can_be_changed() {
        let mine = |status| Post {
            mine: true,
            status,
            ..post("p", "Today")
        };
        // Offline, a pending send is queued; online it is on its way.
        assert!(waiting_to_send(&mine(PostStatus::Pending), true));
        assert!(!waiting_to_send(&mine(PostStatus::Pending), false));
        assert!(waiting_to_send(&mine(PostStatus::Failed), false));
        assert!(!waiting_to_send(&mine(PostStatus::Sent), true));
        let theirs = Post {
            status: PostStatus::Pending,
            ..post("p", "Today")
        };
        assert!(!waiting_to_send(&theirs, true));
    }

    /// #113: `@` asks the server once per query, and its matches follow the
    /// channel's own posters without repeating anyone.
    #[gpui_kit::test]
    fn server_matches_follow_the_channels_people(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let person = |name: &str| UserProfile {
            user_id: name.to_owned().into(),
            username: name.to_owned().into(),
            display_name: name.to_owned().into(),
            position: None,
            presence: crate::sidebar::Presence::Offline,
            status_text: None,
            time_zone: None,
        };
        let model = ChannelModel {
            purpose: None,
            offline: false,
            loading: false,
            max_post_size: DEFAULT_MAX_POST_SIZE,
            profiles: vec![person("grace")],
            typing: vec![],
            notifications: ChannelNotifications::default(),
            id: "c".into(),
            name: "c".into(),
            kind: ChannelKind::Public,
            header: None,
            members: 3,
            pinned: 0,
            favorite: false,
            muted: false,
            posts: vec![],
            first_unread: None,
            can_manage: false,
            delete_others: false,
        };
        let window = cx.add_window(|window, cx| ChannelView::new(Some(model), "me", window, cx));
        let view = window.root(cx).unwrap();
        let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|cx| {
            let asked = asked.clone();
            cx.subscribe(&view, move |_, event: &ChannelEvent, _| {
                if let ChannelEvent::SuggestPeople(term) = event {
                    asked.borrow_mut().push(term.to_string());
                }
            })
            .detach();
        });
        let values = |view: &ChannelView, query: &str| {
            view.suggestions('@', query)
                .into_iter()
                .map(|s| s.value.to_string())
                .collect::<Vec<_>>()
        };
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.ask_people('@', "Gr", cx);
                view.ask_people('@', "gr", cx);
                view.ask_people(':', "gr", cx);
                assert_eq!(values(view, "gr"), ["@grace"]);
                view.set_suggested("gr".into(), vec![person("grace"), person("greg")], cx);
                assert_eq!(values(view, "gr"), ["@grace", "@greg"]);
                // A longer query narrows what the server said.
                assert_eq!(values(view, "gre"), ["@greg"]);
            })
        });
        assert_eq!(*asked.borrow(), ["gr"]);
    }

    /// #195: a message queued while offline is edited in place, unmarked,
    /// or discarded, before it sends.
    #[gpui_kit::test]
    fn a_queued_post_can_be_edited_or_discarded(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let queued = Post {
            mine: true,
            status: PostStatus::Pending,
            body: "Pushing the fix now.".into(),
            ..post("me:1", "Today")
        };
        let model = ChannelModel {
            purpose: None,
            offline: true,
            loading: false,
            max_post_size: DEFAULT_MAX_POST_SIZE,
            profiles: vec![],
            typing: vec![],
            notifications: ChannelNotifications::default(),
            id: "c".into(),
            name: "c".into(),
            kind: ChannelKind::Public,
            header: None,
            members: 1,
            pinned: 0,
            favorite: false,
            muted: false,
            posts: vec![post("1", "Today"), queued],
            first_unread: None,
            can_manage: false,
            delete_others: false,
        };
        let window = cx.add_window(|window, cx| ChannelView::new(Some(model), "me", window, cx));
        let view = window.root(cx).unwrap();
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|cx| {
            let events = events.clone();
            cx.subscribe(&view, move |_, event: &ChannelEvent, _| {
                if matches!(
                    event,
                    ChannelEvent::SaveEdit { .. }
                        | ChannelEvent::Discard(_)
                        | ChannelEvent::Delete(_)
                ) {
                    events.borrow_mut().push(event.clone());
                }
            })
            .detach();
        });
        let mut cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let body = |view: &ChannelView| {
            view.model
                .as_ref()
                .and_then(|model| model.posts.iter().find(|post| post.id == "me:1"))
                .map(|post| (post.body.to_string(), post.edited))
        };

        // Online, it is on its way and cannot be edited.
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.set_away(false, cx);
                view.start_edit("me:1".into(), window, cx);
                assert!(view.editing.is_none());
            })
        });

        // Offline, it waits: the edit changes the text, not "(edited)".
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.set_away(true, cx);
                view.start_edit("me:1".into(), window, cx);
                assert!(view.editing.is_some());
                view.save_edit("Pushing the fix tonight.".into(), window, cx);
                assert_eq!(
                    body(view),
                    Some(("Pushing the fix tonight.".to_owned(), false))
                );
            })
        });

        // Discard asks first, then the row goes and sync drops the send.
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.ask_delete("me:1".into(), window, cx);
                assert_eq!(view.deleting.as_deref(), Some("me:1"));
                let id = view.deleting.take().unwrap();
                view.delete_post(id, window, cx);
                assert_eq!(body(view), None);
            })
        });
        assert_eq!(
            *events.borrow(),
            [
                ChannelEvent::SaveEdit {
                    post: "me:1".into(),
                    text: "Pushing the fix tonight.".into(),
                },
                ChannelEvent::Discard("me:1".into()),
            ]
        );
    }

    /// #163: offline, reacting, saving, pinning, editing or deleting a sent
    /// message is refused before the transcript changes, and the window is
    /// asked to say so; back online they go through again.
    #[gpui_kit::test]
    fn offline_server_actions_are_refused_before_anything_changes(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let mine = Post {
            mine: true,
            body: "Shipped.".into(),
            ..post("p1", "Today")
        };
        let model = ChannelModel {
            purpose: None,
            offline: false,
            loading: false,
            max_post_size: DEFAULT_MAX_POST_SIZE,
            profiles: vec![],
            typing: vec![],
            notifications: ChannelNotifications::default(),
            id: "c".into(),
            name: "c".into(),
            kind: ChannelKind::Public,
            header: None,
            members: 1,
            pinned: 0,
            favorite: false,
            muted: false,
            posts: vec![mine],
            first_unread: None,
            can_manage: false,
            delete_others: false,
        };
        let window = cx.add_window(|window, cx| ChannelView::new(Some(model), "me", window, cx));
        let view = window.root(cx).unwrap();
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|cx| {
            let events = events.clone();
            cx.subscribe(&view, move |_, event: &ChannelEvent, _| {
                events.borrow_mut().push(event.clone());
            })
            .detach();
        });
        let mut cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let shown = |view: &ChannelView| view.shown_post("p1").cloned().unwrap();

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.set_away(true, cx);
                view.set_offline(true);
                view.toggle_reaction("p1".into(), "👍".into(), cx);
                view.toggle_save("p1".into(), cx);
                view.toggle_pin("p1".into(), cx);
                view.ask_delete("p1".into(), window, cx);
                assert_eq!(view.deleting, None, "no confirmation for a refused delete");
                view.start_edit("p1".into(), window, cx);
                view.save_edit("Shipped today.".into(), window, cx);
                assert!(view.editing.is_some(), "the edit stays open to try again");
                let post = shown(view);
                assert!(post.reactions.is_empty() && !post.saved && !post.pinned);
                assert_eq!((post.body.as_ref(), post.edited), ("Shipped.", false));
            })
        });
        assert_eq!(
            *events.borrow(),
            [
                ChannelEvent::Offline("add reaction"),
                ChannelEvent::Offline("save message"),
                ChannelEvent::Offline("pin message"),
                ChannelEvent::Offline("delete"),
                ChannelEvent::Offline("edit"),
            ]
        );

        events.borrow_mut().clear();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.set_offline(false);
                view.toggle_reaction("p1".into(), "👍".into(), cx);
            })
        });
        assert_eq!(
            *events.borrow(),
            [ChannelEvent::ToggleReaction {
                post: "p1".into(),
                emoji: "👍".into(),
            }]
        );
    }

    #[test]
    fn rows_interleave_day_and_new_message_dividers() {
        let channel = ChannelModel {
            offline: false,
            purpose: None,
            loading: false,
            max_post_size: DEFAULT_MAX_POST_SIZE,
            can_manage: false,
            delete_others: false,
            profiles: vec![],
            typing: vec![],
            notifications: ChannelNotifications::default(),
            id: "c".into(),
            name: "c".into(),
            kind: ChannelKind::Public,
            header: None,
            members: 1,
            pinned: 0,
            favorite: false,
            muted: false,
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
                Row::History,
                Row::Day("Yesterday".into()),
                Row::Post(0),
                Row::Post(1),
                Row::Day("Today".into()),
                Row::NewMessages,
                Row::Post(2),
                Row::Newer,
            ]
        );
    }

    #[test]
    fn removing_a_post_changes_only_its_rows() {
        let mut channel = ChannelModel {
            purpose: None,
            offline: false,
            loading: false,
            max_post_size: DEFAULT_MAX_POST_SIZE,
            can_manage: false,
            delete_others: false,
            profiles: vec![],
            typing: vec![],
            notifications: ChannelNotifications::default(),
            id: "c".into(),
            name: "c".into(),
            kind: ChannelKind::Public,
            header: None,
            members: 1,
            pinned: 0,
            favorite: false,
            muted: false,
            posts: vec![
                post("1", "Yesterday"),
                post("2", "Today"),
                post("3", "Today"),
                post("4", "Today"),
            ],
            first_unread: Some(2),
        };
        let before = channel.row_keys(&channel.rows());
        // The post above the New messages line goes: one row, and the
        // marker stays on post 3.
        channel.posts.remove(1);
        channel.first_unread = Some(1);
        let after = channel.row_keys(&channel.rows());
        assert_eq!(changed_rows(&before, &after), (4..5, 0));
        // The only post of a day takes its divider with it.
        let before = after;
        channel.posts.remove(0);
        channel.first_unread = Some(0);
        let after = channel.row_keys(&channel.rows());
        assert_eq!(changed_rows(&before, &after), (1..3, 0));
    }

    #[test]
    fn system_posts_get_their_own_rows_and_no_replies() {
        let system = |id: &str| Post {
            kind: PostKind::System { activity: true },
            author: SharedString::default(),
            ..post(id, "Today")
        };
        let channel = ChannelModel {
            purpose: None,
            offline: false,
            loading: false,
            max_post_size: DEFAULT_MAX_POST_SIZE,
            profiles: vec![],
            typing: vec![],
            notifications: ChannelNotifications::default(),
            id: "c".into(),
            name: "c".into(),
            kind: ChannelKind::Public,
            header: None,
            members: 1,
            pinned: 0,
            favorite: false,
            muted: false,
            posts: vec![post("1", "Today"), system("2")],
            first_unread: None,
            can_manage: false,
            delete_others: false,
        };
        assert_eq!(
            channel.rows(),
            vec![
                Row::History,
                Row::Day("Today".into()),
                Row::Post(0),
                Row::System(1),
                Row::Newer,
            ]
        );
        // Shift+Up replies to the newest message, not the join under it.
        assert_eq!(
            channel.last_message().map(|post| post.id.as_ref()),
            Some("1")
        );
    }

    #[test]
    fn formatting_shortcuts_follow_mattermost_and_slack() {
        fn wrap(keys: &'static str) -> Option<(&'static str, &'static str)> {
            format_shortcut(&gpui_kit::Keystroke::parse(keys).unwrap())
        }
        assert_eq!(wrap("cmd-b"), Some(("**", "**")));
        assert_eq!(wrap("ctrl-i"), Some(("_", "_")));
        assert_eq!(wrap("ctrl-alt-k"), Some(("[", "](url)")));
        assert_eq!(wrap("cmd-shift-x"), Some(("~~", "~~")));
        assert_eq!(wrap("ctrl-shift-c"), Some(("`", "`")));
        // Cmd+K is the quick switcher, Shift+B is just a capital letter.
        assert_eq!(wrap("cmd-k"), None);
        assert_eq!(wrap("shift-b"), None);
        assert_eq!(wrap("b"), None);
    }

    #[test]
    fn prepending_history_keeps_the_unread_marker_and_skips_known_posts() {
        let mut channel = ChannelModel {
            offline: false,
            purpose: None,
            loading: false,
            max_post_size: DEFAULT_MAX_POST_SIZE,
            can_manage: false,
            delete_others: false,
            profiles: vec![],
            typing: vec![],
            notifications: ChannelNotifications::default(),
            id: "c".into(),
            name: "c".into(),
            kind: ChannelKind::Public,
            header: None,
            members: 1,
            pinned: 0,
            favorite: false,
            muted: false,
            posts: vec![post("3", "Today"), post("4", "Today")],
            first_unread: Some(1),
        };
        let added = channel.prepend_posts(vec![
            post("1", "Yesterday"),
            post("2", "Yesterday"),
            post("3", "Today"),
        ]);
        assert_eq!(added, 2);
        let ids: Vec<&str> = channel.posts.iter().map(|p| p.id.as_ref()).collect();
        assert_eq!(ids, ["1", "2", "3", "4"]);
        assert_eq!(channel.first_unread, Some(3));
        assert_eq!(channel.rows()[1], Row::Day("Yesterday".into()));
        assert_eq!(channel.prepend_posts(vec![post("1", "Yesterday")]), 0);
    }

    #[test]
    fn typing_text_names_one_two_or_several() {
        let names = |list: &[&str]| -> Vec<SharedString> {
            list.iter()
                .map(|name| SharedString::from(name.to_string()))
                .collect()
        };
        assert_eq!(typing_text(&[]), "");
        assert_eq!(typing_text(&names(&["Ana Lopez"])), "Ana is typing\u{2026}");
        assert_eq!(
            typing_text(&names(&["Ana Lopez", "Ben Ode"])),
            "Ana and Ben are typing\u{2026}"
        );
        assert_eq!(
            typing_text(&names(&["Ana", "Ben", "Cy"])),
            "Several people are typing\u{2026}"
        );
    }

    #[gpui_kit::test]
    fn enter_follows_the_send_key_as_the_text_changes(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::Focusable as _;
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let (composer, cx) =
            cx.add_window_view(|window, cx| super::ComposerState::new(4, window, cx));
        let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        cx.update(|window, cx| {
            let sent = sent.clone();
            cx.subscribe(&composer, move |_, event: &super::ComposerEvent, _| {
                if let super::ComposerEvent::Submit(text) = event {
                    sent.borrow_mut().push(text.to_string());
                }
            })
            .detach();
            let input = composer.read(cx).input().clone();
            let focus = input.read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        let mut type_then_enter = |text: &str, send_on: super::SendKey| {
            cx.update(|window, cx| {
                composer.update(cx, |composer, cx| composer.set_value(text, window, cx));
                super::follow_send_key(&composer, send_on, cx);
                window.draw(cx).clear(cx);
            });
            cx.simulate_keystrokes("enter");
            sent.borrow_mut().pop()
        };
        assert_eq!(
            type_then_enter("hi", super::SendKey::Enter).as_deref(),
            Some("hi")
        );
        assert_eq!(type_then_enter("hi", super::SendKey::ModEnter), None);
        assert_eq!(
            type_then_enter("```rust", super::SendKey::CodeBlocks),
            None,
            "Enter is a new line in an open code block"
        );
        assert_eq!(
            type_then_enter("```\nfn x() {}\n```", super::SendKey::CodeBlocks).as_deref(),
            Some("```\nfn x() {}\n```")
        );
    }

    /// #114: what is typed stays with its channel: switching away saves it,
    /// switching back restores it with the caret at the end, and sending
    /// clears it.
    #[gpui_kit::test]
    fn a_channels_draft_survives_switching_and_sending_clears_it(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use crate::composer_draft::{DraftBook, DraftKey, SAVE_PAUSE};
        use gpui_kit::Focusable as _;
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let general = crate::preview::channel("general").unwrap();
        let design = crate::preview::channel("design").unwrap();
        let (view, cx) = cx.add_window_view(|window, cx| {
            ChannelView::new(Some(general.clone()), "me", window, cx)
        });
        let saves = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|window, cx| {
            let saves = saves.clone();
            cx.subscribe(&view, move |_, event: &ChannelEvent, _| {
                if let ChannelEvent::SaveDraft(save) = event {
                    saves
                        .borrow_mut()
                        .push((save.key.channel_id.to_string(), save.text.to_string()));
                }
            })
            .detach();
            let input = view.read(cx).composer.read(cx).input().clone();
            input.read(cx).focus_handle(cx).focus(window, cx);
        });
        let text = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|_, cx| view.read(cx).composer.read(cx).value(cx).to_string())
        };

        // Typing saves once it pauses.
        cx.simulate_input("half a thought");
        cx.run_until_parked();
        assert!(saves.borrow().is_empty(), "nothing before the pause");
        cx.executor().advance_clock(SAVE_PAUSE);
        cx.run_until_parked();
        assert_eq!(
            saves.borrow_mut().drain(..).collect::<Vec<_>>(),
            [("general".to_owned(), "half a thought".to_owned())]
        );

        // More typing, then leaving before the pause: saved on the way out.
        cx.simulate_input(" and more");
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.set_model(Some(design.clone()), window, cx)
            })
        });
        cx.run_until_parked();
        assert_eq!(text(cx), "");
        assert_eq!(
            saves.borrow_mut().drain(..).collect::<Vec<_>>(),
            [("general".to_owned(), "half a thought and more".to_owned())]
        );

        // Back again: the text returns, caret at the end, and is not saved
        // again or sent as typing.
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.set_model(Some(general.clone()), window, cx)
            })
        });
        cx.executor().advance_clock(SAVE_PAUSE);
        cx.run_until_parked();
        assert_eq!(text(cx), "half a thought and more");
        let caret = cx.update(|_, cx| view.read(cx).composer.read(cx).selection(cx));
        assert_eq!(caret, 23..23);
        assert!(saves.borrow().is_empty());
        assert!(cx.update(|_, cx| DraftBook::channels(cx).contains("general")));

        // Sending clears it here and in sync.
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let text = view.composer.read(cx).value(cx);
                view.send(text, window, cx);
            })
        });
        cx.executor().advance_clock(SAVE_PAUSE);
        cx.run_until_parked();
        assert_eq!(text(cx), "");
        assert_eq!(
            saves.borrow_mut().drain(..).collect::<Vec<_>>(),
            [("general".to_owned(), String::new())]
        );
        assert!(cx.update(|_, cx| DraftBook::get(cx, &DraftKey::channel("general")).is_none()));
    }

    /// #188: the draft's link is asked about once typing pauses, once per
    /// link; its card shows above the composer, Remove preview hides it,
    /// and the sent post goes without a preview.
    #[gpui_kit::test]
    fn a_drafts_link_is_previewed_once_and_removing_it_sends_without_one(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use crate::draft_link::PAUSE;
        use gpui_kit::Focusable as _;
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let general = crate::preview::channel("general").unwrap();
        let (view, cx) = cx.add_window_view(|window, cx| {
            ChannelView::new(Some(general.clone()), "me", window, cx)
        });
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|window, cx| {
            let events = events.clone();
            cx.subscribe(&view, move |_, event: &ChannelEvent, _| match event {
                ChannelEvent::PreviewLink(_) | ChannelEvent::Send { .. } => {
                    events.borrow_mut().push(event.clone());
                }
                _ => {}
            })
            .detach();
            let input = view.read(cx).composer.read(cx).input().clone();
            input.read(cx).focus_handle(cx).focus(window, cx);
        });
        let url = "https://github.com/devaryakjha/matty";
        let shown = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.link
                        .preview("composer.link-preview", cx, |_, _, _| {})
                        .is_some()
                })
            })
        };

        // Nothing is asked before typing pauses, then the link once.
        cx.simulate_input("see https://github.com/devaryakjha/matty");
        cx.run_until_parked();
        cx.executor().advance_clock(PAUSE / 2);
        cx.run_until_parked();
        assert!(events.borrow().is_empty(), "nothing before the pause");
        cx.executor().advance_clock(PAUSE);
        cx.run_until_parked();
        assert_eq!(
            events.borrow_mut().drain(..).collect::<Vec<_>>(),
            [ChannelEvent::PreviewLink(url.into())]
        );

        // More words around the same link ask nothing more.
        cx.simulate_input(" today");
        cx.executor().advance_clock(PAUSE * 2);
        cx.run_until_parked();
        assert!(events.borrow().is_empty(), "same link, no new ask");

        // The answer shows the card; Remove preview hides it.
        assert!(!shown(cx), "no card before the answer");
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.set_link_preview(
                    url,
                    Some(crate::embeds::LinkCard {
                        url: url.into(),
                        site_name: "GitHub".into(),
                        title: "devaryakjha/matty".into(),
                        description: SharedString::default(),
                        image: None,
                    }),
                    cx,
                )
            })
        });
        assert!(shown(cx));
        cx.update(|_, cx| view.update(cx, |view, _| view.link.remove()));
        assert!(!shown(cx));

        // Sent without its preview.
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let text = view.composer.read(cx).value(cx);
                view.send(text, window, cx);
            })
        });
        cx.run_until_parked();
        assert!(matches!(
            events.borrow().as_slice(),
            [ChannelEvent::Send { unfurl: false, .. }]
        ));
    }

    /// #114: a draft changed on another device replaces the composer only
    /// while the user has nothing unsaved in it.
    #[gpui_kit::test]
    fn a_draft_from_elsewhere_never_overwrites_unsaved_typing(cx: &mut gpui_kit::TestAppContext) {
        use crate::composer_draft::{DraftBook, DraftKey, Kept};
        use gpui_kit::Focusable as _;
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let general = crate::preview::channel("general").unwrap();
        let (view, cx) =
            cx.add_window_view(|window, cx| ChannelView::new(Some(general), "me", window, cx));
        let remote = |text: &str, cx: &mut gpui_kit::VisualTestContext| {
            let kept = Kept {
                text: text.to_owned().into(),
                ..Kept::default()
            };
            cx.update(|window, cx| {
                DraftBook::put(DraftKey::channel("general"), kept, cx);
                view.update(cx, |view, cx| view.refresh_draft(window, cx));
            });
            cx.run_until_parked();
            cx.update(|_, cx| view.read(cx).composer.read(cx).value(cx).to_string())
        };
        assert_eq!(remote("from the web app", cx), "from the web app");
        cx.update(|window, cx| {
            let input = view.read(cx).composer.read(cx).input().clone();
            input.read(cx).focus_handle(cx).focus(window, cx);
        });
        cx.simulate_input("!");
        cx.run_until_parked();
        assert_eq!(remote("edited again", cx), "from the web app!");
    }

    /// A channel view on the preview's #general, focus in its transcript.
    fn lightbox_view(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (Entity<ChannelView>, &mut gpui_kit::VisualTestContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let general = crate::preview::channel("general").unwrap();
        cx.add_window_view(|window, cx| ChannelView::new(Some(general), "me", window, cx))
    }

    /// Selects `post` in the transcript, with focus there, as Up does.
    fn select_post(view: &Entity<ChannelView>, post: &str, cx: &mut gpui_kit::VisualTestContext) {
        cx.update(|window, cx| {
            view.update(cx, |this, cx| {
                let posts = &this.model.as_ref().unwrap().posts;
                let row = this
                    .rows
                    .iter()
                    .position(|row| matches!(row, Row::Post(i) if posts[*i].id == post))
                    .unwrap();
                this.selection = Some(row);
                window.focus(&this.list_focus, cx);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
    }

    /// #152: the keyboard journey. Enter on a post with images opens the
    /// lightbox on its first, Left and Right move between the post's
    /// images, and Escape closes it with focus back on the same post.
    #[gpui_kit::test]
    fn the_lightbox_opens_moves_and_closes_back_on_the_post(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx) = lightbox_view(cx);
        let draw = |cx: &mut gpui_kit::VisualTestContext| {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        };
        let state = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| {
                let this = view.read(cx);
                let lightbox = this.lightbox.read(cx);
                (
                    this.lightbox_handle.is_open(),
                    lightbox.index(),
                    lightbox.current().map(|image| image.name.to_string()),
                    this.list_focus.is_focused(window),
                )
            })
        };
        let viewed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|_, cx| {
            let viewed = viewed.clone();
            cx.subscribe(&view, move |_, event: &ChannelEvent, _| {
                if let ChannelEvent::ViewImages(ids) = event {
                    viewed.borrow_mut().push(ids.clone());
                }
            })
            .detach();
        });

        // A post without images leaves Enter alone.
        select_post(&view, "p8", cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(!state(cx).0);

        // Two images and a PDF: the lightbox holds the two images.
        select_post(&view, "p8b", cx);
        let selected = cx.update(|_, cx| view.read(cx).selection);
        cx.simulate_keystrokes("enter");
        draw(cx);
        let name = |name: &str| Some(name.to_owned());
        assert_eq!(state(cx), (true, 0, name("sidebar-mockup.png"), false));
        assert_eq!(
            viewed.borrow().as_slice(),
            [vec![
                SharedString::from("f-mockup"),
                SharedString::from("f-mockup-dark")
            ]],
            "sync is asked for the full-size images"
        );

        cx.simulate_keystrokes("right");
        draw(cx);
        assert_eq!(state(cx), (true, 1, name("sidebar-mockup-dark.png"), false));
        cx.simulate_keystrokes("right");
        draw(cx);
        assert_eq!(state(cx).1, 0, "Right wraps to the first image");
        cx.simulate_keystrokes("left");
        draw(cx);
        assert_eq!(state(cx).1, 1, "Left wraps to the last image");

        cx.simulate_keystrokes("escape");
        draw(cx);
        assert_eq!(state(cx), (false, 1, name("sidebar-mockup-dark.png"), true));
        assert_eq!(
            cx.update(|_, cx| view.read(cx).selection),
            selected,
            "the post it opened from is still selected"
        );
    }

    /// #152: an image the message links to opens in the lightbox, with
    /// Copy link and Open in browser in place of Download, and closing it
    /// returns focus to where it was.
    #[gpui_kit::test]
    fn a_linked_image_opens_in_the_lightbox(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx) = lightbox_view(cx);
        let url = SharedString::from("https://matty.example.com/shots/sidebar-unread.png");
        select_post(&view, "p4b", cx);
        cx.update(|window, cx| {
            view.update(cx, |this, cx| {
                this.open_linked_image("p4b".into(), url.clone(), window, cx)
            });
            window.draw(cx).clear(cx);
        });
        let (open, items, name) = cx.update(|_, cx| {
            let this = view.read(cx);
            (
                this.lightbox_handle.is_open(),
                this.lightbox_items.clone(),
                this.lightbox
                    .read(cx)
                    .current()
                    .map(|image| image.name.clone()),
            )
        });
        assert!(open);
        assert_eq!(items, [LightboxItem::Link(url.clone())]);
        assert_eq!(name.as_deref(), Some("sidebar-unread.png"));
        assert!(cx.debug_bounds("lightbox-browser").is_some());
        assert!(cx.debug_bounds("lightbox-download").is_none());
        let copy = cx.debug_bounds("lightbox-copy-link").unwrap();
        cx.simulate_click(copy.center(), Default::default());
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(url.to_string())
        );

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let (open, focused) = cx.update(|window, cx| {
            let this = view.read(cx);
            (
                this.lightbox_handle.is_open(),
                this.list_focus.is_focused(window),
            )
        });
        assert!(!open);
        assert!(focused, "focus is back in the transcript");
    }
}
