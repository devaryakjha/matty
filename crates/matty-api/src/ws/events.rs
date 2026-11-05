//! Typed WebSocket events (PLAN §6.4). [`WsEvent::decode`] turns a
//! [`RawEvent`] into a variant by name, decoding the values Mattermost sends
//! as JSON inside a string (`post`, `reaction`, `channel`, ...) a second time.

use std::collections::HashMap;

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

use super::{Broadcast, Hello, RawEvent};
use crate::{
    capabilities::OldFormat,
    models::{
        BookmarkUpdate, Channel, ChannelBookmark, ChannelMember, Draft, Emoji, Post,
        PostAcknowledgement, Preference, Reaction, SidebarCategory, Team, User, UserThread,
    },
};

/// A decoded server event, with who it was for and its sequence number.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// What happened.
    pub kind: WsEvent,
    /// Who the event was for.
    pub broadcast: Broadcast,
    /// The server's sequence number on this connection.
    pub seq: i64,
}

impl From<RawEvent> for Event {
    fn from(raw: RawEvent) -> Self {
        Event {
            kind: WsEvent::decode(&raw),
            broadcast: raw.broadcast,
            seq: raw.seq,
        }
    }
}

/// `posted`: a new post.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Posted {
    /// The post.
    pub post: Post,
    /// `O`, `P`, `D` or `G`.
    pub channel_type: String,
    /// Channel URL name.
    pub channel_name: String,
    /// Channel display name.
    pub channel_display_name: String,
    /// Team; empty for DMs and group messages.
    pub team_id: String,
    /// The author's display name, as the server formats it.
    pub sender_name: String,
    /// Users the post mentions who get notified.
    pub mentions: Vec<String>,
    /// Thread followers who get notified (Collapsed Reply Threads).
    pub followers: Vec<String>,
    /// Whether the post sets the author online.
    pub set_online: bool,
}

/// `thread_read_changed`: a thread's read state changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ThreadReadChanged {
    /// Thread id (the root post's id); empty when every thread was marked read.
    pub thread_id: String,
    /// Channel.
    pub channel_id: String,
    /// The new last-read time, in milliseconds since the epoch.
    pub timestamp: i64,
    /// Unread replies now.
    pub unread_replies: i64,
    /// Unread mentions now.
    pub unread_mentions: i64,
    /// Unread replies before.
    pub previous_unread_replies: i64,
    /// Unread mentions before.
    pub previous_unread_mentions: i64,
}

/// `post_unread`: the user marked a post unread.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PostUnread {
    /// The post marked unread.
    pub post_id: String,
    /// The new `last_viewed_at`: just before the post.
    pub last_viewed_at: i64,
    /// Messages now read.
    pub msg_count: i64,
    /// Root posts now read.
    pub msg_count_root: i64,
    /// Mentions now unread.
    pub mention_count: i64,
    /// Root-post mentions now unread.
    pub mention_count_root: i64,
    /// Urgent mentions now unread.
    pub urgent_mention_count: i64,
}

