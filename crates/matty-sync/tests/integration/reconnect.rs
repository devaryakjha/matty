//! Coming back after missing events.

use matty_api::PostPatch;
use matty_store::Store;
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
