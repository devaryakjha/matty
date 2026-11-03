//! Initial load into the store (#35), against a mock server.
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use std::path::Path;
use std::time::Duration;

use matty_api::Client;
use matty_api::models::{Channel, ChannelMember, SidebarCategories, SidebarCategory, Team};
use matty_store::sidebar::{self, UnreadMode};
use matty_store::{Store, posts};
use matty_sync::{ChangeSet, Command, Connection};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ME: &str = "rm9qb436airnmmjzxy7soiff9c";

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../matty-api/tests/fixtures/v10.5/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn ok(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

async fn get(server: &MockServer, route: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/{route}")))
        .respond_with(response)
        .mount(server)
        .await;
}

fn channel(id: &str, team: &str, total: i64) -> Value {
    json!({
        "id": id, "team_id": team, "type": "O", "name": id, "display_name": id,
        "total_msg_count": total, "total_msg_count_root": total, "last_post_at": 10
    })
}

fn member(channel: &str, read: i64, mentions: i64) -> Value {
    json!({
        "channel_id": channel, "user_id": ME, "msg_count": read, "msg_count_root": read,
        "mention_count": mentions, "mention_count_root": mentions,
        "notify_props": {"mark_unread": "all"}
    })
}

fn categories(team: &str, channels: &[&str]) -> Value {
    json!({
        "categories": [{
            "id": format!("{team}-channels"), "team_id": team, "user_id": ME,
            "type": "channels", "display_name": "Channels", "sorting": "alpha",
            "channel_ids": channels
        }],
        "order": [format!("{team}-channels")]
    })
}

/// A server with teams t1 (channels a, b) and t2 (channel c); `users/me`
/// answers after `delay`.
async fn server(delay: Duration) -> MockServer {
    server_with(delay, None).await
}

/// The same, with `first` mounted ahead of the rest so it wins.
async fn server_with(delay: Duration, first: Option<Mock>) -> MockServer {
    let server = MockServer::start().await;
    if let Some(first) = first {
        first.mount(&server).await;
    }
    get(
        &server,
        "users/me",
        ok(fixture("users_me")).set_delay(delay),
    )
    .await;
    get(&server, "config/client", ok(fixture("config_client"))).await;
    get(&server, "license/client", ok(fixture("license_client"))).await;
    get(
        &server,
        "users/me/preferences",
        ok(json!([{"user_id": ME, "category": "teams_order", "name": "", "value": "t2,t1"}])),
    )
    .await;
    get(
        &server,
        "users/me/teams",
        ok(json!([
            {"id": "t1", "name": "one", "display_name": "One", "type": "O"},
            {"id": "t2", "name": "two", "display_name": "Two", "type": "O"}
        ])),
    )
    .await;
    get(
        &server,
        "users/me/teams/t1/channels",
        ok(json!([channel("a", "t1", 10), channel("b", "t1", 4)])),
    )
    .await;
    get(
        &server,
        &format!("users/{ME}/teams/t1/channels/members"),
        ok(json!([member("a", 7, 2), member("b", 4, 0)])),
    )
    .await;
    get(
        &server,
        "users/me/teams/t1/channels/categories",
        ok(categories("t1", &["a", "b"])),
    )
    .await;
    get(
        &server,
        "users/me/teams/t2/channels",
        ok(json!([channel("c", "t2", 3)])),
    )
    .await;
    get(
        &server,
        &format!("users/{ME}/teams/t2/channels/members"),
        ok(json!([member("c", 0, 1)])),
    )
    .await;
    get(
        &server,
        "users/me/teams/t2/channels/categories",
        ok(categories("t2", &["c"])),
    )
    .await;
    get(&server, "emoji", ok(json!([]))).await;
    server
}

fn connect(server: &MockServer) -> Command {
    Command::Connect {
        client: Client::new(server.uri()).unwrap().with_token("token"),
    }
}

/// Change sets until `stop` matches one (included), within five seconds.
async fn until(
    changes: &async_channel::Receiver<ChangeSet>,
    stop: impl Fn(&ChangeSet) -> bool,
) -> Vec<ChangeSet> {
    let mut seen = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let change = changes.recv().await.unwrap();
            let done = stop(&change);
            seen.push(change);
            if done {
                break;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out; saw {seen:#?}"));
    seen
}

fn loaded(change: &ChangeSet) -> bool {
    matches!(change, ChangeSet::Connection(Connection::Loaded))
}

fn open(dir: &Path) -> Store {
    Store::open(&dir.join("account.sqlite")).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn loads_every_team_and_matches_the_server_unreads() {
    let server = server(Duration::ZERO).await;
    let dir = tempfile::tempdir().unwrap();
    let sync = matty_sync::spawn(open(dir.path())).unwrap();
    let changes = sync.subscribe();
    sync.send(connect(&server));
    let seen = until(&changes, loaded).await;

    assert!(matches!(
        seen[0],
        ChangeSet::Connection(Connection::Loading)
    ));
    assert!(seen.iter().any(|c| matches!(c, ChangeSet::Config(_))));
    let sidebar_a = seen
        .iter()
        .find_map(|c| match c {
            ChangeSet::Sidebar { team_id, sections } if team_id == "t1" => Some(sections.clone()),
            _ => None,
        })
        .unwrap();
    let a = sidebar_a[0].channels.iter().find(|c| c.id == "a").unwrap();
    assert_eq!((a.unreads.messages, a.unreads.mentions), (3, 2));
    let rail = seen
        .iter()
        .rev()
        .find_map(|c| match c {
            ChangeSet::Unreads { teams } => Some(teams.clone()),
            _ => None,
        })
        .unwrap();
    let order: Vec<&str> = rail.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(order, ["t2", "t1"], "teams_order preference");
    assert_eq!(rail[1].mentions, 2);
    drop(sync);

    // The store agrees with what the server said.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let store = open(dir.path());
    let conn = store.conn();
    assert_eq!(posts::cursor(conn, "me").unwrap().as_deref(), Some(ME));
    let t2 = sidebar::sidebar_snapshot(conn, "t2", ME, UnreadMode::AllPosts).unwrap();
    let c = &t2[0].channels[0];
    assert_eq!(
        (c.id.as_str(), c.unreads.messages, c.unreads.mentions),
        ("c", 3, 1)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cache_is_published_before_the_server_answers() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = open(dir.path());
        let conn = store.conn();
        sidebar::upsert_teams(
            conn,
            &[Team {
                id: "t1".into(),
                name: "one".into(),
                display_name: "One".into(),
                ..Team::default()
            }],
        )
        .unwrap();
        sidebar::upsert_channels(
            conn,
            &[Channel {
                id: "cached".into(),
                team_id: "t1".into(),
                kind: "O".into(),
                name: "cached".into(),
                display_name: "Cached".into(),
                ..Channel::default()
            }],
        )
        .unwrap();
        sidebar::upsert_channel_members(
            conn,
            &[ChannelMember {
                channel_id: "cached".into(),
                user_id: ME.into(),
                ..ChannelMember::default()
            }],
        )
        .unwrap();
        sidebar::replace_categories(
            conn,
            "t1",
            &SidebarCategories {
                categories: vec![SidebarCategory {
                    id: "t1-channels".into(),
                    team_id: "t1".into(),
                    kind: "channels".into(),
                    display_name: "Channels".into(),
                    channel_ids: vec!["cached".into()],
                    ..SidebarCategory::default()
                }],
                order: vec!["t1-channels".into()],
            },
        )
        .unwrap();
        posts::set_cursor(conn, "me", ME).unwrap();
    }

    let server = server(Duration::from_secs(3)).await;
    let sync = matty_sync::spawn(open(dir.path())).unwrap();
    let changes = sync.subscribe();
    let started = std::time::Instant::now();
    sync.send(connect(&server));
    let seen = until(&changes, |c| matches!(c, ChangeSet::Unreads { .. })).await;
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    let cached = seen.iter().any(|c| match c {
        ChangeSet::Sidebar { team_id, sections } => {
            team_id == "t1" && sections[0].channels[0].id == "cached"
        }
        _ => false,
    });
    assert!(cached, "{seen:#?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn one_team_failing_does_not_stop_the_others() {
    let broken = Mock::given(method("GET"))
        .and(path("/api/v4/users/me/teams/t2/channels"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "id": "app.channel.get_channels.get.app_error",
            "message": "Unable to get the channels.", "status_code": 500
        })));
    let server = server_with(Duration::ZERO, Some(broken)).await;
    let dir = tempfile::tempdir().unwrap();
    let sync = matty_sync::spawn(open(dir.path())).unwrap();
    let changes = sync.subscribe();
    sync.send(connect(&server));
    let seen = until(&changes, loaded).await;

    let failed = seen.iter().any(|c| {
        matches!(c, ChangeSet::Connection(Connection::TeamFailed { team_id, .. }) if team_id == "t2")
    });
    assert!(failed, "{seen:#?}");
    let t1 = seen
        .iter()
        .any(|c| matches!(c, ChangeSet::Sidebar { team_id, .. } if team_id == "t1"));
    assert!(t1, "{seen:#?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_open_channel_gets_its_posts_after_the_account_loads() {
    let server = server(Duration::ZERO).await;
    get(
        &server,
        &format!("users/{ME}/channels/a/posts/unread"),
        ok(json!({
            "order": ["p2", "p1"],
            "posts": {
                "p1": {"id": "p1", "channel_id": "a", "user_id": ME, "create_at": 1, "message": "one"},
                "p2": {"id": "p2", "channel_id": "a", "user_id": ME, "create_at": 2, "message": "two"}
            },
            "next_post_id": "", "prev_post_id": ""
        })),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let sync = matty_sync::spawn(open(dir.path())).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::OpenChannel {
        channel_id: "a".into(),
    });
    sync.send(connect(&server));
    until(&changes, |c| match c {
        ChangeSet::Posts { channel_id, posts } => channel_id == "a" && posts.len() == 2,
        _ => false,
    })
    .await;
}
