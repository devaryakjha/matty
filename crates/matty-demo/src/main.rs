//! Snapshots a seeded Mattermost server into the web demo's sample
//! workspace (docs/web.md): signs in as the demo user, has the sync engine
//! load every channel, thread, avatar and image the way the app would, then
//! packs the cache, with its media, into one SQLite file the web build opens
//! without a server.
//!
//!     matty-demo --server http://localhost:8065 --login priya --out demo.sqlite
//!
//! `scripts/demo/run` runs the whole pipeline: dev server, seed, snapshot.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_channel::Receiver;
use matty_api::{Client, Credentials};
use matty_sync::{ChangeSet, Command, Connection, ImageSize, SyncHandle};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// How long the engine may stay quiet before a step counts as done.
const QUIET: Duration = Duration::from_millis(400);
/// The longest any one step may take.
const STEP: Duration = Duration::from_secs(60);
/// Pages of history to load per channel, past the first.
const MAX_PAGES: usize = 40;

struct Args {
    server: String,
    login: String,
    password: String,
    out: PathBuf,
}

fn args() -> Result<Args> {
    let mut server = "http://localhost:8065".to_owned();
    let mut login = None;
    let mut password = "Password1!".to_owned();
    let mut out = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--server" => server = value()?,
            "--login" => login = Some(value()?),
            "--password" => password = value()?,
            "--out" => out = Some(PathBuf::from(value()?)),
            _ => return Err(format!("unknown argument {arg}; see the top of src/main.rs").into()),
        }
    }
    Ok(Args {
        server,
        login: login.ok_or("--login is required")?,
        password,
        out: out.ok_or("--out is required")?,
    })
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(std::io::stderr)
        .init();
    let args = args()?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(snapshot(args))
}

/// What the snapshot has seen so far.
#[derive(Default)]
struct Seen {
    teams: BTreeSet<String>,
    channels: BTreeSet<String>,
    /// Users by id, with their picture's update time.
    users: BTreeMap<String, i64>,
    /// Thread roots with replies.
    roots: BTreeSet<String>,
    /// Image file ids.
    images: BTreeSet<String>,
    me: Option<matty_api::models::User>,
}

impl Seen {
    fn note(&mut self, change: &ChangeSet) {
        match change {
            ChangeSet::Sidebar { team_id, sections } => {
                self.teams.insert(team_id.clone());
                for section in sections.iter() {
                    for channel in section.channels.iter() {
                        self.channels.insert(channel.id.clone());
                        if let Some(user) = &channel.dm_user {
                            self.users.insert(user.id.clone(), user.last_picture_update);
                        }
                    }
                }
            }
            ChangeSet::Posts { posts, .. } | ChangeSet::Thread { posts, .. } => self.posts(posts),
            ChangeSet::ThreadList { roots, .. } | ChangeSet::ReplyRoots { roots, .. } => {
                self.posts(roots)
            }
            ChangeSet::Users(users) => {
                for user in users.iter() {
                    self.users.insert(user.id.clone(), user.last_picture_update);
                }
            }
            ChangeSet::Me(user) => self.me = Some((**user).clone()),
            _ => {}
        }
    }

    fn posts(&mut self, posts: &[matty_api::models::Post]) {
        for post in posts {
            if post.root_id.is_empty() && post.reply_count > 0 {
                self.roots.insert(post.id.clone());
            }
            for file in &post.metadata.files {
                if file.mime_type.starts_with("image/") {
                    self.images.insert(file.id.clone());
                }
            }
            for user in &post.participants {
                self.users.insert(user.id.clone(), user.last_picture_update);
            }
        }
    }
}

/// Applies changes until `done` holds for one (then a quiet moment more),
/// or until the engine has been `quiet` that long when `done` is `None`.
async fn pump(
    changes: &Receiver<ChangeSet>,
    seen: &mut Seen,
    quiet: Duration,
    mut done: Option<&mut dyn FnMut(&ChangeSet) -> bool>,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + STEP;
    let mut finished = done.is_none();
    loop {
        let wait = if finished { quiet } else { STEP };
        let next = tokio::time::timeout_at(
            deadline.min(tokio::time::Instant::now() + wait),
            changes.recv(),
        )
        .await;
        match next {
            Ok(Ok(change)) => {
                if let ChangeSet::Connection(Connection::Failed { error }) = &change {
                    return Err(format!("the server load failed: {error}").into());
                }
                seen.note(&change);
                if let Some(done) = done.as_mut()
                    && done(&change)
                {
                    finished = true;
                }
            }
            Ok(Err(_)) => return Err("the sync engine stopped".into()),
            Err(_) if finished => return Ok(()),
            Err(_) if done.is_some() => {
                eprintln!("note: a step timed out; going on");
                return Ok(());
            }
            Err(_) => return Err("the server took too long to answer".into()),
        }
    }
}

