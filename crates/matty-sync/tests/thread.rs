//! Opening a thread reads it from the server, so a thread whose replies
//! the cache never saw still shows them.
#![allow(clippy::unwrap_used)]

mod common;

use common::{connected, next, server};
use matty_sync::{ChangeSet, Command};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn an_opened_thread_comes_from_the_server() {
    let server = server().await;
    let post = |id: &str, root: &str, at: i64, message: &str| {
        json!({"id": id, "channel_id": "c1", "user_id": "ada", "root_id": root,
               "create_at": at, "update_at": at, "message": message})
    };
    Mock::given(method("GET"))
        .and(path("/api/v4/posts/root1/thread"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "order": ["r1", "root1"],
            "posts": {
                "root1": post("root1", "", 100, "Plan?"),
                "r1": post("r1", "root1", 200, "Yes"),
            },
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;

    sync.send(Command::OpenThread {
        root_id: "root1".into(),
    });
    let ChangeSet::Thread { posts, .. } = next(&changes, |c| {
        matches!(c, ChangeSet::Thread { root_id, posts } if root_id == "root1" && posts.len() == 2)
    })
    .await
    else {
        unreachable!()
    };
    let messages: Vec<_> = posts.iter().map(|p| p.message.as_str()).collect();
    assert_eq!(messages, ["Plan?", "Yes"]);
}
