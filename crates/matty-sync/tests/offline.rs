//! What works from the cache with no server, and how commands that need
//! one are refused (#163).
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::{ME, get, next, server_unpooled};
use matty_api::Client;
use matty_sync::{ChangeSet, Command, CommandError, Listing};
use serde_json::json;
use wiremock::MockServer;

/// A server whose channel holds a pinned post and a post the user saved.
async fn cached_server() -> MockServer {
    let server = server_unpooled().await;
    get(
        &format!("users/{ME}/channels/c1/posts/unread"),
        json!({"order": ["p2", "p1"], "posts": {
            "p1": {"id": "p1", "channel_id": "c1", "user_id": "u2", "create_at": 100,
                   "update_at": 100, "message": "pinned", "is_pinned": true},
            "p2": {"id": "p2", "channel_id": "c1", "user_id": "u2", "create_at": 200,
                   "update_at": 200, "message": "saved"},
        }}),
    )
    .with_priority(1)
    .mount(&server)
    .await;
    get(
        "users/me/preferences",
        json!([{"user_id": ME, "category": "flagged_post", "name": "p2", "value": "true"}]),
    )
    .with_priority(1)
    .mount(&server)
    .await;
    server
}

fn ids(change: &ChangeSet) -> Vec<&str> {
    match change {
        ChangeSet::Listing { posts, .. } | ChangeSet::Posts { posts, .. } => {
            posts.iter().map(|post| post.id.as_str()).collect()
        }
        _ => Vec::new(),
    }
}

fn listed(want: Listing) -> impl Fn(&ChangeSet) -> bool {
    move |c| matches!(c, ChangeSet::Listing { listing, .. } if *listing == want)
}

fn refused(change: &ChangeSet) -> Option<&'static str> {
    match change {
        ChangeSet::Failed(CommandError::Offline { action }) => Some(action),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_the_cache_answers_and_server_commands_are_refused() {
    let server = cached_server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("account.sqlite");
    {
        let sync = matty_sync::spawn_at(path.clone(), None).unwrap();
        let changes = sync.subscribe();
        sync.send(Command::OpenChannel {
            channel_id: "c1".into(),
        });
        sync.send(Command::Connect {
            client: Client::new(server.uri()).unwrap().with_token("token"),
        });
        next(&changes, |c| ids(c) == ["p1", "p2"]).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = server.received_requests().await.unwrap().len();

    // `matty --offline`: the cache with no session.
    let sync = matty_sync::spawn_at(path, None).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::ShowCache);
    sync.send(Command::OpenChannel {
        channel_id: "c1".into(),
    });
    next(&changes, |c| ids(c) == ["p1", "p2"]).await;

    // Saved and pinned come from the cache instead of loading forever.
    sync.send(Command::LoadListing(Listing::Saved));
    assert_eq!(ids(&next(&changes, listed(Listing::Saved)).await), ["p2"]);
    let pinned = Listing::Pinned {
        channel_id: "c1".into(),
    };
    sync.send(Command::LoadListing(pinned.clone()));
    assert_eq!(ids(&next(&changes, listed(pinned)).await), ["p1"]);
    // Mentions need the server's search: refused, not empty.
    sync.send(Command::LoadListing(Listing::Mentions {
        team_id: "t1".into(),
        terms: "@me".into(),
    }));
    let change = next(&changes, |c| refused(c).is_some()).await;
    assert_eq!(refused(&change), Some("load recent mentions"));

    // Each command that needs the server says which it was, and changes
    // nothing here.
    let commands = [
        (
            Command::ToggleReaction {
                post_id: "p1".into(),
                emoji_name: "+1".into(),
            },
            "add reaction",
        ),
        (
            Command::SetSaved {
                post_id: "p1".into(),
                saved: true,
            },
            "save message",
        ),
        (
            Command::SetPinned {
                post_id: "p2".into(),
                pinned: true,
            },
            "pin message",
        ),
        (
            Command::EditPost {
                post_id: "p1".into(),
                message: "changed".into(),
            },
            "edit",
        ),
        (
            Command::DeletePost {
                post_id: "p2".into(),
            },
            "delete",
        ),
        (
            Command::Search {
                team_id: "t1".into(),
                terms: "pinned".into(),
                time_zone_offset: 0,
                page: 0,
            },
            "search",
        ),
    ];
    for (command, action) in commands {
        sync.send(command);
        let change = next(&changes, |c| refused(c).is_some()).await;
        assert_eq!(refused(&change), Some(action));
    }
    // Still as cached: no reaction, not saved, nothing edited or gone.
    sync.send(Command::LoadListing(Listing::Saved));
    assert_eq!(ids(&next(&changes, listed(Listing::Saved)).await), ["p2"]);
    sync.send(Command::OpenChannel {
        channel_id: "c1".into(),
    });
    let ChangeSet::Posts { posts, .. } = next(&changes, |c| ids(c) == ["p1", "p2"]).await else {
        unreachable!()
    };
    assert!(posts[0].metadata.reactions.is_empty());
    assert_eq!(posts[0].message, "pinned");
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        before,
        "offline made requests"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_network_reads_as_offline_and_listings_fall_back_to_the_cache() {
    let server = cached_server().await;
    let sync = matty_sync::spawn(matty_store::Store::open_in_memory().unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::OpenChannel {
        channel_id: "c1".into(),
    });
    sync.send(Command::Connect {
        client: Client::new(server.uri()).unwrap().with_token("token"),
    });
    next(&changes, |c| ids(c) == ["p1", "p2"]).await;
    // The network goes: nothing answers at the server's address.
    drop(server);

    sync.send(Command::ToggleReaction {
        post_id: "p2".into(),
        emoji_name: "+1".into(),
    });
    let change = next(&changes, |c| matches!(c, ChangeSet::Failed(_))).await;
    assert_eq!(refused(&change), Some("add reaction"));
    // The reaction shown at once is taken back.
    next(&changes, |c| match c {
        ChangeSet::Posts { posts, .. } => {
            posts.iter().all(|post| post.metadata.reactions.is_empty())
        }
        _ => false,
    })
    .await;

    sync.send(Command::EditPost {
        post_id: "p1".into(),
        message: "changed".into(),
    });
    let change = next(&changes, |c| matches!(c, ChangeSet::Failed(_))).await;
    assert_eq!(refused(&change), Some("edit"));

    let pinned = Listing::Pinned {
        channel_id: "c1".into(),
    };
    sync.send(Command::LoadListing(pinned.clone()));
    assert_eq!(ids(&next(&changes, listed(pinned)).await), ["p1"]);
}
