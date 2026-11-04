//! Coming back after missing events.

use matty_api::models::Post;
use matty_api::{PostPatch, PostsQuery};
use matty_store::Store;
use matty_store::outbox::{self, Outgoing};
use matty_store::posts;
use matty_sync::{ChangeSet, Command, Connection};
use serde_json::{Value, json};

use crate::world::{Engine, World};

#[tokio::test(flavor = "multi_thread")]
async fn coming_back_catches_up_on_what_was_missed() {
    let Some(world) = World::new().await else {
        return;
    };
    let kept = world.post(&world.me, "reconnect: kept", "").await;
    let doomed = world.post(&world.me, "reconnect: deleted", "").await;

    // Online once, so the channel and both posts are cached.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("account.sqlite");
    let engine = Engine::start_at(&world.me, path.clone()).await;
    engine
        .open(&world.channel.id, |posts| {
            posts.iter().any(|p| p.id == doomed.id)
        })
        .await;
    engine.stop().await;

    // Away: an edit, a delete, a new post and someone's reaction.
    let client = &world.me.client;
    client
        .patch_post(
            &kept.id,
            &PostPatch {
                message: Some("reconnect: kept, edited".into()),
                ..PostPatch::default()
            },
        )
        .await
        .unwrap();
    client.delete_post(&doomed.id).await.unwrap();
    let added = world.post(&world.other, "reconnect: new", "").await;
    world
        .other
        .client
        .post::<_, Value>(
            "reactions",
            &json!({"user_id": world.other.id, "post_id": kept.id, "emoji_name": "tada"}),
        )
        .await
        .unwrap();

    // Back: the load, then a catch-up.
    let engine = Engine::start_at(&world.me, path.clone()).await;
    engine.send(Command::Resync);
    engine
        .wait("the catch-up", |change| {
            matches!(change, ChangeSet::Connection(Connection::Resynced))
        })
        .await;
    engine.stop().await;

    let store = Store::open(&path).unwrap();
    let conn = store.conn();
    let edited = posts::post(conn, &kept.id).unwrap().unwrap();
    assert_eq!(edited.message, "reconnect: kept, edited");
    let emoji: Vec<&str> = edited
        .metadata
        .reactions
        .iter()
        .map(|r| r.emoji_name.as_str())
        .collect();
    assert_eq!(emoji, ["tada"]);
    let gone = posts::post(conn, &doomed.id).unwrap().unwrap();
    assert!(gone.delete_at > 0, "the delete was applied");
    assert!(posts::post(conn, &added.id).unwrap().is_some());
}

