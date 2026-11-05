//! Applying WebSocket events to the store (#36, PLAN §6.4). Each event is
//! written inside the batch's transaction and marks what the views must
//! redraw. Events that name something the cache does not hold ask for a
//! fetch instead of guessing.

use std::sync::Arc;

use matty_api::models::{Post, Status};
use matty_api::ws::WsEvent;
use matty_store::sidebar::{self, NewPost, ReadState};
use matty_store::{StoreError, outbox, posts};
use rusqlite::Connection;

use crate::change::{Arrived, ChangeSet};
use crate::engine::Dirty;

/// What an event needs from the server before the cache is right again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Fetch {
    /// A channel and the user's membership of it.
    Channel(String),
    /// A team the user joined, with its channels and sidebar.
    Team(String),
    /// A team's sidebar categories.
    Categories(String),
    /// The server's config and license.
    Capabilities,
}

/// Applies one event. `me` is the signed-in user's id, once known.
pub(crate) fn apply(
    conn: &Connection,
    event: WsEvent,
    me: Option<&str>,
    dirty: &mut Dirty,
    fetches: &mut Vec<Fetch>,
) -> Result<(), StoreError> {
    let me_is = |user_id: &str| me == Some(user_id);
    match event {
        WsEvent::Posted(posted) => {
            let post = posted.post;
            // Already stored: a duplicate, or `post_edited` got here first.
            if posts::post(conn, &post.id)?.is_some() {
                return Ok(());
            }
            let mentions_me = me.is_some_and(|me| posted.mentions.iter().any(|id| id == me));
            new_post(conn, &post, me, mentions_me, dirty, fetches)?;
            if let Some(me) = me.filter(|me| post.user_id != *me) {
                let channel_props = sidebar::channel_member(conn, &post.channel_id, me)?
                    .map(|member| member.notify_props)
                    .unwrap_or_default();
                dirty.direct.push(ChangeSet::Arrived(Arc::new(Arrived {
                    followed: posted.followers.iter().any(|id| id == me),
                    post,
                    channel_type: posted.channel_type,
                    channel_display_name: posted.channel_display_name,
                    sender_name: posted.sender_name,
                    mentioned: mentions_me,
                    channel_props,
                    collapsed_threads: crate::engine::cached_crt(conn)?,
                })));
            }
        }
        // The server changes and removes an ephemeral post with the usual
        // events; it stays out of the store all the same.
        WsEvent::PostEdited(post) | WsEvent::PostDeleted(post) if is_ephemeral(&post) => {
            dirty.direct.push(ChangeSet::Ephemeral(Arc::from(post)));
        }
        WsEvent::PostEdited(post) => {
            let known = posts::post(conn, &post.id)?.is_some();
            if !known {
                // The edit outran `posted`: count it now, so the late
                // `posted` is a no-op and nothing counts twice.
                return new_post(conn, &post, me, false, dirty, fetches);
            }
            posts::insert_posts(conn, std::slice::from_ref(&*post))?;
            touch_posts(&post, dirty);
        }
        WsEvent::PostDeleted(post) => {
            let Some(stored) = posts::post(conn, &post.id)? else {
                return Ok(());
            };
            posts::mark_deleted(conn, &post.id, post.delete_at)?;
            if !stored.root_id.is_empty() {
                posts::add_replies(conn, &stored.root_id, -1)?;
            }
            touch_posts(&stored, dirty);
        }
        WsEvent::EphemeralMessage(post) => {
            dirty.direct.push(ChangeSet::Ephemeral(Arc::from(post)));
        }
        WsEvent::OpenDialog(dialog) => {
            dirty.direct.push(ChangeSet::OpenDialog(Arc::from(dialog)));
        }
        WsEvent::ReactionAdded(reaction) => {
            if let Some(post) = posts::post(conn, &reaction.post_id)? {
                posts::add_reaction(conn, &reaction)?;
                touch_posts(&post, dirty);
            }
        }
        WsEvent::ReactionRemoved(reaction) => {
            if let Some(post) = posts::post(conn, &reaction.post_id)? {
                posts::remove_reaction(conn, &reaction)?;
                touch_posts(&post, dirty);
            }
        }
        WsEvent::PostUnread { channel_id, unread } => {
            if let Some(me) = me {
                let state = ReadState {
                    last_viewed_at: unread.last_viewed_at,
                    msg_count: unread.msg_count,
                    msg_count_root: unread.msg_count_root,
                    mention_count: unread.mention_count,
                    mention_count_root: unread.mention_count_root,
                    urgent_mention_count: unread.urgent_mention_count,
                };
                sidebar::set_read_state(conn, &channel_id, me, &state)?;
                touch_channel(conn, &channel_id, dirty)?;
            }
        }
        WsEvent::ChannelViewed { channel_id } => {
            if let Some(me) = me {
                sidebar::mark_viewed(conn, &channel_id, me, 0)?;
                touch_channel(conn, &channel_id, dirty)?;
            }
        }
        WsEvent::MultipleChannelsViewed { channel_times } => {
            if let Some(me) = me {
                for (channel_id, viewed_at) in channel_times {
                    sidebar::mark_viewed(conn, &channel_id, me, viewed_at)?;
                    touch_channel(conn, &channel_id, dirty)?;
                }
            }
        }
        WsEvent::ChannelCreated { channel_id, .. }
        | WsEvent::ChannelRestored { channel_id }
        | WsEvent::ChannelConverted { channel_id }
        | WsEvent::DirectAdded { channel_id, .. }
        | WsEvent::GroupAdded { channel_id, .. } => fetches.push(Fetch::Channel(channel_id)),
        WsEvent::ChannelUpdated(channel) => {
            // Keep the counts events have moved; the rest is the server's.
            let mut channel = *channel;
            if let Some(cached) = sidebar::channel(conn, &channel.id)? {
                channel.total_msg_count = channel.total_msg_count.max(cached.total_msg_count);
                channel.total_msg_count_root = channel
                    .total_msg_count_root
                    .max(cached.total_msg_count_root);
                channel.last_post_at = channel.last_post_at.max(cached.last_post_at);
            }
            sidebar::upsert_channels(conn, std::slice::from_ref(&channel))?;
            touch_channel(conn, &channel.id, dirty)?;
        }
        WsEvent::ChannelDeleted {
            channel_id,
            delete_at,
        } => {
            touch_channel(conn, &channel_id, dirty)?;
            sidebar::set_channel_deleted(conn, &channel_id, delete_at.max(1))?;
        }
        WsEvent::ChannelMemberUpdated(member) => {
            if me_is(&member.user_id) {
                sidebar::upsert_channel_members(conn, std::slice::from_ref(&*member))?;
                touch_channel(conn, &member.channel_id, dirty)?;
            }
        }
        WsEvent::UserAdded {
            user_id,
            channel_id,
            ..
        } => {
            if me_is(&user_id) {
                fetches.push(Fetch::Channel(channel_id));
            }
        }
        WsEvent::UserRemoved {
            user_id,
            channel_id,
            ..
        } => {
            if me_is(&user_id) {
                touch_channel(conn, &channel_id, dirty)?;
                sidebar::delete_channel(conn, &channel_id)?;
            }
        }
        WsEvent::UserUpdated(user) => {
            sidebar::upsert_users(conn, std::slice::from_ref(&*user))?;
            // Display names in the sidebar and in posts may change, and a
            // new `last_picture_update` means a new profile picture (#153).
            dirty.all_teams = true;
            dirty.direct.push(ChangeSet::Users(Arc::from(vec![*user])));
        }
        WsEvent::StatusChange { user_id, status } => {
            dirty
                .direct
                .push(ChangeSet::Presence(Arc::from(vec![Status {
                    user_id,
                    status,
                    ..Status::default()
                }])));
        }
        WsEvent::PreferencesChanged(preferences) => {
            sidebar::upsert_preferences(conn, &preferences)?;
            dirty.preferences = true;
            dirty.all_teams |= preferences.iter().any(shows_conversation);
            if let Some(order) = preferences.iter().find(|p| p.category == "teams_order") {
                let ids: Vec<String> = order
                    .value
                    .split(',')
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .collect();
                sidebar::set_team_order(conn, &ids)?;
                dirty.unreads = true;
            }
        }
        WsEvent::PreferencesDeleted(preferences) => {
            sidebar::delete_preferences(conn, &preferences)?;
            dirty.preferences = true;
            dirty.all_teams |= preferences.iter().any(shows_conversation);
        }
        WsEvent::UpdateTeam(team) => {
            sidebar::upsert_teams(conn, std::slice::from_ref(&*team))?;
            dirty.unreads = true;
        }
        WsEvent::DeleteTeam(team) => {
            sidebar::delete_team(conn, &team.id)?;
            dirty.unreads = true;
        }
        WsEvent::AddedToTeam { team_id, user_id } => {
            if me_is(&user_id) {
                fetches.push(Fetch::Team(team_id));
            }
        }
        WsEvent::LeaveTeam { team_id, user_id } => {
            if me_is(&user_id) {
                sidebar::delete_team(conn, &team_id)?;
                dirty.unreads = true;
            }
        }
        WsEvent::SidebarCategoryCreated { team_id, .. }
        | WsEvent::SidebarCategoryDeleted { team_id, .. } => {
            // A deleted category's channels move to another one; only the
            // server knows where.
            fetches.push(Fetch::Categories(team_id));
        }
        WsEvent::SidebarCategoryUpdated(categories) => {
            for category in categories {
                match sidebar::category_sort_order(conn, &category.id)? {
                    Some(order) => {
                        sidebar::upsert_category(conn, &category.team_id, &category, order)?;
                        dirty.teams.insert(category.team_id);
                    }
                    None => fetches.push(Fetch::Categories(category.team_id)),
                }
            }
        }
        WsEvent::SidebarCategoryOrderUpdated { team_id, order } => {
            sidebar::set_category_order(conn, &team_id, &order)?;
            dirty.teams.insert(team_id);
        }
        WsEvent::ThreadUpdated { thread, .. } => {
            let team_id = match sidebar::channel(conn, &thread.post.channel_id)? {
                Some(channel) => channel.team_id,
                None => posts::thread_team(conn, &thread.id)?.unwrap_or_default(),
            };
            posts::upsert_threads(conn, &team_id, std::slice::from_ref(&*thread))?;
            dirty.thread_lists.insert(team_id);
            dirty.threads.insert(thread.id.clone());
        }
        WsEvent::ThreadReadChanged(read) => {
            posts::mark_thread_read(
                conn,
                &read.thread_id,
                read.timestamp,
                read.unread_replies,
                read.unread_mentions,
            )?;
            if let Some(team_id) = posts::thread_team(conn, &read.thread_id)? {
                dirty.thread_lists.insert(team_id);
            }
        }
        WsEvent::ThreadFollowChanged {
            thread_id,
            following,
            ..
        } => {
            posts::set_following(conn, &thread_id, following)?;
            if let Some(team_id) = posts::thread_team(conn, &thread_id)? {
                dirty.thread_lists.insert(team_id);
            }
        }
        WsEvent::EmojiAdded(emoji) => sidebar::upsert_emoji(conn, std::slice::from_ref(&emoji))?,
        WsEvent::ConfigChanged(_) | WsEvent::LicenseChanged(_) => {
            fetches.push(Fetch::Capabilities);
        }
        WsEvent::DraftCreated(draft) | WsEvent::DraftUpdated(draft) => {
            posts::upsert_draft(conn, &draft)?;
            let stored = posts::draft(conn, &draft.channel_id, &draft.root_id)?;
            dirty.direct.push(ChangeSet::Draft {
                channel_id: draft.channel_id,
                root_id: draft.root_id,
                draft: stored.map(Arc::new),
            });
        }
        WsEvent::DraftDeleted(draft) => {
            posts::delete_draft(conn, &draft.channel_id, &draft.root_id)?;
            dirty.direct.push(ChangeSet::Draft {
                channel_id: draft.channel_id,
                root_id: draft.root_id,
                draft: None,
            });
        }
        WsEvent::PostAcknowledgementAdded(ack) => {
            crate::priority::acknowledge_locally(conn, &ack, true, dirty)?;
        }
        WsEvent::PostAcknowledgementRemoved(ack) => {
            crate::priority::acknowledge_locally(conn, &ack, false, dirty)?;
        }
        WsEvent::ScheduledPostCreated(post) | WsEvent::ScheduledPostUpdated(post) => dirty
            .scheduled
            .push(crate::priority::ScheduledChange::Upsert(post)),
        WsEvent::ScheduledPostDeleted(post) => dirty
            .scheduled
            .push(crate::priority::ScheduledChange::Remove(post.id)),
        WsEvent::Typing {
            user_id,
            channel_id,
            parent_id,
        } => dirty.direct.push(ChangeSet::Typing {
            user_id,
            channel_id,
            parent_id,
        }),
        event @ (WsEvent::ChannelBookmarkCreated { .. }
        | WsEvent::ChannelBookmarkUpdated { .. }
        | WsEvent::ChannelBookmarkDeleted { .. }
        | WsEvent::ChannelBookmarkSorted { .. }) => crate::bookmarks::event(conn, event, dirty)?,
        WsEvent::Undecodable { raw, error } => {
            // Something changed that the cache cannot follow; #37 resyncs.
            tracing::warn!(event = %raw.event, %error, "could not decode an event");
            dirty.resync = true;
        }
        // Not cached, or nothing to do yet: the hello is the WebSocket
        // client's, roles and new accounts load lazily, and scheme changes
        // come with their feature.
        WsEvent::Hello(_)
        | WsEvent::NewUser { .. }
        | WsEvent::UserRoleUpdated { .. }
        | WsEvent::ChannelSchemeUpdated { .. }
        | WsEvent::Unknown(_) => {}
    }
    Ok(())
}

