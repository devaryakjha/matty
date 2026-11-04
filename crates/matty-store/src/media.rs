//! The on-disk media cache: avatars, file thumbnails and previews, custom
//! emoji (PLAN §6.5).
//!
//! Bytes live in files under the cache folder; the `media` table indexes
//! them with their size and last access for eviction. Decoding to an image
//! is the caller's job.

use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension as _, params};

use crate::Result;

/// The key for a user's avatar. The picture's update time is part of the key,
/// so a new picture is a new entry and the old one ages out.
pub fn avatar_key(user_id: &str, last_picture_update: i64) -> String {
    format!("avatar/{user_id}/{last_picture_update}")
}

/// Which rendition of an uploaded file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileVariant {
    /// The small thumbnail shown in the message list.
    Thumb,
    /// The larger preview shown in the viewer.
    Preview,
    /// The original file.
    Full,
}

/// The key for one rendition of a file.
pub fn file_key(file_id: &str, variant: FileVariant) -> String {
    let variant = match variant {
        FileVariant::Thumb => "thumb",
        FileVariant::Preview => "preview",
        FileVariant::Full => "full",
    };
    format!("file/{file_id}/{variant}")
}

/// The key for an image from outside the server, such as a link preview's.
/// URLs can be longer than a file name allows, so the key is a 128-bit hash
/// of the URL: two FNV-1a passes with different offsets.
pub fn remote_key(url: &str) -> String {
    let fnv = |offset: u64| {
        url.bytes().fold(offset, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
    };
    format!(
        "remote/{:016x}{:016x}",
        fnv(0xcbf2_9ce4_8422_2325),
        fnv(0x6c62_272e_07bb_0142)
    )
}

/// The key for a custom emoji's image. Its `update_at` is part of the key,
/// so a replaced image is a new entry, like [`avatar_key`].
pub fn emoji_key(emoji_id: &str, update_at: i64) -> String {
    format!("emoji/{emoji_id}/{update_at}")
}

/// The key for a team's icon, versioned by its `last_team_icon_update`.
pub fn team_icon_key(team_id: &str, last_team_icon_update: i64) -> String {
    format!("team/{team_id}/{last_team_icon_update}")
}

/// A cached item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Media {
    /// The stored bytes.
    pub bytes: Vec<u8>,
    /// The MIME type given to [`MediaCache::put`].
    pub content_type: String,
}

/// Media files for one account, indexed in that account's store.
#[derive(Clone, Debug)]
pub struct MediaCache {
    dir: PathBuf,
}

impl MediaCache {
    /// A cache rooted at `dir`, created on the first `put`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The folder holding the files.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Stores `bytes` under `key`, replacing what was there. The file is
    /// written beside its final name and renamed, so a crash never leaves a
    /// half-written file under a key.
    pub fn put(
        &self,
        conn: &Connection,
        key: &str,
        bytes: &[u8],
        content_type: &str,
    ) -> Result<()> {
        matty_api::rt::assert_off_ui_thread("writing the media cache");
        let relative = relative_path(key);
        let path = self.dir.join(&relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let partial = path.with_extension("partial");
        {
            let mut file = fs::File::create(&partial)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        fs::rename(&partial, &path)?;
        let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
        conn.prepare_cached(
            "INSERT INTO media (key, path, size, content_type, accessed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (key) DO UPDATE SET path = excluded.path, size = excluded.size,
                 content_type = excluded.content_type, accessed_at = excluded.accessed_at",
        )?
        .execute(params![
            key,
            relative,
            size,
            content_type,
            next_access(conn)?
        ])?;
        Ok(())
    }

    /// The item under `key`, marking it used. A missing or damaged file
    /// (wrong size) drops the entry and returns `None`, so the caller fetches
    /// it again.
    pub fn get(&self, conn: &Connection, key: &str) -> Result<Option<Media>> {
        matty_api::rt::assert_off_ui_thread("reading the media cache");
        let Some((relative, size, content_type)) = conn
            .prepare_cached("SELECT path, size, content_type FROM media WHERE key = ?1")?
            .query_row([key], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .optional()?
        else {
            return Ok(None);
        };
        let bytes = match fs::read(self.dir.join(&relative)) {
            Ok(bytes) if i64::try_from(bytes.len()).ok() == Some(size) => bytes,
            Ok(_) => {
                self.remove(conn, key)?;
                return Ok(None);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                self.remove(conn, key)?;
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        conn.prepare_cached("UPDATE media SET accessed_at = ?2 WHERE key = ?1")?
            .execute(params![key, next_access(conn)?])?;
        Ok(Some(Media {
            bytes,
            content_type,
        }))
    }

    /// Whether `key` is indexed, without reading or touching it.
    pub fn contains(&self, conn: &Connection, key: &str) -> Result<bool> {
        Ok(conn
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM media WHERE key = ?1)")?
            .query_row([key], |row| row.get(0))?)
    }

    /// Drops `key` and its file.
    pub fn remove(&self, conn: &Connection, key: &str) -> Result<()> {
        conn.prepare_cached("DELETE FROM media WHERE key = ?1")?
            .execute([key])?;
        match fs::remove_file(self.dir.join(relative_path(key))) {
            Err(error) if error.kind() != ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }

    /// Index entries oldest access first, as (key, size), for eviction.
    pub(crate) fn least_recent(&self, conn: &Connection) -> Result<Vec<(String, i64)>> {
        let mut statement =
            conn.prepare_cached("SELECT key, size FROM media ORDER BY accessed_at, key")?;
        let entries = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(entries)
    }

    /// Bytes indexed in the cache.
    pub fn total_size(&self, conn: &Connection) -> Result<i64> {
        Ok(conn
            .prepare_cached("SELECT COALESCE(SUM(size), 0) FROM media")?
            .query_row([], |row| row.get(0))?)
    }
}

/// The access stamp for now: the local clock in ms, but always after the
/// latest stamp, so two accesses in the same millisecond still order.
fn next_access(conn: &Connection) -> Result<i64> {
    let now = crate::posts::now_ms();
    let latest: i64 = conn
        .prepare_cached("SELECT COALESCE(MAX(accessed_at), 0) FROM media")?
        .query_row([], |row| row.get(0))?;
    Ok(now.max(latest.saturating_add(1)))
}

/// Where a key's file lives, relative to the cache folder: one folder per
/// kind, then the whole key hex-encoded so any key is a safe file name.
fn relative_path(key: &str) -> String {
    let kind = key
        .split('/')
        .next()
        .filter(|kind| matches!(*kind, "avatar" | "file" | "emoji" | "remote"))
        .unwrap_or("other");
    let mut name = String::with_capacity(key.len() * 2);
    for byte in key.bytes() {
        name.push_str(&format!("{byte:02x}"));
    }
    format!("{kind}/{name}")
}

/// Where an account's media lives: `<cache>/Matty/media/<account>`, named
/// like its database.
pub fn default_dir(server: &url::Url, user_id: &str) -> Option<PathBuf> {
    dir_for_key(&crate::account_key(server, user_id))
}

/// Where the media for the account `key` lives, as [`default_dir`] puts
/// it; `None` for a key [`crate::account_key`] could not have made.
pub fn dir_for_key(key: &str) -> Option<PathBuf> {
    if !crate::is_account_key(key) {
        return None;
    }
    dirs::cache_dir().map(|dir| dir.join("Matty").join("media").join(key))
}
