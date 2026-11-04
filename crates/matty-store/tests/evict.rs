//! LRU eviction under a size cap (#31).
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use matty_api::models::{Draft, Post, UserThread};
use matty_store::evict::{self, Keep};
use matty_store::media::{self, MediaCache};
use matty_store::{Store, posts};
use rusqlite::Connection;

const MB: i64 = 1024 * 1024;

fn post(id: String, channel: &str, at: i64) -> Post {
    Post {
        id,
        channel_id: channel.into(),
        user_id: "u".into(),
        create_at: at,
        message: "hello".into(),
        ..Post::default()
    }
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).unwrap()
}

#[test]
fn media_goes_least_recently_used_first() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let cache = MediaCache::new(dir.path());
    let blob = vec![7u8; MB as usize];
    let keys: Vec<String> = (0..20)
        .map(|i| media::emoji_key(&format!("e{i:02}"), 0))
        .collect();
    for key in &keys {
        cache.put(conn, key, &blob, "image/png").unwrap();
    }
    // Use the five oldest again, so they are now the newest.
    for key in &keys[..5] {
        cache.get(conn, key).unwrap().unwrap();
    }
    assert!(evict::usage(conn, &cache).unwrap() > 20 * MB);

    let evicted = evict::evict_to(conn, &cache, 10 * MB, &Keep::default()).unwrap();
    assert!(evicted.bytes_after <= 10 * MB, "{evicted:?}");
    assert_eq!(evicted.bytes_after, evict::usage(conn, &cache).unwrap());
    assert_eq!(evicted.posts, 0);
    for key in &keys[..5] {
        assert!(cache.contains(conn, key).unwrap(), "{key} was used last");
    }
    // The survivors are the most recently used ones: the five reused and
    // the newest of the rest.
    let kept: Vec<bool> = keys
        .iter()
        .map(|key| cache.contains(conn, key).unwrap())
        .collect();
    let first_kept = kept[5..].iter().position(|&k| k).unwrap() + 5;
    assert!(kept[first_kept..].iter().all(|&k| k));
    assert_eq!(evicted.media, kept.iter().filter(|&&k| !k).count());

    // Under the cap, nothing happens.
    let again = evict::evict_to(conn, &cache, 10 * MB, &Keep::default()).unwrap();
    assert_eq!((again.media, again.posts), (0, 0));
}

#[test]
fn history_trimming_spares_drafts_outbox_and_protected_posts() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let cache = MediaCache::new(dir.path());
    for channel in ["open", "old"] {
        let batch: Vec<Post> = (0..300)
            .map(|i| post(format!("{channel}{i:03}"), channel, i))
            .collect();
        posts::insert_posts(conn, &batch).unwrap();
    }
    // A followed thread whose root is old, and a send still in the outbox.
    posts::upsert_threads(
        conn,
        "t",
        &[UserThread {
            id: "old000".into(),
            post: post("old000".into(), "old", 0),
            ..UserThread::default()
        }],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO outbox (pending_post_id, channel_id, message, created_at)
         VALUES ('old001', 'old', 'queued', 1)",
        [],
    )
    .unwrap();
    posts::upsert_draft(
        conn,
        &Draft {
            channel_id: "old".into(),
            message: "half written".into(),
            update_at: 1,
            ..Draft::default()
        },
    )
    .unwrap();

    let keep = Keep {
        channels: &["open"],
        posts_per_channel: 200,
    };
    let evicted = evict::evict_to(conn, &cache, 0, &keep).unwrap();
    assert_eq!(evicted.posts, 98);
    assert_eq!(
        count(conn, "SELECT COUNT(*) FROM posts WHERE channel_id = 'open'"),
        300
    );
    assert_eq!(
        count(conn, "SELECT COUNT(*) FROM posts WHERE channel_id = 'old'"),
        202
    );
    assert_eq!(
        count(
            conn,
            "SELECT MIN(create_at) FROM posts WHERE channel_id = 'old' AND create_at > 1"
        ),
        100
    );
    assert!(posts::post(conn, "old000").unwrap().is_some());
    assert!(posts::post(conn, "old001").unwrap().is_some());
    assert_eq!(count(conn, "SELECT COUNT(*) FROM outbox"), 1);
    assert_eq!(
        posts::draft(conn, "old", "").unwrap().unwrap().message,
        "half written"
    );
}

#[test]
fn channels_viewed_longest_ago_are_trimmed_first() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let cache = MediaCache::new(dir.path());
    for channel in ["a", "b"] {
        let batch: Vec<Post> = (0..2000)
            .map(|i| Post {
                message: "x".repeat(200),
                ..post(format!("{channel}{i:04}"), channel, i)
            })
            .collect();
        posts::insert_posts(conn, &batch).unwrap();
    }
    conn.execute("UPDATE posts SET touched_at = 1 WHERE channel_id = 'a'", [])
        .unwrap();
    conn.execute("UPDATE posts SET touched_at = 2 WHERE channel_id = 'b'", [])
        .unwrap();
    // Just under the current size: trimming one channel is enough.
    let cap = evict::usage(conn, &cache).unwrap() - 1;
    evict::evict_to(conn, &cache, cap, &Keep::default()).unwrap();
    assert_eq!(
        count(conn, "SELECT COUNT(*) FROM posts WHERE channel_id = 'a'"),
        200
    );
    assert_eq!(
        count(conn, "SELECT COUNT(*) FROM posts WHERE channel_id = 'b'"),
        2000
    );
}
