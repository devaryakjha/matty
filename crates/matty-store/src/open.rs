//! Opening a database: where it lives, connection settings and migration.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;

use crate::Result;
use crate::migrations::{LATEST_VERSION, migrate_to};

/// How long a connection waits for another's write lock before failing.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// One account's cache: a SQLite connection with Matty's settings applied and
/// the schema at [`LATEST_VERSION`].
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens or creates the database at `path`, creating its folder, and
    /// migrates it to the latest schema.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::setup(Connection::open(path)?, LATEST_VERSION)
    }

    /// A private database in memory, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::setup(Connection::open_in_memory()?, LATEST_VERSION)
    }

    /// Opens `path` migrated only to `version`, for upgrade tests.
    #[doc(hidden)]
    pub fn open_at_version(path: &Path, version: i64) -> Result<Self> {
        Self::setup(Connection::open(path)?, version)
    }

    fn setup(mut conn: Connection, version: i64) -> Result<Self> {
        // Set first, so a connection that finds the file locked waits.
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // WAL lets readers proceed while the sync engine writes. In-memory
        // databases answer "memory", which is fine.
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;",
        )?;
        migrate_to(&mut conn, version)?;
        Ok(Self { conn })
    }

    /// The underlying connection.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// The underlying connection, for transactions.
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

/// The file name for one account: the server's host, port and path made safe
/// for a file name, then the user id. `https://Chat.Example.com:8065/team/`
/// and user `abc` give `chat.example.com_8065_team-abc.sqlite`.
pub fn database_file_name(server: &url::Url, user_id: &str) -> String {
    let mut place = server.host_str().unwrap_or("server").to_ascii_lowercase();
    if let Some(port) = server.port() {
        place.push('_');
        place.push_str(&port.to_string());
    }
    let path = server.path().trim_matches('/');
    if !path.is_empty() {
        place.push('_');
        place.push_str(path);
    }
    // Dots only ever come from the host, which cannot hold "..".
    let safe = |text: &str, keep: &[char]| -> String {
        text.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || keep.contains(&c) {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    let place = place.replace("..", "_");
    format!(
        "{}-{}.sqlite",
        safe(&place, &['.', '-']),
        safe(user_id, &[])
    )
}

/// Where an account's database lives: `<app data>/Matty/accounts/<file>`.
pub fn default_path(server: &url::Url, user_id: &str) -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| {
        dir.join("Matty")
            .join("accounts")
            .join(database_file_name(server, user_id))
    })
}
