//! An open channel's header and counts reach the app.
#![allow(clippy::unwrap_used)]

mod common;

use common::{next, server};
use matty_api::Client;
use matty_store::Store;
use matty_sync::{ChangeSet, Command};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn an_open_channel_sends_itself_and_its_counts() {
    let server = server().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/stats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "channel_id": "c1", "member_count": 12, "guest_count": 0,
            "pinnedpost_count": 3, "files_count": 0
        })))
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
    let ChangeSet::Channel { channel, member } =
        next(&changes, |c| matches!(c, ChangeSet::Channel { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(channel.display_name, "C1");
    assert_eq!(member.unwrap().notify_props["mark_unread"], "all");
    let ChangeSet::Stats(stats) = next(&changes, |c| matches!(c, ChangeSet::Stats(_))).await else {
        unreachable!()
    };
    assert_eq!((stats.member_count, stats.pinnedpost_count), (12, 3));
}

#[tokio::test(flavor = "multi_thread")]
async fn channel_settings_reach_the_server_and_come_back() {
    use common::ME;
    use wiremock::matchers::body_json;
    let server = server().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/stats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "channel_id": "c1", "member_count": 3, "pinnedpost_count": 0
        })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v4/channels/c1/patch"))
        .and(body_json(json!({"header": "Ship it"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "c1", "team_id": "t1", "type": "O", "name": "c1", "display_name": "C1",
            "header": "Ship it", "update_at": 5
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!(
            "/api/v4/channels/c1/members/{ME}/notify_props"
        )))
        .and(body_json(json!({"desktop": "mention"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/channels/c1/members/{ME}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "channel_id": "c1", "user_id": ME,
            "notify_props": {"mark_unread": "all", "desktop": "mention"}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "c1", "team_id": "t1", "type": "O", "name": "c1", "display_name": "C1",
            "header": "Ship it", "update_at": 5
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/channels/c1/members"))
        .and(body_json(json!({"user_id": "ada"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "channel_id": "c1", "user_id": "ada"
        })))
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
    next(&changes, |c| matches!(c, ChangeSet::Stats(_))).await;

    sync.send(Command::PatchChannel {
        channel_id: "c1".into(),
        display_name: None,
        header: Some("Ship it".into()),
        purpose: None,
    });
    next(
        &changes,
        |c| matches!(c, ChangeSet::Channel { channel, .. } if channel.header == "Ship it"),
    )
    .await;

    sync.send(Command::SetChannelNotifications {
        channel_id: "c1".into(),
        props: std::collections::HashMap::from([("desktop".into(), "mention".into())]),
    });
    next(&changes, |c| {
        matches!(c, ChangeSet::Channel { member: Some(m), .. }
            if m.notify_props.get("desktop").map(String::as_str) == Some("mention"))
    })
    .await;

    sync.send(Command::AddMembers {
        channel_id: "c1".into(),
        user_ids: vec!["ada".into()],
    });
    next(&changes, |c| matches!(c, ChangeSet::Stats(_))).await;
}
