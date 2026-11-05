//! Sending through the outbox (#38), against a mock server.
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use std::time::Duration;

use matty_api::Client;
use matty_store::Store;
use matty_sync::{ChangeSet, Command, CommandError, Connection};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

mod common;

use common::{ME, connected, created, next, server};

fn send() -> Command {
    Command::SendPost {
        pending_post_id: "me:1".into(),
        channel_id: "c1".into(),
        root_id: String::new(),
        message: "hello".into(),
        file_ids: Vec::new(),
        local_files: Vec::new(),
        props: Default::default(),
        priority: None,
    }
}

fn window(change: &ChangeSet) -> Vec<String> {
    match change {
        ChangeSet::Posts { posts, .. } => posts.iter().map(|p| p.id.clone()).collect(),
        _ => Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_send_keeps_its_text_and_a_retry_sends_it() {
    let server = server().await;
    // A refusal, not a server fault: it waits for Retry.
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "id": "api.post.create_post.bad", "message": "Bad post.", "status_code": 400
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .respond_with(created())
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;

    sync.send(send());
    let optimistic = next(&changes, |c| window(c) == ["me:1"]).await;
    let ChangeSet::Posts { posts, .. } = optimistic else {
        unreachable!()
    };
    assert_eq!(posts[0].message, "hello");
    let failed = next(&changes, |c| {
        matches!(c, ChangeSet::Outbox { sends, .. } if sends.first().is_some_and(|s| s.last_error.is_some()))
    })
    .await;
    let ChangeSet::Outbox { sends, .. } = failed else {
        unreachable!()
    };
    assert_eq!(sends[0].message, "hello");
    assert_eq!(sends[0].attempts, 1);

    sync.send(Command::Retry {
        pending_post_id: "me:1".into(),
    });
    // The outbox empties, then the row carries the server's id.
    next(
        &changes,
        |c| matches!(c, ChangeSet::Outbox { sends, .. } if sends.is_empty()),
    )
    .await;
    next(&changes, |c| window(c) == ["server1"]).await;
}

/// A send on its way may already be on the server, so it can be neither
/// changed nor dropped until it comes back.
#[tokio::test(flavor = "multi_thread")]
async fn a_send_on_its_way_cannot_be_edited_or_discarded() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .respond_with(created().set_delay(Duration::from_secs(2)))
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    sync.send(send());
    // On its way: one attempt started.
    next(&changes, |c| {
        matches!(c, ChangeSet::Outbox { sends, .. } if sends.first().is_some_and(|s| s.attempts == 1))
    })
    .await;
    sync.send(Command::EditPost {
        post_id: "me:1".into(),
        message: "changed".into(),
    });
    sync.send(Command::Discard {
        pending_post_id: "me:1".into(),
    });
    sync.send(Command::DeletePost {
        post_id: "me:1".into(),
    });
    for _ in 0..3 {
        let refused = next(&changes, |c| matches!(c, ChangeSet::Failed(_))).await;
        assert_eq!(
            refused,
            ChangeSet::Failed(CommandError::NotSentYet {
                post_id: "me:1".into()
            })
        );
    }
    // Once sent, the edit goes through.
    next(&changes, |c| window(c) == ["server1"]).await;
    Mock::given(method("PUT"))
        .and(path("/api/v4/posts/server1/patch"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "server1", "channel_id": "c1", "user_id": ME, "create_at": 100,
            "update_at": 200, "edit_at": 200, "message": "changed"
        })))
        .expect(1)
        .mount(&server)
        .await;
    sync.send(Command::EditPost {
        post_id: "server1".into(),
        message: "changed".into(),
    });
    next(&changes, |c| {
        matches!(c, ChangeSet::Posts { posts, .. } if posts.first().is_some_and(|p| p.message == "changed"))
    })
    .await;
}

/// A send still waiting its turn can be edited, keeping its place, or
/// discarded, before it goes (#195).
#[tokio::test(flavor = "multi_thread")]
async fn a_waiting_send_can_be_edited_or_discarded() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .respond_with(Echo(Duration::from_millis(800)))
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    // The first goes at once; the others wait behind it.
    for (id, message) in [("me:1", "hello"), ("me:2", "second"), ("me:3", "third")] {
        sync.send(post_in("c1", id, message));
    }
    sync.send(Command::EditPost {
        post_id: "me:2".into(),
        message: "second, fixed".into(),
    });
    sync.send(Command::Discard {
        pending_post_id: "me:3".into(),
    });
    let edited = next(&changes, |c| {
        matches!(c, ChangeSet::Outbox { sends, .. } if sends.iter().any(|s| s.message == "second, fixed"))
    })
    .await;
    let ChangeSet::Outbox { sends, .. } = edited else {
        unreachable!()
    };
    let queued: Vec<&str> = sends.iter().map(|s| s.pending_post_id.as_str()).collect();
    assert_eq!(queued, ["me:1", "me:2"]);
    next(
        &changes,
        |c| matches!(c, ChangeSet::Outbox { sends, .. } if sends.is_empty()),
    )
    .await;
    assert_eq!(
        sent(&server).await,
        [
            ("c1".to_owned(), "hello".to_owned()),
            ("c1".to_owned(), "second, fixed".to_owned())
        ]
    );
}

