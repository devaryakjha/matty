//! Team switching (#89): sync keeps every team's sidebar and the rail's
//! badges, so the app can show any team at once, and the Threads list
//! follows the team on screen.
#![allow(clippy::unwrap_used)]

mod common;

use common::{ME, connected, next, server};
use matty_sync::{ChangeSet, Command};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn get(route: &str, body: serde_json::Value) -> Mock {
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/{route}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        // Ahead of the one-team server's own answers.
        .with_priority(1)
}

/// The common server with a second team, t2, holding channel c2 with one
/// unread mention and an icon.
async fn two_teams() -> MockServer {
    let server = server().await;
    for mock in [
        get(
            "users/me/teams",
            json!([
                {"id": "t1", "name": "one", "display_name": "One", "type": "O"},
                {"id": "t2", "name": "two", "display_name": "Two", "type": "O",
                 "last_team_icon_update": 1700}
            ]),
        ),
        get(
            "users/me/teams/t2/channels",
            json!([{"id": "c2", "team_id": "t2", "type": "O", "name": "c2",
                    "display_name": "C2", "total_msg_count": 3, "total_msg_count_root": 3}]),
        ),
        get(
            &format!("users/{ME}/teams/t2/channels/members"),
            json!([{"channel_id": "c2", "user_id": ME, "msg_count": 0, "mention_count": 1,
                    "mention_count_root": 1, "notify_props": {"mark_unread": "all"}}]),
        ),
        get(
            "users/me/teams/t2/channels/categories",
            json!({"categories": [{"id": "cat2", "team_id": "t2", "type": "channels",
                    "display_name": "Channels", "channel_ids": ["c2"]}], "order": ["cat2"]}),
        ),
    ] {
        mock.mount(&server).await;
    }
    server
}

fn thread(id: &str, channel: &str) -> serde_json::Value {
    json!({"id": id, "reply_count": 1, "last_reply_at": 300, "last_viewed_at": 100,
           "participants": [], "unread_replies": 1, "unread_mentions": 0,
           "post": {"id": id, "channel_id": channel, "user_id": "ada", "root_id": "",
                    "create_at": 100, "update_at": 100, "message": id}})
}

#[tokio::test(flavor = "multi_thread")]
async fn every_teams_sidebar_and_badge_arrive_for_the_rail() {
    let server = two_teams().await;
    let sync = matty_sync::spawn(matty_store::Store::open_in_memory().unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::Connect {
        client: matty_api::Client::new(server.uri())
            .unwrap()
            .with_token("token"),
    });
    // The other team's sidebar comes too, so switching paints from it.
    let ChangeSet::Sidebar { sections, .. } = next(&changes, |c| {
        matches!(c, ChangeSet::Sidebar { team_id, sections }
            if team_id == "t2" && !sections.is_empty())
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(sections[0].channels[0].id, "c2");
    let ChangeSet::Unreads { teams } = next(&changes, |c| {
        matches!(c, ChangeSet::Unreads { teams }
            if teams.len() == 2 && teams.iter().any(|t| t.id == "t2" && t.mentions == 1))
    })
    .await
    else {
        unreachable!()
    };
    let t2 = teams.iter().find(|t| t.id == "t2").unwrap();
    assert!(t2.has_unreads, "{t2:?}");
    assert_eq!(t2.icon, 1700, "the rail shows the team's icon");
    let t1 = teams.iter().find(|t| t.id == "t1").unwrap();
    assert_eq!((t1.has_unreads, t1.mentions, t1.icon), (false, 0, 0));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_threads_list_follows_the_team_on_screen() {
    let server = two_teams().await;
    for (team, root) in [("t1", "r1"), ("t2", "r2")] {
        Mock::given(method("GET"))
            .and(path(format!("/api/v4/users/{ME}/teams/{team}/threads")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "threads": [thread(root, if team == "t1" { "c1" } else { "c2" })],
                "total": 1,
            })))
            .expect(1)
            .mount(&server)
            .await;
    }
    let (sync, changes) = connected(&server).await;

    sync.send(Command::OpenThreadList {
        team_id: "t1".into(),
    });
    next(&changes, |c| {
        matches!(c, ChangeSet::ThreadList { team_id, threads, .. }
            if team_id == "t1" && threads.len() == 1)
    })
    .await;

    // The app switches to t2: t1's list stops following, t2's is read.
    sync.send(Command::CloseThreadList {
        team_id: "t1".into(),
    });
    sync.send(Command::OpenThreadList {
        team_id: "t2".into(),
    });
    let ChangeSet::ThreadList { threads, roots, .. } = next(&changes, |c| {
        matches!(c, ChangeSet::ThreadList { team_id, threads, .. }
            if team_id == "t2" && threads.len() == 1)
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(threads[0].id, "r2");
    assert_eq!(threads[0].channel_id, "c2");
    assert_eq!(roots[0].id, "r2");
    // Let the server calls land before the mocks check their counts.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}
