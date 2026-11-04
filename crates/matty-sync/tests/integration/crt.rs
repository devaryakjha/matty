//! Unread counts with Collapsed Reply Threads on and off.

use matty_api::capabilities::CollapsedThreads;
use matty_api::models::Preference;
use matty_store::sidebar::{self, UnreadMode};

use matty_sync::{ChangeSet, Command};

use crate::world::{Engine, World, eventually};

/// Sets the user's collapsed-threads choice on the server.
async fn choose(world: &World, on: bool) {
    world
        .me
        .client
        .save_preferences(
            &world.me.id,
            &[Preference {
                user_id: world.me.id.clone(),
                category: "display_settings".into(),
                name: "collapsed_reply_threads".into(),
                value: if on { "on" } else { "off" }.into(),
            }],
        )
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn unreads_count_roots_with_threads_collapsed_and_every_post_without() {
    let Some(world) = World::new().await else {
        return;
    };
    let capabilities = world.me.client.capabilities().await.unwrap();
    if !matches!(
        capabilities.collapsed_threads,
        CollapsedThreads::DefaultOn | CollapsedThreads::DefaultOff
    ) {
        eprintln!("the server does not let users choose collapsed threads; skipping");
        return;
    }
    // One root and two replies, one of which mentions the user.
    let root = world.post(&world.other, "crt: root", "").await;
    world.post(&world.other, "crt: a reply", &root.id).await;
    let mention = format!("crt: @{} look", world.me.username);
    world.post(&world.other, &mention, &root.id).await;
    let channel = world.me.client.channel(&world.channel.id).await.unwrap();
    let member = world
        .me
        .client
        .my_channel_member(&world.channel.id)
        .await
        .unwrap();

    // Off: every post counts, and so does the mention in the reply.
    choose(&world, false).await;
    let engine = Engine::start(&world.me).await;
    let off = sidebar::unreads(&channel, &member, UnreadMode::AllPosts);
    assert_eq!((off.messages, off.mentions), (3, 1), "the server's counts");
    engine
        .sidebar(&world.channel.id, |c| c.unreads == off)
        .await;

    // On, while running: only the root counts, and the reply's mention
    // belongs to the thread.
    choose(&world, true).await;
    let on = sidebar::unreads(&channel, &member, UnreadMode::CollapsedThreads);
    assert_eq!(
        (on.messages, on.mentions),
        (1, 0),
        "the server's root counts"
    );
    engine.sidebar(&world.channel.id, |c| c.unreads == on).await;
}

/// The user's followed threads in the world's team, as the server has them.
async fn followed(world: &World) -> Vec<matty_api::models::UserThread> {
    world
        .me
        .client
        .user_threads(
            &world.me.id,
            &world.team_id,
            &matty_api::ThreadsQuery::default(),
        )
        .await
        .unwrap()
        .threads
}

#[tokio::test(flavor = "multi_thread")]
async fn the_threads_view_reads_and_unfollows_on_the_server() {
    let Some(world) = World::new().await else {
        return;
    };
    let capabilities = world.me.client.capabilities().await.unwrap();
    if matches!(capabilities.collapsed_threads, CollapsedThreads::Disabled) {
        eprintln!("the server has collapsed threads off; skipping");
        return;
    }
    choose(&world, true).await;
    // A mention in a reply makes the user follow the thread, unread.
    let root = world.post(&world.other, "threads: root", "").await;
    let mention = format!("threads: @{} look", world.me.username);
    world.post(&world.other, &mention, &root.id).await;
    let engine = Engine::start(&world.me).await;

    engine.send(Command::OpenThreadList {
        team_id: world.team_id.clone(),
    });
    engine
        .wait("the followed thread, unread", |change| {
            matches!(change, ChangeSet::ThreadList { threads, roots, .. }
                if threads.iter().any(|t| t.id == root.id && t.unread_replies > 0)
                    && roots.iter().any(|r| r.id == root.id))
        })
        .await;

    engine.send(Command::ReadThread {
        team_id: world.team_id.clone(),
        root_id: root.id.clone(),
    });
    eventually("the server to have the thread read", || async {
        followed(&world)
            .await
            .iter()
            .any(|t| t.id == root.id && t.unread_replies == 0)
    })
    .await;

    engine.send(Command::FollowThread {
        team_id: world.team_id.clone(),
        root_id: root.id.clone(),
        follow: false,
    });
    eventually("the server to drop the thread", || async {
        !followed(&world).await.iter().any(|t| t.id == root.id)
    })
    .await;
}
