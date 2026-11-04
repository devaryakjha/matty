//! The on-disk media cache (#30).
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use std::fs;

use matty_store::Store;
use matty_store::media::{self, FileVariant, MediaCache};
use rusqlite::Connection;

fn accessed(conn: &Connection, key: &str) -> i64 {
    conn.query_row(
        "SELECT accessed_at FROM media WHERE key = ?1",
        [key],
        |row| row.get(0),
    )
    .unwrap()
}

fn only_file(dir: &std::path::Path) -> std::path::PathBuf {
    let kind = fs::read_dir(dir).unwrap().next().unwrap().unwrap().path();
    fs::read_dir(kind).unwrap().next().unwrap().unwrap().path()
}

#[test]
fn keys_name_the_kind_and_version() {
    assert_eq!(media::avatar_key("u1", 1700), "avatar/u1/1700");
    assert_eq!(media::file_key("f1", FileVariant::Thumb), "file/f1/thumb");
    assert_eq!(
        media::file_key("f1", FileVariant::Preview),
        "file/f1/preview"
    );
    assert_eq!(media::file_key("f1", FileVariant::Full), "file/f1/full");
    assert_eq!(media::emoji_key("e1"), "emoji/e1");
}

#[test]
fn a_one_megabyte_blob_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let cache = MediaCache::new(dir.path().join("media"));
    let bytes: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
    let key = media::file_key("f1", FileVariant::Full);

    assert!(cache.get(conn, &key).unwrap().is_none());
    cache.put(conn, &key, &bytes, "image/png").unwrap();
    let got = cache.get(conn, &key).unwrap().unwrap();
    assert_eq!(got.bytes, bytes);
    assert_eq!(got.content_type, "image/png");
    assert_eq!(cache.total_size(conn).unwrap(), 1024 * 1024);

    // Replacing keeps one entry.
    cache.put(conn, &key, b"small", "image/png").unwrap();
    assert_eq!(cache.get(conn, &key).unwrap().unwrap().bytes, b"small");
    assert_eq!(cache.total_size(conn).unwrap(), 5);
}

#[test]
fn get_updates_last_access() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let cache = MediaCache::new(dir.path());
    let (a, b) = (media::emoji_key("a"), media::emoji_key("b"));
    cache.put(conn, &a, b"a", "image/png").unwrap();
    cache.put(conn, &b, b"b", "image/png").unwrap();
    assert!(accessed(conn, &a) < accessed(conn, &b));

    cache.get(conn, &a).unwrap().unwrap();
    assert!(accessed(conn, &a) > accessed(conn, &b));
    // `contains` does not count as a use.
    let before = accessed(conn, &b);
    assert!(cache.contains(conn, &b).unwrap());
    assert_eq!(accessed(conn, &b), before);
}

#[test]
fn a_missing_or_damaged_file_drops_the_entry() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let cache = MediaCache::new(dir.path());
    let key = media::avatar_key("u1", 1);

    cache
        .put(conn, &key, b"picture bytes", "image/jpeg")
        .unwrap();
    fs::write(only_file(dir.path()), b"trunc").unwrap();
    assert!(cache.get(conn, &key).unwrap().is_none());
    assert!(!cache.contains(conn, &key).unwrap());

    cache
        .put(conn, &key, b"picture bytes", "image/jpeg")
        .unwrap();
    fs::remove_file(only_file(dir.path())).unwrap();
    assert!(cache.get(conn, &key).unwrap().is_none());
    assert!(!cache.contains(conn, &key).unwrap());
}

#[test]
fn remove_deletes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let cache = MediaCache::new(dir.path());
    let key = media::file_key("f/../../escape", FileVariant::Thumb);
    cache.put(conn, &key, b"x", "image/png").unwrap();
    // Odd keys still land inside the cache folder.
    assert!(only_file(dir.path()).starts_with(dir.path()));
    cache.remove(conn, &key).unwrap();
    assert!(!cache.contains(conn, &key).unwrap());
    assert!(
        fs::read_dir(dir.path().join("file"))
            .unwrap()
            .next()
            .is_none()
    );
    // Removing again is fine.
    cache.remove(conn, &key).unwrap();
}

#[test]
fn remote_keys_are_short_stable_and_distinct() {
    let url = format!("https://example.com/{}.png", "a".repeat(400));
    let key = matty_store::media::remote_key(&url);
    assert_eq!(key, matty_store::media::remote_key(&url));
    assert_eq!(key.len(), "remote/".len() + 32);
    assert_ne!(
        key,
        matty_store::media::remote_key("https://example.com/b.png")
    );
}

#[test]
fn a_cache_in_the_database_round_trips_and_forgets() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let cache = MediaCache::in_database();
    let key = media::avatar_key("u1", 7);

    assert!(cache.dir().is_none());
    assert!(cache.get(conn, &key).unwrap().is_none());
    cache.put(conn, &key, b"picture", "image/png").unwrap();
    let got = cache.get(conn, &key).unwrap().unwrap();
    assert_eq!(got.bytes, b"picture");
    assert_eq!(got.content_type, "image/png");

    cache.remove(conn, &key).unwrap();
    assert!(cache.get(conn, &key).unwrap().is_none());
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_bytes", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}
