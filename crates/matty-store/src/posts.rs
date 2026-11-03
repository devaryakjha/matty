//! Posts, threads, reactions, files, drafts and sync cursors.
//!
//! Posts come back as [`Post`] with `metadata.reactions` and `metadata.files`
//! filled from their own tables, which are the source of truth once stored:
//! `reaction_added` and friends change the tables, not the post's JSON.

use std::collections::HashMap;

use matty_api::models::{Draft, FileInfo, Post, PostMetadata, Reaction, UserThread};
use rusqlite::{Connection, OptionalExtension as _, Row, params};

use crate::Result;

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned())
}

/// The local clock in ms, for `touched_at`.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Marks a channel's cached posts as just viewed, so eviction trims it last.
pub fn touch_channel(conn: &Connection, channel_id: &str) -> Result<()> {
    conn.prepare_cached("UPDATE posts SET touched_at = ?2 WHERE channel_id = ?1")?
        .execute(params![channel_id, now_ms()])?;
    Ok(())
}

fn from_json<T: serde::de::DeserializeOwned + Default>(text: &str) -> T {
    serde_json::from_str(text).unwrap_or_default()
}

/// Inserts or updates posts in one go, with their reactions and files.
///
/// A post that carries a `pending_post_id` replaces the optimistic row with
/// that id (see [`replace_pending`]), whichever of the REST response or the
/// WebSocket `posted` event arrives first, so a send never shows twice.
/// Reactions and files are taken from `metadata` and replace what was stored
/// for the post. Call inside a transaction for batches.
pub fn insert_posts(conn: &Connection, posts: &[Post]) -> Result<()> {
    let mut drop_pending = conn
        .prepare_cached("DELETE FROM posts WHERE (pending_post_id = ?1 OR id = ?1) AND id != ?2")?;
    let mut upsert = conn.prepare_cached(
        "INSERT INTO posts (id, channel_id, user_id, root_id, create_at, update_at, edit_at,
             delete_at, kind, message, reply_count, is_pinned, pending_post_id, props, metadata,
             touched_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT (id) DO UPDATE SET
             channel_id = excluded.channel_id, user_id = excluded.user_id,
             root_id = excluded.root_id, create_at = excluded.create_at,
             update_at = excluded.update_at, edit_at = excluded.edit_at,
             delete_at = excluded.delete_at, kind = excluded.kind,
             message = excluded.message, reply_count = excluded.reply_count,
             is_pinned = excluded.is_pinned, pending_post_id = excluded.pending_post_id,
             props = excluded.props, metadata = excluded.metadata,
             touched_at = excluded.touched_at",
    )?;
    let now = now_ms();
    let mut clear_reactions = conn.prepare_cached("DELETE FROM reactions WHERE post_id = ?1")?;
    let mut clear_files = conn.prepare_cached("DELETE FROM files WHERE post_id = ?1")?;
    for post in posts {
        if !post.pending_post_id.is_empty() {
            // The optimistic row's id is the pending id itself.
            drop_pending.execute(params![post.pending_post_id, post.id])?;
        }
        // Reactions and files live in their tables; keep the rest of metadata.
        let rest = PostMetadata {
            reactions: Vec::new(),
            files: Vec::new(),
            ..post.metadata.clone()
        };
        upsert.execute(params![
            post.id,
            post.channel_id,
            post.user_id,
            post.root_id,
            post.create_at,
            post.update_at,
            post.edit_at,
            post.delete_at,
            post.kind,
            post.message,
            post.reply_count,
            post.is_pinned,
            post.pending_post_id,
            json(&post.props),
            json(&rest),
            now
        ])?;
        clear_reactions.execute([&post.id])?;
        clear_files.execute([&post.id])?;
        if post.delete_at == 0 {
            for reaction in &post.metadata.reactions {
                add_reaction(
                    conn,
                    &Reaction {
                        post_id: post.id.clone(),
                        ..reaction.clone()
                    },
                )?;
            }
            upsert_files(conn, &post.metadata.files)?;
        }
    }
    Ok(())
}

