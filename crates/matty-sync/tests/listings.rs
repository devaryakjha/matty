//! Saved, pinned and recent-mention listings, and saving and pinning a post
//! (#130–#132).
#![allow(clippy::unwrap_used)]

mod common;

use common::{ME, connected, next, server};
use matty_sync::{ChangeSet, Command, Listing};
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, ResponseTemplate};

fn post_list(ids: &[(&str, i64)]) -> serde_json::Value {
    let posts: serde_json::Map<String, serde_json::Value> = ids
        .iter()
        .map(|(id, at)| {
            (
                (*id).to_string(),
                json!({"id": id, "channel_id": "c1", "user_id": "ada", "message": "hi", "create_at": at}),
            )
        })
        .collect();
    json!({"order": ids.iter().map(|(id, _)| id).collect::<Vec<_>>(), "posts": posts})
}

async fn posts_of(changes: &async_channel::Receiver<ChangeSet>, want: Listing) -> Vec<String> {
    let ChangeSet::Listing { posts, .. } = next(
        changes,
        |c| matches!(c, ChangeSet::Listing { listing, .. } if *listing == want),
    )
    .await
    else {
        unreachable!()
    };
    posts.iter().map(|post| post.id.clone()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn listings_come_newest_first() {
    let server = server().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/users/{ME}/posts/flagged")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(post_list(&[("old", 1), ("new", 2)])),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/pinned"))
        .respond_with(ResponseTemplate::new(200).set_body_json(post_list(&[("pin", 5)])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/teams/t1/posts/search"))
        .and(body_partial_json(
            json!({"terms": "@me deploy", "is_or_search": true}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(post_list(&[("m1", 3)])))
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;

    sync.send(Command::LoadListing(Listing::Saved));
    assert_eq!(posts_of(&changes, Listing::Saved).await, ["new", "old"]);
    let pinned = Listing::Pinned {
        channel_id: "c1".into(),
    };
    sync.send(Command::LoadListing(pinned.clone()));
    assert_eq!(posts_of(&changes, pinned).await, ["pin"]);
    let mentions = Listing::Mentions {
        team_id: "t1".into(),
        terms: "@me deploy".into(),
    };
    sync.send(Command::LoadListing(mentions.clone()));
    assert_eq!(posts_of(&changes, mentions).await, ["m1"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn saving_shows_at_once_and_tells_the_server() {
    let server = server().await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v4/users/{ME}/preferences")))
        .and(body_partial_json(
            json!([{"category": "flagged_post", "name": "p1"}]),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v4/users/{ME}/preferences/delete")))
        .and(body_partial_json(
            json!([{"category": "flagged_post", "name": "p1"}]),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts/p1/pin"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;

    sync.send(Command::SetSaved {
        post_id: "p1".into(),
        saved: true,
    });
    next(
        &changes,
        |c| matches!(c, ChangeSet::Saved(ids) if ids.iter().any(|id| id == "p1")),
    )
    .await;
    sync.send(Command::SetSaved {
        post_id: "p1".into(),
        saved: false,
    });
    next(
        &changes,
        |c| matches!(c, ChangeSet::Saved(ids) if ids.is_empty()),
    )
    .await;
    sync.send(Command::SetPinned {
        post_id: "p1".into(),
        pinned: true,
    });
    for _ in 0..100 {
        let requests = server.received_requests().await.unwrap_or_default();
        if requests
            .iter()
            .any(|r| r.url.path() == "/api/v4/posts/p1/pin")
            && requests
                .iter()
                .any(|r| r.url.path().ends_with("/preferences/delete"))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    server.verify().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reminder_reaches_the_server() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v4/users/{ME}/posts/p1/reminder")))
        .and(body_partial_json(json!({"target_time": 1_800_000_000})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, _changes) = connected(&server).await;
    sync.send(Command::SetReminder {
        post_id: "p1".into(),
        target_time: 1_800_000_000,
    });
    for _ in 0..100 {
        let requests = server.received_requests().await.unwrap_or_default();
        if requests.iter().any(|r| r.url.path().ends_with("/reminder")) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    server.verify().await;
}
