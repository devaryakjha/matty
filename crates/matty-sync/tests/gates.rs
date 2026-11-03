//! Features a server can lack or turn off are skipped, not called and
//! failed (#199): one test per gate in `ServerCapabilities`.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::{ME, connected, next, requested, server_with_config};
use matty_sync::{ChangeSet, Command};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn custom_emoji_are_not_listed_when_the_server_has_them_off() {
    let server = server_with_config(json!({"EnableCustomEmoji": "false"})).await;
    let (_sync, _changes) = connected(&server).await;
    assert!(
        !requested(&server)
            .await
            .iter()
            .any(|p| p == "/api/v4/emoji")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_emoji_are_listed_when_the_server_has_them_on() {
    let server = server_with_config(json!({"EnableCustomEmoji": "true"})).await;
    // The list comes before the load reports it is done.
    let (_sync, _changes) = connected(&server).await;
    assert!(
        requested(&server)
            .await
            .iter()
            .any(|p| p == "/api/v4/emoji")
    );
}

fn save_draft() -> Command {
    Command::SaveDraft {
        channel_id: "c1".into(),
        root_id: String::new(),
        message: "half a thought".into(),
        file_ids: Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn drafts_stay_on_this_device_when_the_server_does_not_sync_them() {
    // Older servers lack synced drafts and an admin can turn them off;
    // either way the drafts endpoints must not be called.
    let server = server_with_config(json!({"AllowSyncedDrafts": "false"})).await;
    let (sync, changes) = connected(&server).await;
    sync.send(save_draft());
    let ChangeSet::Draft { draft, .. } =
        next(&changes, |c| matches!(c, ChangeSet::Draft { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(draft.unwrap().message, "half a thought");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let paths = requested(&server).await;
    assert!(
        !paths.iter().any(|p| p.contains("drafts")),
        "called {paths:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn drafts_are_saved_to_the_server_when_it_syncs_them() {
    let server = server_with_config(json!({"AllowSyncedDrafts": "true"})).await;
    Mock::given(method("POST"))
        .and(path("/api/v4/drafts"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "user_id": ME, "channel_id": "c1", "root_id": "",
            "message": "half a thought", "create_at": 1, "update_at": 1
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    sync.send(save_draft());
    next(&changes, |c| matches!(c, ChangeSet::Draft { .. })).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !requested(&server)
            .await
            .iter()
            .any(|p| p == "/api/v4/drafts")
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