/// Swaps the optimistic row `pending_post_id` for the server's `post`. The
/// stored row is updated in place and keeps its place in the channel: the
/// server's `create_at` is the moment it accepted the send, at or just after
/// the local one, so nothing else sorts between them.
pub fn replace_pending(conn: &Connection, pending_post_id: &str, post: &Post) -> Result<()> {
    let post = Post {
        pending_post_id: pending_post_id.to_owned(),
        ..post.clone()
    };
    if post.id == pending_post_id {
        return insert_posts(conn, &[post]);
    }
    // Rename the optimistic row first so it keeps its rowid, unless the server
    // row already arrived (the WebSocket beat the REST response).
    let exists: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM posts WHERE id = ?1)",
        [&post.id],
        |row| row.get(0),
    )?;
    if !exists {
        // Reactions point at the old id; the server's post brings its own.
        conn.execute(
            "DELETE FROM reactions WHERE post_id = ?1",
            [pending_post_id],
        )?;
        conn.execute(
            "UPDATE posts SET id = ?2 WHERE id = ?1",
            params![pending_post_id, post.id],
        )?;
    }
    insert_posts(conn, &[post])
}

/// Marks a post deleted (`post_deleted`), clearing its text, reactions and
/// files. It stays so a `since` catch-up does not bring it back.
pub fn mark_deleted(conn: &Connection, post_id: &str, delete_at: i64) -> Result<()> {
    conn.execute(
        "UPDATE posts SET delete_at = ?2, message = '', props = '{}', metadata = '{}'
         WHERE id = ?1",
        params![post_id, delete_at.max(1)],
    )?;
    conn.execute("DELETE FROM reactions WHERE post_id = ?1", [post_id])?;
    conn.execute("DELETE FROM files WHERE post_id = ?1", [post_id])?;
    Ok(())
}

/// Which slice of a channel to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    /// The newest posts.
    Latest,
    /// Posts created strictly before this time, newest first, for scrolling up.
    Before(i64),
    /// Posts created strictly after this time, oldest first, for scrolling down.
    After(i64),
}

const POST_COLUMNS: &str = "id, channel_id, user_id, root_id, create_at, update_at, edit_at,
    delete_at, kind, message, reply_count, is_pinned, pending_post_id, props, metadata";

fn post_from_row(row: &Row<'_>) -> rusqlite::Result<Post> {
    let props: String = row.get(13)?;
    let metadata: String = row.get(14)?;
    Ok(Post {
        id: row.get(0)?,
        channel_id: row.get(1)?,
        user_id: row.get(2)?,
        root_id: row.get(3)?,
        create_at: row.get(4)?,
        update_at: row.get(5)?,
        edit_at: row.get(6)?,
        delete_at: row.get(7)?,
        kind: row.get(8)?,
        message: row.get(9)?,
        reply_count: row.get(10)?,
        is_pinned: row.get(11)?,
        pending_post_id: row.get(12)?,
        props: from_json(&props),
        metadata: from_json(&metadata),
        ..Post::default()
    })
}

/// Fills `metadata.reactions` and `metadata.files` for `posts` with one query
/// per table.
fn attach(conn: &Connection, posts: &mut [Post]) -> Result<()> {
    if posts.is_empty() {
        return Ok(());
    }
    let ids = json(&posts.iter().map(|p| p.id.as_str()).collect::<Vec<_>>());
    let index: HashMap<String, usize> = posts
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.clone(), i))
        .collect();
    let mut reactions = conn.prepare_cached(
        "SELECT post_id, user_id, emoji_name, create_at FROM reactions
         WHERE post_id IN (SELECT value FROM json_each(?1))
         ORDER BY create_at, user_id, emoji_name",
    )?;
    let mut rows = reactions.query([&ids])?;
    while let Some(row) = rows.next()? {
        let post_id: String = row.get(0)?;
        if let Some(&i) = index.get(&post_id) {
            posts[i].metadata.reactions.push(Reaction {
                post_id,
                user_id: row.get(1)?,
                emoji_name: row.get(2)?,
                create_at: row.get(3)?,
            });
        }
    }
    let mut files = conn.prepare_cached(&format!(
        "SELECT {FILE_COLUMNS} FROM files
         WHERE post_id IN (SELECT value FROM json_each(?1)) AND delete_at = 0
         ORDER BY create_at, id"
    ))?;
    let mut rows = files.query([&ids])?;
    while let Some(row) = rows.next()? {
        let file = file_from_row(row)?;
        if let Some(&i) = index.get(&file.post_id) {
            posts[i].metadata.files.push(file);
        }
    }
    Ok(())
}

