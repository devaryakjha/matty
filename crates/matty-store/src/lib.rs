//! SQLite cache for Matty: schema, migrations and typed queries.
//!
//! Only the sync engine writes here, and the main thread never opens the
//! database (PLAN §6.2, §6.5). Plain Rust: this crate must never depend on GPUI.

pub mod evict;
pub mod media;
mod migrations;
mod open;
pub mod outbox;
pub mod posts;
pub mod sidebar;

pub use migrations::{LATEST_VERSION, schema_version};
pub use open::{
    BUSY_TIMEOUT, Store, account_key, database_file_name, default_path, forget, is_account_key,
    path_for_key,
};

/// Why the store could not be opened or used.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite reported an error.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The database directory could not be created.
    #[error("could not create the database folder: {0}")]
    Io(#[from] std::io::Error),
    /// The file was written by a newer Matty, whose schema this one cannot read.
    #[error("the database is at schema version {found}, newer than this Matty's {supported}")]
    TooNew {
        /// The version in the file.
        found: i64,
        /// The newest version this build knows.
        supported: i64,
    },
}

/// Store results.
pub type Result<T, E = StoreError> = std::result::Result<T, E>;
