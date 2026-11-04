//! Sending through the outbox (#38), against a mock server.
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use std::time::Duration;

use matty_sync::{ChangeSet, Command, CommandError};
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