/// Up to `limit` live posts of a channel in `create_at` order (oldest first),
/// with their reactions and files. With `roots_only`, replies are left out
/// (the channel view under Collapsed Reply Threads).
pub fn posts_window(
    conn: &Connection,
    channel_id: &str,
    window: Window,
    limit: usize,
    roots_only: bool,
) -> Result<Vec<Post>> {
    let (bound, order, time) = match window {
        Window::Latest => ("1", "DESC", 0),
        Window::Before(time) => ("create_at < ?3", "DESC", time),
        Window::After(time) => ("create_at > ?3", "ASC", time),
    };
    let roots = if roots_only { "AND root_id = ''" } else { "" };
    let sql = format!(
        "SELECT {POST_COLUMNS} FROM posts
         WHERE channel_id = ?1 AND delete_at = 0 {roots} AND {bound}
         ORDER BY create_at {order}, id {order} LIMIT ?2"
    );
    let mut statement = conn.prepare_cached(&sql)?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = if matches!(window, Window::Latest) {
        statement.query_map(params![channel_id, limit], post_from_row)?
    } else {
        statement.query_map(params![channel_id, limit, time], post_from_row)?
    };
    let mut posts: Vec<Post> = rows.collect::<rusqlite::Result<_>>()?;
    if order == "DESC" {
        posts.reverse();
    }
    attach(conn, &mut posts)?;
    Ok(posts)
}

/// One post with its reactions and files, deleted or not.
pub fn post(conn: &Connection, post_id: &str) -> Result<Option<Post>> {
    let mut statement =
        conn.prepare_cached(&format!("SELECT {POST_COLUMNS} FROM posts WHERE id = ?1"))?;
    let Some(post) = statement.query_row([post_id], post_from_row).optional()? else {
        return Ok(None);
    };
    let mut posts = [post];
    attach(conn, &mut posts)?;
    let [post] = posts;
    Ok(Some(post))
}

/// A thread: the root then its live replies, oldest first, with reactions and
/// files. Empty when the root is not cached.
pub fn thread(conn: &Connection, root_id: &str) -> Result<Vec<Post>> {
    let mut statement = conn.prepare_cached(&format!(
        "SELECT {POST_COLUMNS} FROM posts
         WHERE (id = ?1 OR root_id = ?1) AND delete_at = 0
         ORDER BY root_id != '', create_at, id"
    ))?;
    let mut posts: Vec<Post> = statement
        .query_map([root_id], post_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    if posts.first().is_none_or(|first| first.id != root_id) {
        return Ok(Vec::new());
    }
    attach(conn, &mut posts)?;
    Ok(posts)
}

/// Records a reaction (`reaction_added`); repeating it changes nothing.
/// Ignored when the post is not cached.
pub fn add_reaction(conn: &Connection, reaction: &Reaction) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO reactions (post_id, user_id, emoji_name, create_at)
         SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM posts WHERE id = ?1)
         ON CONFLICT (post_id, user_id, emoji_name) DO NOTHING",
    )?
    .execute(params![
        reaction.post_id,
        reaction.user_id,
        reaction.emoji_name,
        reaction.create_at
    ])?;
    Ok(())
}

/// Removes a reaction (`reaction_removed`).
pub fn remove_reaction(conn: &Connection, reaction: &Reaction) -> Result<()> {
    conn.prepare_cached(
        "DELETE FROM reactions WHERE post_id = ?1 AND user_id = ?2 AND emoji_name = ?3",
    )?
    .execute(params![
        reaction.post_id,
        reaction.user_id,
        reaction.emoji_name
    ])?;
    Ok(())
}

