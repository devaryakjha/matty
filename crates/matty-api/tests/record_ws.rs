//! Records the WebSocket event fixtures in `tests/fixtures/ws/` from the
//! seeded dev server (docs/dev-server.md). Opt-in, because it changes the
//! server (it creates and archives channels and a team, among others):
//!
//! ```sh
//! scripts/dev-server reset && scripts/seed.py
//! MATTY_RECORD_WS=1 MATTY_TEST_SERVER=http://localhost:8065 \
//!     MATTY_TEST_PASSWORD='Password1!' cargo test -p matty-api --test record_ws
//! ```
//!
//! It signs in as `tester` and `admin`, opens a WebSocket for each, drives
//! every event it can through the REST API, and writes the first event of
//! each name to `<name>.json`. Events the server cannot produce here
//! (licensed features) are hand-written; see `fixtures/ws/README.md`.

#![allow(clippy::unwrap_used)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use matty_api::{
    Client, Credentials, NewPost, PostPatch,
    models::{Preference, User},
    ws::WsConnection,
};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

type Recorded = Arc<Mutex<BTreeMap<String, Value>>>;

async fn sign_in(url: &str, login: &str, password: &str) -> (Client, User) {
    let credentials = Credentials {
        login_id: login.into(),
        password: password.into(),
        ..Default::default()
    };
    let signed_in = Client::new(url).unwrap().login(&credentials).await.unwrap();
    (signed_in.client, signed_in.user)
}

