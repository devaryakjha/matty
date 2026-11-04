//! Sidebar data: typed upserts for teams, channels, members, categories,
//! users, preferences and emoji, and the snapshot queries the sidebar and
//! team rail render from.
//!
//! Every upsert is idempotent: writing the same row twice leaves one row with
//! the latest values. Callers group writes in a transaction; each function
//! takes a [`Connection`], which a `Transaction` derefs to.

use matty_api::models::{
    Channel, ChannelMember, Emoji, Preference, SidebarCategories, SidebarCategory, Team, User,
};
use rusqlite::{Connection, OptionalExtension as _, params};

use crate::Result;

/// Serializes a value the server sent so it can be read back later.
fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned())
}

/// Inserts or replaces teams.
pub fn upsert_teams(conn: &Connection, teams: &[Team]) -> Result<()> {
    let mut statement = conn.prepare_cached(
        "INSERT INTO teams (id, name, display_name, kind, delete_at, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (id) DO UPDATE SET
             name = excluded.name, display_name = excluded.display_name,
             kind = excluded.kind, delete_at = excluded.delete_at, data = excluded.data",
    )?;
    for team in teams {
        statement.execute(params![
            team.id,
            team.name,
            team.display_name,
            team.kind,
            team.delete_at,
            json(team)
        ])?;
    }
    Ok(())
}

/// Sets the team rail order (the `teams_order` preference); teams not listed
/// sort after these, by name.
pub fn set_team_order(conn: &Connection, team_ids: &[String]) -> Result<()> {
    conn.execute("UPDATE teams SET sort_order = NULL", [])?;
    let mut statement = conn.prepare_cached("UPDATE teams SET sort_order = ?2 WHERE id = ?1")?;
    for (position, id) in team_ids.iter().enumerate() {
        statement.execute(params![id, position as i64])?;
    }
    Ok(())
}

/// Removes a team the user left, with its channels and categories.
pub fn delete_team(conn: &Connection, team_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM channel_members WHERE channel_id IN (SELECT id FROM channels WHERE team_id = ?1)",
        [team_id],
    )?;
    conn.execute("DELETE FROM channels WHERE team_id = ?1", [team_id])?;
    conn.execute("DELETE FROM categories WHERE team_id = ?1", [team_id])?;
    conn.execute("DELETE FROM teams WHERE id = ?1", [team_id])?;
    Ok(())
}

/// Inserts or replaces channels.
pub fn upsert_channels(conn: &Connection, channels: &[Channel]) -> Result<()> {
    let mut statement = conn.prepare_cached(
        "INSERT INTO channels (id, team_id, kind, name, display_name, last_post_at,
             last_root_post_at, total_msg_count, total_msg_count_root, update_at, delete_at, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (id) DO UPDATE SET
             team_id = excluded.team_id, kind = excluded.kind, name = excluded.name,
             display_name = excluded.display_name, last_post_at = excluded.last_post_at,
             last_root_post_at = excluded.last_root_post_at,
             total_msg_count = excluded.total_msg_count,
             total_msg_count_root = excluded.total_msg_count_root,
             update_at = excluded.update_at, delete_at = excluded.delete_at, data = excluded.data",
    )?;
    for channel in channels {
        statement.execute(params![
            channel.id,
            channel.team_id,
            channel.kind,
            channel.name,
            channel.display_name,
            channel.last_post_at,
            channel.last_root_post_at,
            channel.total_msg_count,
            channel.total_msg_count_root,
            channel.update_at,
            channel.delete_at,
            json(channel)
        ])?;
    }
    Ok(())
}

/// Removes a channel the user left, and their membership.
pub fn delete_channel(conn: &Connection, channel_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM channel_members WHERE channel_id = ?1",
        [channel_id],
    )?;
    conn.execute(
        "DELETE FROM category_channels WHERE channel_id = ?1",
        [channel_id],
    )?;
    conn.execute("DELETE FROM channels WHERE id = ?1", [channel_id])?;
    Ok(())
}

