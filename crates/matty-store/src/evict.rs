//! Keeping the cache under its size cap (PLAN §6.5).
//!
//! Media goes first, least recently used first. If that is not enough, post
//! history is trimmed to the newest posts of each channel, channels viewed
//! longest ago first. Drafts and the outbox are never touched, nor are the
//! channels the caller protects (the open one), followed thread roots, or
//! posts still waiting in the outbox.

use rusqlite::{Connection, OptionalExtension as _, params};

use crate::Result;
use crate::media::MediaCache;

/// Posts kept per channel when history is trimmed.
pub const KEEP_POSTS_PER_CHANNEL: usize = 200;

/// The default cap, media included.
pub const DEFAULT_CAP_BYTES: i64 = 500 * 1024 * 1024;

/// What eviction should leave alone and how much history to keep.
#[derive(Clone, Debug)]
pub struct Keep<'a> {
    /// Channels whose posts are never trimmed, such as the open channel.
    pub channels: &'a [&'a str],
    /// Newest posts kept in each other channel.
    pub posts_per_channel: usize,
}

impl Default for Keep<'_> {
    fn default() -> Self {
        Self {
            channels: &[],
            posts_per_channel: KEEP_POSTS_PER_CHANNEL,
        }
    }
}

/// What one eviction pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Evicted {
    /// Media entries dropped.
    pub media: usize,
    /// Posts dropped.
    pub posts: usize,
    /// Cache size before, in bytes.
    pub bytes_before: i64,
    /// Cache size after, in bytes.
    pub bytes_after: i64,
}

/// Bytes the cache uses: media plus the database's used pages.
pub fn usage(conn: &Connection, media: &MediaCache) -> Result<i64> {
    Ok(database_bytes(conn)? + media.total_size(conn)?)
}

fn database_bytes(conn: &Connection) -> Result<i64> {
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    let free: i64 = conn.query_row("PRAGMA freelist_count", [], |row| row.get(0))?;
    let size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    Ok((pages - free) * size)
}

/// Drops cached data until the cache uses at most `cap_bytes`, or nothing
/// more may go. Run it on the sync thread after large writes and on a timer.
/// Post trimming runs in one transaction; the caller must not hold one.
pub fn evict_to(
    conn: &Connection,
    media: &MediaCache,
    cap_bytes: i64,
    keep: &Keep<'_>,
) -> Result<Evicted> {
    let bytes_before = usage(conn, media)?;
    let mut evicted = Evicted {
        bytes_before,
        bytes_after: bytes_before,
        ..Evicted::default()
    };
    if bytes_before <= cap_bytes {
        return Ok(evicted);
    }

    let mut used = bytes_before;
    for (key, size) in media.least_recent(conn)? {
        if used <= cap_bytes {
            break;
        }
        media.remove(conn, &key)?;
        used -= size;
        evicted.media += 1;
    }

    if used > cap_bytes {
        evicted.posts = trim_history(conn, keep, || Ok(usage(conn, media)? <= cap_bytes))?;
    }
    evicted.bytes_after = usage(conn, media)?;
    Ok(evicted)
}

/// Trims channels to their newest posts, viewed longest ago first, until
/// `done` says the cap is met. Returns the number of posts dropped.
fn trim_history(
    conn: &Connection,
    keep: &Keep<'_>,
    mut done: impl FnMut() -> Result<bool>,
) -> Result<usize> {
    let limit = i64::try_from(keep.posts_per_channel).unwrap_or(i64::MAX);
    let channels: Vec<String> = conn
        .prepare(
            "SELECT channel_id FROM posts GROUP BY channel_id
             HAVING COUNT(*) > ?1 ORDER BY MAX(touched_at), channel_id",
        )?
        .query_map([limit], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<usize> {
        let mut removed = 0;
        for channel in channels {
            if keep.channels.contains(&channel.as_str()) {
                continue;
            }
            removed += trim_channel(conn, &channel, limit)?;
            if done()? {
                break;
            }
        }
        Ok(removed)
    })();
    match result {
        Ok(removed) => {
            conn.execute_batch("COMMIT")?;
            Ok(removed)
        }
        Err(error) => {
            conn.execute_batch("ROLLBACK")?;
            Err(error)
        }
    }
}

/// Drops a channel's posts older than its newest `limit`, except followed
/// thread roots and posts still in the outbox, with their files.
fn trim_channel(conn: &Connection, channel: &str, limit: i64) -> Result<usize> {
    // The oldest post that stays; everything before it may go.
    let Some((at, id)) = conn
        .prepare_cached(
            "SELECT create_at, id FROM posts WHERE channel_id = ?1
             ORDER BY create_at DESC, id DESC LIMIT 1 OFFSET ?2",
        )?
        .query_row(params![channel, limit - 1], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .optional()?
    else {
        return Ok(0);
    };
    let count = conn
        .prepare_cached(
            "DELETE FROM posts
             WHERE channel_id = ?1 AND (create_at, id) < (?2, ?3)
               AND id NOT IN (SELECT id FROM threads WHERE is_following = 1)
               AND id NOT IN (SELECT pending_post_id FROM outbox)",
        )?
        .execute(params![channel, at, id])?;
    conn.prepare_cached(
        "DELETE FROM files WHERE channel_id = ?1 AND post_id != ''
             AND post_id NOT IN (SELECT id FROM posts WHERE channel_id = ?1)",
    )?
    .execute([channel])?;
    Ok(count)
}
