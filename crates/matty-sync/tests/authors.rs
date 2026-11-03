//! The people behind the posts on screen reach the app (#98).
#![allow(clippy::unwrap_used)]

mod common;

use common::{ME, next, server};
use matty_api::Client;
use matty_store::Store;
use matty_sync::{ChangeSet, Command};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_author_is_fetched_once_and_sent() {
    let server = server().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/users/{ME}/channels/c1/posts/unread")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "order": ["p1", "p2"],
            "posts": {
                "p1": {"id": "p1", "channel_id": "c1", "user_id": "ada", "create_at": 1, "update_at": 1, "message": "one"},
                "p2": {"id": "p2", "channel_id": "c1", "user_id": "ada", "create_at": 2, "update_at": 2, "message": "two"}
            }
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/ids"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "ada", "username": "ada", "first_name": "Ada", "last_name": "Lovelace"}
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let sync = matty_sync::spawn(Store::open_in_memory().unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::OpenChannel {
        channel_id: "c1".into(),
    });
    sync.send(Command::Connect {
        client: Client::new(server.uri()).unwrap().with_token("token"),
    });
    let ChangeSet::Users(users) = next(&changes, |c| matches!(c, ChangeSet::Users(_))).await else {
        unreachable!()
    };
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].first_name, "Ada");

    // Known now: the next window of the same posts asks for no one, and a
    // second subscriber gets Ada from the cache.
    let again = sync.subscribe();
    sync.send(Command::OpenChannel {
        channel_id: "c1".into(),
    });
    let ChangeSet::Users(users) = next(&again, |c| matches!(c, ChangeSet::Users(_))).await else {
        unreachable!()
    };
    assert_eq!(users[0].id, "ada");
}
