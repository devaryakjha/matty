//! A window that starts showing a server whose engine is already connected
//! (#206) subscribes late and asks for the cache: the sidebars and unreads
//! come at once, without a new connection.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use matty_sync::{ChangeSet, Command};

#[tokio::test(flavor = "multi_thread")]
async fn a_late_subscriber_gets_the_sidebars_without_reconnecting() {
    let server = server().await;
    let (sync, _first) = connected(&server).await;
    let loads = requested(&server)
        .await
        .iter()
        .filter(|path| *path == "/api/v4/users/me")
        .count();
    let late = sync.subscribe();
    sync.send(Command::ShowCache);
    next(&late, |c| {
        matches!(c, ChangeSet::Sidebar { team_id, sections }
            if team_id == "t1" && sections.iter().any(|s| s.channels.iter().any(|c| c.id == "c1")))
    })
    .await;
    next(
        &late,
        |c| matches!(c, ChangeSet::Unreads { teams } if teams.iter().any(|t| t.id == "t1")),
    )
    .await;
    let again = requested(&server)
        .await
        .iter()
        .filter(|path| *path == "/api/v4/users/me")
        .count();
    assert_eq!(again, loads, "showing the cache loads nothing");
}