/// A post's reactions, oldest first.
pub fn reactions(conn: &Connection, post_id: &str) -> Result<Vec<Reaction>> {
    let mut statement = conn.prepare_cached(
        "SELECT post_id, user_id, emoji_name, create_at FROM reactions
         WHERE post_id = ?1 ORDER BY create_at, user_id, emoji_name",
    )?;
    let reactions = statement
        .query_map([post_id], |row| {
            Ok(Reaction {
                post_id: row.get(0)?,
                user_id: row.get(1)?,
                emoji_name: row.get(2)?,
                create_at: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(reactions)
}

const FILE_COLUMNS: &str = "id, post_id, channel_id, name, extension, size, mime_type, width,
    height, has_preview, create_at, delete_at";

fn file_from_row(row: &Row<'_>) -> rusqlite::Result<FileInfo> {
    Ok(FileInfo {
        id: row.get(0)?,
        post_id: row.get(1)?,
        channel_id: row.get(2)?,
        name: row.get(3)?,
        extension: row.get(4)?,
        size: row.get(5)?,
        mime_type: row.get(6)?,
        width: row.get(7)?,
        height: row.get(8)?,
        has_preview_image: row.get(9)?,
        create_at: row.get(10)?,
        delete_at: row.get(11)?,
        ..FileInfo::default()
    })
}

/// Inserts or replaces file metadata.
pub fn upsert_files(conn: &Connection, files: &[FileInfo]) -> Result<()> {
    let mut statement = conn.prepare_cached(&format!(
        "INSERT OR REPLACE INTO files ({FILE_COLUMNS})
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
    ))?;
    for file in files {
        statement.execute(params![
            file.id,
            file.post_id,
            file.channel_id,
            file.name,
            file.extension,
            file.size,
            file.mime_type,
            file.width,
            file.height,
            file.has_preview_image,
            file.create_at,
            file.delete_at
        ])?;
    }
    Ok(())
}

/// A post's files, in upload order.
pub fn files_for_post(conn: &Connection, post_id: &str) -> Result<Vec<FileInfo>> {
    let mut statement = conn.prepare_cached(&format!(
        "SELECT {FILE_COLUMNS} FROM files WHERE post_id = ?1 AND delete_at = 0
         ORDER BY create_at, id"
    ))?;
    let files = statement
        .query_map([post_id], file_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(files)
}

/// Saves a draft for a channel (`root_id` empty) or a thread. An empty draft
/// with no files deletes it.
pub fn upsert_draft(conn: &Connection, draft: &Draft) -> Result<()> {
    if draft.message.is_empty() && draft.file_ids.is_empty() {
        return delete_draft(conn, &draft.channel_id, &draft.root_id);
    }
    conn.prepare_cached(
        "INSERT INTO drafts (channel_id, root_id, message, file_ids, props, update_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (channel_id, root_id) DO UPDATE SET
             message = excluded.message, file_ids = excluded.file_ids,
             props = excluded.props, update_at = excluded.update_at
         WHERE excluded.update_at >= drafts.update_at",
    )?
    .execute(params![
        draft.channel_id,
        draft.root_id,
        draft.message,
        json(&draft.file_ids),
        json(&draft.props),
        draft.update_at
    ])?;
    Ok(())
}

/// The draft for a channel or thread.
pub fn draft(conn: &Connection, channel_id: &str, root_id: &str) -> Result<Option<Draft>> {
    let draft = conn
        .prepare_cached(
            "SELECT message, file_ids, props, update_at FROM drafts
             WHERE channel_id = ?1 AND root_id = ?2",
        )?
        .query_row([channel_id, root_id], |row| {
            let file_ids: String = row.get(1)?;
            let props: String = row.get(2)?;
            Ok(Draft {
                channel_id: channel_id.to_owned(),
                root_id: root_id.to_owned(),
                message: row.get(0)?,
                file_ids: from_json(&file_ids),
                props: from_json(&props),
                update_at: row.get(3)?,
                ..Draft::default()
            })
        })
        .optional()?;
    Ok(draft)
}

/// Removes a draft (`draft_deleted`, or the message was sent).
pub fn delete_draft(conn: &Connection, channel_id: &str, root_id: &str) -> Result<()> {
    conn.prepare_cached("DELETE FROM drafts WHERE channel_id = ?1 AND root_id = ?2")?
        .execute([channel_id, root_id])?;
    Ok(())
}

/// A followed thread as the Threads view lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadSummary {
    /// The root post's id.
    pub id: String,
    /// The root post's channel.
    pub channel_id: String,
    /// Replies in the thread.
    pub reply_count: i64,
    /// When the last reply arrived.
    pub last_reply_at: i64,
    /// When the user last read it.
    pub last_viewed_at: i64,
    /// Replies since then.
    pub unread_replies: i64,
    /// Mentions of the user in them.
    pub unread_mentions: i64,
    /// Ids of people who replied.
    pub participants: Vec<String>,
}

/// Inserts or replaces followed threads from `users/{id}/teams/{team}/threads`
/// (or `thread_updated`), and stores each root post.
pub fn upsert_threads(conn: &Connection, team_id: &str, threads: &[UserThread]) -> Result<()> {
    let mut statement = conn.prepare_cached(
        "INSERT INTO threads (id, team_id, channel_id, reply_count, last_reply_at,
             last_viewed_at, unread_replies, unread_mentions, is_following, participants, delete_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10)
         ON CONFLICT (id) DO UPDATE SET
             team_id = excluded.team_id, channel_id = excluded.channel_id,
             reply_count = excluded.reply_count, last_reply_at = excluded.last_reply_at,
             last_viewed_at = excluded.last_viewed_at,
             unread_replies = excluded.unread_replies,
             unread_mentions = excluded.unread_mentions, is_following = 1,
             participants = excluded.participants, delete_at = excluded.delete_at",
    )?;
    for thread in threads {
        let participants: Vec<&str> = thread.participants.iter().map(|u| u.id.as_str()).collect();
        statement.execute(params![
            thread.id,
            team_id,
            thread.post.channel_id,
            thread.reply_count,
            thread.last_reply_at,
            thread.last_viewed_at,
            thread.unread_replies,
            thread.unread_mentions,
            json(&participants),
            thread.delete_at
        ])?;
        if !thread.post.id.is_empty() {
            insert_posts(conn, std::slice::from_ref(&thread.post))?;
        }
    }
    Ok(())
}

/// Records that the user read a thread up to `viewed_at` (`thread_read_changed`).
pub fn mark_thread_read(
    conn: &Connection,
    thread_id: &str,
    viewed_at: i64,
    unread_replies: i64,
    unread_mentions: i64,
) -> Result<()> {
    conn.prepare_cached(
        "UPDATE threads SET last_viewed_at = ?2, unread_replies = ?3, unread_mentions = ?4
         WHERE id = ?1",
    )?
    .execute(params![
        thread_id,
        viewed_at,
        unread_replies,
        unread_mentions
    ])?;
    Ok(())
}

/// Stops listing a thread the user unfollowed (`thread_follow_changed`).
pub fn set_following(conn: &Connection, thread_id: &str, following: bool) -> Result<()> {
    conn.prepare_cached("UPDATE threads SET is_following = ?2 WHERE id = ?1")?
        .execute(params![thread_id, following])?;
    Ok(())
}

/// Up to `limit` followed threads in a team, latest reply first, optionally
/// only those with unread replies.
pub fn threads(
    conn: &Connection,
    team_id: &str,
    unread_only: bool,
    limit: usize,
) -> Result<Vec<ThreadSummary>> {
    let unread = if unread_only {
        "AND unread_replies > 0"
    } else {
        ""
    };
    let mut statement = conn.prepare_cached(&format!(
        "SELECT id, channel_id, reply_count, last_reply_at, last_viewed_at, unread_replies,
                unread_mentions, participants
         FROM threads
         WHERE team_id = ?1 AND is_following = 1 AND delete_at = 0 {unread}
         ORDER BY last_reply_at DESC, id LIMIT ?2"
    ))?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let threads = statement
        .query_map(params![team_id, limit], |row| {
            let participants: String = row.get(7)?;
            Ok(ThreadSummary {
                id: row.get(0)?,
                channel_id: row.get(1)?,
                reply_count: row.get(2)?,
                last_reply_at: row.get(3)?,
                last_viewed_at: row.get(4)?,
                unread_replies: row.get(5)?,
                unread_mentions: row.get(6)?,
                participants: from_json(&participants),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(threads)
}

/// The key under which a channel's `since` cursor is stored.
pub fn channel_since_key(channel_id: &str) -> String {
    format!("posts_since:{channel_id}")
}

/// The key for the WebSocket resume state.
pub const WS_RESUME_KEY: &str = "ws_resume";

/// A sync cursor's value.
pub fn cursor(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .prepare_cached("SELECT value FROM sync_cursors WHERE key = ?1")?
        .query_row([key], |row| row.get(0))
        .optional()?)
}

/// Sets a sync cursor.
pub fn set_cursor(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO sync_cursors (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )?
    .execute([key, value])?;
    Ok(())
}

/// Moves a `since` cursor forward, never back: server timestamps only grow,
/// and an out-of-order write must not make the next catch-up miss posts.
pub fn advance_since(conn: &Connection, key: &str, at: i64) -> Result<i64> {
    conn.prepare_cached(
        "INSERT INTO sync_cursors (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = MAX(CAST(value AS INTEGER), CAST(excluded.value AS INTEGER))",
    )?
    .execute(params![key, at.to_string()])?;
    let value = cursor(conn, key)?.unwrap_or_default();
    Ok(value.parse().unwrap_or(at))
}
