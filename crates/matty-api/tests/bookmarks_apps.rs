//! Channel bookmarks and the Apps framework's endpoints (#226). The bodies
//! follow Mattermost's `ChannelBookmarkWithFileInfo` and the Apps plugin's
//! `CallResponse`.

#![allow(clippy::unwrap_used)]

use matty_api::{
    ApiError, Client,
    models::{
        AppCall, AppCallRequest, AppContext, AppResponseKind, BookmarkKind, BookmarkPatch,
        NewBookmark, location,
    },
};
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path, query_param},
};

fn bookmark(id: &str, order: i64) -> serde_json::Value {
    json!({
        "id": id, "create_at": 1, "update_at": 1, "delete_at": 0, "channel_id": "c1",
        "owner_id": "u1", "file_id": "", "display_name": id.to_uppercase(),
        "sort_order": order, "link_url": format!("https://example.com/{id}"),
        "type": "link"
    })
}

async fn client(server: &MockServer) -> Client {
    Client::new(server.uri()).unwrap().with_token("token")
}

#[tokio::test]
async fn bookmarks_round_trip() {
    let server = MockServer::start().await;
    let file = json!({
        "id": "b2", "channel_id": "c1", "owner_id": "u1", "file_id": "f1",
        "display_name": "spec.pdf", "sort_order": 1, "type": "file",
        "file": {"id": "f1", "name": "spec.pdf", "extension": "pdf", "size": 10,
                 "mime_type": "application/pdf"}
    });
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/bookmarks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([bookmark("b1", 0), file])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/channels/c1/bookmarks"))
        .and(body_json(json!({
            "channel_id": "c1", "display_name": "Docs", "type": "link",
            "link_url": "https://example.com/docs", "emoji": "book"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(bookmark("b3", 2)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/api/v4/channels/c1/bookmarks/b1"))
        .and(body_json(json!({"display_name": "Renamed"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "updated": bookmark("b1", 0), "deleted": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/channels/c1/bookmarks/b1/sort_order"))
        .and(body_json(json!(1)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([bookmark("b2", 0), bookmark("b1", 1)])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v4/channels/c1/bookmarks/b1"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "id": "api.context.permissions.app_error", "message": "You do not have the appropriate permissions.",
            "status_code": 403
        })))
        .mount(&server)
        .await;
    let client = client(&server).await;

    let list = client.channel_bookmarks("c1").await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[1].kind, BookmarkKind::File);
    assert_eq!(list[1].file.as_ref().unwrap().name, "spec.pdf");

    let made = client
        .create_bookmark(&NewBookmark {
            channel_id: "c1".into(),
            display_name: "Docs".into(),
            kind: BookmarkKind::Link,
            link_url: "https://example.com/docs".into(),
            file_id: String::new(),
            emoji: "book".into(),
        })
        .await
        .unwrap();
    assert_eq!(made.id, "b3");

    let patch = BookmarkPatch {
        display_name: Some("Renamed".into()),
        ..BookmarkPatch::default()
    };
    let update = client.patch_bookmark("c1", "b1", &patch).await.unwrap();
    assert_eq!(update.updated.unwrap().id, "b1");
    assert!(update.deleted.is_none());

    let sorted = client.sort_bookmark("c1", "b1", 1).await.unwrap();
    assert_eq!(sorted[1].id, "b1");

    let refused = client.delete_bookmark("c1", "b1").await.unwrap_err();
    assert!(refused.is_forbidden());
}

#[tokio::test]
async fn apps_bindings_and_calls_go_to_the_plugin() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/plugins/com.mattermost.apps/api/v1/bindings"))
        .and(query_param("channel_id", "c1"))
        .and(query_param("team_id", "t1"))
        .and(query_param("user_agent", "desktop"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "location": "/post_menu",
            "bindings": [{"app_id": "hello", "location": "send", "label": "Send hello",
                          "submit": {"path": "/send", "expand": {"post": "all"}}}]
        }])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/plugins/com.mattermost.apps/api/v1/call"))
        .and(body_json(json!({
            "path": "/send", "expand": {"post": "all"},
            "context": {"app_id": "hello", "location": "/post_menu/send",
                        "user_agent": "desktop", "channel_id": "c1", "post_id": "p1"}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "type": "ok", "text": "Hello sent"
        })))
        .mount(&server)
        .await;
    let client = client(&server).await;
    let bindings = client.app_bindings("c1", "t1").await.unwrap();
    assert_eq!(bindings[0].location, location::POST_MENU);
    let send = &bindings[0].bindings[0];
    assert_eq!(send.name(), "send");
    let request = AppCallRequest::new(
        send.submit.as_ref().unwrap(),
        AppContext {
            app_id: "hello".into(),
            location: "/post_menu/send".into(),
            user_agent: matty_api::APPS_USER_AGENT.into(),
            channel_id: "c1".into(),
            post_id: "p1".into(),
            ..AppContext::default()
        },
    );
    let answer = client.app_call(&request).await.unwrap();
    assert_eq!(answer.kind, AppResponseKind::Ok);
    assert_eq!(answer.text, "Hello sent");
}

#[tokio::test]
async fn a_server_without_apps_answers_not_found_and_errors_come_back_as_answers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/plugins/com.mattermost.apps/api/v1/bindings"))
        .respond_with(ResponseTemplate::new(404).set_body_string("404 page not found"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/plugins/com.mattermost.apps/api/v1/call"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "id": "", "message": "The App is not reachable."
        })))
        .mount(&server)
        .await;
    let client = client(&server).await;
    let missing = client.app_bindings("c1", "t1").await.unwrap_err();
    assert!(missing.is_not_found(), "{missing:?}");
    assert!(!matches!(missing, ApiError::Decode(_)));
    let request = AppCallRequest::new(&AppCall::default(), AppContext::default());
    let answer = client.app_call(&request).await.unwrap();
    assert_eq!(answer.kind, AppResponseKind::Error);
    assert_eq!(answer.text, "The App is not reachable.");
}
