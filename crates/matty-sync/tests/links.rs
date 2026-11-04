//! Links in messages find their channel or person from the cache, else the
//! server (#85, #103).
#![allow(clippy::unwrap_used)]

mod common;

use common::{connected, get, next, server};
use matty_sync::{ChangeSet, Command, LinkAnswer, LinkQuery};
use serde_json::json;
use wiremock::ResponseTemplate;
use wiremock::matchers::{method, path};

async fn resolve(
    sync: &matty_sync::SyncHandle,
    changes: &async_channel::Receiver<ChangeSet>,
    query: LinkQuery,
) -> Option<LinkAnswer> {
    sync.send(Command::ResolveLink {
        query: query.clone(),
    });
    let ChangeSet::LinkResolved { answer, .. } = next(
        changes,
        |c| matches!(c, ChangeSet::LinkResolved { query: asked, .. } if *asked == query),
    )
    .await
    else {
        unreachable!()
    };
    answer
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cached_channel_answers_without_the_server() {
    let server = server().await;
    let (sync, changes) = connected(&server).await;
    let answer = resolve(
        &sync,
        &changes,
        LinkQuery::Channel {
            team_id: "t1".into(),
            name: "c1".into(),
        },
    )
    .await;
    assert!(matches!(answer, Some(LinkAnswer::Channel(channel)) if channel.id == "c1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_permalink_finds_its_channel_on_the_server() {
    let server = server().await;
    get(
        "posts/p9",
        json!({"id": "p9", "channel_id": "c9", "user_id": "u2", "message": "hi"}),
    )
    .mount(&server)
    .await;
    get(
        "channels/c9",
        json!({"id": "c9", "team_id": "t1", "type": "O", "name": "elsewhere", "display_name": "Elsewhere"}),
    )
    .mount(&server)
    .await;
    wiremock::Mock::given(method("GET"))
        .and(path("/api/v4/posts/gone"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "id": "app.post.get.app_error", "message": "not found", "status_code": 404
        })))
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    let found = resolve(
        &sync,
        &changes,
        LinkQuery::Post {
            post_id: "p9".into(),
        },
    )
    .await;
    assert!(matches!(found, Some(LinkAnswer::Channel(channel)) if channel.name == "elsewhere"));
    let gone = resolve(
        &sync,
        &changes,
        LinkQuery::Post {
            post_id: "gone".into(),
        },
    )
    .await;
    assert_eq!(gone, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_username_is_asked_about() {
    let server = server().await;
    wiremock::Mock::given(method("POST"))
        .and(path("/api/v4/users/usernames"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{"id": "u7", "username": "ana"}])),
        )
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    let answer = resolve(
        &sync,
        &changes,
        LinkQuery::User {
            username: "ana".into(),
        },
    )
    .await;
    assert!(matches!(answer, Some(LinkAnswer::User(user)) if user.id == "u7"));
}
