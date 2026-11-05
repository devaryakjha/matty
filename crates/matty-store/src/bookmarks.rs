//! Channel bookmarks (#226), so a channel's bar shows offline. The sync
//! engine writes what the server answers and what its events say; the bar
//! reads them back in order.

use matty_api::models::{BookmarkUpdate, ChannelBookmark, sort_bookmarks};
use rusqlite::{Connection, params};

use crate::Result;

/// Stores a bookmark, or removes it when the server says it is deleted.
pub fn upsert(conn: &Connection, bookmark: &ChannelBookmark) -> Result<()> {
    if bookmark.delete_at != 0 {
        return remove(conn, &bookmark.id);
    }
    let data = serde_json::to_string(bookmark).unwrap_or_else(|_| "{}".to_owned());
    conn.prepare_cached(
        "INSERT INTO channel_bookmarks (id, channel_id, sort_order, create_at, data)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (id) DO UPDATE SET
             channel_id = excluded.channel_id, sort_order = excluded.sort_order,
             create_at = excluded.create_at, data = excluded.data",
    )?
    .execute(params![
        bookmark.id,
        bookmark.channel_id,
        bookmark.sort_order,
        bookmark.create_at,
        data
    ])?;
    Ok(())
}

/// Removes a bookmark.
pub fn remove(conn: &Connection, bookmark_id: &str) -> Result<()> {
    conn.prepare_cached("DELETE FROM channel_bookmarks WHERE id = ?1")?
        .execute([bookmark_id])?;
    Ok(())
}

/// Replaces a channel's bookmarks with the server's whole list.
pub fn replace(conn: &Connection, channel_id: &str, bookmarks: &[ChannelBookmark]) -> Result<()> {
    conn.prepare_cached("DELETE FROM channel_bookmarks WHERE channel_id = ?1")?
        .execute([channel_id])?;
    for bookmark in bookmarks {
        upsert(conn, bookmark)?;
    }
    Ok(())
}

/// Applies an edit: the bookmark it replaced goes, the new one stays.
pub fn apply_update(conn: &Connection, update: &BookmarkUpdate) -> Result<()> {
    if let Some(deleted) = &update.deleted {
        remove(conn, &deleted.id)?;
    }
    if let Some(updated) = &update.updated {
        upsert(conn, updated)?;
    }
    Ok(())
}

/// A channel's bookmarks in bar order.
pub fn for_channel(conn: &Connection, channel_id: &str) -> Result<Vec<ChannelBookmark>> {
    let mut bookmarks: Vec<ChannelBookmark> = conn
        .prepare_cached("SELECT data FROM channel_bookmarks WHERE channel_id = ?1")?
        .query_map([channel_id], |row| row.get::<_, String>(0))?
        .filter_map(|data| serde_json::from_str(&data.ok()?).ok())
        .collect();
    sort_bookmarks(&mut bookmarks);
    Ok(bookmarks)
}

/// The channel a stored bookmark belongs to.
pub fn channel_of(conn: &Connection, bookmark_id: &str) -> Result<Option<String>> {
    use rusqlite::OptionalExtension as _;
    Ok(conn
        .prepare_cached("SELECT channel_id FROM channel_bookmarks WHERE id = ?1")?
        .query_row([bookmark_id], |row| row.get(0))
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    fn bookmark(id: &str, order: i64) -> ChannelBookmark {
        ChannelBookmark {
            id: id.into(),
            channel_id: "c".into(),
            display_name: id.to_uppercase(),
            sort_order: order,
            link_url: format!("https://example.com/{id}"),
            ..ChannelBookmark::default()
        }
    }

    #[test]
    fn bookmarks_round_trip_in_order() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        replace(conn, "c", &[bookmark("b", 1), bookmark("a", 0)]).unwrap();
        let names: Vec<_> = for_channel(conn, "c")
            .unwrap()
            .into_iter()
            .map(|b| b.id)
            .collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(for_channel(conn, "c").unwrap()[0], bookmark("a", 0));
        assert_eq!(channel_of(conn, "b").unwrap().as_deref(), Some("c"));

        // An edit of someone else's bookmark replaces it under a new id.
        apply_update(
            conn,
            &BookmarkUpdate {
                updated: Some(bookmark("c", 2)),
                deleted: Some(bookmark("b", 1)),
            },
        )
        .unwrap();
        let names: Vec<_> = for_channel(conn, "c")
            .unwrap()
            .into_iter()
            .map(|b| b.id)
            .collect();
        assert_eq!(names, ["a", "c"]);

        let mut gone = bookmark("a", 0);
        gone.delete_at = 5;
        upsert(conn, &gone).unwrap();
        assert_eq!(for_channel(conn, "c").unwrap().len(), 1);
        replace(conn, "c", &[]).unwrap();
        assert!(for_channel(conn, "c").unwrap().is_empty());
    }
}
