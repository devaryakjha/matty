//! Resync and backfill after missed events (#37), against a mock server.
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use std::path::{Path, PathBuf};
use std::time::Duration;

use matty_api::Client;
use matty_api::models::Post;
use matty_store::posts::{self, Window};
use matty_store::{Store, sidebar};
use matty_sync::{ChangeSet, Command, Connection};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ME: &str = "me";
const HOUR: i64 = 60 * 60 * 1000;

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

fn post_json(id: &str, at: i64, delete_at: i64) -> Value {
    json!({
        "id": id, "channel_id": "c1", "user_id": "u2", "create_at": at,
        "update_at": at.max(delete_at), "delete_at": delete_at, "message": format!("post {id}")
    })
}

fn list(posts: &[Value]) -> Value {
    let order: Vec<&str> = posts.iter().map(|p| p["id"].as_str().unwrap()).collect();
    let by_id: serde_json::Map<String, Value> = posts
        .iter()
        .map(|p| (p["id"].as_str().unwrap().to_owned(), p.clone()))
        .collect();
    json!({"order": order, "posts": by_id, "next_post_id": "", "prev_post_id": ""})
}

/// Team t1 with channel c1, whose newest post on the server is at
/// `last_post_at`.
async fn server(last_post_at: i64) -> MockServer {
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
            json!([{"id": "c1", "team_id": "t1", "type": "O", "name": "c1",
                    "display_name": "C1", "last_post_at": last_post_at,
                    "total_msg_count": 3, "total_msg_count_root": 3}]),
        ),
        get(
            &format!("users/{ME}/teams/t1/channels/members"),
            json!([{"channel_id": "c1", "user_id": ME, "msg_count": 3, "msg_count_root": 3,
                    "notify_props": {"mark_unread": "all"}}]),
        ),
        get(
            "users/me/teams/t1/channels/categories",
            json!({"categories": [{"id": "cat", "team_id": "t1", "type": "channels",
                    "display_name": "Channels", "channel_ids": ["c1"]}], "order": ["cat"]}),
        ),
        get("emoji", json!([])),
    ] {
        mock.mount(&server).await;
    }
    server
}

/// A cache holding c1's posts p1 and p2, read up to 2000.
fn cache(dir: &Path) -> PathBuf {
    let path = dir.join("account.sqlite");
    let store = Store::open(&path).unwrap();
    let post = |id: &str, at: i64| Post {
        id: id.into(),
        channel_id: "c1".into(),
        user_id: "u2".into(),
        create_at: at,
        update_at: at,
        message: format!("post {id}"),
        ..Post::default()
    };
    posts::insert_posts(store.conn(), &[post("p1", 1000), post("p2", 2000)]).unwrap();
    posts::set_cursor(store.conn(), "me", ME).unwrap();
    path
}

/// Connects, waits for the load, asks for a resync and waits for it.
async fn resync(server: &MockServer, path: &Path) -> Store {
    let sync = matty_sync::spawn(Store::open(path).unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::Connect {
        client: Client::new(server.uri()).unwrap().with_token("token"),
    });
    let wait = |target: Connection| {
        let changes = changes.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let ChangeSet::Connection(state) = changes.recv().await.unwrap()
                        && state == target
                    {
                        break;
                    }
                }
            })
            .await
            .unwrap();
        }
    };
    wait(Connection::Loaded).await;
    sync.send(Command::Resync);
    wait(Connection::Resynced).await;
    drop(sync);
    tokio::time::sleep(Duration::from_millis(100)).await;
    Store::open(path).unwrap()
}

fn window(store: &Store) -> Vec<String> {
    posts::posts_window(store.conn(), "c1", Window::Latest, 100, false)
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn since_adds_new_posts_and_marks_deletes() {
    let server = server(4000).await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/posts"))
        .and(query_param("since", "2000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(list(&[
            post_json("p3", 4000, 0),
            post_json("p2", 2000, 4500),
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let store = resync(&server, &cache(dir.path())).await;
    assert_eq!(window(&store), ["p1", "p3"]);
    assert_eq!(
        posts::post(store.conn(), "p2").unwrap().unwrap().delete_at,
        4500
    );
    let cursor = posts::cursor(store.conn(), &posts::channel_since_key("c1")).unwrap();
    assert_eq!(cursor.as_deref(), Some("4500"));
    // The membership was refreshed too.
    assert!(
        sidebar::channel_member(store.conn(), "c1", ME)
            .unwrap()
            .is_some()
    );
    server.verify().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gap_over_six_hours_replaces_the_channel_with_its_latest_page() {
    let server = server(2000 + 7 * HOUR).await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/posts"))
        .and(query_param("per_page", "60"))
        .respond_with(ResponseTemplate::new(200).set_body_json(list(&[post_json(
            "p9",
            2000 + 7 * HOUR,
            0,
        )])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/posts"))
        .and(query_param("since", "2000"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let store = resync(&server, &cache(dir.path())).await;
    assert_eq!(window(&store), ["p9"]);
    assert!(posts::post(store.conn(), "p1").unwrap().is_none());
    server.verify().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_huge_since_response_reloads_the_channel_instead() {
    let server = server(4000).await;
    let big: Vec<Value> = (0..30)
        .map(|i| {
            let mut post = post_json(&format!("big{i}"), 3000 + i, 0);
            post["message"] = json!("x".repeat(80_000));
            post
        })
        .collect();
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/posts"))
        .and(query_param("since", "2000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(list(&big)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/posts"))
        .and(query_param("per_page", "60"))
        .respond_with(ResponseTemplate::new(200).set_body_json(list(&[post_json("p9", 4000, 0)])))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let store = resync(&server, &cache(dir.path())).await;
    assert_eq!(window(&store), ["p9"]);
    server.verify().await;
}