/// Every event Matty understands. Each variant names its event; fields that
/// the server puts in `broadcast` rather than `data` (the channel of a
/// `typing` event, for example) are copied in.
#[derive(Clone, Debug, PartialEq)]
pub enum WsEvent {
    /// `hello`.
    Hello(Hello),
    /// `posted`.
    Posted(Box<Posted>),
    /// `post_edited`.
    PostEdited(Box<Post>),
    /// `post_deleted`.
    PostDeleted(Box<Post>),
    /// `ephemeral_message`: a post only this user sees, not stored.
    EphemeralMessage(Box<Post>),
    /// `open_dialog`: an integration opens an interactive dialog, in
    /// answer to a button press or slash command of this user's.
    OpenDialog(Box<crate::models::OpenDialogRequest>),
    /// `post_unread`.
    PostUnread {
        /// Channel.
        channel_id: String,
        /// The new read state.
        unread: PostUnread,
    },
    /// `reaction_added`.
    ReactionAdded(Reaction),
    /// `reaction_removed`.
    ReactionRemoved(Reaction),
    /// `post_acknowledgement_added`.
    PostAcknowledgementAdded(PostAcknowledgement),
    /// `post_acknowledgement_removed`.
    PostAcknowledgementRemoved(PostAcknowledgement),
    /// `typing`.
    Typing {
        /// Who is typing.
        user_id: String,
        /// Where.
        channel_id: String,
        /// The thread root, when typing a reply.
        parent_id: String,
    },
    /// `status_change`.
    StatusChange {
        /// Whose status.
        user_id: String,
        /// `online`, `away`, `dnd` or `offline`.
        status: String,
    },
    /// `channel_viewed`: the user read a channel on another device (older servers).
    ChannelViewed {
        /// Channel.
        channel_id: String,
    },
    /// `multiple_channels_viewed`: the user read channels, with new `last_viewed_at`s.
    MultipleChannelsViewed {
        /// Channel id to `last_viewed_at`.
        channel_times: HashMap<String, i64>,
    },
    /// `channel_created`.
    ChannelCreated {
        /// Channel.
        channel_id: String,
        /// Team.
        team_id: String,
    },
    /// `channel_updated`.
    ChannelUpdated(Box<Channel>),
    /// `channel_deleted`: archived.
    ChannelDeleted {
        /// Channel.
        channel_id: String,
        /// Milliseconds since the epoch.
        delete_at: i64,
    },
    /// `channel_restored`: unarchived.
    ChannelRestored {
        /// Channel.
        channel_id: String,
    },
    /// `channel_converted`: made private.
    ChannelConverted {
        /// Channel.
        channel_id: String,
    },
    /// `channel_scheme_updated`: the channel's permissions changed.
    ChannelSchemeUpdated {
        /// Channel.
        channel_id: String,
    },
    /// `channel_member_updated`: the user's membership (notify props, roles) changed.
    ChannelMemberUpdated(Box<ChannelMember>),
    /// `direct_added`: a DM with this user was created.
    DirectAdded {
        /// Channel.
        channel_id: String,
        /// Who created it.
        creator_id: String,
        /// The other user.
        teammate_id: String,
    },
    /// `group_added`: a group message with this user was created.
    GroupAdded {
        /// Channel.
        channel_id: String,
        /// The members.
        teammate_ids: Vec<String>,
    },
    /// `user_added`: a user joined a channel.
    UserAdded {
        /// Who joined.
        user_id: String,
        /// Channel.
        channel_id: String,
        /// The channel's team.
        team_id: String,
    },
    /// `user_removed`: a user left or was removed from a channel.
    UserRemoved {
        /// Who left.
        user_id: String,
        /// Channel.
        channel_id: String,
        /// Who removed them; themselves when they left.
        remover_id: String,
    },
    /// `user_updated`.
    UserUpdated(Box<User>),
    /// `new_user`: an account was created.
    NewUser {
        /// The new user.
        user_id: String,
    },
    /// `user_role_updated`.
    UserRoleUpdated {
        /// Whose roles.
        user_id: String,
        /// Space-separated role names.
        roles: String,
    },
    /// `preferences_changed`, and the older single `preference_changed`.
    PreferencesChanged(Vec<Preference>),
    /// `preferences_deleted`.
    PreferencesDeleted(Vec<Preference>),
    /// `update_team`.
    UpdateTeam(Box<Team>),
    /// `delete_team`: archived.
    DeleteTeam(Box<Team>),
    /// `added_to_team`: this user joined a team.
    AddedToTeam {
        /// Team.
        team_id: String,
        /// The user.
        user_id: String,
    },
    /// `leave_team`: a user left a team.
    LeaveTeam {
        /// Team.
        team_id: String,
        /// The user.
        user_id: String,
    },
    /// `sidebar_category_created`.
    SidebarCategoryCreated {
        /// Category.
        category_id: String,
        /// Team.
        team_id: String,
    },
    /// `sidebar_category_updated`.
    SidebarCategoryUpdated(Vec<SidebarCategory>),
    /// `sidebar_category_deleted`.
    SidebarCategoryDeleted {
        /// Category.
        category_id: String,
        /// Team.
        team_id: String,
    },
    /// `sidebar_category_order_updated`.
    SidebarCategoryOrderUpdated {
        /// Team.
        team_id: String,
        /// Category ids in display order.
        order: Vec<String>,
    },
    /// `thread_updated`: a followed thread got a reply or changed.
    ThreadUpdated {
        /// The thread.
        thread: Box<UserThread>,
        /// Unread replies before.
        previous_unread_replies: i64,
        /// Unread mentions before.
        previous_unread_mentions: i64,
    },
    /// `thread_read_changed`.
    ThreadReadChanged(ThreadReadChanged),
    /// `thread_follow_changed`.
    ThreadFollowChanged {
        /// Thread.
        thread_id: String,
        /// Whether the user now follows it.
        following: bool,
        /// Replies in the thread.
        reply_count: i64,
    },
    /// `emoji_added`.
    EmojiAdded(Emoji),
    /// `config_changed`: the client config, in `format=old`.
    ConfigChanged(OldFormat),
    /// `license_changed`: the client license, in `format=old`.
    LicenseChanged(OldFormat),
    /// `draft_created`.
    DraftCreated(Draft),
    /// `draft_updated`.
    DraftUpdated(Draft),
    /// `draft_deleted`.
    DraftDeleted(Draft),
    /// `channel_bookmark_created` (#226).
    ChannelBookmarkCreated {
        /// The channel.
        channel_id: String,
        /// The new bookmark.
        bookmark: Box<ChannelBookmark>,
    },
    /// `channel_bookmark_updated`: the bookmark as it now stands, and the
    /// one it replaced when the edit gave it a new id.
    ChannelBookmarkUpdated {
        /// The channel.
        channel_id: String,
        /// What changed.
        update: Box<BookmarkUpdate>,
    },
    /// `channel_bookmark_deleted`.
    ChannelBookmarkDeleted {
        /// The channel.
        channel_id: String,
        /// The bookmark, as deleted.
        bookmark: Box<ChannelBookmark>,
    },
    /// `channel_bookmark_sorted`: the bookmarks whose place changed.
    ChannelBookmarkSorted {
        /// The channel.
        channel_id: String,
        /// The moved bookmarks with their new `sort_order`.
        bookmarks: Vec<ChannelBookmark>,
    },
    /// An event Matty does not know. Not an error: servers add events.
    Unknown(RawEvent),
    /// A known event whose data did not decode. The sync engine treats it
    /// as a gap and resyncs.
    Undecodable {
        /// The event.
        raw: RawEvent,
        /// What failed.
        error: String,
    },
}

