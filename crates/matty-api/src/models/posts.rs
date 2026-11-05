//! Posts, threads and their metadata (PLAN §6.3 "Posts").

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Emoji, User, null_as_default};

/// A post (a message or a thread reply).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Post {
    /// Post id.
    pub id: String,
    /// Milliseconds since the epoch.
    pub create_at: i64,
    /// Milliseconds since the epoch; changes on any change, including reactions.
    pub update_at: i64,
    /// Milliseconds since the epoch of the last edit to the message; 0 if never edited.
    pub edit_at: i64,
    /// Milliseconds since the epoch; nonzero when deleted.
    pub delete_at: i64,
    /// Whether the post is pinned to the channel.
    pub is_pinned: bool,
    /// Author.
    pub user_id: String,
    /// Channel.
    pub channel_id: String,
    /// Thread root; empty for a root post.
    pub root_id: String,
    /// Message text (Markdown).
    pub message: String,
    /// Empty for a normal post, or a system type such as `system_join_channel`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Free-form properties: `attachments`, `from_webhook`, `override_username`, ...
    #[serde(deserialize_with = "null_as_default")]
    pub props: HashMap<String, Value>,
    /// Space-separated hashtags found in the message.
    pub hashtags: String,
    /// Attached file ids.
    #[serde(deserialize_with = "null_as_default")]
    pub file_ids: Vec<String>,
    /// The client's id for a post it created, echoed back so the client can
    /// match the server's copy to its pending one (PLAN §6.5).
    pub pending_post_id: String,
    /// Replies in the thread, on a root post.
    pub reply_count: i64,
    /// Milliseconds since the epoch of the newest reply, on a root post.
    pub last_reply_at: i64,
    /// Users who replied, newest first, on a root post with Collapsed Reply Threads.
    #[serde(deserialize_with = "null_as_default")]
    pub participants: Vec<User>,
    /// Whether the user follows the thread, when the server says.
    pub is_following: Option<bool>,
    /// Data the server attaches for rendering.
    pub metadata: PostMetadata,
}

impl Post {
    /// Whether the post is deleted.
    pub fn is_deleted(&self) -> bool {
        self.delete_at != 0
    }

    /// Whether the post is a thread reply.
    pub fn is_reply(&self) -> bool {
        !self.root_id.is_empty()
    }

    /// Message attachments from `props.attachments` (integrations and bots).
    /// Attachments that do not decode are skipped.
    pub fn attachments(&self) -> Vec<MessageAttachment> {
        let Some(Value::Array(items)) = self.props.get("attachments") else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|item| serde_json::from_value(item.clone()).ok())
            .collect()
    }
}

/// What the server attaches to a post for rendering.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct PostMetadata {
    /// Previews: link, image, Open Graph and message attachment embeds.
    #[serde(deserialize_with = "null_as_default")]
    pub embeds: Vec<PostEmbed>,
    /// Custom emoji used in the message and reactions.
    #[serde(deserialize_with = "null_as_default")]
    pub emojis: Vec<Emoji>,
    /// Attached files.
    #[serde(deserialize_with = "null_as_default")]
    pub files: Vec<FileInfo>,
    /// Sizes of the images the message and its embeds show, by URL.
    #[serde(deserialize_with = "null_as_default")]
    pub images: HashMap<String, PostImage>,
    /// Reactions.
    #[serde(deserialize_with = "null_as_default")]
    pub reactions: Vec<Reaction>,
    /// Message priority, when set (licensed servers).
    pub priority: Option<PostPriority>,
    /// Who acknowledged the post, when it asked for acknowledgements.
    #[serde(deserialize_with = "null_as_default")]
    pub acknowledgements: Vec<PostAcknowledgement>,
}

/// A preview the server made for a post.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct PostEmbed {
    /// What kind of preview this is.
    #[serde(rename = "type")]
    pub kind: EmbedKind,
    /// The URL previewed; empty for message attachments.
    pub url: String,
    /// Open Graph data for `opengraph`, the post for `permalink`, else absent.
    pub data: Option<Value>,
}

impl PostEmbed {
    /// The Open Graph data of an `opengraph` embed.
    pub fn opengraph(&self) -> Option<OpenGraph> {
        match (&self.kind, &self.data) {
            (EmbedKind::OpenGraph, Some(data)) => serde_json::from_value(data.clone()).ok(),
            _ => None,
        }
    }
}

/// The kind of a [`PostEmbed`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbedKind {
    /// A direct link to an image.
    Image,
    /// A link to a page without Open Graph data.
    Link,
    /// A link to a page with Open Graph data.
    #[serde(rename = "opengraph")]
    OpenGraph,
    /// The post has `props.attachments`; see [`Post::attachments`].
    MessageAttachment,
    /// A link to another post on the same server.
    Permalink,
    /// A kind this version of Matty does not know.
    #[default]
    #[serde(other)]
    Unknown,
}

