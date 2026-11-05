//! Numbered SQL migrations, applied in order inside a transaction and recorded
//! in `schema_version`.

use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior};

use crate::{Result, StoreError};

/// Every migration, in order. Migration `n` (1-based) moves the schema from
/// version `n - 1` to `n`. Never edit a migration once it has shipped; add a
/// new one.
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_media.sql"),
    include_str!("../migrations/0003_post_threads.sql"),
    include_str!("../migrations/0004_outbox_local_files.sql"),
    include_str!("../migrations/0005_channel_bookmarks.sql"),
    include_str!("../migrations/0006_outbox_priority.sql"),
];

/// The schema version a fresh database migrates to.
pub const LATEST_VERSION: i64 = MIGRATIONS.len() as i64;

/// The schema version recorded in `conn`; 0 for an empty database.
pub fn schema_version(conn: &Connection) -> Result<i64> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_version')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(0);
    }
    let version = conn
        .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
        .optional()?;
    Ok(version.unwrap_or(0))
}

/// Brings `conn` up to `target`. All pending migrations run in one write
/// transaction, taken before the version is read, so two connections opening
/// the same new file cannot both apply them.
pub(crate) fn migrate_to(conn: &mut Connection, target: i64) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL) STRICT",
    )?;
    let current = schema_version(&tx)?;
    if current > LATEST_VERSION {
        return Err(StoreError::TooNew {
            found: current,
            supported: LATEST_VERSION,
        });
    }
    if current >= target {
        return Ok(());
    }
    for sql in &MIGRATIONS[current as usize..target as usize] {
        tx.execute_batch(sql)?;
    }
    tx.execute("DELETE FROM schema_version", [])?;
    tx.execute("INSERT INTO schema_version (version) VALUES (?1)", [target])?;
    tx.commit()?;
    Ok(())
}