/// Whether `new` shows `old` without a reset, as the app's view patches
/// it (#164): the sent posts of `old` that are still there sit together
/// in their order, so new posts go above or below them, never between.
/// Unsent rows may move to the bottom.
fn patches(old: &[Post], new: &[Post]) -> bool {
    let sent = |post: &&Post| post.id != post.pending_post_id;
    let shown: Vec<&str> = old.iter().filter(sent).map(|p| p.id.as_str()).collect();
    let at: Vec<usize> = shown
        .iter()
        .filter_map(|id| new.iter().filter(sent).position(|p| p.id == *id))
        .collect();
    at.windows(2).all(|pair| pair[1] == pair[0] + 1)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backfill_brings_the_gap_once_in_order_and_leaves_a_failed_send() {
    let Some(world) = World::new().await else {
        return;
    };
    let edited = world.post(&world.me, "backfill: to edit", "").await;
    let reacted = world.post(&world.other, "backfill: to react to", "").await;

    // Online once, so the channel is cached.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("account.sqlite");
    let engine = Engine::start_at(&world.me, path.clone()).await;
    engine
        .open(&world.channel.id, |posts| {
            posts.iter().any(|p| p.id == reacted.id)
        })
        .await;
    engine.stop().await;

    // A send the server refused, waiting for Retry.
    let pending = format!("{}:backfill", world.me.id);
    {
        let store = Store::open(&path).unwrap();
        let conn = store.conn();
        let at = posts::newest_create_at(conn, &world.channel.id).unwrap() + 1;
        outbox::push(
            conn,
            &Outgoing {
                pending_post_id: pending.clone(),
                channel_id: world.channel.id.clone(),
                message: "backfill: not sent".into(),
                created_at: at,
                attempts: 1,
                last_error: Some("refused".into()),
                ..Outgoing::default()
            },
        )
        .unwrap();
        let row = Post {
            id: pending.clone(),
            pending_post_id: pending.clone(),
            channel_id: world.channel.id.clone(),
            user_id: world.me.id.clone(),
            message: "backfill: not sent".into(),
            create_at: at,
            ..Post::default()
        };
        posts::insert_posts(conn, &[row]).unwrap();
    }

    // Away: fifty posts, an edit and a reaction.
    let mut gap = Vec::new();
    for i in 0..50 {
        gap.push(
            world
                .post(&world.other, &format!("backfill: gap {i}"), "")
                .await,
        );
    }
    gap.sort_by(|a, b| (a.create_at, &a.id).cmp(&(b.create_at, &b.id)));
    world
        .me
        .client
        .patch_post(
            &edited.id,
            &PostPatch {
                message: Some("backfill: edited".into()),
                ..PostPatch::default()
            },
        )
        .await
        .unwrap();
    world
        .other
        .client
        .post::<_, Value>(
            "reactions",
            &json!({"user_id": world.other.id, "post_id": reacted.id, "emoji_name": "tada"}),
        )
        .await
        .unwrap();

    // Back: the cached window first, then the catch-up patches it.
    let engine = Engine::start_at(&world.me, path.clone()).await;
    let cached = engine
        .open(&world.channel.id, |posts| {
            posts.iter().any(|p| p.id == pending)
        })
        .await;
    engine.send(Command::Resync);
    let mut shown = cached;
    let mut caught_up = false;
    engine
        .wait("the catch-up", |change| {
            match change {
                ChangeSet::Posts { channel_id, posts } if *channel_id == world.channel.id => {
                    assert!(patches(&shown, posts), "a snapshot would reset the view");
                    shown = posts.to_vec();
                }
                ChangeSet::Connection(Connection::Resynced) => caught_up = true,
                _ => {}
            }
            caught_up && gap.iter().all(|post| shown.iter().any(|p| p.id == post.id))
        })
        .await;
    engine.stop().await;

    // Each gap post once, in the server's order, after what was cached.
    let ids: Vec<&str> = shown
        .iter()
        .filter(|p| p.message.starts_with("backfill: gap"))
        .map(|p| p.id.as_str())
        .collect();
    let want: Vec<&str> = gap.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, want);
    let edited_at = shown.iter().position(|p| p.id == edited.id).unwrap();
    let first_gap = shown.iter().position(|p| p.id == gap[0].id).unwrap();
    assert!(edited_at < first_gap);

    let store = Store::open(&path).unwrap();
    let conn = store.conn();
    let post = posts::post(conn, &edited.id).unwrap().unwrap();
    assert_eq!(post.message, "backfill: edited");
    let post = posts::post(conn, &reacted.id).unwrap().unwrap();
    let emoji: Vec<&str> = post
        .metadata
        .reactions
        .iter()
        .map(|r| r.emoji_name.as_str())
        .collect();
    assert_eq!(emoji, ["tada"]);

    // The failed send is still a failed row, and the server never got it.
    assert!(shown.iter().any(|p| p.id == pending));
    let queued = outbox::get(conn, &pending).unwrap().unwrap();
    assert_eq!(queued.last_error.as_deref(), Some("refused"));
    assert_eq!(queued.attempts, 1);
    let on_server = world
        .me
        .client
        .channel_posts(&world.channel.id, &PostsQuery::latest(100))
        .await
        .unwrap();
    assert!(
        on_server
            .posts
            .values()
            .all(|p| p.message != "backfill: not sent")
    );
}
