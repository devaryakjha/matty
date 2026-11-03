//! Mattermost data models. Fields Matty does not read yet are ignored when
//! decoding, so new server fields never break Matty.

use serde::{Deserialize, Serialize};

/// A user, as `users/me` and `users/ids` return it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct User {
    /// User id.
    pub id: String,
    /// Username, without `@`.
    pub username: String,
    /// Email address, when the server shows it.
    pub email: String,
    /// Given name.
    pub first_name: String,
    /// Family name.
    pub last_name: String,
    /// Nickname.
    pub nickname: String,
    /// Space-separated role names.
    pub roles: String,
    /// The sign-in service (`""` for email, `ldap`, `gitlab`, `saml`, ...).
    pub auth_service: String,
    /// Milliseconds since the epoch; changes when the avatar changes.
    pub last_picture_update: i64,
    /// Milliseconds since the epoch; nonzero when deactivated.
    pub delete_at: i64,
    /// Whether the user is a bot.
    pub is_bot: bool,
    /// The user's locale, such as `en`.
    pub locale: String,
}

/// A session, as `users/{id}/sessions` returns it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Session {
    /// Session id (not the token).
    pub id: String,
    /// The device id the session was created with.
    pub device_id: String,
    /// Milliseconds since the epoch.
    pub create_at: i64,
    /// Milliseconds since the epoch; 0 for no expiry.
    pub expires_at: i64,
    /// Milliseconds since the epoch.
    pub last_activity_at: i64,
}

/// A team the user belongs to.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Team {
    /// Team id.
    pub id: String,
    /// URL name, as in `/{name}/channels/...`.
    pub name: String,
    /// Display name.
    pub display_name: String,
    /// `O` (open) or `I` (invite only).
    #[serde(rename = "type")]
    pub kind: String,
    /// Milliseconds since the epoch.
    pub last_team_icon_update: i64,
    /// Milliseconds since the epoch; nonzero when archived.
    pub delete_at: i64,
}

/// The user's membership in a team.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct TeamMember {
    /// Team id.
    pub team_id: String,
    /// User id.
    pub user_id: String,
    /// Space-separated role names.
    pub roles: String,
    /// Milliseconds since the epoch; nonzero when the user left.
    pub delete_at: i64,
}

/// Unread counts for a team, from `users/{uid}/teams/unread`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct TeamUnread {
    /// Team id.
    pub team_id: String,
    /// Unread messages, counting replies.
    pub msg_count: i64,
    /// Mentions, counting replies.
    pub mention_count: i64,
    /// Unread root posts (Collapsed Reply Threads).
    pub msg_count_root: i64,
    /// Mentions in root posts (Collapsed Reply Threads).
    pub mention_count_root: i64,
    /// Followed threads with unread replies.
    pub thread_count: i64,
    /// Mentions in followed threads.
    pub thread_mention_count: i64,
}

/// A channel.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Channel {
    /// Channel id.
    pub id: String,
    /// Owning team; empty for direct and group messages.
    pub team_id: String,
    /// `O` public, `P` private, `D` direct, `G` group.
    #[serde(rename = "type")]
    pub kind: String,
    /// Display name (empty for DMs, which are named after the other user).
    pub display_name: String,
    /// URL name; for a DM, `{user_a}__{user_b}`.
    pub name: String,
    /// Channel header (Markdown).
    pub header: String,
    /// Channel purpose.
    pub purpose: String,
    /// Milliseconds since the epoch of the newest post.
    pub last_post_at: i64,
    /// Milliseconds since the epoch of the newest root post.
    pub last_root_post_at: i64,
    /// Posts in the channel, counting replies.
    pub total_msg_count: i64,
    /// Root posts in the channel.
    pub total_msg_count_root: i64,
    /// Milliseconds since the epoch; nonzero when archived.
    pub delete_at: i64,
    /// Milliseconds since the epoch.
    pub update_at: i64,
}

/// The user's membership in a channel, with unread state.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ChannelMember {
    /// Channel id.
    pub channel_id: String,
    /// User id.
    pub user_id: String,
    /// Space-separated role names.
    pub roles: String,
    /// Milliseconds since the epoch when the user last viewed the channel.
    pub last_viewed_at: i64,
    /// Messages the user has seen, counting replies.
    pub msg_count: i64,
    /// Mentions, counting replies.
    pub mention_count: i64,
    /// Root posts the user has seen (Collapsed Reply Threads).
    pub msg_count_root: i64,
    /// Mentions in root posts (Collapsed Reply Threads).
    pub mention_count_root: i64,
    /// Urgent mentions (message priority).
    pub urgent_mention_count: i64,
    /// Per-channel notification settings (`desktop`, `mark_unread`, ...).
    pub notify_props: std::collections::HashMap<String, String>,
    /// Milliseconds since the epoch.
    pub last_update_at: i64,
}

impl ChannelMember {
    /// Whether the user muted the channel.
    pub fn is_muted(&self) -> bool {
        self.notify_props.get("mark_unread").map(String::as_str) == Some("mention")
    }
}

/// One sidebar category (Favorites, Channels, Direct Messages or custom).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct SidebarCategory {
    /// Category id.
    pub id: String,
    /// User id.
    pub user_id: String,
    /// Team id.
    pub team_id: String,
    /// `favorites`, `channels`, `direct_messages` or `custom`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Display name.
    pub display_name: String,
    /// `alpha`, `recent` or `manual`.
    pub sorting: String,
    /// Whether the category is collapsed.
    pub collapsed: bool,
    /// Whether the category is muted.
    pub muted: bool,
    /// Channel ids in display order.
    pub channel_ids: Vec<String>,
}

/// The user's categories for a team and their order.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct SidebarCategories {
    /// Categories.
    pub categories: Vec<SidebarCategory>,
    /// Category ids in display order.
    pub order: Vec<String>,
}

/// One preference row.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Preference {
    /// User id.
    pub user_id: String,
    /// Category, such as `display_settings` or `flagged_post`.
    pub category: String,
    /// Name within the category.
    pub name: String,
    /// Value, always a string.
    pub value: String,
}

/// A custom emoji.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Emoji {
    /// Emoji id; its image is `emoji/{id}/image`.
    pub id: String,
    /// Who added it.
    pub creator_id: String,
    /// Shortcode name, without colons.
    pub name: String,
    /// Milliseconds since the epoch.
    pub create_at: i64,
    /// Milliseconds since the epoch.
    pub update_at: i64,
    /// Milliseconds since the epoch; nonzero when deleted.
    pub delete_at: i64,
}

/// A user's presence.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Status {
    /// User id.
    pub user_id: String,
    /// `online`, `away`, `dnd` or `offline`.
    pub status: String,
    /// Whether the user set it themselves.
    pub manual: bool,
    /// Milliseconds since the epoch.
    pub last_activity_at: i64,
    /// Seconds since the epoch when Do Not Disturb ends; 0 for no end.
    pub dnd_end_time: i64,
}

/// Counts shown in the channel header, from `channels/{id}/stats`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ChannelStats {
    /// Channel id.
    pub channel_id: String,
    /// Members.
    pub member_count: i64,
    /// Guests.
    pub guest_count: i64,
    /// Pinned posts.
    pub pinnedpost_count: i64,
    /// Files.
    pub files_count: i64,
}