/// Opens a raw WebSocket as `token`'s user and records every event frame
/// exactly as the server sent it, keeping, per event name, the one with the
/// most data (the first of a kind can be an empty update).
async fn record(url: &str, token: String, recorded: Recorded) {
    let ws_url = format!("{}/api/v4/websocket", url.replacen("http", "ws", 1));
    let (mut socket, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
    let challenge =
        json!({"seq": 1, "action": "authentication_challenge", "data": {"token": token}});
    socket
        .send(Message::text(challenge.to_string()))
        .await
        .unwrap();
    tokio::spawn(async move {
        while let Some(Ok(frame)) = socket.next().await {
            let Message::Text(text) = frame else { continue };
            let value: Value = serde_json::from_str(text.as_str()).unwrap();
            let Some(name) = value.get("event").and_then(Value::as_str) else {
                continue;
            };
            let size = |v: &Value| v["data"].as_object().map_or(0, |d| d.len());
            let mut map = recorded.lock().unwrap();
            if map.get(name).is_none_or(|old| size(&value) > size(old)) {
                map.insert(name.to_string(), value);
            }
        }
    });
}

async fn rest(client: &Client, method: &str, path: &str, body: Value) -> Value {
    let result = match method {
        "POST" => client.post::<_, Value>(path, &body).await,
        "PUT" => client.put::<_, Value>(path, &body).await,
        "DELETE" => client.delete::<Value>(path).await,
        _ => client.get::<Value>(path).await,
    };
    result.unwrap_or_else(|e| panic!("{method} {path}: {e}"))
}

async fn pause() {
    tokio::time::sleep(Duration::from_millis(300)).await;
}

#[tokio::test]
async fn record_ws_fixtures() {
    if std::env::var("MATTY_RECORD_WS").is_err() {
        eprintln!("MATTY_RECORD_WS not set; skipping");
        return;
    }
    let url = std::env::var("MATTY_TEST_SERVER").unwrap();
    let password = std::env::var("MATTY_TEST_PASSWORD").unwrap();
    let (tester, me) = sign_in(&url, "tester", &password).await;
    let (admin, admin_user) = sign_in(&url, "admin", &password).await;

    let recorded: Recorded = Arc::default();
    record(
        &url,
        session_token(&url, "tester", &password).await,
        recorded.clone(),
    )
    .await;
    // Admin-only events (config_changed, new_user for admins) fill gaps.
    let admin_recorded: Recorded = Arc::default();
    record(
        &url,
        session_token(&url, "admin", &password).await,
        admin_recorded.clone(),
    )
    .await;
    // The admin's own connection sends user_typing.
    let (admin_ws, _admin_events) = WsConnection::connect(&admin).await.unwrap();

    let team = tester
        .my_teams()
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.name == "dev")
        .unwrap();
    let channels = tester.my_channels(&team.id).await.unwrap();
    let channel = |name: &str| channels.iter().find(|c| c.name == name).unwrap().id.clone();
    let random = channel("random");
    let design = channel("design");

    // Posts and reactions.
    let pending = format!("{}:{}", me.id, now());
    let root = admin
        .create_post(&NewPost::new(
            &random,
            format!("@{} ws fixture root", me.username),
            &pending,
        ))
        .await
        .unwrap();
    pause().await;
    admin
        .patch_post(
            &root.id,
            &PostPatch {
                message: Some("ws fixture root, edited".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    rest(
        &admin,
        "POST",
        "reactions",
        json!({"user_id": admin_user.id, "post_id": root.id, "emoji_name": "tada"}),
    )
    .await;
    pause().await;
    rest(
        &admin,
        "DELETE",
        &format!("users/{}/posts/{}/reactions/tada", admin_user.id, root.id),
        json!(null),
    )
    .await;

    // Threads: the tester replies (and so follows), the admin replies again.
    let reply = tester
        .create_post(
            &NewPost::new(&random, "tester reply", format!("{pending}-r")).reply_to(&root.id),
        )
        .await
        .unwrap();
    pause().await;
    admin
        .create_post(
            &NewPost::new(&random, "admin reply", format!("{pending}-a")).reply_to(&root.id),
        )
        .await
        .unwrap();
    pause().await;
    tester
        .mark_thread_read(&me.id, &team.id, &root.id, now())
        .await
        .unwrap();
    tester
        .unfollow_thread(&me.id, &team.id, &root.id)
        .await
        .unwrap();
    tester
        .follow_thread(&me.id, &team.id, &root.id)
        .await
        .unwrap();
    tester
        .set_post_unread(&me.id, &reply.id, false)
        .await
        .unwrap();
    pause().await;
    tester.view_channel(&random, &design).await.unwrap();
    let doomed = admin
        .create_post(&NewPost::new(&random, "to delete", format!("{pending}-d")))
        .await
        .unwrap();
    pause().await;
    admin.delete_post(&doomed.id).await.unwrap();

    // Typing and status.
    admin_ws.user_typing(&random, "").await.unwrap();
    rest(
        &tester,
        "PUT",
        &format!("users/{}/status", me.id),
        json!({"user_id": me.id, "status": "dnd"}),
    )
    .await;
    rest(
        &tester,
        "PUT",
        &format!("users/{}/status", me.id),
        json!({"user_id": me.id, "status": "online"}),
    )
    .await;

    // Channels.
    let suffix = now() % 1_000_000;
    let created = rest(&admin, "POST", "channels", json!({
        "team_id": team.id, "name": format!("ws-fixture-{suffix}"), "display_name": "WS fixture", "type": "O"
    })).await;
    let created_id = created["id"].as_str().unwrap().to_string();
    rest(
        &admin,
        "POST",
        &format!("channels/{created_id}/members"),
        json!({"user_id": me.id}),
    )
    .await;
    pause().await;
    rest(
        &admin,
        "PUT",
        &format!("channels/{created_id}/patch"),
        json!({"header": "new header"}),
    )
    .await;
    rest(
        &tester,
        "PUT",
        &format!("channels/{created_id}/members/{}/notify_props", me.id),
        json!({"channel_id": created_id, "user_id": me.id, "mark_unread": "mention"}),
    )
    .await;
    rest(
        &admin,
        "PUT",
        &format!("channels/{created_id}/privacy"),
        json!({"privacy": "P"}),
    )
    .await;
    pause().await;
    rest(
        &admin,
        "DELETE",
        &format!("channels/{created_id}/members/{}", me.id),
        json!(null),
    )
    .await;
    rest(
        &admin,
        "POST",
        &format!("channels/{created_id}/members"),
        json!({"user_id": me.id}),
    )
    .await;
    pause().await;
    rest(
        &admin,
        "DELETE",
        &format!("channels/{created_id}"),
        json!(null),
    )
    .await;
    pause().await;
    rest(
        &admin,
        "POST",
        &format!("channels/{created_id}/restore"),
        json!({}),
    )
    .await;
    let other = admin
        .users_by_usernames(&["ada.lovelace".to_string()])
        .await
        .unwrap();
    rest(
        &admin,
        "POST",
        "channels/direct",
        json!([admin_user.id, me.id]),
    )
    .await;
    rest(
        &admin,
        "POST",
        "channels/group",
        json!([admin_user.id, me.id, other[0].id]),
    )
    .await;

    // Users.
    rest(
        &admin,
        "PUT",
        &format!("users/{}/patch", me.id),
        json!({"nickname": format!("Tester {suffix}")}),
    )
    .await;
    rest(
        &admin,
        "PUT",
        &format!("users/{}/roles", me.id),
        json!({"roles": "system_user system_admin"}),
    )
    .await;
    rest(
        &admin,
        "PUT",
        &format!("users/{}/roles", me.id),
        json!({"roles": "system_user"}),
    )
    .await;
    rest(&admin, "POST", "users", json!({
        "username": format!("ws-fixture-{suffix}"), "email": format!("ws-{suffix}@example.com"), "password": password
    })).await;

    // Preferences and sidebar categories.
    let preference = Preference {
        user_id: me.id.clone(),
        category: "display_settings".into(),
        name: "use_military_time".into(),
        value: "true".into(),
    };
    tester
        .save_preferences(&me.id, std::slice::from_ref(&preference))
        .await
        .unwrap();
    pause().await;
    tester
        .delete_preferences(&me.id, &[preference])
        .await
        .unwrap();
    let categories_path = format!("users/{}/teams/{}/channels/categories", me.id, team.id);
    let category = rest(&tester, "POST", &categories_path, json!({
        "user_id": me.id, "team_id": team.id, "display_name": format!("WS {suffix}"), "type": "custom", "channel_ids": [design]
    })).await;
    let category_id = category["id"].as_str().unwrap();
    pause().await;
    rest(&tester, "PUT", &format!("{categories_path}/{category_id}"), json!({
        "id": category_id, "user_id": me.id, "team_id": team.id, "display_name": format!("WS {suffix} renamed"), "type": "custom", "channel_ids": [design]
    })).await;
    let order = rest(
        &tester,
        "GET",
        &format!("{categories_path}/order"),
        json!(null),
    )
    .await;
    let mut order: Vec<Value> = order.as_array().unwrap().clone();
    order.reverse();
    rest(
        &tester,
        "PUT",
        &format!("{categories_path}/order"),
        Value::Array(order),
    )
    .await;
    pause().await;
    rest(
        &tester,
        "DELETE",
        &format!("{categories_path}/{category_id}"),
        json!(null),
    )
    .await;

    // Teams.
    rest(
        &admin,
        "PUT",
        &format!("teams/{}/patch", team.id),
        json!({"description": format!("dev team {suffix}")}),
    )
    .await;
    let temp = rest(
        &admin,
        "POST",
        "teams",
        json!({"name": format!("ws-{suffix}"), "display_name": "WS temp", "type": "O"}),
    )
    .await;
    let temp_id = temp["id"].as_str().unwrap();
    rest(
        &admin,
        "POST",
        &format!("teams/{temp_id}/members"),
        json!({"team_id": temp_id, "user_id": me.id}),
    )
    .await;
    pause().await;
    rest(
        &admin,
        "DELETE",
        &format!("teams/{temp_id}/members/{}", me.id),
        json!(null),
    )
    .await;
    rest(
        &admin,
        "POST",
        &format!("teams/{temp_id}/members"),
        json!({"team_id": temp_id, "user_id": me.id}),
    )
    .await;
    pause().await;
    rest(&admin, "DELETE", &format!("teams/{temp_id}"), json!(null)).await;

    // Drafts.
    let draft =
        json!({"user_id": me.id, "channel_id": design, "root_id": "", "message": "draft text"});
    rest(&tester, "POST", "drafts", draft.clone()).await;
    pause().await;
    let mut edited = draft.clone();
    edited["message"] = json!("draft text, edited");
    rest(&tester, "POST", "drafts", edited).await;
    pause().await;
    rest(
        &tester,
        "DELETE",
        &format!("users/{}/channels/{design}/drafts", me.id),
        json!(null),
    )
    .await;

    // An ephemeral message from a slash command error.
    let _ = tester
        .post::<_, Value>(
            "commands/execute",
            &json!({"channel_id": design, "command": "/join ~does-not-exist"}),
        )
        .await;

    // Custom emoji (multipart) and config.
    upload_emoji(&url, &password, &admin_user.id, &format!("ws_{suffix}")).await;
    let mut config = rest(&admin, "GET", "config", json!(null)).await;
    let enabled = config["ServiceSettings"]["EnableGifPicker"]
        .as_bool()
        .unwrap_or(false);
    config["ServiceSettings"]["EnableGifPicker"] = json!(!enabled);
    rest(&admin, "PUT", "config", config.clone()).await;
    config["ServiceSettings"]["EnableGifPicker"] = json!(enabled);
    rest(&admin, "PUT", "config", config).await;

    tokio::time::sleep(Duration::from_secs(2)).await;
    drop(admin_ws);

    let mut events = recorded.lock().unwrap().clone();
    for (name, event) in admin_recorded.lock().unwrap().iter() {
        events.entry(name.clone()).or_insert_with(|| event.clone());
    }
    let dir = format!("{}/tests/fixtures/ws", env!("CARGO_MANIFEST_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, event) in &events {
        let text = serde_json::to_string_pretty(event).unwrap();
        std::fs::write(format!("{dir}/{name}.json"), text + "\n").unwrap();
    }
    eprintln!("recorded: {:?}", events.keys().collect::<Vec<_>>());
}

/// `POST emoji` is multipart, which the client does not speak, so this uses
/// reqwest directly with its own admin session.
async fn upload_emoji(url: &str, password: &str, creator: &str, name: &str) {
    // A 1x1 PNG.
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xdd, 0x8d, 0xb0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    let boundary = "mattyboundary";
    let meta = json!({"creator_id": creator, "name": name}).to_string();
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"emoji\"\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"e.png\"\r\nContent-Type: image/png\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(PNG);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let token = session_token(url, "admin", password).await;
    let response = reqwest::Client::new()
        .post(format!("{url}/api/v4/emoji"))
        .header("Authorization", format!("Bearer {token}"))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
}

/// A token from a separate session (the client keeps its own private).
async fn session_token(url: &str, login: &str, password: &str) -> String {
    let response = reqwest::Client::new()
        .post(format!("{url}/api/v4/users/login"))
        .json(&json!({"login_id": login, "password": password}))
        .send()
        .await
        .unwrap();
    response.headers()["Token"].to_str().unwrap().to_string()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
