//! Every shipped schema version upgrades to the latest (#32).
//!
//! `fixtures/db/v<N>.sqlite` holds the same small account written at schema
//! version N by `scripts/make-db-fixture.sh`. Each one is copied, opened
//! (which migrates it) and checked. A schema change adds a migration and its
//! fixture; shipped fixtures never change.
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use std::path::PathBuf;

use matty_store::posts::{self, Window};
use matty_store::sidebar::{self, UnreadMode};
use matty_store::{LATEST_VERSION, Store, schema_version};

fn fixture(version: i64) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/db")
        .join(format!("v{version}.sqlite"))
}

fn count(store: &Store, table: &str) -> i64 {
    store
        .conn()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn there_is_a_fixture_for_every_version() {
    for version in 1..=LATEST_VERSION {
        assert!(
            fixture(version).exists(),
            "no fixture for schema version {version}: run scripts/make-db-fixture.sh"
        );
    }
}

#[test]
fn every_fixture_migrates_to_the_latest_version() {
    for version in 1..=LATEST_VERSION {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("account.sqlite");
        std::fs::copy(fixture(version), &path).unwrap();
        {
            let raw = rusqlite::Connection::open(&path).unwrap();
            assert_eq!(schema_version(&raw).unwrap(), version, "fixture v{version}");
        }

        let store = Store::open(&path).unwrap();
        assert_eq!(schema_version(store.conn()).unwrap(), LATEST_VERSION);
        for (table, rows) in [
            ("teams", 1),
            ("users", 2),
            ("channels", 3),
            ("channel_members", 3),
            ("categories", 2),
            ("category_channels", 3),
            ("posts", 5),
            ("reactions", 1),
            ("files", 1),
            ("threads", 1),
            ("drafts", 1),
            ("outbox", 1),
        ] {
            assert_eq!(count(&store, table), rows, "{table} in fixture v{version}");
        }
        // Tables added after this version start empty.
        let media = if version >= 2 { 1 } else { 0 };
        assert_eq!(count(&store, "media"), media, "media in fixture v{version}");
        let bookmarks = if version >= 5 { 1 } else { 0 };
        assert_eq!(
            matty_store::bookmarks::for_channel(store.conn(), "town")
                .unwrap()
                .len(),
            bookmarks,
            "bookmarks in fixture v{version}"
        );

        let conn = store.conn();
        // Files attached offline are kept from v4; older rows have none.
        let queued = matty_store::outbox::get(conn, "pending1").unwrap().unwrap();
        let local: &[&str] = if version >= 4 {
            &["/home/me/notes.txt"]
        } else {
            &[]
        };
        assert_eq!(queued.local_files, local, "fixture v{version}");
        let window = posts::posts_window(conn, "town", Window::Latest, 10, true).unwrap();
        let ids: Vec<&str> = window.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["p1", "p2", "p5"], "fixture v{version}");
        assert_eq!(window[1].metadata.reactions.len(), 1);
        assert_eq!(window[1].metadata.files[0].name, "shot.png");
        assert_eq!(posts::thread(conn, "p1").unwrap().len(), 3);

        let sections =
            sidebar::sidebar_snapshot(conn, "team1", "me", UnreadMode::CollapsedThreads).unwrap();
        let shown: usize = sections.iter().map(|s| s.channels.len()).sum();
        assert_eq!(shown, 3, "fixture v{version}");
        assert_eq!(
            posts::cursor(conn, &posts::channel_since_key("town"))
                .unwrap()
                .as_deref(),
            Some("50")
        );
    }
}