/// Inserts or replaces channel memberships, keeping both the root and the
/// non-root counts. A scheme admin is stored with `channel_admin` in its
/// roles, so the store needs no column for the flag.
pub fn upsert_channel_members(conn: &Connection, members: &[ChannelMember]) -> Result<()> {
    let mut statement = conn.prepare_cached(
        "INSERT INTO channel_members (channel_id, user_id, roles, last_viewed_at, msg_count,
             mention_count, msg_count_root, mention_count_root, urgent_mention_count,
             notify_props, last_update_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (channel_id, user_id) DO UPDATE SET
             roles = excluded.roles, last_viewed_at = excluded.last_viewed_at,
             msg_count = excluded.msg_count, mention_count = excluded.mention_count,
             msg_count_root = excluded.msg_count_root,
             mention_count_root = excluded.mention_count_root,
             urgent_mention_count = excluded.urgent_mention_count,
             notify_props = excluded.notify_props, last_update_at = excluded.last_update_at",
    )?;
    for member in members {
        let admin = member.scheme_admin
            && !member
                .roles
                .split_whitespace()
                .any(|r| r == "channel_admin");
        let roles = if admin {
            format!("{} channel_admin", member.roles).trim().to_owned()
        } else {
            member.roles.clone()
        };
        statement.execute(params![
            member.channel_id,
            member.user_id,
            roles,
            member.last_viewed_at,
            member.msg_count,
            member.mention_count,
            member.msg_count_root,
            member.mention_count_root,
            member.urgent_mention_count,
            json(&member.notify_props),
            member.last_update_at
        ])?;
    }
    Ok(())
}

/// Replaces a team's categories and their channel order with `categories`,
/// as `users/me/teams/{team}/channels/categories` returns them.
pub fn replace_categories(
    conn: &Connection,
    team_id: &str,
    categories: &SidebarCategories,
) -> Result<()> {
    conn.execute("DELETE FROM categories WHERE team_id = ?1", [team_id])?;
    let position = |id: &str| {
        categories
            .order
            .iter()
            .position(|ordered| ordered == id)
            .unwrap_or(categories.order.len())
    };
    for (index, category) in categories.categories.iter().enumerate() {
        // Categories missing from `order` keep their response order, after the rest.
        let order = position(&category.id) * categories.categories.len() + index;
        upsert_category(conn, team_id, category, order as i64)?;
    }
    Ok(())
}

/// Inserts or replaces one category at `sort_order`, with its channel order.
pub fn upsert_category(
    conn: &Connection,
    team_id: &str,
    category: &SidebarCategory,
    sort_order: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO categories (id, team_id, kind, display_name, sorting, collapsed, muted, sort_order)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (id) DO UPDATE SET
             team_id = excluded.team_id, kind = excluded.kind,
             display_name = excluded.display_name, sorting = excluded.sorting,
             collapsed = excluded.collapsed, muted = excluded.muted,
             sort_order = excluded.sort_order",
        params![
            category.id,
            team_id,
            category.kind,
            category.display_name,
            category.sorting,
            category.collapsed,
            category.muted,
            sort_order
        ],
    )?;
    conn.execute(
        "DELETE FROM category_channels WHERE category_id = ?1",
        [&category.id],
    )?;
    let mut statement = conn.prepare_cached(
        "INSERT OR REPLACE INTO category_channels (category_id, channel_id, sort_order)
         VALUES (?1, ?2, ?3)",
    )?;
    for (position, channel_id) in category.channel_ids.iter().enumerate() {
        statement.execute(params![category.id, channel_id, position as i64])?;
    }
    Ok(())
}

