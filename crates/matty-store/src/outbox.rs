//! Posts written but not yet acknowledged by the server (PLAN §6.5).
//!
//! Each send has a row here and an optimistic row in `posts` whose id is
//! its `pending_post_id`. The server echoes that id, so whichever of the
//! REST response or the WebSocket `posted` event arrives first replaces the
//! optimistic row and the other changes nothing.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension as _, Row, params};
use serde_json::Value;

use crate::Result;

/// A queued send.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outgoing {
    /// The client's id for the post, `{user_id}:{millis}` by convention.
    pub pending_post_id: String,
    /// Channel.
    pub channel_id: String,
    /// Thread root for a reply, or empty.
    pub root_id: String,
    /// Message text.
    pub message: String,
    /// Uploaded file ids.
    pub file_ids: Vec<String>,
    /// Post properties.
    pub props: HashMap<String, Value>,
    /// When it was written, on the local clock (ms).
    pub created_at: i64,
    /// Sends tried so far.
    pub attempts: i64,
    /// Why the last attempt failed; `None` while it is pending.
    pub last_error: Option<String>,
}

const COLUMNS: &str = "pending_post_id, channel_id, root_id, message, file_ids, props, created_at,
    attempts, last_error";

fn from_row(row: &Row<'_>) -> rusqlite::Result<Outgoing> {
    let file_ids: String = row.get(4)?;
    let props: String = row.get(5)?;
    Ok(Outgoing {
        pending_post_id: row.get(0)?,
        channel_id: row.get(1)?,
        root_id: row.get(2)?,
        message: row.get(3)?,
        file_ids: serde_json::from_str(&file_ids).unwrap_or_default(),
        props: serde_json::from_str(&props).unwrap_or_default(),
        created_at: row.get(6)?,
        attempts: row.get(7)?,
        last_error: row.get(8)?,
    })
}

/// Queues a send. Queuing the same id again changes nothing.
pub fn push(conn: &Connection, outgoing: &Outgoing) -> Result<()> {
    conn.prepare_cached(&format!(
        "INSERT INTO outbox ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (pending_post_id) DO NOTHING"
    ))?
    .execute(params![
        outgoing.pending_post_id,
        outgoing.channel_id,
        outgoing.root_id,
        outgoing.message,
        serde_json::to_string(&outgoing.file_ids).unwrap_or_else(|_| "[]".into()),
        serde_json::to_string(&outgoing.props).unwrap_or_else(|_| "{}".into()),
        outgoing.created_at,
        outgoing.attempts,
        outgoing.last_error
    ])?;
    Ok(())
}

/// A queued send, if still queued.
pub fn get(conn: &Connection, pending_post_id: &str) -> Result<Option<Outgoing>> {
    Ok(conn
        .prepare_cached(&format!(
            "SELECT {COLUMNS} FROM outbox WHERE pending_post_id = ?1"
        ))?
        .query_row([pending_post_id], from_row)
        .optional()?)
}

/// A channel's queued sends, oldest first.
pub fn for_channel(conn: &Connection, channel_id: &str) -> Result<Vec<Outgoing>> {
    let mut statement = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM outbox WHERE channel_id = ?1 ORDER BY seq"
    ))?;
    let rows = statement
        .query_map([channel_id], from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// The channel's next send: its oldest one that has not failed. Failed
/// sends wait for Retry and do not hold up the ones after them.
pub fn next(conn: &Connection, channel_id: &str) -> Result<Option<Outgoing>> {
    Ok(conn
        .prepare_cached(&format!(
            "SELECT {COLUMNS} FROM outbox WHERE channel_id = ?1 AND last_error IS NULL
             ORDER BY seq LIMIT 1"
        ))?
        .query_row([channel_id], from_row)
        .optional()?)
}

/// Changes an unsent post's text in place (#195): the queued send and its
/// optimistic row both take it, and its place in the queue stays. `false`
/// when it is not queued.
pub fn edit(conn: &Connection, pending_post_id: &str, message: &str) -> Result<bool> {
    let changed = conn
        .prepare_cached("UPDATE outbox SET message = ?2 WHERE pending_post_id = ?1")?
        .execute(params![pending_post_id, message])?;
    if changed > 0 {
        conn.prepare_cached("UPDATE posts SET message = ?2 WHERE id = ?1")?
            .execute(params![pending_post_id, message])?;
    }
    Ok(changed > 0)
}

/// Records an attempt starting: one more try, no error.
pub fn attempt(conn: &Connection, pending_post_id: &str) -> Result<()> {
    conn.prepare_cached(
        "UPDATE outbox SET attempts = attempts + 1, last_error = NULL WHERE pending_post_id = ?1",
    )?
    .execute([pending_post_id])?;
    Ok(())
}

/// Records why an attempt failed. The send stays queued for a retry.
pub fn fail(conn: &Connection, pending_post_id: &str, error: &str) -> Result<()> {
    conn.prepare_cached("UPDATE outbox SET last_error = ?2 WHERE pending_post_id = ?1")?
        .execute(params![pending_post_id, error])?;
    Ok(())
}

/// Clears a failure so the send queues again, for Retry.
pub fn unfail(conn: &Connection, pending_post_id: &str) -> Result<()> {
    conn.prepare_cached("UPDATE outbox SET last_error = NULL WHERE pending_post_id = ?1")?
        .execute([pending_post_id])?;
    Ok(())
}

/// Every channel with a queued send.
pub fn channels(conn: &Connection) -> Result<Vec<String>> {
    let mut statement =
        conn.prepare_cached("SELECT DISTINCT channel_id FROM outbox ORDER BY channel_id")?;
    let rows = statement
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Removes a send: the server has it, or the user discarded it.
pub fn remove(conn: &Connection, pending_post_id: &str) -> Result<bool> {
    let removed = conn
        .prepare_cached("DELETE FROM outbox WHERE pending_post_id = ?1")?
        .execute([pending_post_id])?;
    Ok(removed > 0)
}

/// Drops every queued send with its optimistic post and files, and says
/// how many there were. Sign out calls it so nothing written in this
/// session goes out under the next one (#194).
pub fn clear(conn: &Connection) -> Result<usize> {
    let ids: Vec<String> = conn
        .prepare_cached("SELECT pending_post_id FROM outbox")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in &ids {
        crate::posts::remove_post(conn, id)?;
    }
    conn.prepare_cached("DELETE FROM outbox")?.execute([])?;
    Ok(ids.len())
}