/// Open Graph data from a page's `og:` tags.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct OpenGraph {
    /// `og:type`.
    #[serde(rename = "type")]
    pub kind: String,
    /// `og:url`.
    pub url: String,
    /// `og:title`.
    pub title: String,
    /// `og:description`.
    pub description: String,
    /// `og:site_name`.
    pub site_name: String,
    /// `og:image` entries.
    #[serde(deserialize_with = "null_as_default")]
    pub images: Vec<OpenGraphImage>,
}

/// One `og:image`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct OpenGraphImage {
    /// Image URL.
    pub url: String,
    /// HTTPS URL, when the page gives one.
    pub secure_url: String,
    /// Width the page declares; 0 if none. [`PostMetadata::images`] has the real size.
    pub width: i64,
    /// Height the page declares; 0 if none.
    pub height: i64,
}

/// The size of an image a post shows, measured by the server.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct PostImage {
    /// Pixels.
    pub width: i64,
    /// Pixels.
    pub height: i64,
    /// `png`, `jpeg`, `gif`, ...
    pub format: String,
    /// Frames, for an animated GIF; 0 otherwise.
    pub frame_count: i64,
}

/// An uploaded file.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct FileInfo {
    /// File id; the file is `files/{id}`, its preview `files/{id}/preview`.
    pub id: String,
    /// Uploader.
    pub user_id: String,
    /// Post the file is attached to.
    pub post_id: String,
    /// Channel.
    pub channel_id: String,
    /// Milliseconds since the epoch.
    pub create_at: i64,
    /// Milliseconds since the epoch; nonzero when deleted.
    pub delete_at: i64,
    /// File name.
    pub name: String,
    /// Extension, without the dot.
    pub extension: String,
    /// Bytes.
    pub size: i64,
    /// MIME type.
    pub mime_type: String,
    /// Pixels, for an image; 0 otherwise.
    pub width: i64,
    /// Pixels, for an image; 0 otherwise.
    pub height: i64,
    /// Whether `files/{id}/preview` exists.
    pub has_preview_image: bool,
    /// A tiny base64 JPEG to show while the preview loads.
    pub mini_preview: Option<String>,
}

/// A page of files, as `POST teams/{id}/files/search` answers.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct FileInfoList {
    /// File ids in the order the server ranked them.
    #[serde(deserialize_with = "null_as_default")]
    pub order: Vec<String>,
    /// The files by id.
    #[serde(deserialize_with = "null_as_default")]
    pub file_infos: HashMap<String, FileInfo>,
}

impl FileInfoList {
    /// The files in the server's order.
    pub fn ordered(&self) -> impl Iterator<Item = &FileInfo> {
        self.order.iter().filter_map(|id| self.file_infos.get(id))
    }
}

/// One user's reaction to a post.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Reaction {
    /// Who reacted.
    pub user_id: String,
    /// Post.
    pub post_id: String,
    /// Emoji name, without colons; a system or custom emoji.
    pub emoji_name: String,
    /// Milliseconds since the epoch.
    pub create_at: i64,
}

/// Message priority (licensed servers).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(default)]
pub struct PostPriority {
    /// `""`, `important` or `urgent`.
    #[serde(deserialize_with = "null_as_default")]
    pub priority: String,
    /// Whether the author asked for acknowledgements. The server sends
    /// `null` for a post that only has a label.
    #[serde(deserialize_with = "null_as_default")]
    pub requested_ack: bool,
    /// Whether mentioned users get repeated notifications; `null` like
    /// [`Self::requested_ack`].
    #[serde(deserialize_with = "null_as_default")]
    pub persistent_notifications: bool,
}

/// A message attachment, from `props.attachments`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct MessageAttachment {
    /// Plain-text summary.
    pub fallback: String,
    /// Left border color, such as `#ff0000` or `good`.
    pub color: String,
    /// Text above the attachment.
    pub pretext: String,
    /// Author name.
    pub author_name: String,
    /// Author link.
    pub author_link: String,
    /// Author icon URL.
    pub author_icon: String,
    /// Title.
    pub title: String,
    /// Title link.
    pub title_link: String,
    /// Body (Markdown).
    pub text: String,
    /// Fields.
    #[serde(deserialize_with = "null_as_default")]
    pub fields: Vec<AttachmentField>,
    /// Image URL.
    pub image_url: String,
    /// Thumbnail URL.
    pub thumb_url: String,
    /// Footer.
    pub footer: String,
    /// Footer icon URL.
    pub footer_icon: String,
    /// Buttons and menus an integration offers (interactive messages).
    #[serde(deserialize_with = "null_as_default")]
    pub actions: Vec<PostAction>,
}

