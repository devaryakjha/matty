//! The sidebar's row menu: favorites and closing a conversation reach the
//! server as preferences (PLAN §4.1).
#![allow(clippy::unwrap_used)]

mod common;

use common::{ME, connected, next, server};
use matty_sync::{ChangeSet, Command};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

async fn saved_preferences(server: &wiremock::MockServer) -> Vec<Value> {
    let requests = server.received_requests().await.unwrap_or_default();
    requests
        .iter()
        .filter(|r| {
            r.method.as_str() == "PUT" && r.url.path() == format!("/api/v4/users/{ME}/preferences")
        })
        .flat_map(|r| serde_json::from_slice::<Vec<Value>>(&r.body).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_conversation_hides_it_and_tells_the_server() {
    let server = server().await;
    let dm = format!("{ME}__ada");
    Mock::given(method("GET"))
        .and(path("/api/v4/users/me/teams/t1/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "c1", "team_id": "t1", "type": "O", "name": "c1", "display_name": "C1"},
            {"id": "d1", "team_id": "", "type": "D", "name": dm, "display_name": ""}
        ])))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v4/users/{ME}/teams/t1/channels/members"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"channel_id": "c1", "user_id": ME, "notify_props": {}},
            {"channel_id": "d1", "user_id": ME, "notify_props": {}}
        ])))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/me/teams/t1/channels/categories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "categories": [
                {"id": "cat", "team_id": "t1", "type": "channels", "display_name": "Channels", "channel_ids": ["c1"]},
                {"id": "dms", "team_id": "t1", "type": "direct_messages", "display_name": "Direct Messages", "channel_ids": ["d1"]}
            ],
            "order": ["cat", "dms"]
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/ids"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{"id": "ada", "username": "ada"}])),
        )
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v4/users/{ME}/preferences")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;

    sync.send(Command::CloseDirect {
        channel_id: "d1".into(),
    });
    let ChangeSet::Sidebar { sections, .. } = next(&changes, |c| {
        matches!(c, ChangeSet::Sidebar { sections, .. }
            if sections.iter().any(|s| s.kind == "direct_messages"))
    })
    .await
    else {
        unreachable!()
    };
    let dms = sections
        .iter()
        .find(|s| s.kind == "direct_messages")
        .unwrap();
    assert!(dms.channels.is_empty(), "{:?}", dms.channels);

    sync.send(Command::SetFavorite {
        channel_id: "c1".into(),
        favorite: true,
    });
    for _ in 0..50 {
        if saved_preferences(&server).await.len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let saved: Vec<(String, String, String)> = saved_preferences(&server)
        .await
        .into_iter()
        .map(|p| {
            let text = |key: &str| p[key].as_str().unwrap().to_owned();
            (text("category"), text("name"), text("value"))
        })
        .collect();
    assert_eq!(
        saved,
        vec![
            ("direct_channel_show".into(), "ada".into(), "false".into()),
            ("favorite_channel".into(), "c1".into(), "true".into()),
        ]
    );
}
