//! Starting from a database file, showing the cache offline, signing out,
//! and the I/O thread the app uses before a session exists.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::{next, server};
use matty_api::Client;
use matty_sync::{ChangeSet, Command, Connection};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

fn connect(server: &wiremock::MockServer) -> Command {
    Command::Connect {
        client: Client::new(server.uri()).unwrap().with_token("token"),
    }
}

fn sidebar_shows_c1(change: &ChangeSet) -> bool {
    matches!(change, ChangeSet::Sidebar { team_id, sections }
        if team_id == "t1" && sections.iter().any(|s| s.channels.iter().any(|c| c.id == "c1")))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cache_shows_offline_from_the_database_file() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("account.sqlite");
    {
        let sync = matty_sync::spawn_at(path.clone(), None).unwrap();
        let changes = sync.subscribe();
        sync.send(connect(&server));
        next(&changes, |c| {
            matches!(c, ChangeSet::Connection(Connection::Loaded))
        })
        .await;
    }
    // Let the first engine close the database.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = server.received_requests().await.unwrap().len();

    let sync = matty_sync::spawn_at(path, None).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::ShowCache);
    next(&changes, sidebar_shows_c1).await;
    let unreads = next(&changes, |c| matches!(c, ChangeSet::Unreads { .. })).await;
    let ChangeSet::Unreads { teams } = unreads else {
        unreachable!()
    };
    assert_eq!(
        teams.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["t1"]
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        before,
        "showing the cache made requests"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_database_that_will_not_open_still_gives_a_running_engine() {
    let dir = tempfile::tempdir().unwrap();
    // A directory is not a database.
    let sync = matty_sync::spawn_at(dir.path().to_path_buf(), None).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::SignOut);
    next(&changes, |c| {
        matches!(c, ChangeSet::Connection(Connection::SignedOut))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_out_ends_the_session_on_the_server() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/logout"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status": "OK"})))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = common::connected(&server).await;
    sync.send(Command::SignOut);
    next(&changes, |c| {
        matches!(c, ChangeSet::Connection(Connection::SignedOut))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_out_finishes_when_the_server_cannot_be_reached() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/logout"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let (sync, changes) = common::connected(&server).await;
    sync.send(Command::SignOut);
    next(&changes, |c| {
        matches!(c, ChangeSet::Connection(Connection::SignedOut))
    })
    .await;
}

#[test]
fn io_runs_futures_for_a_caller_without_tokio() {
    let answer = matty_sync::io::run(async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        42
    });
    assert_eq!(answer.recv_blocking().unwrap(), 42);
    let pinged =
        matty_sync::io::run(async { Client::new("http://127.0.0.1:9").unwrap().ping().await });
    assert!(pinged.recv_blocking().unwrap().is_err());
}
