//! Message priority, acknowledgements and scheduled messages (#220),
//! against a mock server.
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use std::time::Duration;

use matty_api::models::{PostPriority, ScheduledPost};
use matty_sync::{ChangeSet, Command, CommandError};
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

use common::{ME, connected, next, requested, server};

/// Mounts `p1` in c1, from someone else, asking for acknowledgements.
async fn with_urgent_post(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/users/{ME}/channels/c1/posts/unread")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "order": ["p1"],
            "posts": {"p1": {
                "id": "p1", "channel_id": "c1", "user_id": "u2", "create_at": 10,
                "message": "@me the deploy is broken",
                "metadata": {"priority": {"priority": "urgent", "requested_ack": true}}
            }}
        })))
        .with_priority(1)
        .mount(server)
        .await;
}

fn acks(change: &ChangeSet) -> Option<Vec<String>> {
    match change {
        ChangeSet::Posts { posts, .. } => posts.iter().find(|p| p.id == "p1").map(|post| {
            post.metadata
                .acknowledgements
                .iter()
                .map(|ack| ack.user_id.clone())
                .collect()
        }),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_urgent_post_asking_for_acknowledgement_is_sent_with_its_priority() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .and(body_partial_json(json!({
            "message": "all hands",
            "metadata": {"priority": {"priority": "urgent", "requested_ack": true}}
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "server1", "channel_id": "c1", "user_id": ME, "create_at": 100,
            "message": "all hands", "pending_post_id": "me:1",
            "metadata": {"priority": {"priority": "urgent", "requested_ack": true}}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    sync.send(Command::SendPost {
        pending_post_id: "me:1".into(),
        channel_id: "c1".into(),
        root_id: String::new(),
        message: "all hands".into(),
        file_ids: Vec::new(),
        local_files: Vec::new(),
        props: Default::default(),
        priority: Some(Box::new(PostPriority {
            priority: PostPriority::URGENT.into(),
            requested_ack: true,
            persistent_notifications: false,
        })),
    });
    // The optimistic row shows the label at once, before the server answers.
    let ChangeSet::Posts { posts, .. } = next(
        &changes,
        |c| matches!(c, ChangeSet::Posts { posts, .. } if posts.iter().any(|p| p.id == "me:1")),
    )
    .await
    else {
        unreachable!()
    };
    let optimistic = posts.iter().find(|p| p.id == "me:1").unwrap();
    assert!(optimistic.metadata.priority.as_ref().unwrap().is_urgent());
    next(
        &changes,
        |c| matches!(c, ChangeSet::Posts { posts, .. } if posts.iter().any(|p| p.id == "server1")),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn acknowledging_shows_at_once_and_reaches_the_server() {
    let server = server().await;
    with_urgent_post(&server).await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v4/users/{ME}/posts/p1/ack")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "user_id": ME, "post_id": "p1", "acknowledged_at": 500
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    sync.send(Command::Acknowledge {
        post_id: "p1".into(),
        acknowledged: true,
    });
    next(&changes, |c| acks(c) == Some(vec![ME.to_owned()])).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !requested(&server).await.iter().any(|p| p.ends_with("/ack")) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_acknowledgement_is_undone() {
    let server = server().await;
    with_urgent_post(&server).await;
    Mock::given(method("DELETE"))
        .and(path(format!("/api/v4/users/{ME}/posts/p1/ack")))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "id": "api.post.acknowledge_post.expired", "message": "Too late.",
            "status_code": 403
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v4/users/{ME}/posts/p1/ack")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "user_id": ME, "post_id": "p1", "acknowledged_at": 500
        })))
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    sync.send(Command::Acknowledge {
        post_id: "p1".into(),
        acknowledged: true,
    });
    next(&changes, |c| acks(c) == Some(vec![ME.to_owned()])).await;
    sync.send(Command::Acknowledge {
        post_id: "p1".into(),
        acknowledged: false,
    });
    next(&changes, |c| acks(c) == Some(Vec::new())).await;
    let ChangeSet::Failed(CommandError::Server { action, .. }) =
        next(&changes, |c| matches!(c, ChangeSet::Failed(_))).await
    else {
        unreachable!()
    };
    assert_eq!(action, "remove acknowledgement");
    // Back as the server has it.
    next(&changes, |c| acks(c) == Some(vec![ME.to_owned()])).await;
}

fn scheduled() -> ScheduledPost {
    ScheduledPost {
        channel_id: "c1".into(),
        message: "standup notes".into(),
        scheduled_at: 1_800_000_000_000,
        ..ScheduledPost::default()
    }
}

fn listed(change: &ChangeSet) -> Option<Vec<String>> {
    match change {
        ChangeSet::Scheduled(posts) => Some(posts.iter().map(|p| p.id.clone()).collect()),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_is_scheduled_then_cancelled() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts/schedule"))
        .and(body_partial_json(json!({
            "channel_id": "c1", "message": "standup notes", "user_id": ME,
            "scheduled_at": 1_800_000_000_000_i64
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "s1", "channel_id": "c1", "user_id": ME, "message": "standup notes",
            "scheduled_at": 1_800_000_000_000_i64
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v4/posts/schedule/s1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "s1"})))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    sync.send(Command::Schedule(Box::new(scheduled())));
    next(&changes, |c| listed(c) == Some(vec!["s1".to_owned()])).await;
    sync.send(Command::DeleteScheduled { id: "s1".into() });
    next(&changes, |c| listed(c) == Some(Vec::new())).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !requested(&server)
            .await
            .iter()
            .any(|p| p == "/api/v4/posts/schedule/s1")
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn send_now_posts_the_scheduled_message_with_its_priority() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts/schedule"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "s1", "channel_id": "c1", "user_id": ME, "message": "standup notes",
            "scheduled_at": 1_800_000_000_000_i64,
            "metadata": {"priority": {"priority": "important"}}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .and(body_partial_json(json!({
            "message": "standup notes",
            "metadata": {"priority": {"priority": "important"}}
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "server1", "channel_id": "c1", "user_id": ME, "create_at": 100,
            "message": "standup notes"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v4/posts/schedule/s1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "s1"})))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    sync.send(Command::Schedule(Box::new(scheduled())));
    next(&changes, |c| listed(c) == Some(vec!["s1".to_owned()])).await;
    sync.send(Command::SendScheduledNow { id: "s1".into() });
    next(&changes, |c| listed(c) == Some(Vec::new())).await;
    next(
        &changes,
        |c| matches!(c, ChangeSet::Posts { posts, .. } if posts.iter().any(|p| p.id == "server1")),
    )
    .await;
}