/// Stores a post the cache has not seen and counts it in its channel and
/// thread. A post in a channel the cache does not hold fetches the channel.
fn new_post(
    conn: &Connection,
    post: &Post,
    me: Option<&str>,
    mentions_me: bool,
    dirty: &mut Dirty,
    fetches: &mut Vec<Fetch>,
) -> Result<(), StoreError> {
    posts::insert_posts(conn, std::slice::from_ref(post))?;
    // Our own send, echoed before the REST response: it is delivered.
    if !post.pending_post_id.is_empty() && outbox::remove(conn, &post.pending_post_id)? {
        dirty.outbox.insert(post.channel_id.clone());
    }
    let is_root = post.root_id.is_empty();
    if !is_root {
        posts::add_replies(conn, &post.root_id, 1)?;
        posts::note_reply(conn, &post.root_id, &post.user_id, post.create_at)?;
    }
    if let Some(me) = me {
        let counted = sidebar::count_post(
            conn,
            &post.channel_id,
            me,
            &NewPost {
                create_at: post.create_at,
                is_root,
                by_me: post.user_id == me,
                mentions_me,
                urgent: mentions_me
                    && post
                        .metadata
                        .priority
                        .as_ref()
                        .is_some_and(matty_api::models::PostPriority::is_urgent),
            },
        )?;
        if !counted {
            fetches.push(Fetch::Channel(post.channel_id.clone()));
        }
    }
    touch_channel(conn, &post.channel_id, dirty)?;
    touch_posts(post, dirty);
    Ok(())
}

