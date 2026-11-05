//! Writes the upgrade-test fixture for one schema version:
//! `cargo run -p matty-store --example db_fixture -- [version] [path]`.
//! The version defaults to the latest and the path to
//! `crates/matty-store/fixtures/db/v<version>.sqlite`. Use
//! `scripts/make-db-fixture.sh`, and only for a version that has not shipped:
//! a shipped fixture must never change (#32).

use std::path::PathBuf;

use matty_api::models::{
    Channel, ChannelMember, Draft, FileInfo, Post, PostMetadata, Reaction, SidebarCategories,
    SidebarCategory, Team, User, UserThread,
};
use matty_store::{LATEST_VERSION, Store, posts, sidebar};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let version: i64 = match args.next() {
        Some(version) => version.parse()?,
        None => LATEST_VERSION,
    };
    if !(1..=LATEST_VERSION).contains(&version) {
        return Err(format!("version must be 1 to {LATEST_VERSION}").into());
    }
    let path = args.next().map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/db")
            .join(format!("v{version}.sqlite"))
    });
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    let mut store = Store::open_at_version(&path, version)?;
    let tx = store.conn_mut().transaction()?;
    fill(&tx, version)?;
    tx.commit()?;
    // One self-contained, compact file.
    store
        .conn()
        .execute_batch("PRAGMA journal_mode = DELETE; VACUUM;")?;
    drop(store);
    eprintln!("wrote {}", path.display());
    Ok(())
}

/// The same small account at every version: what the upgrade test expects.
fn fill(conn: &rusqlite::Connection, version: i64) -> Result<(), Box<dyn std::error::Error>> {
    sidebar::upsert_teams(
        conn,
        &[Team {
            id: "team1".into(),
            name: "team1".into(),
            display_name: "Team One".into(),
            kind: "O".into(),
            ..Team::default()
        }],
    )?;
    sidebar::upsert_users(
        conn,
        &[
            User {
                id: "me".into(),
                username: "me".into(),
                ..User::default()
            },
            User {
                id: "alice".into(),
                username: "alice".into(),
                ..User::default()
            },
        ],
    )?;
    let channel = |id: &str, team: &str, kind: &str, name: &str| Channel {
        id: id.into(),
        team_id: team.into(),
        kind: kind.into(),
        name: name.into(),
        display_name: id.into(),
        total_msg_count: 5,
        total_msg_count_root: 4,
        last_post_at: 50,
        ..Channel::default()
    };
    sidebar::upsert_channels(
        conn,
        &[
            channel("town", "team1", "O", "town-square"),
            channel("dev", "team1", "P", "dev"),
            channel("dm", "", "D", "alice__me"),
        ],
    )?;
    let member = |channel: &str| ChannelMember {
        channel_id: channel.into(),
        user_id: "me".into(),
        msg_count: 3,
        msg_count_root: 3,
        mention_count: 1,
        ..ChannelMember::default()
    };
    sidebar::upsert_channel_members(conn, &[member("town"), member("dev"), member("dm")])?;
    let category = |id: &str, kind: &str, channels: &[&str]| SidebarCategory {
        id: id.into(),
        team_id: "team1".into(),
        kind: kind.into(),
        display_name: id.into(),
        channel_ids: channels.iter().map(|c| (*c).to_owned()).collect(),
        ..SidebarCategory::default()
    };
    sidebar::replace_categories(
        conn,
        "team1",
        &SidebarCategories {
            categories: vec![
                category("channels", "channels", &["town", "dev"]),
                category("dms", "direct_messages", &["dm"]),
            ],
            order: vec!["channels".into(), "dms".into()],
        },
    )?;

    let post = |id: &str, root: &str, at: i64| Post {
        id: id.into(),
        channel_id: "town".into(),
        user_id: "alice".into(),
        root_id: root.into(),
        create_at: at,
        message: format!("post {id}"),
        ..Post::default()
    };
    let mut with_extras = post("p2", "", 20);
    with_extras.metadata = PostMetadata {
        reactions: vec![Reaction {
            user_id: "me".into(),
            post_id: "p2".into(),
            emoji_name: "+1".into(),
            create_at: 21,
        }],
        files: vec![FileInfo {
            id: "f1".into(),
            post_id: "p2".into(),
            channel_id: "town".into(),
            name: "shot.png".into(),
            extension: "png".into(),
            size: 2048,
            mime_type: "image/png".into(),
            create_at: 20,
            ..FileInfo::default()
        }],
        ..PostMetadata::default()
    };
    posts::insert_posts(
        conn,
        &[
            post("p1", "", 10),
            with_extras,
            post("p3", "p1", 30),
            post("p4", "p1", 40),
            post("p5", "", 50),
        ],
    )?;
    posts::upsert_threads(
        conn,
        "team1",
        &[UserThread {
            id: "p1".into(),
            reply_count: 2,
            last_reply_at: 40,
            unread_replies: 1,
            post: post("p1", "", 10),
            ..UserThread::default()
        }],
    )?;
    posts::upsert_draft(
        conn,
        &Draft {
            channel_id: "dev".into(),
            message: "unsent".into(),
            update_at: 60,
            ..Draft::default()
        },
    )?;
    conn.execute(
        "INSERT INTO outbox (pending_post_id, channel_id, message, created_at)
         VALUES ('pending1', 'dev', 'queued', 70)",
        [],
    )?;
    posts::set_cursor(conn, &posts::channel_since_key("town"), "50")?;

    if version >= 4 {
        conn.execute(
            "UPDATE outbox SET local_files = '[\"/home/me/notes.txt\"]'
             WHERE pending_post_id = 'pending1'",
            [],
        )?;
    }
    if version >= 6 {
        conn.execute(
            "UPDATE outbox SET priority = '{\"priority\":\"urgent\",\"requested_ack\":true,\"persistent_notifications\":false}'
             WHERE pending_post_id = 'pending1'",
            [],
        )?;
    }
    if version >= 2 {
        conn.execute(
            "INSERT INTO media (key, path, size, content_type, accessed_at)
             VALUES ('avatar/alice/1', 'avatar/x', 10, 'image/png', 1)",
            [],
        )?;
    }
    Ok(())
}
