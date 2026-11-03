//! Schema, migrations and connection settings (#27).
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use std::sync::{Arc, Barrier};
use std::thread;

use matty_store::{LATEST_VERSION, Store, StoreError, database_file_name, schema_version};

const TABLES: &[&str] = &[
    "teams",
    "channels",
    "channel_members",
    "categories",
    "category_channels",
    "users",
    "posts",
    "reactions",
    "files",
    "threads",
    "drafts",
    "outbox",
    "preferences",
    "emoji",
    "sync_cursors",
    "schema_version",
];

fn names(store: &Store, kind: &str) -> Vec<String> {
    let mut statement = store
        .conn()
        .prepare("SELECT name FROM sqlite_master WHERE type = ?1 ORDER BY name")
        .unwrap();
    statement
        .query_map([kind], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// The indexed columns of `index`, in order.
fn index_columns(store: &Store, index: &str) -> (String, Vec<String>) {
    let table: String = store
        .conn()
        .query_row(
            "SELECT tbl_name FROM sqlite_master WHERE type = 'index' AND name = ?1",
            [index],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| panic!("index {index} is missing"));
    let mut statement = store
        .conn()
        .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
        .unwrap();
    let columns = statement
        .query_map([index], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    (table, columns)
}

#[test]
fn a_fresh_database_migrates_to_the_latest_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("account.sqlite");
    let store = Store::open(&path).unwrap();
    assert!(path.exists());
    assert_eq!(schema_version(store.conn()).unwrap(), LATEST_VERSION);
    let tables = names(&store, "table");
    for table in TABLES {
        assert!(
            tables.iter().any(|t| t == table),
            "{table} missing: {tables:?}"
        );
    }
    drop(store);

    // Reopening is a no-op and keeps data.
    let store = Store::open(&path).unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO sync_cursors (key, value) VALUES ('k', 'v')",
            [],
        )
        .unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    let value: String = store
        .conn()
        .query_row(
            "SELECT value FROM sync_cursors WHERE key = 'k'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(value, "v");
    assert_eq!(schema_version(store.conn()).unwrap(), LATEST_VERSION);
}

#[test]
fn migrating_from_an_empty_file_starts_at_zero() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.sqlite");
    let store = Store::open_at_version(&path, 0).unwrap();
    assert_eq!(schema_version(store.conn()).unwrap(), 0);
    assert!(!names(&store, "table").iter().any(|t| t == "posts"));
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(schema_version(store.conn()).unwrap(), LATEST_VERSION);
}

#[test]
fn the_planned_indexes_exist() {
    let store = Store::open_in_memory().unwrap();
    let expected = [
        (
            "posts_channel_create",
            "posts",
            vec!["channel_id", "create_at"],
        ),
        ("posts_root", "posts", vec!["root_id"]),
        ("channel_members_user", "channel_members", vec!["user_id"]),
    ];
    for (index, table, columns) in expected {
        let (found_table, found_columns) = index_columns(&store, index);
        assert_eq!(found_table, table, "{index}");
        assert_eq!(found_columns, columns, "{index}");
    }
}

#[test]
fn connection_settings_are_applied() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("a.sqlite")).unwrap();
    let conn = store.conn();
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    let keys: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();
    assert_eq!(keys, 1);
    let timeout: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .unwrap();
    assert_eq!(timeout, matty_store::BUSY_TIMEOUT.as_millis() as i64);

    // Foreign keys are enforced: a reaction needs its post.
    let orphan = conn.execute(
        "INSERT INTO reactions (post_id, user_id, emoji_name) VALUES ('nope', 'u', 'smile')",
        [],
    );
    assert!(orphan.is_err());
}

#[test]
fn a_newer_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.sqlite");
    let store = Store::open(&path).unwrap();
    store
        .conn()
        .execute(
            "UPDATE schema_version SET version = ?1",
            [LATEST_VERSION + 1],
        )
        .unwrap();
    drop(store);
    match Store::open(&path) {
        Err(StoreError::TooNew { found, supported }) => {
            assert_eq!((found, supported), (LATEST_VERSION + 1, LATEST_VERSION));
        }
        other => panic!("expected TooNew, got {other:?}"),
    }
}

#[test]
fn two_threads_opening_and_writing_the_same_file_do_not_deadlock() {
    let dir = tempfile::tempdir().unwrap();
    let path = Arc::new(dir.path().join("shared.sqlite"));
    let start = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|worker| {
            let path = Arc::clone(&path);
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                // Both open (and migrate) a brand-new file at the same moment.
                let mut store = Store::open(&path).unwrap();
                for round in 0..50 {
                    let tx = store
                        .conn_mut()
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                        .unwrap();
                    tx.execute(
                        "INSERT INTO sync_cursors (key, value) VALUES (?1, 'x')",
                        [format!("{worker}-{round}")],
                    )
                    .unwrap();
                    tx.commit().unwrap();
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("worker finished without error");
    }
    let store = Store::open(&path).unwrap();
    let rows: i64 = store
        .conn()
        .query_row("SELECT COUNT(*) FROM sync_cursors", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 100);
    assert_eq!(schema_version(store.conn()).unwrap(), LATEST_VERSION);
}

#[test]
fn database_names_are_per_server_and_user() {
    let url = |s: &str| url::Url::parse(s).unwrap();
    assert_eq!(
        database_file_name(&url("https://Chat.Example.com:8065/team/"), "abc"),
        "chat.example.com_8065_team-abc.sqlite"
    );
    assert_eq!(
        database_file_name(&url("https://chat.example.com"), "abc"),
        "chat.example.com-abc.sqlite"
    );
    assert_ne!(
        database_file_name(&url("https://chat.example.com"), "abc"),
        database_file_name(&url("https://chat.example.com"), "xyz")
    );
    assert_ne!(
        database_file_name(&url("https://a.example.com"), "abc"),
        database_file_name(&url("https://b.example.com"), "abc")
    );
    // Nothing that could escape the folder.
    let odd = database_file_name(&url("https://x.example.com/../../etc"), "../u");
    assert!(!odd.contains('/') && !odd.contains(".."), "{odd}");
}