/// One stored category with its channels in order, as the server would
/// send it back; `None` when it is not stored.
pub fn category(conn: &Connection, category_id: &str) -> Result<Option<SidebarCategory>> {
    let row = conn
        .query_row(
            "SELECT team_id, kind, display_name, sorting, collapsed, muted
             FROM categories WHERE id = ?1",
            [category_id],
            |row| {
                Ok(SidebarCategory {
                    id: category_id.to_owned(),
                    team_id: row.get(0)?,
                    kind: row.get(1)?,
                    display_name: row.get(2)?,
                    sorting: row.get(3)?,
                    collapsed: row.get(4)?,
                    muted: row.get(5)?,
                    ..SidebarCategory::default()
                })
            },
        )
        .optional()?;
    let Some(mut category) = row else {
        return Ok(None);
    };
    let mut statement = conn.prepare_cached(
        "SELECT channel_id FROM category_channels WHERE category_id = ?1 ORDER BY sort_order",
    )?;
    category.channel_ids = statement
        .query_map([category_id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Some(category))
}

/// Removes a category; its channels go with it only from the category.
pub fn delete_category(conn: &Connection, category_id: &str) -> Result<()> {
    conn.execute("DELETE FROM categories WHERE id = ?1", [category_id])?;
    Ok(())
}

/// The stored users among `ids`, in no particular order; unknown ids are
/// left out.
pub fn users(conn: &Connection, ids: &[String]) -> Result<Vec<User>> {
    let mut statement = conn.prepare_cached("SELECT data FROM users WHERE id = ?1")?;
    let mut users = Vec::new();
    for id in ids {
        let data: Option<String> = statement.query_row([id], |row| row.get(0)).optional()?;
        if let Some(user) = data.and_then(|data| serde_json::from_str(&data).ok()) {
            users.push(user);
        }
    }
    Ok(users)
}

/// Inserts or replaces users.
pub fn upsert_users(conn: &Connection, users: &[User]) -> Result<()> {
    let mut statement = conn.prepare_cached(
        "INSERT INTO users (id, username, first_name, last_name, nickname, is_bot,
             last_picture_update, delete_at, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (id) DO UPDATE SET
             username = excluded.username, first_name = excluded.first_name,
             last_name = excluded.last_name, nickname = excluded.nickname,
             is_bot = excluded.is_bot, last_picture_update = excluded.last_picture_update,
             delete_at = excluded.delete_at, data = excluded.data",
    )?;
    for user in users {
        statement.execute(params![
            user.id,
            user.username,
            user.first_name,
            user.last_name,
            user.nickname,
            user.is_bot,
            user.last_picture_update,
            user.delete_at,
            json(user)
        ])?;
    }
    Ok(())
}

/// Inserts or replaces preferences.
pub fn upsert_preferences(conn: &Connection, preferences: &[Preference]) -> Result<()> {
    let mut statement = conn.prepare_cached(
        "INSERT INTO preferences (category, name, value) VALUES (?1, ?2, ?3)
         ON CONFLICT (category, name) DO UPDATE SET value = excluded.value",
    )?;
    for preference in preferences {
        statement.execute(params![
            preference.category,
            preference.name,
            preference.value
        ])?;
    }
    Ok(())
}

/// Removes preferences (`preferences_deleted`).
pub fn delete_preferences(conn: &Connection, preferences: &[Preference]) -> Result<()> {
    let mut statement =
        conn.prepare_cached("DELETE FROM preferences WHERE category = ?1 AND name = ?2")?;
    for preference in preferences {
        statement.execute(params![preference.category, preference.name])?;
    }
    Ok(())
}

/// One preference's value, if set.
pub fn preference(conn: &Connection, category: &str, name: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM preferences WHERE category = ?1 AND name = ?2",
            [category, name],
            |row| row.get(0),
        )
        .optional()?)
}

/// Inserts or replaces custom emoji.
pub fn upsert_emoji(conn: &Connection, emoji: &[Emoji]) -> Result<()> {
    let mut statement = conn.prepare_cached(
        "INSERT INTO emoji (id, name, creator_id, update_at, delete_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (id) DO UPDATE SET
             name = excluded.name, creator_id = excluded.creator_id,
             update_at = excluded.update_at, delete_at = excluded.delete_at",
    )?;
    for one in emoji {
        statement.execute(params![
            one.id,
            one.name,
            one.creator_id,
            one.update_at,
            one.delete_at
        ])?;
    }
    Ok(())
}

