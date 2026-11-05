//! M1 feature endpoints (#17): search, saved and pinned posts, browsing and
//! joining channels, direct and group messages, reactions and autocomplete.
//! Responses were recorded from the seeded Mattermost 10.5 dev server.

#![allow(clippy::unwrap_used)]

use matty_api::{
    Client, SearchQuery,
    models::{Channel, ChannelMember, PostList, Reaction, SearchResults, UserAutocomplete},
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path, query_param, query_param_is_missing},
};

fn raw(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/v10.5/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn fixture<T: DeserializeOwned>(name: &str) -> T {
    serde_json::from_value(raw(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn post_lists_decode() {
    let found: SearchResults = fixture("search_posts");
    let hits: Vec<_> = found.list.ordered().collect();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].message.contains("zebrafish"));
    // The database backend does not report matches; the server sends null.
    assert!(found.matches.is_empty());
    let with_matches: SearchResults = serde_json::from_value(json!({
        "order": [], "posts": {}, "matches": { "p1": ["zebrafish"] }
    }))
    .unwrap();
    assert_eq!(with_matches.matches["p1"], ["zebrafish"]);

    let saved: PostList = fixture("saved_posts");
    assert!(!saved.order.is_empty());
    assert_eq!(saved.ordered().count(), saved.order.len());

    let pinned: PostList = fixture("pinned_posts");
    assert!(!pinned.order.is_empty());
    assert!(pinned.ordered().all(|p| p.is_pinned));
}

#[test]
fn reactions_decode() {
    let reactions: Vec<Reaction> = fixture("reactions");
    assert_eq!(reactions.len(), 2);
    assert!(reactions.iter().any(|r| r.emoji_name == "+1"));
    let post = reactions[0].post_id.clone();
    assert!(
        reactions
            .iter()
            .all(|r| r.post_id == post && r.create_at > 0)
    );
}

#[test]
fn channels_decode() {
    let public: Vec<Channel> = fixture("public_channels");
    assert!(public.iter().all(|c| c.kind == "O"));
    assert!(public.iter().any(|c| c.name == "busy"));

    let searched: Vec<Channel> = fixture("search_channels");
    assert!(searched.iter().any(|c| c.name == "design"));
    let completed: Vec<Channel> = fixture("autocomplete_channels");
    assert!(completed.iter().any(|c| c.name == "engineering"));

    let joined: ChannelMember = fixture("channel_member_added");
    assert!(!joined.channel_id.is_empty() && !joined.user_id.is_empty());
    assert!(joined.roles.contains("channel_user"));

    let direct: Channel = fixture("direct_channel");
    assert_eq!(direct.kind, "D");
    assert!(direct.team_id.is_empty());
    assert!(direct.name.contains("__"));
    let group: Channel = fixture("group_channel");
    assert_eq!(group.kind, "G");
    assert!(group.display_name.contains(','));
}

#[test]
fn user_autocomplete_decodes() {
    let in_team: UserAutocomplete = fixture("users_autocomplete");
    assert!(!in_team.users.is_empty());
    assert!(in_team.out_of_channel.is_empty());
    let in_channel: UserAutocomplete = fixture("users_autocomplete_in_channel");
    assert!(!in_channel.users.is_empty());
    let split: UserAutocomplete = serde_json::from_value(json!({
        "users": [{ "id": "u1", "username": "ada" }],
        "out_of_channel": [{ "id": "u2", "username": "alan" }]
    }))
    .unwrap();
    assert_eq!(split.out_of_channel[0].username, "alan");
}

#[tokio::test]
async fn search_sends_every_field() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/teams/t1/posts/search"))
        .and(body_json(json!({
            "terms": "from:ada release",
            "is_or_search": true,
            "time_zone_offset": 19800,
            "page": 2,
            "per_page": 50,
            "include_deleted_channels": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(raw("search_posts")))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    let query = SearchQuery {
        is_or_search: true,
        time_zone_offset: 19800,
        page: 2,
        per_page: 50,
        include_deleted_channels: true,
        ..SearchQuery::new("from:ada release")
    };
    let found = client.search_posts("t1", &query).await.unwrap();
    assert_eq!(found.list.order.len(), 1);
    assert_eq!(
        serde_json::to_value(SearchQuery::new("x")).unwrap(),
        json!({
            "terms": "x", "is_or_search": false, "time_zone_offset": 0, "page": 0,
            "per_page": 20, "include_deleted_channels": false
        })
    );
}

#[tokio::test]
async fn requests_use_the_expected_paths_and_bodies() {
    let server = MockServer::start().await;
    let ok = || ResponseTemplate::new(200).set_body_json(json!({ "status": "OK" }));
    let mocks = [
        Mock::given(method("GET"))
            .and(path("/api/v4/users/me1/posts/flagged"))
            .and(query_param("page", "1"))
            .and(query_param("per_page", "30"))
            .respond_with(ResponseTemplate::new(200).set_body_json(raw("saved_posts"))),
        Mock::given(method("GET"))
            .and(path("/api/v4/channels/c1/pinned"))
            .respond_with(ResponseTemplate::new(200).set_body_json(raw("pinned_posts"))),
        Mock::given(method("POST"))
            .and(path("/api/v4/posts/p1/pin"))
            .respond_with(ok()),
        Mock::given(method("POST"))
            .and(path("/api/v4/posts/p1/unpin"))
            .respond_with(ok()),
        Mock::given(method("GET"))
            .and(path("/api/v4/posts/p1/reactions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(Value::Null)),
        Mock::given(method("GET"))
            .and(path("/api/v4/teams/t1/channels"))
            .and(query_param("page", "0"))
            .and(query_param("per_page", "60"))
            .respond_with(ResponseTemplate::new(200).set_body_json(raw("public_channels"))),
        Mock::given(method("POST"))
            .and(path("/api/v4/teams/t1/channels/search"))
            .and(body_json(json!({ "term": "des" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(raw("search_channels"))),
        Mock::given(method("GET"))
            .and(path("/api/v4/teams/t1/channels/autocomplete"))
            .and(query_param("name", "eng"))
            .respond_with(ResponseTemplate::new(200).set_body_json(raw("autocomplete_channels"))),
        Mock::given(method("POST"))
            .and(path("/api/v4/channels/c1/members"))
            .and(body_json(json!({ "user_id": "me1" })))
            .respond_with(ResponseTemplate::new(201).set_body_json(raw("channel_member_added"))),
        Mock::given(method("DELETE"))
            .and(path("/api/v4/channels/c1/members/me1"))
            .respond_with(ok()),
        Mock::given(method("POST"))
            .and(path("/api/v4/channels/direct"))
            .and(body_json(json!(["me1", "u2"])))
            .respond_with(ResponseTemplate::new(201).set_body_json(raw("direct_channel"))),
        Mock::given(method("POST"))
            .and(path("/api/v4/channels/group"))
            .and(body_json(json!(["me1", "u2", "u3"])))
            .respond_with(ResponseTemplate::new(201).set_body_json(raw("group_channel"))),
        Mock::given(method("GET"))
            .and(path("/api/v4/users/autocomplete"))
            .and(query_param("name", "ad"))
            .and(query_param("in_team", "t1"))
            .and(query_param_is_missing("in_channel"))
            .respond_with(ResponseTemplate::new(200).set_body_json(raw("users_autocomplete"))),
    ];
    for mock in mocks {
        mock.expect(1).mount(&server).await;
    }
    let client = Client::new(server.uri()).unwrap().with_token("t");
    client.saved_posts("me1", 1, 30).await.unwrap();
    client.pinned_posts("c1").await.unwrap();
    client.pin_post("p1").await.unwrap();
    client.unpin_post("p1").await.unwrap();
    assert!(client.post_reactions("p1").await.unwrap().is_empty());
    client.public_channels("t1", 0, 60).await.unwrap();
    client.search_channels("t1", "des").await.unwrap();
    client.autocomplete_channels("t1", "eng").await.unwrap();
    client.add_channel_member("c1", "me1").await.unwrap();
    client.remove_channel_member("c1", "me1").await.unwrap();
    assert_eq!(client.direct_channel("me1", "u2").await.unwrap().kind, "D");
    let group = ["me1", "u2", "u3"].map(String::from);
    assert_eq!(client.group_channel(&group).await.unwrap().kind, "G");
    client
        .autocomplete_users("ad", Some("t1"), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn browsing_creating_and_team_people_use_the_expected_requests() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/channels/stats/member_count"))
        .and(body_json(json!(["c1", "c2"])))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"c1": 3, "c2": 12})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/channels"))
        .and(body_json(json!({
            "team_id": "t1", "name": "ship-it", "display_name": "Ship it",
            "purpose": "", "type": "P"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "c9", "team_id": "t1", "type": "P", "name": "ship-it", "display_name": "Ship it"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users"))
        .and(query_param("in_team", "t1"))
        .and(query_param("active", "true"))
        .and(query_param("page", "0"))
        .and(query_param("per_page", "200"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{"id": "u2", "username": "ada"}])),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    let counts = client
        .channel_member_counts(&["c1".into(), "c2".into()])
        .await
        .unwrap();
    assert_eq!(counts["c2"], 12);
    let created = client
        .create_channel(&matty_api::NewChannel {
            team_id: "t1".into(),
            name: "ship-it".into(),
            display_name: "Ship it".into(),
            purpose: String::new(),
            kind: "P".into(),
        })
        .await
        .unwrap();
    assert_eq!(created.id, "c9");
    assert_eq!(
        client.team_users("t1", 0, 200).await.unwrap()[0].username,
        "ada"
    );
}

#[tokio::test]
async fn channel_settings_use_the_expected_requests() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/api/v4/channels/c1/patch"))
        .and(body_json(json!({ "header": "Ship it" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "c1", "team_id": "t1", "type": "O", "name": "c1", "display_name": "C1", "header": "Ship it"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v4/channels/c1/members/me1/notify_props"))
        .and(body_json(
            json!({ "mark_unread": "mention", "desktop": "all" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "OK" })))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    let patch = matty_api::ChannelPatch {
        header: Some("Ship it".into()),
        ..matty_api::ChannelPatch::default()
    };
    assert_eq!(
        client.patch_channel("c1", &patch).await.unwrap().header,
        "Ship it"
    );
    let props = std::collections::HashMap::from([
        ("mark_unread".to_string(), "mention".to_string()),
        ("desktop".to_string(), "all".to_string()),
    ]);
    client
        .update_channel_notify_props("c1", "me1", &props)
        .await
        .unwrap();
}

/// #208: a channel's members with their roles, and making one an admin.
#[tokio::test]
async fn channel_members_and_admins_use_the_expected_requests() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users"))
        .and(query_param("in_channel", "c1"))
        .and(query_param("active", "true"))
        .and(query_param("sort", "admin"))
        .and(query_param("page", "0"))
        .and(query_param("per_page", "200"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{"id": "u2", "username": "ada"}])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/members"))
        .and(query_param("page", "0"))
        .and(query_param("per_page", "200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"channel_id": "c1", "user_id": "u2", "roles": "channel_user", "scheme_admin": true}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v4/channels/c1/members/u2/schemeRoles"))
        .and(body_json(
            json!({ "scheme_admin": false, "scheme_user": true }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "OK" })))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    assert_eq!(
        client.channel_users("c1", 0, 200).await.unwrap()[0].username,
        "ada"
    );
    let members: Vec<ChannelMember> = client.channel_members("c1", 0, 200).await.unwrap();
    assert!(members[0].scheme_admin);
    client.set_channel_admin("c1", "u2", false).await.unwrap();
}