impl WsEvent {
    /// Decodes `raw` by its event name.
    pub fn decode(raw: &RawEvent) -> WsEvent {
        match decode_known(raw) {
            Ok(Some(event)) => event,
            Ok(None) => WsEvent::Unknown(raw.clone()),
            Err(error) => WsEvent::Undecodable {
                raw: raw.clone(),
                error,
            },
        }
    }
}

/// Reads `data[key]`, decoding it a second time when the server sent JSON
/// inside a string. A missing key is the type's default.
fn field<T: DeserializeOwned + Default>(data: &Value, key: &str) -> Result<T, String> {
    match data.get(key) {
        None | Some(Value::Null) => Ok(T::default()),
        Some(Value::String(text)) => match serde_json::from_str(text) {
            Ok(value) => Ok(value),
            // A plain string field, such as an id.
            Err(_) => serde_json::from_value(Value::String(text.clone()))
                .map_err(|e| format!("{key}: {e}")),
        },
        Some(value) => serde_json::from_value(value.clone()).map_err(|e| format!("{key}: {e}")),
    }
}

/// Like [`field`], for values that must be present.
fn required<T: DeserializeOwned>(data: &Value, key: &str) -> Result<T, String> {
    let value = data.get(key).ok_or_else(|| format!("missing {key}"))?;
    match value {
        Value::String(text) => serde_json::from_str(text)
            .or_else(|_| serde_json::from_value(value.clone()))
            .map_err(|e| format!("{key}: {e}")),
        _ => serde_json::from_value(value.clone()).map_err(|e| format!("{key}: {e}")),
    }
}