/// The post's channel window and, for a reply or a root, its thread.
pub(crate) fn touch_posts(post: &Post, dirty: &mut Dirty) {
    dirty.channels.insert(post.channel_id.clone());
    let root = if post.root_id.is_empty() {
        &post.id
    } else {
        &post.root_id
    };
    dirty.threads.insert(root.clone());
}

/// The sidebar the channel is in and the team rail. Direct and group
/// messages show in every team's sidebar.
pub(crate) fn touch_channel(
    conn: &Connection,
    channel_id: &str,
    dirty: &mut Dirty,
) -> Result<(), StoreError> {
    dirty.unreads = true;
    match sidebar::channel(conn, channel_id)? {
        Some(channel) if !channel.team_id.is_empty() => {
            dirty.teams.insert(channel.team_id);
        }
        _ => dirty.all_teams = true,
    }
    Ok(())
}

/// Whether a preference shows or hides a direct or group conversation.
fn shows_conversation(preference: &matty_api::models::Preference) -> bool {
    matches!(
        preference.category.as_str(),
        "direct_channel_show" | "group_channel_show"
    )
}

/// Whether `post` is one only this user sees, which is never stored.
fn is_ephemeral(post: &matty_api::models::Post) -> bool {
    post.kind == "system_ephemeral"
}