async fn snapshot(args: Args) -> Result<()> {
    let signed = Client::new(&args.server)?
        .login(&Credentials {
            login_id: args.login.clone(),
            password: args.password.clone(),
            mfa_token: None,
            ldap_only: false,
            device_id: String::new(),
        })
        .await?;
    let me = signed.user.clone();
    let work = args.out.with_extension("work");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let database = work.join("cache.sqlite");
    let media = work.join("media");

    let sync = matty_sync::spawn_at(database.clone(), Some(media.clone()))?;
    let changes = sync.subscribe();
    let mut seen = Seen::default();
    sync.send(Command::Connect {
        client: signed.client,
    });
    pump(
        &changes,
        &mut seen,
        QUIET,
        Some(&mut |change| matches!(change, ChangeSet::Connection(Connection::Loaded))),
    )
    .await?;
    eprintln!(
        "loaded {} teams and {} channels",
        seen.teams.len(),
        seen.channels.len()
    );

    for team_id in seen.teams.clone() {
        sync.send(Command::OpenThreadList {
            team_id: team_id.clone(),
        });
        pump(&changes, &mut seen, QUIET, None).await?;
        sync.send(Command::CloseThreadList { team_id });
    }
    for channel_id in seen.channels.clone() {
        load_channel(&sync, &changes, &mut seen, &channel_id).await?;
    }
    eprintln!("loading {} threads", seen.roots.len());
    for root_id in seen.roots.clone() {
        sync.send(Command::OpenThread {
            root_id: root_id.clone(),
        });
        let id = root_id.clone();
        pump(
            &changes,
            &mut seen,
            Duration::from_millis(50),
            Some(
                &mut |change| matches!(change, ChangeSet::Thread { root_id, .. } if *root_id == id),
            ),
        )
        .await?;
        sync.send(Command::CloseThread { root_id });
    }
    eprintln!(
        "loading {} avatars and {} images",
        seen.users.len(),
        seen.images.len()
    );
    // The store's picture times, which the app asks with too.
    for user_id in seen.users.keys().cloned() {
        sync.send(Command::LoadAvatar {
            user_id,
            last_picture_update: None,
        });
    }
    for file_id in seen.images.clone() {
        for size in [ImageSize::Thumbnail, ImageSize::Preview] {
            sync.send(Command::LoadImage {
                file_id: file_id.clone(),
                size,
            });
        }
    }
    pump(&changes, &mut seen, QUIET, None).await?;
    // Let the engine finish its writes and close the database.
    drop(sync);
    drop(changes);
    tokio::time::sleep(Duration::from_secs(1)).await;

    pack(&database, &media, &args, &me)?;
    let size = std::fs::metadata(&args.out)?.len();
    eprintln!(
        "wrote {} ({:.1} MB)",
        args.out.display(),
        size as f64 / 1_000_000.
    );
    std::fs::remove_dir_all(&work)?;
    Ok(())
}

/// Opens `channel_id` and loads its history back to the start, or
/// [`MAX_PAGES`] pages.
async fn load_channel(
    sync: &SyncHandle,
    changes: &Receiver<ChangeSet>,
    seen: &mut Seen,
    channel_id: &str,
) -> Result<()> {
    let id = channel_id.to_owned();
    sync.send(Command::OpenChannel {
        channel_id: id.clone(),
    });
    // The cached posts come at once, the server's a moment later.
    pump(changes, seen, Duration::from_millis(800), None).await?;
    for _ in 0..MAX_PAGES {
        sync.send(Command::LoadOlder {
            channel_id: id.clone(),
        });
        let mut at_start = false;
        pump(
            changes,
            seen,
            Duration::from_millis(50),
            Some(&mut |change| match change {
                ChangeSet::History {
                    channel_id,
                    at_start: start,
                    failed,
                    ..
                } if *channel_id == id => {
                    at_start = *start || *failed;
                    true
                }
                _ => false,
            }),
        )
        .await?;
        if at_start {
            break;
        }
    }
    sync.send(Command::CloseChannel {
        channel_id: id.clone(),
    });
    Ok(())
}

/// Writes the cache to `args.out` as one file, without seeding's system
/// messages: the media bytes move into
/// the `media_bytes` table the web build's cache reads, the session is not
/// kept, and `demo` says whose workspace it is.
fn pack(database: &Path, media: &Path, args: &Args, me: &matty_api::models::User) -> Result<()> {
    let conn = rusqlite::Connection::open(database)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS media_bytes (
             key TEXT PRIMARY KEY NOT NULL,
             bytes BLOB NOT NULL
         ) STRICT;
         CREATE TABLE demo (
             server TEXT NOT NULL,
             user_id TEXT NOT NULL,
             username TEXT NOT NULL,
             display_name TEXT NOT NULL,
             taken_at INTEGER NOT NULL
         ) STRICT;",
    )?;
    // "Joined the channel" and the like are from seeding, not the story.
    conn.execute("DELETE FROM posts WHERE kind LIKE 'system_%'", [])?;
    let entries: Vec<(String, String)> = conn
        .prepare("SELECT key, path FROM media")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (key, path) in entries {
        match std::fs::read(media.join(&path)) {
            Ok(bytes) => {
                conn.execute(
                    "INSERT OR REPLACE INTO media_bytes (key, bytes) VALUES (?1, ?2)",
                    rusqlite::params![key, bytes],
                )?;
            }
            Err(_) => {
                conn.execute("DELETE FROM media WHERE key = ?1", [&key])?;
            }
        }
    }
    let display_name = match (me.first_name.as_str(), me.last_name.as_str()) {
        ("", "") => me.username.clone(),
        (first, "") => first.to_owned(),
        (first, last) => format!("{first} {last}"),
    };
    let taken_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
    conn.execute(
        "INSERT INTO demo (server, user_id, username, display_name, taken_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![args.server, me.id, me.username, display_name, taken_at],
    )?;
    let _ = std::fs::remove_file(&args.out);
    // A plain rollback-journal file, compacted, which SQLite can open from
    // bytes in memory.
    conn.execute("VACUUM INTO ?1", [args.out.to_string_lossy()])?;
    drop(conn);
    let out = rusqlite::Connection::open(&args.out)?;
    out.query_row("PRAGMA journal_mode = DELETE", [], |_| Ok(()))?;
    Ok(())
}
