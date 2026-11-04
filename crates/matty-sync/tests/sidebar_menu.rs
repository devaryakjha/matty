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

#[tokio::test(flavor = "multi_thread")]
async fn choosing_collapsed_threads_tells_the_server_and_the_window() {
    let server = common::server_with_config(json!({"CollapsedThreads": "default_off"})).await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v4/users/{ME}/preferences")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;

    sync.send(Command::SetCollapsedThreads { on: true });
    next(&changes, |c| matches!(c, ChangeSet::CollapsedThreads(true))).await;
    for _ in 0..50 {
        if !saved_preferences(&server).await.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let saved = saved_preferences(&server).await;
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert_eq!(saved[0]["category"], "display_settings");
    assert_eq!(saved[0]["name"], "collapsed_reply_threads");
    assert_eq!(saved[0]["value"], "on");
}

/// The categories requests the server received, as (method, path, body).
async fn category_requests(server: &wiremock::MockServer) -> Vec<(String, String, Value)> {
    let requests = server.received_requests().await.unwrap_or_default();
    requests
        .iter()
        .filter(|r| r.method.as_str() != "GET" && r.url.path().contains("/channels/categories"))
        .map(|r| {
            let body = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
            (r.method.to_string(), r.url.path().to_owned(), body)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn categories_are_created_renamed_moved_into_and_deleted_on_the_server() {
    let server = server().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/me/teams/t1/channels/categories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "categories": [
                {"id": "cat", "team_id": "t1", "type": "channels", "display_name": "Channels", "channel_ids": ["c1"]},
                {"id": "mine", "team_id": "t1", "type": "custom", "display_name": "Mine", "channel_ids": ["c2"]}
            ],
            "order": ["mine", "cat"]
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/me/teams/t1/channels/categories"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "new"})))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v4/users/me/teams/t1/channels/categories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v4/users/me/teams/t1/channels/categories/mine"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    // Loaded: the categories are stored.
    let (sync, _changes) = connected(&server).await;

    sync.send(Command::CreateCategory {
        team_id: "t1".into(),
        name: "Projects".into(),
        channel_id: Some("c1".into()),
    });
    sync.send(Command::RenameCategory {
        category_id: "mine".into(),
        name: "Ours".into(),
    });
    sync.send(Command::MoveToCategory {
        channel_id: "c1".into(),
        category_id: "mine".into(),
    });
    sync.send(Command::DeleteCategory {
        category_id: "mine".into(),
    });
    let mut sent = Vec::new();
    for _ in 0..100 {
        sent = category_requests(&server).await;
        if sent.len() == 4 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(sent.len(), 4, "{sent:?}");
    let find = |verb: &'static str| sent.iter().filter(move |(m, _, _)| m == verb);
    let created = &find("POST").next().unwrap().2;
    assert_eq!(created["display_name"], "Projects");
    assert_eq!(created["type"], "custom");
    assert_eq!(created["user_id"], ME);
    assert_eq!(created["channel_ids"], json!(["c1"]));
    let updates: Vec<&Value> = find("PUT").map(|(_, _, body)| &body[0]).collect();
    assert!(
        updates
            .iter()
            .any(|c| c["display_name"] == "Ours" && c["user_id"] == ME),
        "{updates:?}"
    );
    assert!(
        updates
            .iter()
            .any(|c| c["channel_ids"] == json!(["c1", "c2"])),
        "{updates:?}"
    );
    assert_eq!(
        find("DELETE").next().unwrap().1,
        "/api/v4/users/me/teams/t1/channels/categories/mine"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn archiving_and_making_private_reach_the_server() {
    let server = server().await;
    Mock::given(method("DELETE"))
        .and(path("/api/v4/channels/c1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v4/channels/c2/privacy"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"id": "c2", "team_id": "t1", "type": "P", "name": "c2", "display_name": "C2"}),
        ))
        .mount(&server)
        .await;
    let (sync, _changes) = connected(&server).await;

    sync.send(Command::ConvertToPrivate {
        channel_id: "c2".into(),
    });
    sync.send(Command::ArchiveChannel {
        channel_id: "c1".into(),
    });
    let sent = |requests: &[wiremock::Request]| {
        let archived = requests
            .iter()
            .any(|r| r.method.as_str() == "DELETE" && r.url.path() == "/api/v4/channels/c1");
        let private = requests
            .iter()
            .any(|r| r.url.path() == "/api/v4/channels/c2/privacy");
        archived && private
    };
    for _ in 0..100 {
        if sent(&server.received_requests().await.unwrap_or_default()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(sent(&server.received_requests().await.unwrap_or_default()));
    let requests = server.received_requests().await.unwrap_or_default();
    let privacy = requests
        .iter()
        .find(|r| r.url.path() == "/api/v4/channels/c2/privacy")
        .unwrap();
    let body: Value = serde_json::from_slice(&privacy.body).unwrap();
    assert_eq!(body, json!({"privacy": "P"}));
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_a_member_reaches_the_server_and_recounts() {
    let server = server().await;
    Mock::given(method("DELETE"))
        .and(path("/api/v4/channels/c1/members/ada"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/stats"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"channel_id": "c1", "member_count": 4})),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;

    sync.send(Command::RemoveMember {
        channel_id: "c1".into(),
        user_id: "ada".into(),
    });
    next(
        &changes,
        |c| matches!(c, ChangeSet::Stats(stats) if stats.member_count == 4),
    )
    .await;
    let requests = server.received_requests().await.unwrap_or_default();
    assert!(requests.iter().any(
        |r| r.method.as_str() == "DELETE" && r.url.path() == "/api/v4/channels/c1/members/ada"
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn editing_the_profile_patches_only_the_changed_fields() {
    let server = server().await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v4/users/{ME}/patch")))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"id": ME, "username": "me", "first_name": "Ada", "position": "Engineer"}),
        ))
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;

    sync.send(Command::UpdateProfile {
        patch: matty_api::ProfilePatch {
            first_name: Some("Ada".into()),
            position: Some("Engineer".into()),
            ..Default::default()
        },
    });
    next(&changes, |c| {
        matches!(c, ChangeSet::Users(users)
            if users.iter().any(|u| u.id == ME && u.position == "Engineer"))
    })
    .await;
    let requests = server.received_requests().await.unwrap_or_default();
    let patch = requests
        .iter()
        .find(|r| r.method.as_str() == "PUT" && r.url.path().ends_with("/patch"))
        .unwrap();
    let body: Value = serde_json::from_slice(&patch.body).unwrap();
    assert_eq!(body, json!({"first_name": "Ada", "position": "Engineer"}));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_picture_uploads_and_a_removed_one_resets() {
    let server = server().await;
    for verb in ["POST", "DELETE"] {
        Mock::given(method(verb))
            .and(path(format!("/api/v4/users/{ME}/image")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/v4/users/me"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": ME, "username": "me", "last_picture_update": 42})),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;

    sync.send(Command::SetPicture {
        picture: Some((b"\x89PNG fake".to_vec(), "me.png".into())),
    });
    next(&changes, |c| {
        matches!(c, ChangeSet::Users(users)
            if users.iter().any(|u| u.id == ME && u.last_picture_update == 42))
    })
    .await;
    sync.send(Command::SetPicture { picture: None });
    next(&changes, |c| matches!(c, ChangeSet::Users(_))).await;

    let requests = server.received_requests().await.unwrap_or_default();
    let image = |verb: &str| {
        requests
            .iter()
            .find(|r| r.method.as_str() == verb && r.url.path().ends_with("/image"))
            .cloned()
    };
    let upload = image("POST").expect("the picture was uploaded");
    let body = String::from_utf8_lossy(&upload.body);
    assert!(body.contains("name=\"image\""), "{body}");
    assert!(body.contains("filename=\"me.png\""), "{body}");
    assert!(image("DELETE").is_some(), "the picture was reset");
}

#[tokio::test(flavor = "multi_thread")]
async fn categories_and_channels_move_up_and_down() {
    let server = server().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/me/teams/t1/channels/categories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "categories": [
                {"id": "cat", "team_id": "t1", "type": "channels", "display_name": "Channels",
                 "sorting": "alpha", "channel_ids": ["c1", "c2", "c3"]},
                {"id": "mine", "team_id": "t1", "type": "custom", "display_name": "Mine", "channel_ids": []}
            ],
            "order": ["cat", "mine"]
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v4/users/me/teams/t1/channels/categories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v4/users/me/teams/t1/channels/categories/order"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(["mine", "cat"])))
        .mount(&server)
        .await;
    let (sync, _changes) = connected(&server).await;

    // Already at the top: nothing is sent.
    sync.send(Command::MoveCategory {
        category_id: "cat".into(),
        up: true,
    });
    sync.send(Command::MoveCategory {
        category_id: "mine".into(),
        up: true,
    });
    sync.send(Command::MoveChannel {
        category_id: "cat".into(),
        channel_id: "c3".into(),
        up: true,
    });
    let mut sent = Vec::new();
    for _ in 0..100 {
        sent = category_requests(&server).await;
        if sent.len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(sent.len(), 2, "{sent:?}");
    let order = sent.iter().find(|(_, p, _)| p.ends_with("/order")).unwrap();
    assert_eq!(order.2, json!(["mine", "cat"]));
    let moved = &sent
        .iter()
        .find(|(_, p, _)| !p.ends_with("/order"))
        .unwrap()
        .2[0];
    assert_eq!(moved["channel_ids"], json!(["c1", "c3", "c2"]));
    assert_eq!(moved["sorting"], "manual");
}

#[tokio::test(flavor = "multi_thread")]
async fn dragged_channels_and_categories_land_where_they_are_dropped() {
    let server = server().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/me/teams/t1/channels/categories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "categories": [
                {"id": "cat", "team_id": "t1", "type": "channels", "display_name": "Channels",
                 "sorting": "alpha", "channel_ids": ["c1", "c2", "c3"]},
                {"id": "mine", "team_id": "t1", "type": "custom", "display_name": "Mine",
                 "channel_ids": ["c4"]},
                {"id": "dms", "team_id": "t1", "type": "direct_messages", "display_name": "DMs",
                 "channel_ids": []}
            ],
            "order": ["cat", "mine", "dms"]
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v4/users/me/teams/t1/channels/categories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v4/users/me/teams/t1/channels/categories/order"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(["dms", "cat", "mine"])))
        .mount(&server)
        .await;
    let (sync, _changes) = connected(&server).await;

    // Already in place: nothing is sent.
    sync.send(Command::PlaceChannel {
        channel_id: "c1".into(),
        category_id: "cat".into(),
        index: 0,
    });
    // Into another category, between its rows.
    sync.send(Command::PlaceChannel {
        channel_id: "c2".into(),
        category_id: "mine".into(),
        index: 1,
    });
    // A category to the top.
    sync.send(Command::PlaceCategory {
        category_id: "dms".into(),
        index: 0,
    });
    let mut sent = Vec::new();
    for _ in 0..100 {
        sent = category_requests(&server).await;
        if sent.len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(sent.len(), 2, "{sent:?}");
    let order = sent.iter().find(|(_, p, _)| p.ends_with("/order")).unwrap();
    assert_eq!(order.2, json!(["dms", "cat", "mine"]));
    let moved = &sent
        .iter()
        .find(|(_, p, _)| !p.ends_with("/order"))
        .unwrap()
        .2[0];
    assert_eq!(moved["id"], "mine");
    assert_eq!(moved["channel_ids"], json!(["c4", "c2"]));
    assert_eq!(moved["sorting"], "manual");
}