/// A button or menu on a [`MessageAttachment`]. Choosing it calls
/// `posts/{post_id}/actions/{id}`; the server passes it on to the
/// integration, whose URL it keeps to itself.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct PostAction {
    /// Action id, unique within the post.
    pub id: String,
    /// `button` or `select`; a button when empty.
    #[serde(rename = "type")]
    pub kind: String,
    /// The button's label, or the menu's placeholder.
    pub name: String,
    /// Whether the integration has turned it off.
    pub disabled: bool,
    /// A button's look: `default`, `primary`, `success`, `good`,
    /// `warning`, `danger`, or a hex color.
    pub style: String,
    /// A menu's options: empty for [`options`](Self::options), `users`
    /// or `channels` for the server's own lists.
    pub data_source: String,
    /// A static menu's options.
    #[serde(deserialize_with = "null_as_default")]
    pub options: Vec<PostActionOption>,
    /// The value a menu starts on.
    pub default_option: String,
    /// The server's sealed copy of the action, sent back with the choice so
    /// it can run an action on a post it does not store (an ephemeral one).
    pub cookie: String,
}

impl PostAction {
    /// Whether this is a menu rather than a button.
    pub fn is_select(&self) -> bool {
        self.kind == "select"
    }
}

/// One option of a menu [`PostAction`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct PostActionOption {
    /// What the menu shows.
    pub text: String,
    /// What is sent when it is chosen.
    pub value: String,
}

/// What `posts/{post_id}/actions/{action_id}` answers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct PostActionResponse {
    /// `OK`.
    pub status: String,
    /// Set when the integration wants to open an interactive dialog.
    pub trigger_id: String,
}

/// One field of a [`MessageAttachment`].
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct AttachmentField {
    /// Title.
    pub title: String,
    /// Value (Markdown); the server allows a string or a number.
    pub value: Value,
    /// Whether the field shares a row with the next short field.
    pub short: bool,
}

/// Posts in display order, from the post list endpoints.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct PostList {
    /// Post ids, newest first for channel lists, oldest first for threads.
    #[serde(deserialize_with = "null_as_default")]
    pub order: Vec<String>,
    /// Posts by id. With `since`, this includes deleted posts and the hidden
    /// copies an edit leaves behind, both with `delete_at` set.
    #[serde(deserialize_with = "null_as_default")]
    pub posts: HashMap<String, Post>,
    /// The id of the next newer post outside this page, if any.
    pub next_post_id: String,
    /// The id of the next older post outside this page, if any.
    pub prev_post_id: String,
    /// Whether more posts follow this page (threads).
    pub has_next: bool,
}

impl PostList {
    /// The posts in `order`.
    pub fn ordered(&self) -> impl Iterator<Item = &Post> {
        self.order.iter().filter_map(|id| self.posts.get(id))
    }

    /// Every post in the response as a change to apply: deleted posts as
    /// [`PostChange::Deleted`], the rest as [`PostChange::Upsert`]. This is
    /// how `since` responses report deletes. Oldest change first.
    pub fn changes(&self) -> Vec<PostChange<'_>> {
        let mut posts: Vec<&Post> = self.posts.values().collect();
        posts.sort_by_key(|p| (p.update_at, p.create_at));
        posts
            .into_iter()
            .map(|p| {
                if p.is_deleted() {
                    PostChange::Deleted {
                        id: &p.id,
                        channel_id: &p.channel_id,
                        root_id: &p.root_id,
                    }
                } else {
                    PostChange::Upsert(p)
                }
            })
            .collect()
    }
}

/// `teams/{id}/posts/search`: matching posts, newest first, and the words
/// that matched in each.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct SearchResults {
    /// The posts, in [`PostList`] form.
    #[serde(flatten)]
    pub list: PostList,
    /// The terms that matched, by post id, for highlighting.
    #[serde(deserialize_with = "null_as_default")]
    pub matches: HashMap<String, Vec<String>>,
}

/// One entry of [`PostList::changes`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PostChange<'a> {
    /// A new or changed post.
    Upsert(&'a Post),
    /// A deleted post.
    Deleted {
        /// Post id.
        id: &'a str,
        /// Channel.
        channel_id: &'a str,
        /// Thread root; empty for a root post.
        root_id: &'a str,
    },
}

/// A thread the user follows (Collapsed Reply Threads).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct UserThread {
    /// Thread id, which is the root post's id.
    pub id: String,
    /// Replies.
    pub reply_count: i64,
    /// Milliseconds since the epoch of the newest reply.
    pub last_reply_at: i64,
    /// Milliseconds since the epoch when the user last read the thread.
    pub last_viewed_at: i64,
    /// Users who replied.
    #[serde(deserialize_with = "null_as_default")]
    pub participants: Vec<User>,
    /// The root post.
    pub post: Post,
    /// Replies the user has not read.
    pub unread_replies: i64,
    /// Mentions the user has not read.
    pub unread_mentions: i64,
    /// Whether an unread mention is urgent.
    pub is_urgent: bool,
    /// Milliseconds since the epoch; nonzero when the root post was deleted.
    pub delete_at: i64,
}