/// The custom emoji not deleted, by name.
pub fn custom_emoji(conn: &Connection) -> Result<Vec<Emoji>> {
    let mut statement = conn.prepare_cached(
        "SELECT id, name, creator_id, update_at FROM emoji WHERE delete_at = 0 ORDER BY name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(Emoji {
            id: row.get(0)?,
            name: row.get(1)?,
            creator_id: row.get(2)?,
            update_at: row.get(3)?,
            ..Emoji::default()
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Which counts unread math reads (PLAN §6.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnreadMode {
    /// Collapsed Reply Threads on: replies count in threads, not channels,
    /// so channels use the `*_root` fields.
    CollapsedThreads,
    /// Collapsed Reply Threads off: every post counts, from the non-root fields.
    AllPosts,
}

/// A channel's unread state for the user.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unreads {
    /// Messages since the user last viewed the channel.
    pub messages: i64,
    /// Mentions of the user in them.
    pub mentions: i64,
    /// Mentions in urgent-priority posts.
    pub urgent_mentions: i64,
}

impl Unreads {
    /// Whether the channel shows as unread.
    pub fn is_unread(&self) -> bool {
        self.messages > 0 || self.mentions > 0
    }
}

/// Unread math: total minus seen, and the member's mention count, from the
/// root fields under CRT or the non-root ones otherwise. Never negative,
/// since the two counts can arrive out of order.
pub fn unreads(channel: &Channel, member: &ChannelMember, mode: UnreadMode) -> Unreads {
    let (total, seen, mentions) = match mode {
        UnreadMode::CollapsedThreads => (
            channel.total_msg_count_root,
            member.msg_count_root,
            member.mention_count_root,
        ),
        UnreadMode::AllPosts => (
            channel.total_msg_count,
            member.msg_count,
            member.mention_count,
        ),
    };
    Unreads {
        messages: (total - seen).max(0),
        mentions: mentions.max(0),
        urgent_mentions: member.urgent_mention_count.max(0),
    }
}

/// A channel row as the sidebar shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarChannel {
    /// Channel id.
    pub id: String,
    /// `O` public, `P` private, `D` direct, `G` group.
    pub kind: String,
    /// The URL handle, such as `town-square`, which search's `in:` takes.
    pub name: String,
    /// The channel's name for display; for a direct message, the other
    /// person's username (or the user's own for a self DM).
    pub display_name: String,
    /// For a direct message, the other person's id.
    pub dm_user_id: Option<String>,
    /// For a direct message, the other person's names as stored (id,
    /// username, first and last name, nickname), so the app can name the
    /// row in the user's name format (#156). `None` until the user is known.
    pub dm_user: Option<User>,
    /// Unread and mention counts in the requested mode.
    pub unreads: Unreads,
    /// Muted: the member's `mark_unread` is `mention`, or the category is muted.
    pub muted: bool,
    /// When the last post arrived, for recency sorting.
    pub last_post_at: i64,
}

/// A category and its channels, in sidebar order.
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarSection {
    /// Category id.
    pub id: String,
    /// `favorites`, `channels`, `direct_messages` or `custom`.
    pub kind: String,
    /// Its title.
    pub display_name: String,
    /// `alpha`, `recent`, `manual` or empty (the server default).
    pub sorting: String,
    /// Collapsed in the sidebar.
    pub collapsed: bool,
    /// Muted as a whole.
    pub muted: bool,
    /// Channels in the category's order. Channels the user is not a member of
    /// or that were archived are left out.
    pub channels: Vec<SidebarChannel>,
}

const SNAPSHOT: &str = "
    SELECT cat.id, cat.kind, cat.display_name, cat.sorting, cat.collapsed, cat.muted,
           ch.id, ch.kind, ch.name, ch.display_name, ch.last_post_at,
           ch.total_msg_count, ch.total_msg_count_root,
           m.msg_count, m.mention_count, m.msg_count_root, m.mention_count_root,
           m.urgent_mention_count, m.notify_props,
           u.username, u.first_name, u.last_name, u.nickname
    FROM categories cat
    LEFT JOIN category_channels cc ON cc.category_id = cat.id
    LEFT JOIN channels ch ON ch.id = cc.channel_id AND ch.delete_at = 0
    LEFT JOIN channel_members m ON m.channel_id = ch.id AND m.user_id = ?2
    LEFT JOIN users u ON ch.kind = 'D' AND u.id = (
        CASE
            WHEN ch.name = ?2 || '__' || ?2 THEN ?2
            WHEN substr(ch.name, 1, length(?2) + 2) = ?2 || '__'
                THEN substr(ch.name, length(?2) + 3)
            ELSE substr(ch.name, 1, instr(ch.name, '__') - 1)
        END)
    WHERE cat.team_id = ?1
    ORDER BY cat.sort_order, cat.id, cc.sort_order";

/// The preference (category, name) that shows a direct or group
/// conversation in the sidebar; Mattermost hides one the user closed by
/// setting it to `false`. `None` for other channels.
pub fn show_preference(
    kind: &str,
    channel_id: &str,
    channel_name: &str,
    me: &str,
) -> Option<(&'static str, String)> {
    match kind {
        "D" => Some(("direct_channel_show", dm_partner(channel_name, me)?)),
        "G" => Some(("group_channel_show", channel_id.to_owned())),
        _ => None,
    }
}

/// The other person in a direct channel named `a__b`.
fn dm_partner(name: &str, me: &str) -> Option<String> {
    let (first, second) = name.split_once("__")?;
    Some(if first == me { second } else { first }.to_owned())
}

/// The team's sidebar: categories in order with their channels, unread and
/// mention counts computed for `mode`, muted flags and direct-message names.
pub fn sidebar_snapshot(
    conn: &Connection,
    team_id: &str,
    user_id: &str,
    mode: UnreadMode,
) -> Result<Vec<SidebarSection>> {
    let mut statement = conn.prepare_cached(SNAPSHOT)?;
    let mut rows = statement.query(params![team_id, user_id])?;
    let mut sections: Vec<SidebarSection> = Vec::new();
    while let Some(row) = rows.next()? {
        let category_id: String = row.get(0)?;
        if sections.last().map(|s| &s.id) != Some(&category_id) {
            sections.push(SidebarSection {
                id: category_id,
                kind: row.get(1)?,
                display_name: row.get(2)?,
                sorting: row.get(3)?,
                collapsed: row.get(4)?,
                muted: row.get(5)?,
                channels: Vec::new(),
            });
        }
        let Some(section) = sections.last_mut() else {
            continue;
        };
        let Some(channel_id) = row.get::<_, Option<String>>(6)? else {
            continue;
        };
        // Not a member (left, or not loaded yet): not in the sidebar.
        let Some(msg_count) = row.get::<_, Option<i64>>(13)? else {
            continue;
        };
        let kind: String = row.get(7)?;
        let name: String = row.get(8)?;
        // Closed by the user, until they open it again.
        if let Some((category, key)) = show_preference(&kind, &channel_id, &name, user_id)
            && preference(conn, category, &key)?.as_deref() == Some("false")
        {
            continue;
        }
        let channel = Channel {
            total_msg_count: row.get(11)?,
            total_msg_count_root: row.get(12)?,
            ..Channel::default()
        };
        let notify_props: String = row.get(18)?;
        let member = ChannelMember {
            msg_count,
            mention_count: row.get(14)?,
            msg_count_root: row.get(15)?,
            mention_count_root: row.get(16)?,
            urgent_mention_count: row.get(17)?,
            notify_props: serde_json::from_str(&notify_props).unwrap_or_default(),
            ..ChannelMember::default()
        };
        let dm_user_id = (kind == "D").then(|| dm_partner(&name, user_id)).flatten();
        let username = row.get::<_, Option<String>>(19)?;
        let dm_user = match (&dm_user_id, &username) {
            (Some(id), Some(username)) => Some(User {
                id: id.clone(),
                username: username.clone(),
                first_name: row.get(20)?,
                last_name: row.get(21)?,
                nickname: row.get(22)?,
                ..User::default()
            }),
            _ => None,
        };
        let display_name = match username {
            Some(username) => username,
            None => row.get(9)?,
        };
        section.channels.push(SidebarChannel {
            id: channel_id,
            muted: section.muted || member.is_muted(),
            unreads: unreads(&channel, &member, mode),
            kind,
            name,
            display_name,
            dm_user_id,
            dm_user,
            last_post_at: row.get(10)?,
        });
    }
    Ok(sections)
}

/// A team in the rail with its badge.
#[derive(Clone, Debug, PartialEq)]
pub struct TeamWithUnreads {
    /// Team id.
    pub id: String,
    /// URL name.
    pub name: String,
    /// Shown name.
    pub display_name: String,
    /// Some unmuted channel in the team is unread.
    pub has_unreads: bool,
    /// Mentions across the team's channels, muted ones included, as the
    /// webapp counts them.
    pub mentions: i64,
    /// The team's `last_team_icon_update`: 0 when it has no icon, else the
    /// version of the icon to show on the rail (#89).
    pub icon: i64,
}

/// Every team the user is on, in rail order, with unread state from its
/// channels (direct messages belong to no team and are not counted).
pub fn teams_with_unreads(
    conn: &Connection,
    user_id: &str,
    mode: UnreadMode,
) -> Result<Vec<TeamWithUnreads>> {
    let mut teams = conn.prepare_cached(
        "SELECT id, name, display_name, data FROM teams WHERE delete_at = 0
         ORDER BY sort_order IS NULL, sort_order, display_name COLLATE NOCASE, id",
    )?;
    let mut result: Vec<TeamWithUnreads> = teams
        .query_map([], |row| {
            let data: String = row.get(3)?;
            let icon = serde_json::from_str::<Team>(&data)
                .map(|team| team.last_team_icon_update)
                .unwrap_or_default();
            Ok(TeamWithUnreads {
                id: row.get(0)?,
                name: row.get(1)?,
                display_name: row.get(2)?,
                has_unreads: false,
                mentions: 0,
                icon,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut channels = conn.prepare_cached(
        "SELECT ch.team_id, ch.total_msg_count, ch.total_msg_count_root,
                m.msg_count, m.mention_count, m.msg_count_root, m.mention_count_root,
                m.notify_props
         FROM channels ch JOIN channel_members m ON m.channel_id = ch.id AND m.user_id = ?1
         WHERE ch.delete_at = 0 AND ch.team_id != ''",
    )?;
    let mut rows = channels.query([user_id])?;
    while let Some(row) = rows.next()? {
        let team_id: String = row.get(0)?;
        let Some(team) = result.iter_mut().find(|team| team.id == team_id) else {
            continue;
        };
        let channel = Channel {
            total_msg_count: row.get(1)?,
            total_msg_count_root: row.get(2)?,
            ..Channel::default()
        };
        let notify_props: String = row.get(7)?;
        let member = ChannelMember {
            msg_count: row.get(3)?,
            mention_count: row.get(4)?,
            msg_count_root: row.get(5)?,
            mention_count_root: row.get(6)?,
            notify_props: serde_json::from_str(&notify_props).unwrap_or_default(),
            ..ChannelMember::default()
        };
        let unread = unreads(&channel, &member, mode);
        team.mentions += unread.mentions;
        team.has_unreads |= unread.messages > 0 && !member.is_muted();
    }
    Ok(result)
}

// Live updates from WebSocket events (#36).

/// A cached channel: what the server last sent, with the counts and dates
/// events have moved since.
pub fn channel(conn: &Connection, channel_id: &str) -> Result<Option<Channel>> {
    let channel = conn
        .prepare_cached(
            "SELECT data, team_id, last_post_at, last_root_post_at, total_msg_count,
                    total_msg_count_root, delete_at
             FROM channels WHERE id = ?1",
        )?
        .query_row([channel_id], |row| {
            let data: String = row.get(0)?;
            let mut channel: Channel = serde_json::from_str(&data).unwrap_or_default();
            channel.id = channel_id.to_owned();
            channel.team_id = row.get(1)?;
            channel.last_post_at = row.get(2)?;
            channel.last_root_post_at = row.get(3)?;
            channel.total_msg_count = row.get(4)?;
            channel.total_msg_count_root = row.get(5)?;
            channel.delete_at = row.get(6)?;
            Ok(channel)
        })
        .optional()?;
    Ok(channel)
}

/// A cached channel by its team and URL name, as channel links name it.
pub fn channel_by_name(conn: &Connection, team_id: &str, name: &str) -> Result<Option<Channel>> {
    let id: Option<String> = conn
        .prepare_cached("SELECT id FROM channels WHERE team_id = ?1 AND name = ?2")?
        .query_row([team_id, name], |row| row.get(0))
        .optional()?;
    match id {
        Some(id) => channel(conn, &id),
        None => Ok(None),
    }
}

/// A cached user by username, ignoring case as Mattermost does.
pub fn user_by_username(conn: &Connection, username: &str) -> Result<Option<User>> {
    let data: Option<String> = conn
        .prepare_cached("SELECT data FROM users WHERE username = ?1 COLLATE NOCASE")?
        .query_row([username], |row| row.get(0))
        .optional()?;
    Ok(data.and_then(|data| serde_json::from_str(&data).ok()))
}

/// A user's cached membership of a channel.
pub fn channel_member(
    conn: &Connection,
    channel_id: &str,
    user_id: &str,
) -> Result<Option<ChannelMember>> {
    let member = conn
        .prepare_cached(
            "SELECT roles, last_viewed_at, msg_count, mention_count, msg_count_root,
                    mention_count_root, urgent_mention_count, notify_props, last_update_at
             FROM channel_members WHERE channel_id = ?1 AND user_id = ?2",
        )?
        .query_row([channel_id, user_id], |row| {
            let notify_props: String = row.get(7)?;
            Ok(ChannelMember {
                channel_id: channel_id.to_owned(),
                user_id: user_id.to_owned(),
                roles: row.get(0)?,
                last_viewed_at: row.get(1)?,
                msg_count: row.get(2)?,
                mention_count: row.get(3)?,
                msg_count_root: row.get(4)?,
                mention_count_root: row.get(5)?,
                urgent_mention_count: row.get(6)?,
                notify_props: serde_json::from_str(&notify_props).unwrap_or_default(),
                last_update_at: row.get(8)?,
                // Stored as `channel_admin` in `roles`.
                scheme_admin: false,
            })
        })
        .optional()?;
    Ok(member)
}

/// A new post, as the counts see it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NewPost {
    /// When it was created.
    pub create_at: i64,
    /// A root post rather than a reply.
    pub is_root: bool,
    /// The user wrote it, so it is read.
    pub by_me: bool,
    /// It mentions the user (always true in a direct message).
    pub mentions_me: bool,
    /// It is an urgent-priority mention.
    pub urgent: bool,
}

/// Counts a new post in its channel, as the server does: the channel's
/// totals and last post time, and the user's read count when they wrote it
/// or mention count when it mentions them. `false` when the channel is not
/// cached.
pub fn count_post(
    conn: &Connection,
    channel_id: &str,
    user_id: &str,
    post: &NewPost,
) -> Result<bool> {
    let root = i64::from(post.is_root);
    let changed = conn
        .prepare_cached(
            "UPDATE channels SET
                 total_msg_count = total_msg_count + 1,
                 total_msg_count_root = total_msg_count_root + ?2,
                 last_post_at = MAX(last_post_at, ?3),
                 last_root_post_at = CASE WHEN ?2 = 1 THEN MAX(last_root_post_at, ?3)
                                          ELSE last_root_post_at END
             WHERE id = ?1",
        )?
        .execute(params![channel_id, root, post.create_at])?;
    if changed == 0 {
        return Ok(false);
    }
    if post.by_me {
        conn.prepare_cached(
            "UPDATE channel_members SET msg_count = msg_count + 1,
                 msg_count_root = msg_count_root + ?3,
                 last_viewed_at = MAX(last_viewed_at, ?4)
             WHERE channel_id = ?1 AND user_id = ?2",
        )?
        .execute(params![channel_id, user_id, root, post.create_at])?;
    } else if post.mentions_me {
        conn.prepare_cached(
            "UPDATE channel_members SET mention_count = mention_count + 1,
                 mention_count_root = mention_count_root + ?3,
                 urgent_mention_count = urgent_mention_count + ?4
             WHERE channel_id = ?1 AND user_id = ?2",
        )?
        .execute(params![channel_id, user_id, root, i64::from(post.urgent)])?;
    }
    Ok(true)
}

/// The user read the channel up to `viewed_at` (`channel_viewed`,
/// `multiple_channels_viewed`): everything in it counts as seen.
pub fn mark_viewed(
    conn: &Connection,
    channel_id: &str,
    user_id: &str,
    viewed_at: i64,
) -> Result<()> {
    conn.prepare_cached(
        "UPDATE channel_members SET
             msg_count = (SELECT total_msg_count FROM channels WHERE id = ?1),
             msg_count_root = (SELECT total_msg_count_root FROM channels WHERE id = ?1),
             mention_count = 0, mention_count_root = 0, urgent_mention_count = 0,
             last_viewed_at = MAX(last_viewed_at, ?3)
         WHERE channel_id = ?1 AND user_id = ?2
           AND EXISTS (SELECT 1 FROM channels WHERE id = ?1)",
    )?
    .execute(params![channel_id, user_id, viewed_at])?;
    Ok(())
}

/// A read position the server computed (`post_unread`, after marking a post
/// unread).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadState {
    /// The new last-viewed time.
    pub last_viewed_at: i64,
    /// Messages seen.
    pub msg_count: i64,
    /// Root messages seen.
    pub msg_count_root: i64,
    /// Unread mentions.
    pub mention_count: i64,
    /// Unread mentions in root posts.
    pub mention_count_root: i64,
    /// Unread urgent mentions.
    pub urgent_mention_count: i64,
}

/// Sets the user's read position in a channel to what the server says.
pub fn set_read_state(
    conn: &Connection,
    channel_id: &str,
    user_id: &str,
    state: &ReadState,
) -> Result<()> {
    conn.prepare_cached(
        "UPDATE channel_members SET last_viewed_at = ?3, msg_count = ?4, msg_count_root = ?5,
             mention_count = ?6, mention_count_root = ?7, urgent_mention_count = ?8
         WHERE channel_id = ?1 AND user_id = ?2",
    )?
    .execute(params![
        channel_id,
        user_id,
        state.last_viewed_at,
        state.msg_count,
        state.msg_count_root,
        state.mention_count,
        state.mention_count_root,
        state.urgent_mention_count
    ])?;
    Ok(())
}

/// Archives a channel (`channel_deleted`), or restores it with 0. Archived
/// channels leave the sidebar but keep their posts.
pub fn set_channel_deleted(conn: &Connection, channel_id: &str, delete_at: i64) -> Result<()> {
    conn.prepare_cached("UPDATE channels SET delete_at = ?2 WHERE id = ?1")?
        .execute(params![channel_id, delete_at])?;
    Ok(())
}

/// A category's place in its team's order, if cached.
pub fn category_sort_order(conn: &Connection, category_id: &str) -> Result<Option<i64>> {
    Ok(conn
        .prepare_cached("SELECT sort_order FROM categories WHERE id = ?1")?
        .query_row([category_id], |row| row.get(0))
        .optional()?)
}

/// A team's category ids in sidebar order.
pub fn category_order(conn: &Connection, team_id: &str) -> Result<Vec<String>> {
    let mut statement =
        conn.prepare_cached("SELECT id FROM categories WHERE team_id = ?1 ORDER BY sort_order")?;
    let ids = statement
        .query_map([team_id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// Reorders a team's categories (`sidebar_category_order_updated`).
/// Categories not listed keep their relative order after the listed ones.
pub fn set_category_order(conn: &Connection, team_id: &str, order: &[String]) -> Result<()> {
    let listed = i64::try_from(order.len()).unwrap_or(i64::MAX);
    conn.prepare_cached("UPDATE categories SET sort_order = ?2 + sort_order WHERE team_id = ?1")?
        .execute(params![team_id, listed])?;
    let mut statement = conn
        .prepare_cached("UPDATE categories SET sort_order = ?3 WHERE id = ?1 AND team_id = ?2")?;
    for (position, id) in order.iter().enumerate() {
        statement.execute(params![id, team_id, position as i64])?;
    }
    Ok(())
}
