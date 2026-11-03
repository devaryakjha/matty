//! Initial-load models decode the responses recorded from Mattermost 10.5
//! (`scripts/record-fixtures.py` against the seeded dev server), and the paged and conditional helpers work.

#![allow(clippy::unwrap_used)]

use matty_api::{
    Client, Fetched, ServerCapabilities, Version,
    capabilities::OldFormat,
    models::{
        Channel, ChannelMember, ChannelStats, Emoji, Preference, SidebarCategories, Status, Team,
        TeamMember, TeamUnread, User,
    },
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path, query_param},
};

fn fixture<T: DeserializeOwned>(name: &str) -> T {
    let path = format!(
        "{}/tests/fixtures/v10.5/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn recorded_responses_decode() {
    let me: User = fixture("users_me");
    assert_eq!(me.username, "tester");
    let teams: Vec<Team> = fixture("my_teams");
    assert_eq!(teams[0].name, "dev");
    let members: Vec<TeamMember> = fixture("my_team_members");
    assert_eq!(members[0].user_id, me.id);
    let unreads: Vec<TeamUnread> = fixture("team_unreads");
    assert!(unreads.is_empty() || !unreads[0].team_id.is_empty());
    let channels: Vec<Channel> = fixture("my_channels");
    assert!(channels.iter().any(|c| c.name == "design" && c.kind == "O"));
    let categories: SidebarCategories = fixture("sidebar_categories");
    let kinds: Vec<&str> = categories
        .categories
        .iter()
        .map(|c| c.kind.as_str())
        .collect();
    assert!(kinds.contains(&"favorites") && kinds.contains(&"channels"));
    assert_eq!(categories.order.len(), categories.categories.len());
    let _: Vec<Preference> = fixture("preferences");
    let emoji: Vec<Emoji> = fixture("emoji_page");
    assert_eq!(emoji[0].name, "matty");
    let users: Vec<User> = fixture("users_page");
    assert!(users.iter().any(|u| u.username == "ada.lovelace"));
    let by_id: Vec<User> = fixture("users_by_ids");
    assert_eq!(by_id[0].id, me.id);
    let statuses: Vec<Status> = fixture("statuses");
    assert_eq!(statuses[0].user_id, me.id);
    let stats: ChannelStats = fixture("channel_stats");
    assert!(stats.member_count >= 1);
}

#[test]
fn channel_members_decode_root_and_non_root_counts() {
    let members: Vec<ChannelMember> = fixture("my_channel_members");
    assert!(!members.is_empty());
    let raw: Vec<Value> = fixture("my_channel_members");
    for (member, raw) in members.iter().zip(&raw) {
        assert_eq!(member.msg_count, raw["msg_count"].as_i64().unwrap());
        assert_eq!(
            member.msg_count_root,
            raw["msg_count_root"].as_i64().unwrap()
        );
        assert_eq!(member.mention_count, raw["mention_count"].as_i64().unwrap());
        assert_eq!(
            member.mention_count_root,
            raw["mention_count_root"].as_i64().unwrap()
        );
        assert!(!member.is_muted());
    }
    let muted: ChannelMember = serde_json::from_value(
        json!({"channel_id": "c", "notify_props": {"mark_unread": "mention"}}),
    )
    .unwrap();
    assert!(muted.is_muted());
    assert_eq!(muted.msg_count_root, 0, "missing fields default");
}

#[test]
fn the_recorded_server_config_gives_capabilities() {
    let config: OldFormat = fixture("config_client");
    let license: OldFormat = fixture("license_client");
    let caps = ServerCapabilities::from_old_format(&config, &license).unwrap();
    assert_eq!((caps.version.major, caps.version.minor), (10, 5));
    assert!(caps.version >= Version::new(10, 5, 0));
    assert!(!caps.licensed);
    assert!(caps.minimum_supported().is_ok());
}

#[test]
fn the_recorded_login_error_has_an_id() {
    let body: Value = fixture("login_invalid_credentials");
    assert_eq!(
        body["id"],
        "api.user.login.invalid_credentials_email_username"
    );
}

fn emoji(n: usize) -> Vec<Value> {
    (0..n)
        .map(|i| json!({"id": format!("e{i}"), "name": format!("e{i}")}))
        .collect()
}

#[tokio::test]
async fn emoji_pages_until_a_short_page() {
    let server = MockServer::start().await;
    for (page, count) in [("0", 200), ("1", 200), ("2", 3)] {
        Mock::given(method("GET"))
            .and(path("/api/v4/emoji"))
            .and(query_param("page", page))
            .and(query_param("per_page", "200"))
            .respond_with(ResponseTemplate::new(200).set_body_json(emoji(count)))
            .expect(1)
            .mount(&server)
            .await;
    }
    let all = Client::new(server.uri())
        .unwrap()
        .all_emoji()
        .await
        .unwrap();
    assert_eq!(all.len(), 403);
}

#[tokio::test]
async fn users_stop_at_an_empty_page() {
    let server = MockServer::start().await;
    let page = |n: usize| -> Vec<Value> {
        (0..n)
            .map(|i| json!({"id": format!("u{i}"), "username": format!("u{i}")}))
            .collect()
    };
    Mock::given(method("GET"))
        .and(path("/api/v4/users"))
        .and(query_param("page", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(200)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(0)))
        .expect(1)
        .mount(&server)
        .await;
    let all = Client::new(server.uri())
        .unwrap()
        .all_users()
        .await
        .unwrap();
    assert_eq!(all.len(), 200);
}

#[tokio::test]
async fn avatar_is_not_modified_when_the_etag_matches() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/u1/image"))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/u1/image"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"v1\"")
                .insert_header("Content-Type", "image/png")
                .set_body_bytes(vec![0x89, b'P', b'N', b'G']),
        )
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    let Fetched::Fresh {
        bytes,
        etag,
        content_type,
    } = client.avatar("u1", None).await.unwrap()
    else {
        panic!("expected the image");
    };
    assert_eq!(bytes, vec![0x89, b'P', b'N', b'G']);
    assert_eq!(content_type.as_deref(), Some("image/png"));
    let etag = etag.unwrap();
    assert_eq!(
        client.avatar("u1", Some(&etag)).await.unwrap(),
        Fetched::NotModified
    );
}

#[tokio::test]
async fn set_status_sends_the_dnd_end_time() {
    use wiremock::matchers::body_partial_json;
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/api/v4/users/u1/status"))
        .and(body_partial_json(
            json!({"user_id": "u1", "status": "dnd", "dnd_end_time": 1800000000}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"user_id": "u1", "status": "dnd", "dnd_end_time": 1800000000}),
            ),
        )
        .mount(&server)
        .await;
    let status = Client::new(server.uri())
        .unwrap()
        .set_status("u1", "dnd", Some(1_800_000_000))
        .await
        .unwrap();
    assert_eq!(status.dnd_end_time, 1_800_000_000);
}
