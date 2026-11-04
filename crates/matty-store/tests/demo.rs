//! The web demo's snapshot: opened from bytes, with its times moved to now.
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use matty_store::Store;
use matty_store::demo;
use matty_store::media::{self, MediaCache};

/// A database file like `matty-demo` writes, taken at `taken_at`, with one
/// post written an hour before that and one avatar.
fn snapshot(taken_at: i64) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("demo.sqlite");
    {
        let store = Store::open(&path).unwrap();
        let conn = store.conn();
        conn.execute(
            "INSERT INTO posts (id, channel_id, user_id, create_at, update_at, message)
             VALUES ('p1', 'c1', 'u1', ?1, ?1, 'hello')",
            [taken_at - 3_600_000],
        )
        .unwrap();
        MediaCache::in_database()
            .put(conn, &media::avatar_key("u1", 5), b"png", "image/png")
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE demo (server TEXT NOT NULL, user_id TEXT NOT NULL,
                 username TEXT NOT NULL, display_name TEXT NOT NULL, taken_at INTEGER NOT NULL) STRICT;
             PRAGMA journal_mode = DELETE;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO demo VALUES ('http://localhost:8065', 'u1', 'maya', 'Maya Chen', ?1)",
            [taken_at],
        )
        .unwrap();
    }
    std::fs::read(&path).unwrap()
}

#[test]
fn a_snapshot_opens_with_its_story_moved_to_now() {
    let taken_at = 1_700_000_000_000;
    let (store, account) = demo::open(&snapshot(taken_at)).unwrap().unwrap();
    assert_eq!(account.username, "maya");
    assert_eq!(account.display_name, "Maya Chen");

    let created: i64 = store
        .conn()
        .query_row("SELECT create_at FROM posts WHERE id = 'p1'", [], |row| {
            row.get(0)
        })
        .unwrap();
    let hour_ago = matty_store::posts::now_ms() - 3_600_000;
    assert!(
        (created - hour_ago).abs() < 60_000,
        "{created} vs {hour_ago}"
    );

    // The media inside comes with it, under the same key.
    let avatar = MediaCache::in_database()
        .get(store.conn(), &media::avatar_key("u1", 5))
        .unwrap()
        .unwrap();
    assert_eq!(avatar.bytes, b"png");
}

#[test]
fn a_cache_that_is_not_a_demo_opens_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    drop(Store::open(&path).unwrap());
    // A WAL database keeps its pages in the -wal file until checkpointed;
    // closing checkpoints it.
    let bytes = std::fs::read(&path).unwrap();
    assert!(demo::open(&bytes).unwrap().is_none());
}
