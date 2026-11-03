//! Sending through the outbox (#38), against a mock server.
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use std::time::Duration;

use matty_api::Client;
use matty_store::Store;
use matty_sync::{ChangeSet, Command, CommandError, Connection, SyncHandle};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ME: &str = "me";

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../matty-api/tests/fixtures/v10.5/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn get(route: &str, body: Value) -> Mock {
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/{route}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
}

async fn server() -> MockServer {
    let server = MockServer::start().await;
    for mock in [
        get("users/me", json!({"id": ME, "username": "me"})),
        get("config/client", fixture("config_client")),
        get("license/client", fixture("license_client")),
        get("users/me/preferences", json!([])),
        get(
            "users/me/teams",
            json!([{"id": "t1", "name": "one", "display_name": "One", "type": "O"}]),
        ),
        get(
            "users/me/teams/t1/channels",
            json!([{"id": "c1", "team_id": "t1", "type": "O", "name": "c1", "display_name": "C1"}]),
        ),
        get(
            &format!("users/{ME}/teams/t1/channels/members"),
            json!([{"channel_id": "c1", "user_id": ME, "notify_props": {"mark_unread": "all"}}]),
        ),
        get(
            "users/me/teams/t1/channels/categories",
            json!({"categories": [{"id": "cat", "team_id": "t1", "type": "channels",
                    "display_name": "Channels", "channel_ids": ["c1"]}], "order": ["cat"]}),
        ),
        get("emoji", json!([])),
        get(
            &format!("users/{ME}/channels/c1/posts/unread"),
            json!({"order": [], "posts": {}}),
        ),
    ] {
        mock.mount(&server).await;
    }
    server
}

fn created() -> ResponseTemplate {
    ResponseTemplate::new(201).set_body_json(json!({
        "id": "server1", "channel_id": "c1", "user_id": ME, "create_at": 100,
        "update_at": 100, "message": "hello", "pending_post_id": "me:1"
    }))
}

async fn connected(server: &MockServer) -> (SyncHandle, async_channel::Receiver<ChangeSet>) {
    let sync = matty_sync::spawn(Store::open_in_memory().unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::OpenChannel {
        channel_id: "c1".into(),
    });
    sync.send(Command::Connect {
        client: Client::new(server.uri()).unwrap().with_token("token"),
    });
    next(&changes, |c| {
        matches!(c, ChangeSet::Connection(Connection::Loaded))
    })
    .await;
    (sync, changes)
}

async fn next(
    changes: &async_channel::Receiver<ChangeSet>,
    stop: impl Fn(&ChangeSet) -> bool,
) -> ChangeSet {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let change = changes.recv().await.unwrap();
            if stop(&change) {
                return change;
            }
        }
    })
    .await
    .unwrap()
}

fn send() -> Command {
    Command::SendPost {
        pending_post_id: "me:1".into(),
        channel_id: "c1".into(),
        root_id: String::new(),
        message: "hello".into(),
        file_ids: Vec::new(),
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
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "id": "app.post.save.app_error", "message": "Unable to save.", "status_code": 500
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

#[tokio::test(flavor = "multi_thread")]
async fn an_unsent_post_cannot_be_edited_or_deleted() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .respond_with(created().set_delay(Duration::from_secs(2)))
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    sync.send(send());
    sync.send(Command::EditPost {
        post_id: "me:1".into(),
        message: "changed".into(),
    });
    sync.send(Command::DeletePost {
        post_id: "me:1".into(),
    });
    for _ in 0..2 {
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