/// Posts written while the server cannot take them are still sent after
/// the app is killed and launched again: each once, in order per channel
/// (#194).
#[tokio::test(flavor = "multi_thread")]
async fn queued_posts_survive_a_relaunch_and_go_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let path_on_disk = dir.path().join("account.sqlite");
    let written = [
        ("c1", "me:1", "one"),
        ("c2", "me:2", "two"),
        ("c1", "me:3", "three"),
        ("c2", "me:4", "four"),
        ("c1", "me:5", "five"),
    ];

    // First run: the server takes nothing, so everything stays queued.
    {
        let down = server().await;
        Mock::given(method("POST"))
            .and(path("/api/v4/posts"))
            .respond_with(ResponseTemplate::new(503).set_body_json(json!({
                "id": "app.busy", "message": "Busy.", "status_code": 503
            })))
            .mount(&down)
            .await;
        let sync = matty_sync::spawn(Store::open(&path_on_disk).unwrap()).unwrap();
        let changes = sync.subscribe();
        sync.send(Command::Connect {
            client: Client::new(down.uri()).unwrap().with_token("token"),
        });
        next(&changes, |c| {
            matches!(c, ChangeSet::Connection(Connection::Loaded))
        })
        .await;
        for (channel, id, message) in written {
            sync.send(post_in(channel, id, message));
        }
        next(&changes, |c| {
            matches!(c, ChangeSet::Outbox { channel_id, sends } if channel_id == "c1" && sends.len() == 3)
        })
        .await;
        // Killed: the engine stops with nothing delivered.
        drop(sync);
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    {
        let store = Store::open(&path_on_disk).unwrap();
        let queued: Vec<String> = ["c1", "c2"]
            .into_iter()
            .flat_map(|c| matty_store::outbox::for_channel(store.conn(), c).unwrap())
            .map(|outgoing| outgoing.pending_post_id)
            .collect();
        assert_eq!(queued, ["me:1", "me:3", "me:5", "me:2", "me:4"]);
    }

    // Relaunch against a server that is back.
    let up = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .respond_with(Echo(Duration::from_millis(50)))
        .mount(&up)
        .await;
    let sync = matty_sync::spawn(Store::open(&path_on_disk).unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::Connect {
        client: Client::new(up.uri()).unwrap().with_token("token"),
    });
    tokio::time::timeout(Duration::from_secs(15), async {
        while sent(&up).await.len() < written.len() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    // Let anything extra arrive before counting.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let arrived = sent(&up).await;
    for channel in ["c1", "c2"] {
        let got: Vec<&str> = arrived
            .iter()
            .filter(|(c, _)| c == channel)
            .map(|(_, m)| m.as_str())
            .collect();
        let want: Vec<&str> = written
            .iter()
            .filter(|(c, _, _)| *c == channel)
            .map(|(_, _, m)| *m)
            .collect();
        assert_eq!(got, want, "{channel}");
    }
    drop(changes);
    drop(sync);
    let store = Store::open(&path_on_disk).unwrap();
    assert!(
        matty_store::outbox::channels(store.conn())
            .unwrap()
            .is_empty()
    );
}

fn post_in(channel: &str, id: &str, message: &str) -> Command {
    Command::SendPost {
        pending_post_id: id.into(),
        channel_id: channel.into(),
        root_id: String::new(),
        message: message.into(),
        file_ids: Vec::new(),
        local_files: Vec::new(),
        props: Default::default(),
        priority: None,
    }
}

/// Creates whatever post it is sent, as the server would, after a pause.
struct Echo(Duration);

impl wiremock::Respond for Echo {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        let pending = body["pending_post_id"].as_str().unwrap_or_default();
        ResponseTemplate::new(201)
            .set_body_json(json!({
                "id": format!("s-{pending}"), "channel_id": body["channel_id"], "user_id": ME,
                "create_at": 100, "update_at": 100, "message": body["message"],
                "pending_post_id": pending
            }))
            .set_delay(self.0)
    }
}

/// The posts the server was sent, as (channel, message), in arrival order.
async fn sent(server: &wiremock::MockServer) -> Vec<(String, String)> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == "/api/v4/posts")
        .map(|request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            (
                body["channel_id"].as_str().unwrap().to_owned(),
                body["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_fault_is_tried_again_without_retry() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({
            "id": "app.busy", "message": "Busy.", "status_code": 503
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .respond_with(created())
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    sync.send(send());
    // No failure shows: after a pause the same send goes again.
    next(&changes, |c| window(c) == ["server1"]).await;
}

/// Two sends in one channel reach the server in the order they were
/// written, even when the first is slow.
#[tokio::test(flavor = "multi_thread")]
async fn sends_in_a_channel_go_one_at_a_time_in_order() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .respond_with(created().set_delay(Duration::from_millis(500)))
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    sync.send(send());
    sync.send(Command::SendPost {
        pending_post_id: "me:2".into(),
        channel_id: "c1".into(),
        root_id: String::new(),
        message: "second".into(),
        file_ids: Vec::new(),
        local_files: Vec::new(),
        props: Default::default(),
        priority: None,
    });
    // While the first is on its way, the second waits.
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(post_requests(&server).await, 1);
    next(
        &changes,
        |c| matches!(c, ChangeSet::Outbox { sends, .. } if sends.is_empty()),
    )
    .await;
    let bodies: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == "/api/v4/posts")
        .map(|request| {
            serde_json::from_slice::<serde_json::Value>(&request.body).unwrap()["message"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(bodies, ["hello", "second"]);
}

async fn post_requests(server: &wiremock::MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == "/api/v4/posts")
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_posts_properties_reach_the_server() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .respond_with(created())
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    sync.send(Command::SendPost {
        pending_post_id: "me:3".into(),
        channel_id: "c1".into(),
        root_id: String::new(),
        message: "see https://github.com/devaryakjha/matty".into(),
        file_ids: Vec::new(),
        local_files: Vec::new(),
        props: [("remove_link_preview".to_owned(), "true".into())].into(),
        priority: None,
    });
    next(
        &changes,
        |c| matches!(c, ChangeSet::Outbox { sends, .. } if sends.is_empty()),
    )
    .await;
    let request = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|request| request.url.path() == "/api/v4/posts")
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["props"]["remove_link_preview"], "true");
}

