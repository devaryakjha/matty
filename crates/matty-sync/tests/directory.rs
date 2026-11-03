//! Browse channels, find people and create a channel (PLAN §3 M1).
#![allow(clippy::unwrap_used)]

mod common;

use common::{ME, next, server};
use matty_api::Client;
use matty_store::Store;
use matty_sync::{ChangeSet, Command};
use serde_json::json;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn browse_people_and_create_answer_with_their_change_sets() {
    let server = server().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/teams/t1/channels"))
        .and(query_param("page", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "c1", "team_id": "t1", "type": "O", "name": "c1", "display_name": "C1"},
            {"id": "c2", "team_id": "t1", "type": "O", "name": "design", "display_name": "Design", "purpose": "Pixels"}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/channels/stats/member_count"))
        .and(body_json(json!(["c1", "c2"])))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"c1": 4, "c2": 9})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users"))
        .and(query_param("in_team", "t1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "ada", "username": "ada", "first_name": "Ada"}
        ])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/channels"))
        .and(body_json(json!({
            "team_id": "t1", "name": "ship-it", "display_name": "Ship it",
            "purpose": "", "type": "O"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "c9", "team_id": "t1", "type": "O", "name": "ship-it", "display_name": "Ship it"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/channels/c9/members/{ME}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "channel_id": "c9", "user_id": ME, "notify_props": {"mark_unread": "all"}
        })))
        .mount(&server)
        .await;

    let sync = matty_sync::spawn(Store::open_in_memory().unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::Connect {
        client: Client::new(server.uri()).unwrap().with_token("token"),
    });
    next(&changes, |c| matches!(c, ChangeSet::Sidebar { .. })).await;

    sync.send(Command::Browse {
        team_id: "t1".into(),
    });
    let ChangeSet::Directory {
        channels, members, ..
    } = next(&changes, |c| matches!(c, ChangeSet::Directory { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(channels.len(), 2);
    assert_eq!(members["c2"], 9);

    sync.send(Command::FindPeople {
        team_id: "t1".into(),
    });
    let ChangeSet::People { users, .. } =
        next(&changes, |c| matches!(c, ChangeSet::People { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(users[0].first_name, "Ada");

    sync.send(Command::CreateChannel {
        team_id: "t1".into(),
        name: "ship-it".into(),
        display_name: "Ship it".into(),
        purpose: String::new(),
        private: false,
    });
    let ChangeSet::ChannelReady { channel_id } =
        next(&changes, |c| matches!(c, ChangeSet::ChannelReady { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(channel_id, "c9");
}
