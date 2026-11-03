//! Runs the initial load against a real server when `MATTY_TEST_SERVER` is
//! set, such as the seeded dev server (docs/dev-server.md):
//! `MATTY_TEST_SERVER=http://localhost:8065 MATTY_TEST_PASSWORD='Password1!'
//! cargo test -p matty-sync --test live`. Without it, the test returns at once.
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use std::time::Duration;

use matty_api::{Client, Credentials};
use matty_store::Store;
use matty_store::sidebar::{self, UnreadMode};
use matty_sync::{ChangeSet, Command, Connection};

#[tokio::test(flavor = "multi_thread")]
async fn the_loaded_sidebar_matches_the_server() {
    let Ok(url) = std::env::var("MATTY_TEST_SERVER") else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return;
    };
    let login = std::env::var("MATTY_TEST_LOGIN").unwrap_or_else(|_| "tester".into());
    let password = std::env::var("MATTY_TEST_PASSWORD").unwrap();
    let signed_in = Client::new(url)
        .unwrap()
        .login(&Credentials {
            login_id: login,
            password,
            device_id: "matty-sync-live-test".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let client = signed_in.client;
    let me = signed_in.user.id;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("account.sqlite");
    let sync = matty_sync::spawn(Store::open(&path).unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::Connect {
        client: client.clone(),
    });
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match changes.recv().await.unwrap() {
                ChangeSet::Connection(Connection::Loaded) => break,
                ChangeSet::Connection(Connection::Failed { error })
                | ChangeSet::Connection(Connection::TeamFailed { error, .. }) => {
                    panic!("load failed: {error}")
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    drop(sync);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Every channel's unreads in the store match the server's counts, with
    // threads expanded (the server's own message counts).
    let store = Store::open(&path).unwrap();
    for team in client.my_teams().await.unwrap() {
        let channels = client.my_channels(&team.id).await.unwrap();
        let members = client.my_channel_members(&me, &team.id).await.unwrap();
        let sections =
            sidebar::sidebar_snapshot(store.conn(), &team.id, &me, UnreadMode::AllPosts).unwrap();
        for channel in channels.iter().filter(|c| c.delete_at == 0) {
            let Some(member) = members.iter().find(|m| m.channel_id == channel.id) else {
                continue;
            };
            let expected = sidebar::unreads(channel, member, UnreadMode::AllPosts);
            let shown = sections
                .iter()
                .flat_map(|s| s.channels.iter())
                .find(|c| c.id == channel.id)
                .unwrap_or_else(|| panic!("{} is not in the sidebar", channel.name));
            assert_eq!(shown.unreads, expected, "{}", channel.name);
        }
    }
}