/// Signing out drops what is still queued, so a later sign in to the same
/// cache sends nothing written in this session (#194).
#[tokio::test(flavor = "multi_thread")]
async fn sign_out_clears_the_outbox() {
    let dir = tempfile::tempdir().unwrap();
    let path_on_disk = dir.path().join("account.sqlite");
    let down = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({
            "id": "app.busy", "message": "Busy.", "status_code": 503
        })))
        .mount(&down)
        .await;
    let sync = matty_sync::spawn(Store::open(&path_on_disk).unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::Connect {
        client: Client::new(down.uri()).unwrap().with_token("token"),
    });
    next(&changes, |c| {
        matches!(c, ChangeSet::Connection(Connection::Loaded))
    })
    .await;
    sync.send(post_in("c1", "me:1", "one"));
    sync.send(post_in("c2", "me:2", "two"));
    next(&changes, |c| {
        matches!(c, ChangeSet::Outbox { channel_id, sends } if channel_id == "c2" && sends.len() == 1)
    })
    .await;
    sync.send(Command::SignOut);
    next(&changes, |c| {
        matches!(c, ChangeSet::Connection(Connection::SignedOut))
    })
    .await;
    drop(changes);
    drop(sync);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let store = Store::open(&path_on_disk).unwrap();
    assert!(
        matty_store::outbox::channels(store.conn())
            .unwrap()
            .is_empty()
    );
    assert!(
        matty_store::posts::post(store.conn(), "me:1")
            .unwrap()
            .is_none()
    );
}

/// A file attached while offline waits in the outbox by its path; once
/// the post goes, the file uploads first and the post carries its id
/// (#194). One no longer on disk fails the send for the user to see.
#[tokio::test(flavor = "multi_thread")]
async fn files_attached_offline_upload_before_their_post() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/files"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "file_infos": [{"id": "f9", "name": "notes.txt", "size": 5}],
            "client_ids": ["x"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .respond_with(created())
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, b"notes").unwrap();
    let (sync, changes) = connected(&server).await;

    sync.send(send_with("me:1", vec![file]));
    next(
        &changes,
        |c| matches!(c, ChangeSet::Outbox { sends, .. } if sends.is_empty()),
    )
    .await;
    let requests = server.received_requests().await.unwrap();
    let paths: Vec<&str> = requests.iter().map(|r| r.url.path()).collect();
    let files_at = paths.iter().position(|p| *p == "/api/v4/files").unwrap();
    let post_at = paths.iter().position(|p| *p == "/api/v4/posts").unwrap();
    assert!(files_at < post_at, "{paths:?}");
    let body: serde_json::Value = serde_json::from_slice(&requests[post_at].body).unwrap();
    assert_eq!(body["file_ids"], json!(["f9"]));

    sync.send(send_with("me:2", vec![dir.path().join("gone.txt")]));
    let failed = next(&changes, |c| {
        matches!(c, ChangeSet::Outbox { sends, .. } if sends.first().is_some_and(|s| s.last_error.is_some()))
    })
    .await;
    let ChangeSet::Outbox { sends, .. } = failed else {
        unreachable!()
    };
    assert!(
        sends[0]
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("gone.txt")),
        "{sends:?}"
    );
    assert_eq!(post_requests(&server).await, 1);
}

fn send_with(id: &str, local_files: Vec<std::path::PathBuf>) -> Command {
    Command::SendPost {
        pending_post_id: id.into(),
        channel_id: "c1".into(),
        root_id: String::new(),
        message: "with a file".into(),
        file_ids: Vec::new(),
        local_files,
        props: Default::default(),
        priority: None,
    }
}
