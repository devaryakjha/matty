//! A mock server with one team and one channel, and a connected engine.
#![allow(dead_code)] // Each test file uses some of these.

use std::time::Duration;

use matty_api::Client;
use matty_store::Store;
use matty_sync::{ChangeSet, Command, Connection, SyncHandle};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub const ME: &str = "me";

pub fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../matty-api/tests/fixtures/v10.5/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

pub fn get(route: &str, body: Value) -> Mock {
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/{route}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
}

pub async fn server() -> MockServer {
    server_with_config(json!({})).await
}

/// [`server`], with `overrides` laid over the recorded client config.
pub async fn server_with_config(overrides: Value) -> MockServer {
    account_on(MockServer::start().await, overrides).await
}

/// [`server`] on a server of its own rather than one from wiremock's pool,
/// so dropping it closes its port, as a network that went away (#163).
pub async fn server_unpooled() -> MockServer {
    account_on(MockServer::builder().start().await, json!({})).await
}

async fn account_on(server: MockServer, overrides: Value) -> MockServer {
    let mut config = fixture("config_client");
    for (key, value) in overrides.as_object().unwrap() {
        config[key] = value.clone();
    }
    for mock in [
        get("users/me", json!({"id": ME, "username": "me"})),
        get("config/client", config),
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

pub fn created() -> ResponseTemplate {
    ResponseTemplate::new(201).set_body_json(json!({
        "id": "server1", "channel_id": "c1", "user_id": ME, "create_at": 100,
        "update_at": 100, "message": "hello", "pending_post_id": "me:1"
    }))
}

pub async fn connected(server: &MockServer) -> (SyncHandle, async_channel::Receiver<ChangeSet>) {
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

/// The paths of the requests `server` has had, such as `/api/v4/emoji`.
pub async fn requested(server: &MockServer) -> Vec<String> {
    let requests = server.received_requests().await.unwrap_or_default();
    requests.iter().map(|r| r.url.path().to_owned()).collect()
}

pub async fn next(
    changes: &async_channel::Receiver<ChangeSet>,
    stop: impl Fn(&ChangeSet) -> bool,
) -> ChangeSet {
    tokio::time::timeout(Duration::from_secs(15), async {
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