/// A page of the user's threads, with totals.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct UserThreads {
    /// Threads, most recently replied first.
    #[serde(deserialize_with = "null_as_default")]
    pub threads: Vec<UserThread>,
    /// Followed threads in the team.
    pub total: i64,
    /// Followed threads with unread replies.
    pub total_unread_threads: i64,
    /// Unread mentions across followed threads.
    pub total_unread_mentions: i64,
    /// Unread urgent mentions across followed threads.
    pub total_unread_urgent_mentions: i64,
}

/// What `channels/members/me/view` answers.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ChannelViewed {
    /// `OK`.
    pub status: String,
    /// The new `last_viewed_at` of each channel marked viewed, by channel id.
    #[serde(deserialize_with = "null_as_default")]
    pub last_viewed_at_times: HashMap<String, i64>,
}

/// A channel's unread state after marking a post unread.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ChannelUnreadAt {
    /// Team; empty for direct and group messages.
    pub team_id: String,
    /// Channel.
    pub channel_id: String,
    /// User.
    pub user_id: String,
    /// Messages the user has now seen.
    pub msg_count: i64,
    /// Root posts the user has now seen.
    pub msg_count_root: i64,
    /// Mentions now unread.
    pub mention_count: i64,
    /// Root-post mentions now unread.
    pub mention_count_root: i64,
    /// Urgent mentions now unread.
    pub urgent_mention_count: i64,
    /// The new `last_viewed_at`: just before the post marked unread.
    pub last_viewed_at: i64,
}

/// A message draft, synced across the user's devices.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Draft {
    /// Milliseconds since the epoch.
    pub create_at: i64,
    /// Milliseconds since the epoch.
    pub update_at: i64,
    /// Milliseconds since the epoch; nonzero when deleted.
    pub delete_at: i64,
    /// Author.
    pub user_id: String,
    /// Channel.
    pub channel_id: String,
    /// Thread root, for a reply draft.
    pub root_id: String,
    /// Draft text.
    pub message: String,
    /// Properties.
    #[serde(deserialize_with = "null_as_default")]
    pub props: HashMap<String, Value>,
    /// Attached file ids.
    #[serde(deserialize_with = "null_as_default")]
    pub file_ids: Vec<String>,
}

/// A user's acknowledgement of a post that asked for one (licensed).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct PostAcknowledgement {
    /// Who acknowledged.
    pub user_id: String,
    /// Post.
    pub post_id: String,
    /// Milliseconds since the epoch; 0 once removed.
    pub acknowledged_at: i64,
}

impl PostPriority {
    /// `important`.
    pub const IMPORTANT: &'static str = "important";
    /// `urgent`.
    pub const URGENT: &'static str = "urgent";

    /// Whether the post is urgent.
    pub fn is_urgent(&self) -> bool {
        self.priority == Self::URGENT
    }

    /// Whether anything is set: a label, an acknowledgement request or
    /// persistent notifications. An empty priority is not sent.
    pub fn is_set(&self) -> bool {
        !self.priority.is_empty() || self.requested_ack || self.persistent_notifications
    }
}

/// A message set to post later (`posts/schedule`, Mattermost 10.3+ with a
/// license).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ScheduledPost {
    /// Its id; empty for one not created yet.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// Milliseconds since the epoch.
    pub create_at: i64,
    /// Milliseconds since the epoch.
    pub update_at: i64,
    /// Author.
    pub user_id: String,
    /// Channel.
    pub channel_id: String,
    /// Thread root, for a reply.
    pub root_id: String,
    /// Message text.
    pub message: String,
    /// Properties.
    #[serde(deserialize_with = "null_as_default")]
    pub props: HashMap<String, Value>,
    /// Attached file ids.
    #[serde(deserialize_with = "null_as_default")]
    pub file_ids: Vec<String>,
    /// Priority, under `metadata`, as on a post.
    #[serde(deserialize_with = "null_as_default")]
    pub metadata: ScheduledMetadata,
    /// When it posts, in milliseconds since the epoch.
    pub scheduled_at: i64,
    /// When the server tried to post it; 0 until then.
    pub processed_at: i64,
    /// Why the server could not post it, such as `channel_archived`;
    /// empty when it has not failed.
    #[serde(deserialize_with = "null_as_default")]
    pub error_code: String,
}

/// What a scheduled post carries under `metadata`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ScheduledMetadata {
    /// Its priority, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<PostPriority>,
}
