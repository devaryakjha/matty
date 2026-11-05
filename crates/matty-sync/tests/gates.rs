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

#[tokio::test(flavor = "multi_thread")]
async fn drafts_on_the_server_load_on_connect_when_it_syncs_them() {
    let server = server_with_config(json!({"AllowSyncedDrafts": "true"})).await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/users/{ME}/teams/t1/drafts")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "user_id": ME, "channel_id": "c1", "root_id": "r1",
            "message": "from the web app", "file_ids": ["f1"],
            "create_at": 1, "update_at": 2
        }])))
        .expect(1)
        .mount(&server)
        .await;
    let sync = matty_sync::spawn(matty_store::Store::open_in_memory().unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::Connect {
        client: matty_api::Client::new(server.uri())
            .unwrap()
            .with_token("token"),
    });
    let ChangeSet::Draft {
        channel_id,
        root_id,
        draft,
    } = next(&changes, |c| matches!(c, ChangeSet::Draft { .. })).await
    else {
        unreachable!()
    };
    assert_eq!((channel_id.as_str(), root_id.as_str()), ("c1", "r1"));
    let draft = draft.unwrap();
    assert_eq!(draft.message, "from the web app");
    assert_eq!(draft.file_ids, ["f1"]);
}

/// A draft with files survives a relaunch: the next run publishes it from
/// the store before the server answers, so the composer gets it back.
#[tokio::test(flavor = "multi_thread")]
async fn a_saved_draft_and_its_files_come_back_after_a_relaunch() {
    let dir = tempfile::tempdir().unwrap();
    let on_disk = dir.path().join("account.sqlite");
    let server = server_with_config(json!({"AllowSyncedDrafts": "false"})).await;
    let connect = |sync: &matty_sync::SyncHandle| {
        sync.send(Command::Connect {
            client: matty_api::Client::new(server.uri())
                .unwrap()
                .with_token("token"),
        });
    };
    {
        let sync = matty_sync::spawn(matty_store::Store::open(&on_disk).unwrap()).unwrap();
        let changes = sync.subscribe();
        connect(&sync);
        sync.send(Command::SaveDraft {
            channel_id: "c1".into(),
            root_id: String::new(),
            message: "half a thought".into(),
            file_ids: vec!["f1".into()],
        });
        next(&changes, |c| matches!(c, ChangeSet::Draft { .. })).await;
        drop(sync);
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let sync = matty_sync::spawn(matty_store::Store::open(&on_disk).unwrap()).unwrap();
    let changes = sync.subscribe();
    connect(&sync);
    let ChangeSet::Draft { draft, .. } =
        next(&changes, |c| matches!(c, ChangeSet::Draft { .. })).await
    else {
        unreachable!()
    };
    let draft = draft.unwrap();
    assert_eq!(draft.message, "half a thought");
    assert_eq!(draft.file_ids, ["f1"]);

    // Sending clears it: an empty draft deletes it, here and on relaunch.
    sync.send(Command::SaveDraft {
        channel_id: "c1".into(),
        root_id: String::new(),
        message: String::new(),
        file_ids: Vec::new(),
    });
    let ChangeSet::Draft { draft, .. } = next(&changes, |c| {
        matches!(c, ChangeSet::Draft { draft: None, .. })
    })
    .await
    else {
        unreachable!()
    };
    assert!(draft.is_none());
}

/// A Professional license, laid over the unlicensed fixture.
async fn licensed(server: &wiremock::MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v4/license/client"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "IsLicensed": "true", "SkuShortName": "professional"
        })))
        .with_priority(1)
        .mount(server)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn scheduled_messages_are_not_listed_when_the_server_has_them_off() {
    // Licensed but off, and on but unlicensed: neither lists them.
    for (config, license) in [("false", true), ("true", false)] {
        let server = server_with_config(json!({"ScheduledPosts": config})).await;
        if license {
            licensed(&server).await;
        }
        let (_sync, _changes) = connected(&server).await;
        let paths = requested(&server).await;
        assert!(
            !paths.iter().any(|p| p.contains("scheduled")),
            "called {paths:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn scheduled_messages_load_on_connect_when_the_server_has_them() {
    let server = server_with_config(json!({"ScheduledPosts": "true"})).await;
    licensed(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v4/posts/scheduled/team/t1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "t1": [{"id": "s1", "channel_id": "c1", "message": "later",
                    "scheduled_at": 1_800_000_000_000_i64}],
            "directChannels": []
        })))
        .expect(1)
        .mount(&server)
        .await;
    let sync = matty_sync::spawn(matty_store::Store::open_in_memory().unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::Connect {
        client: matty_api::Client::new(server.uri())
            .unwrap()
            .with_token("token"),
    });
    let ChangeSet::Scheduled(posts) =
        next(&changes, |c| matches!(c, ChangeSet::Scheduled(_))).await
    else {
        unreachable!()
    };
    assert_eq!(posts[0].message, "later");
}
