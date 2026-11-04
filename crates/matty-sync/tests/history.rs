//! Older posts load page by page, until the channel's first post.
#![allow(clippy::unwrap_used)]

mod common;

use common::{ME, next, server};
use matty_api::Client;
use matty_store::Store;
use matty_sync::{ChangeSet, Command};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

fn post(id: &str, at: i64) -> Value {
    json!({"id": id, "channel_id": "c1", "user_id": ME, "create_at": at,
           "update_at": at, "message": id})
}

fn ids(change: &ChangeSet) -> Vec<String> {
    let ChangeSet::Posts { posts, .. } = change else {
        unreachable!()
    };
    posts.iter().map(|post| post.id.clone()).collect()
}

/// Opens `c1` and connects, without waiting: the channel's posts may come
/// before the account finishes loading.
fn open(
    server: &wiremock::MockServer,
) -> (matty_sync::SyncHandle, async_channel::Receiver<ChangeSet>) {
    let sync = matty_sync::spawn(Store::open_in_memory().unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::OpenChannel {
        channel_id: "c1".into(),
    });
    sync.send(Command::Connect {
        client: Client::new(server.uri()).unwrap().with_token("token"),
    });
    (sync, changes)
}

#[tokio::test(flavor = "multi_thread")]
async fn older_posts_reach_back_to_the_first() {
    let server = server().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/users/{ME}/channels/c1/posts/unread")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "order": ["p3", "p2"],
            "posts": {"p2": post("p2", 20), "p3": post("p3", 30)},
            "prev_post_id": "p1"
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/posts"))
        .and(query_param("before", "p2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "order": ["p1"],
            "posts": {"p1": post("p1", 10)},
            "prev_post_id": ""
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let (sync, changes) = open(&server);
    let shown = next(
        &changes,
        |c| matches!(c, ChangeSet::Posts { posts, .. } if posts.len() == 2),
    )
    .await;
    assert_eq!(ids(&shown), ["p2", "p3"]);

    sync.send(Command::LoadOlder {
        channel_id: "c1".into(),
    });
    let history = next(&changes, |c| matches!(c, ChangeSet::History { .. })).await;
    assert!(matches!(
        history,
        ChangeSet::History {
            at_start: true,
            failed: false,
            ..
        }
    ));
    let shown = next(&changes, |c| matches!(c, ChangeSet::Posts { .. })).await;
    assert_eq!(ids(&shown), ["p1", "p2", "p3"]);

    // Nothing is left to load, so asking again only says so.
    sync.send(Command::LoadOlder {
        channel_id: "c1".into(),
    });
    let again = next(&changes, |c| matches!(c, ChangeSet::History { .. })).await;
    assert!(matches!(again, ChangeSet::History { at_start: true, .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_page_says_so_and_can_be_asked_again() {
    let server = server().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/users/{ME}/channels/c1/posts/unread")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "order": ["p2"], "posts": {"p2": post("p2", 20)}, "prev_post_id": "p1"
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/posts"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let (sync, changes) = open(&server);
    next(
        &changes,
        |c| matches!(c, ChangeSet::Posts { posts, .. } if posts.len() == 1),
    )
    .await;
    for _ in 0..2 {
        sync.send(Command::LoadOlder {
            channel_id: "c1".into(),
        });
        let failed = next(&changes, |c| matches!(c, ChangeSet::History { .. })).await;
        assert!(matches!(
            failed,
            ChangeSet::History {
                at_start: false,
                failed: true,
                ..
            }
        ));
    }
}
