//! Runs the initial load against a real server when `MATTY_TEST_SERVER` is
//! set, such as the seeded dev server (docs/dev-server.md):
//! `MATTY_TEST_SERVER=http://localhost:8065 MATTY_TEST_PASSWORD='Password1!'
//! cargo test -p matty-sync --test live`. Without it, the test returns at once.
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use std::time::Duration;

use matty_api::ws::{ConnectionState, WsConnection, WsMessage};
use matty_api::{Client, Credentials, NewPost, PostPatch};
use matty_store::Store;
use matty_store::sidebar::{self, UnreadMode};
use matty_sync::{ChangeSet, Command, Connection};

/// The tests share one server and change its read state, so they take
/// turns.
static SERVER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test(flavor = "multi_thread")]
async fn the_loaded_sidebar_matches_the_server() {
    let Ok(url) = std::env::var("MATTY_TEST_SERVER") else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return;
    };
    let _turn = SERVER.lock().await;
    let login = std::env::var("MATTY_TEST_LOGIN").unwrap_or_else(|_| "tester".into());
    let password = std::env::var("MATTY_TEST_PASSWORD").unwrap();
    let signed_in = Client::new(url)
        .unwrap()
        .login(&Credentials {
            login_id: login,
            password,
            device_id: "matty-sync-live-test".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let client = signed_in.client;
    let me = signed_in.user.id;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("account.sqlite");
    let sync = matty_sync::spawn(Store::open(&path).unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::Connect {
        client: client.clone(),
    });
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match changes.recv().await.unwrap() {
                ChangeSet::Connection(Connection::Loaded) => break,
                ChangeSet::Connection(Connection::Failed { error })
                | ChangeSet::Connection(Connection::TeamFailed { error, .. }) => {
                    panic!("load failed: {error}")
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    drop(sync);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Every channel's unreads in the store match the server's counts, with
    // threads expanded (the server's own message counts).
    let store = Store::open(&path).unwrap();
    for team in client.my_teams().await.unwrap() {
        let channels = client.my_channels(&team.id).await.unwrap();
        let members = client.my_channel_members(&me, &team.id).await.unwrap();
        let sections =
            sidebar::sidebar_snapshot(store.conn(), &team.id, &me, UnreadMode::AllPosts).unwrap();
        for channel in channels.iter().filter(|c| c.delete_at == 0) {
            let Some(member) = members.iter().find(|m| m.channel_id == channel.id) else {
                continue;
            };
            let expected = sidebar::unreads(channel, member, UnreadMode::AllPosts);
            let shown = sections
                .iter()
                .flat_map(|s| s.channels.iter())
                .find(|c| c.id == channel.id)
                .unwrap_or_else(|| panic!("{} is not in the sidebar", channel.name));
            assert_eq!(shown.unreads, expected, "{}", channel.name);
        }
    }
}

/// Waits for a connection change set; panics on a load failure.
async fn wait_for(changes: &async_channel::Receiver<ChangeSet>, target: Connection) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match changes.recv().await.unwrap() {
                ChangeSet::Connection(state) if state == target => break,
                ChangeSet::Connection(Connection::Failed { error })
                | ChangeSet::Connection(Connection::TeamFailed { error, .. }) => {
                    panic!("load failed: {error}")
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resync_catches_up_on_changes_made_while_away() {
    let Ok(url) = std::env::var("MATTY_TEST_SERVER") else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return;
    };
    let _turn = SERVER.lock().await;
    let login = std::env::var("MATTY_TEST_LOGIN").unwrap_or_else(|_| "tester".into());
    let password = std::env::var("MATTY_TEST_PASSWORD").unwrap();
    let signed_in = Client::new(url)
        .unwrap()
        .login(&Credentials {
            login_id: login,
            password,
            device_id: "matty-sync-resync-test".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let client = signed_in.client;
    let team = client.my_teams().await.unwrap().remove(0);
    let channel = client
        .my_channels(&team.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.name == "town-square")
        .unwrap();
    let send = |message: &str| NewPost {
        channel_id: channel.id.clone(),
        message: message.into(),
        ..NewPost::default()
    };
    let kept = client.create_post(&send("resync: kept")).await.unwrap();
    let doomed = client.create_post(&send("resync: deleted")).await.unwrap();

    // Online once, so the channel and both posts are cached.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("account.sqlite");
    {
        let sync = matty_sync::spawn(Store::open(&path).unwrap()).unwrap();
        let changes = sync.subscribe();
        sync.send(Command::OpenChannel {
            channel_id: channel.id.clone(),
        });
        sync.send(Command::Connect {
            client: client.clone(),
        });
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let ChangeSet::Posts { posts, .. } = changes.recv().await.unwrap()
                    && posts.iter().any(|p| p.id == doomed.id)
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Away: an edit, a delete, a new post and a reaction.
    client
        .patch_post(
            &kept.id,
            &PostPatch {
                message: Some("resync: kept, edited".into()),
                ..PostPatch::default()
            },
        )
        .await
        .unwrap();
    client.delete_post(&doomed.id).await.unwrap();
    let added = client.create_post(&send("resync: new")).await.unwrap();
    client
        .post::<_, serde_json::Value>(
            "reactions",
            &serde_json::json!({
                "user_id": signed_in.user.id, "post_id": kept.id, "emoji_name": "tada"
            }),
        )
        .await
        .unwrap();

    // Back: the load, then a catch-up.
    let sync = matty_sync::spawn(Store::open(&path).unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::Connect {
        client: client.clone(),
    });
    wait_for(&changes, Connection::Loaded).await;
    sync.send(Command::Resync);
    wait_for(&changes, Connection::Resynced).await;
    drop(sync);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let store = Store::open(&path).unwrap();
    let conn = store.conn();
    let edited = matty_store::posts::post(conn, &kept.id).unwrap().unwrap();
    assert_eq!(edited.message, "resync: kept, edited");
    let emoji: Vec<&str> = edited
        .metadata
        .reactions
        .iter()
        .map(|r| r.emoji_name.as_str())
        .collect();
    assert_eq!(emoji, ["tada"]);
    let gone = matty_store::posts::post(conn, &doomed.id).unwrap().unwrap();
    assert!(gone.delete_at > 0, "the delete was applied");
    assert!(matty_store::posts::post(conn, &added.id).unwrap().is_some());
}

/// Signs in as `login` (or `MATTY_TEST_LOGIN`, then `tester`).
async fn sign_in(url: &str, login: Option<&str>, device: &str) -> (Client, String) {
    let login = login.map_or_else(
        || std::env::var("MATTY_TEST_LOGIN").unwrap_or_else(|_| "tester".into()),
        str::to_owned,
    );
    let signed_in = Client::new(url)
        .unwrap()
        .login(&Credentials {
            login_id: login,
            password: std::env::var("MATTY_TEST_PASSWORD").unwrap(),
            device_id: device.into(),
            ..Default::default()
        })
        .await
        .unwrap();
    (signed_in.client, signed_in.user.id)
}

/// Polls `check` until it holds, for up to ten seconds.
async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..100 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn commands_reach_the_server() {
    let Ok(url) = std::env::var("MATTY_TEST_SERVER") else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return;
    };
    let _turn = SERVER.lock().await;
    let (client, me) = sign_in(&url, None, "matty-sync-commands-test").await;
    // Someone else posts, so there is something to read and to type at.
    let other_login = std::env::var("MATTY_TEST_OTHER_LOGIN").unwrap_or_else(|_| "admin".into());
    let (other, _) = sign_in(&url, Some(&other_login), "matty-sync-commands-other").await;
    let team = client.my_teams().await.unwrap().remove(0);
    let channel = client
        .my_channels(&team.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.name == "town-square")
        .unwrap();
    let post = other
        .create_post(&NewPost {
            channel_id: channel.id.clone(),
            message: "commands: read me".into(),
            ..NewPost::default()
        })
        .await
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let sync = matty_sync::spawn(Store::open(&dir.path().join("a.sqlite")).unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::Connect {
        client: client.clone(),
    });
    // Loaded, with the socket up for typing; either may come first.
    tokio::time::timeout(Duration::from_secs(30), async {
        let (mut loaded, mut socket) = (false, false);
        while !(loaded && socket) {
            match changes.recv().await.unwrap() {
                ChangeSet::Connection(Connection::Loaded) => loaded = true,
                ChangeSet::Connection(Connection::Socket(ConnectionState::Connected {
                    ..
                })) => socket = true,
                ChangeSet::Connection(Connection::Failed { error }) => panic!("{error}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap();

    let read_up_to = |client: Client, channel_id: String| async move {
        let member = client.my_channel_member(&channel_id).await.unwrap();
        let channel = client.channel(&channel_id).await.unwrap();
        member.msg_count == channel.total_msg_count
    };

    // On screen, as when reacting: toggling reads the cached reactions.
    sync.send(Command::OpenChannel {
        channel_id: channel.id.clone(),
    });
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let ChangeSet::Posts { posts, .. } = changes.recv().await.unwrap()
                && posts.iter().any(|p| p.id == post.id)
            {
                break;
            }
        }
    })
    .await
    .unwrap();

    // Viewing reads the channel on the server.
    sync.send(Command::ViewChannel {
        channel_id: channel.id.clone(),
        prev_channel_id: String::new(),
    });
    eventually("the channel to be read", || {
        read_up_to(client.clone(), channel.id.clone())
    })
    .await;

    // Marking a post unread makes it unread there again.
    sync.send(Command::MarkUnread {
        post_id: post.id.clone(),
    });
    eventually("the post to be unread", || {
        let client = client.clone();
        let channel_id = channel.id.clone();
        async move { !read_up_to(client, channel_id).await }
    })
    .await;

    // A reaction toggles on, then off.
    let reacted = |client: Client, post_id: String, me: String| async move {
        let post = client.get_post(&post_id).await.unwrap();
        post.metadata
            .reactions
            .iter()
            .any(|r| r.user_id == me && r.emoji_name == "+1")
    };
    for on in [true, false] {
        sync.send(Command::ToggleReaction {
            post_id: post.id.clone(),
            emoji_name: "+1".into(),
        });
        eventually("the reaction to toggle", || {
            let check = reacted(client.clone(), post.id.clone(), me.clone());
            async move { check.await == on }
        })
        .await;
    }

    // Drafts sync when the server allows it; an empty one deletes.
    let capabilities = client.capabilities().await.unwrap();
    if capabilities.synced_drafts {
        let has_draft = |client: Client, me: String, team_id: String| async move {
            client
                .drafts(&me, &team_id)
                .await
                .unwrap()
                .iter()
                .any(|d| d.message == "commands: a draft")
        };
        for message in ["commands: a draft", ""] {
            sync.send(Command::SaveDraft {
                channel_id: channel.id.clone(),
                root_id: String::new(),
                message: message.into(),
                file_ids: Vec::new(),
            });
            eventually("the draft to sync", || {
                let check = has_draft(client.clone(), me.clone(), team.id.clone());
                async move { check.await == !message.is_empty() }
            })
            .await;
        }
    }

    // Status changes, then goes back.
    for status in ["dnd", "online"] {
        sync.send(Command::SetStatus {
            status: status.into(),
            dnd_end_time: None,
        });
        eventually("the status to change", || {
            let client = client.clone();
            let me = me.clone();
            async move { client.statuses(&[me]).await.unwrap()[0].status == status }
        })
        .await;
    }

    // Typing twice in a row reaches the other user once.
    let (socket, mut events) = WsConnection::connect(&other).await.unwrap();
    sync.send(Command::Typing {
        channel_id: channel.id.clone(),
        parent_id: String::new(),
    });
    sync.send(Command::Typing {
        channel_id: channel.id.clone(),
        parent_id: String::new(),
    });
    let mut notices = 0;
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(message) = events.recv().await {
            if let WsMessage::Event(raw) = message
                && raw.event == "typing"
                && raw.data["user_id"] == me.as_str()
            {
                notices += 1;
            }
        }
    })
    .await;
    assert_eq!(notices, 1);
    drop(socket);
}
