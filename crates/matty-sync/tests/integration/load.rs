//! Signing in, the initial load, and opening a channel.

use matty_store::Store;
use matty_store::sidebar::{self, UnreadMode};
use matty_sync::Command;

use crate::world::{Engine, User, World, server, sign_in};

#[tokio::test(flavor = "multi_thread")]
async fn the_seeded_account_loads_with_the_servers_counts() {
    let Some((url, password)) = server() else {
        return;
    };
    // The seeded user has many teams, channels and unreads. Nothing else
    // changes this account's read state.
    let login = std::env::var("MATTY_TEST_LOGIN").unwrap_or_else(|_| "tester".into());
    let (client, id) = sign_in(&url, &login, &password).await;
    let me = User {
        client: client.clone(),
        id: id.clone(),
        username: login,
    };
    let engine = Engine::start(&me).await;
    let path = engine.path.clone();
    let _dir = engine.stop().await;

    // Every channel's unreads in the store match the server's counts, with
    // threads expanded (the server's own message counts).
    let store = Store::open(&path).unwrap();
    for team in client.my_teams().await.unwrap() {
        let channels = client.my_channels(&team.id).await.unwrap();
        let members = client.my_channel_members(&id, &team.id).await.unwrap();
        let sections =
            sidebar::sidebar_snapshot(store.conn(), &team.id, &id, UnreadMode::AllPosts).unwrap();
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

#[tokio::test(flavor = "multi_thread")]
async fn a_new_account_loads_its_unread_channel() {
    let Some(world) = World::new().await else {
        return;
    };
    for n in 0..3 {
        world.post(&world.other, &format!("load: {n}"), "").await;
    }
    let engine = Engine::start(&world.me).await;
    let shown = engine
        .sidebar(&world.channel.id, |c| c.unreads.messages == 3)
        .await;
    assert_eq!(shown.unreads.mentions, 0);
    assert_eq!(shown.display_name, world.channel.display_name);
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_a_channel_starts_at_the_first_unread() {
    let Some(world) = World::new().await else {
        return;
    };
    // More read and unread posts than one page around the first unread.
    let mut posts = Vec::new();
    for n in 0..70 {
        posts.push(world.post(&world.other, &format!("read {n}"), "").await);
    }
    world
        .me
        .client
        .view_channel(&world.channel.id, "")
        .await
        .unwrap();
    for n in 0..70 {
        posts.push(world.post(&world.other, &format!("unread {n}"), "").await);
    }
    let first_unread = &posts[70];

    let engine = Engine::start(&world.me).await;
    let window = engine
        .open(&world.channel.id, |posts| {
            posts.iter().any(|p| p.id == first_unread.id)
        })
        .await;
    let has = |id: &str| window.iter().any(|p| p.id == id);
    assert!(has(&posts[69].id), "the post before the first unread");
    assert!(!has(&posts[0].id), "the oldest read post is past the page");
    assert!(
        !has(&posts[139].id),
        "the newest post is past the page after"
    );
    engine.send(Command::CloseChannel {
        channel_id: world.channel.id.clone(),
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_out_revokes_the_session() {
    let Some(world) = World::new().await else {
        return;
    };
    let engine = Engine::start(&world.me).await;
    engine.send(Command::SignOut);
    engine
        .wait("the sign-out", |change| {
            matches!(
                change,
                matty_sync::ChangeSet::Connection(matty_sync::Connection::SignedOut)
            )
        })
        .await;
    let error = world.me.client.me().await.unwrap_err();
    assert!(
        matches!(error, matty_api::ApiError::Server { status: 401, .. }),
        "{error}"
    );
}
