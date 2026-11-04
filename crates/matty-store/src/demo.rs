//! The web demo's sample workspace: a cache that `matty-demo` took from a
//! seeded server, with its media inside and a `demo` row saying whose it
//! is. Opened in memory, with every timestamp moved forward so the story
//! reads as recent whenever the demo is opened.

use rusqlite::OptionalExtension as _;

use crate::{Result, Store};

/// Whose workspace the snapshot is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoAccount {
    /// The server it was taken from. Nothing connects to it.
    pub server: String,
    /// The signed-in user.
    pub user_id: String,
    /// Their username.
    pub username: String,
    /// Their full name.
    pub display_name: String,
}

/// Timestamps before this (2001) are not wall-clock times in milliseconds.
const EPOCH_MS: i64 = 1_000_000_000_000;

/// Opens `snapshot` in memory and moves its times so that the moment it was
/// taken is now. Returns `None` for a database without a `demo` row.
pub fn open(snapshot: &[u8]) -> Result<Option<(Store, DemoAccount)>> {
    let store = Store::open_snapshot(snapshot)?;
    let conn = store.conn();
    let has_demo: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'demo')",
        [],
        |row| row.get(0),
    )?;
    if !has_demo {
        return Ok(None);
    }
    let Some((account, taken_at)) = conn
        .query_row(
            "SELECT server, user_id, username, display_name, taken_at FROM demo",
            [],
            |row| {
                Ok((
                    DemoAccount {
                        server: row.get(0)?,
                        user_id: row.get(1)?,
                        username: row.get(2)?,
                        display_name: row.get(3)?,
                    },
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    shift_times(&store, crate::posts::now_ms() - taken_at)?;
    Ok(Some((store, account)))
}

/// Tables whose times are versions, not part of the story: a custom
/// emoji's `update_at` names its cached image.
const UNSHIFTED: &[&str] = &["emoji"];

/// Adds `by` to every wall-clock time: the `*_at` columns that hold one.
fn shift_times(store: &Store, by: i64) -> Result<()> {
    let conn = store.conn();
    let tables: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for table in tables
        .iter()
        .filter(|table| !UNSHIFTED.contains(&table.as_str()))
    {
        let columns: Vec<String> = conn
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for column in columns.iter().filter(|column| column.ends_with("_at")) {
            conn.execute(
                &format!(
                    "UPDATE \"{table}\" SET \"{column}\" = \"{column}\" + ?1
                     WHERE typeof(\"{column}\") = 'integer' AND \"{column}\" > ?2"
                ),
                rusqlite::params![by, EPOCH_MS],
            )?;
        }
    }
    Ok(())
}