fn text(data: &Value, key: &str) -> String {
    data.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn number(data: &Value, key: &str) -> i64 {
    data.get(key).and_then(Value::as_i64).unwrap_or_default()
}

/// `data` if present, else the broadcast's value.
fn or_broadcast(data: &Value, key: &str, broadcast: &str) -> String {
    let value = text(data, key);
    if value.is_empty() {
        broadcast.to_string()
    } else {
        value
    }
}

/// `own` unless it is empty, then `fallback`.
fn or_text(own: &str, fallback: &str) -> String {
    if own.is_empty() { fallback } else { own }.to_owned()
}

fn whole<T: for<'de> Deserialize<'de>>(data: &Value) -> Result<T, String> {
    serde_json::from_value(data.clone()).map_err(|e| e.to_string())
}

fn decode_known(raw: &RawEvent) -> Result<Option<WsEvent>, String> {
    let data = &raw.data;
    let b = &raw.broadcast;
    let event = match raw.event.as_str() {
        "hello" => WsEvent::Hello(whole(data)?),
        "posted" => WsEvent::Posted(Box::new(Posted {
            post: required(data, "post")?,
            channel_type: text(data, "channel_type"),
            channel_name: text(data, "channel_name"),
            channel_display_name: text(data, "channel_display_name"),
            team_id: text(data, "team_id"),
            sender_name: text(data, "sender_name"),
            mentions: field(data, "mentions")?,
            followers: field(data, "followers")?,
            set_online: data
                .get("set_online")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
        })),
        "post_edited" => WsEvent::PostEdited(required(data, "post")?),
        "post_deleted" => WsEvent::PostDeleted(required(data, "post")?),
        "ephemeral_message" => WsEvent::EphemeralMessage(required(data, "post")?),
        "open_dialog" => WsEvent::OpenDialog(required(data, "dialog")?),
        "post_unread" => WsEvent::PostUnread {
            channel_id: or_broadcast(data, "channel_id", &b.channel_id),
            unread: whole(data)?,
        },
        "reaction_added" => WsEvent::ReactionAdded(required(data, "reaction")?),
        "reaction_removed" => WsEvent::ReactionRemoved(required(data, "reaction")?),
        "post_acknowledgement_added" => {
            WsEvent::PostAcknowledgementAdded(required(data, "acknowledgement")?)
        }
        "post_acknowledgement_removed" => {
            WsEvent::PostAcknowledgementRemoved(required(data, "acknowledgement")?)
        }
        "typing" => WsEvent::Typing {
            user_id: text(data, "user_id"),
            channel_id: or_broadcast(data, "channel_id", &b.channel_id),
            parent_id: text(data, "parent_id"),
        },
        "status_change" => WsEvent::StatusChange {
            user_id: or_broadcast(data, "user_id", &b.user_id),
            status: text(data, "status"),
        },
        "channel_viewed" => WsEvent::ChannelViewed {
            channel_id: text(data, "channel_id"),
        },
        "multiple_channels_viewed" => WsEvent::MultipleChannelsViewed {
            channel_times: field(data, "channel_times")?,
        },
        "channel_created" => WsEvent::ChannelCreated {
            channel_id: text(data, "channel_id"),
            team_id: or_broadcast(data, "team_id", &b.team_id),
        },
        "channel_updated" => WsEvent::ChannelUpdated(required(data, "channel")?),
        "channel_deleted" => WsEvent::ChannelDeleted {
            channel_id: text(data, "channel_id"),
            delete_at: number(data, "delete_at"),
        },
        "channel_restored" => WsEvent::ChannelRestored {
            channel_id: text(data, "channel_id"),
        },
        "channel_converted" => WsEvent::ChannelConverted {
            channel_id: text(data, "channel_id"),
        },
        "channel_scheme_updated" => WsEvent::ChannelSchemeUpdated {
            channel_id: or_broadcast(data, "channel_id", &b.channel_id),
        },
        "channel_member_updated" => WsEvent::ChannelMemberUpdated(required(data, "channelMember")?),
        "direct_added" => WsEvent::DirectAdded {
            channel_id: or_broadcast(data, "channel_id", &b.channel_id),
            creator_id: text(data, "creator_id"),
            teammate_id: text(data, "teammate_id"),
        },
        "group_added" => WsEvent::GroupAdded {
            channel_id: or_broadcast(data, "channel_id", &b.channel_id),
            teammate_ids: field(data, "teammate_ids")?,
        },
        "user_added" => WsEvent::UserAdded {
            user_id: or_broadcast(data, "user_id", &b.user_id),
            channel_id: or_broadcast(data, "channel_id", &b.channel_id),
            team_id: or_broadcast(data, "team_id", &b.team_id),
        },
        "user_removed" => WsEvent::UserRemoved {
            // Sent to the removed user with only the channel, or to the
            // channel with the user's id.
            user_id: or_broadcast(data, "user_id", &b.user_id),
            channel_id: or_broadcast(data, "channel_id", &b.channel_id),
            remover_id: text(data, "remover_id"),
        },
        "user_updated" => WsEvent::UserUpdated(required(data, "user")?),
        "new_user" => WsEvent::NewUser {
            user_id: text(data, "user_id"),
        },
        "user_role_updated" => WsEvent::UserRoleUpdated {
            user_id: or_broadcast(data, "user_id", &b.user_id),
            roles: text(data, "roles"),
        },
        "preferences_changed" => WsEvent::PreferencesChanged(required(data, "preferences")?),
        "preference_changed" => WsEvent::PreferencesChanged(vec![required(data, "preference")?]),
        "preferences_deleted" => WsEvent::PreferencesDeleted(required(data, "preferences")?),
        "update_team" => WsEvent::UpdateTeam(required(data, "team")?),
        "delete_team" => WsEvent::DeleteTeam(required(data, "team")?),
        "added_to_team" => WsEvent::AddedToTeam {
            team_id: text(data, "team_id"),
            user_id: or_broadcast(data, "user_id", &b.user_id),
        },
        "leave_team" => WsEvent::LeaveTeam {
            team_id: or_broadcast(data, "team_id", &b.team_id),
            user_id: text(data, "user_id"),
        },
        "sidebar_category_created" => WsEvent::SidebarCategoryCreated {
            category_id: text(data, "category_id"),
            team_id: b.team_id.clone(),
        },
        "sidebar_category_updated" => {
            WsEvent::SidebarCategoryUpdated(field(data, "updatedCategories")?)
        }
        "sidebar_category_deleted" => WsEvent::SidebarCategoryDeleted {
            category_id: text(data, "category_id"),
            team_id: b.team_id.clone(),
        },
        "sidebar_category_order_updated" => WsEvent::SidebarCategoryOrderUpdated {
            team_id: or_broadcast(data, "team_id", &b.team_id),
            order: field(data, "order")?,
        },
        "thread_updated" => WsEvent::ThreadUpdated {
            thread: required(data, "thread")?,
            previous_unread_replies: number(data, "previous_unread_replies"),
            previous_unread_mentions: number(data, "previous_unread_mentions"),
        },
        "thread_read_changed" => WsEvent::ThreadReadChanged(whole(data)?),
        "thread_follow_changed" => WsEvent::ThreadFollowChanged {
            thread_id: text(data, "thread_id"),
            following: data
                .get("state")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
            reply_count: number(data, "reply_count"),
        },
        "emoji_added" => WsEvent::EmojiAdded(required(data, "emoji")?),
        "config_changed" => WsEvent::ConfigChanged(required(data, "config")?),
        "license_changed" => WsEvent::LicenseChanged(required(data, "license")?),
        "draft_created" => WsEvent::DraftCreated(required(data, "draft")?),
        "draft_updated" => WsEvent::DraftUpdated(required(data, "draft")?),
        "draft_deleted" => WsEvent::DraftDeleted(required(data, "draft")?),
        "channel_bookmark_created" => {
            let bookmark: Box<ChannelBookmark> = required(data, "bookmark")?;
            WsEvent::ChannelBookmarkCreated {
                channel_id: or_text(&bookmark.channel_id, &b.channel_id),
                bookmark,
            }
        }
        "channel_bookmark_updated" => {
            let update: Box<BookmarkUpdate> = required(data, "bookmarks")?;
            let own = update
                .updated
                .as_ref()
                .map(|bookmark| bookmark.channel_id.as_str())
                .unwrap_or_default();
            WsEvent::ChannelBookmarkUpdated {
                channel_id: or_text(own, &b.channel_id),
                update,
            }
        }
        "channel_bookmark_deleted" => {
            let bookmark: Box<ChannelBookmark> = required(data, "bookmark")?;
            WsEvent::ChannelBookmarkDeleted {
                channel_id: or_text(&bookmark.channel_id, &b.channel_id),
                bookmark,
            }
        }
        "channel_bookmark_sorted" => {
            let bookmarks: Vec<ChannelBookmark> = required(data, "bookmarks")?;
            let own = bookmarks
                .first()
                .map(|bookmark| bookmark.channel_id.as_str())
                .unwrap_or_default();
            WsEvent::ChannelBookmarkSorted {
                channel_id: or_text(own, &b.channel_id),
                bookmarks,
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(event))
}
