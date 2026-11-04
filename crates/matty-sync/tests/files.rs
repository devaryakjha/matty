//! Files: uploads ahead of the post that carries them, and thumbnails.
#![allow(clippy::unwrap_used)]

mod common;

use common::{connected, next, server};
use matty_sync::{ChangeSet, Command, UploadState};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn an_upload_reports_progress_then_its_file_id() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/files"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "file_infos": [{"id": "f1", "name": "notes.txt", "size": 200000}],
            "client_ids": ["u1"]
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/files"))
        .respond_with(ResponseTemplate::new(413))
        .mount(&server)
        .await;

    let (sync, changes) = connected(&server).await;

    sync.send(Command::Upload {
        upload_id: "u1".into(),
        channel_id: "c1".into(),
        file_name: "notes.txt".into(),
        bytes: vec![b'x'; 200_000],
    });
    let ChangeSet::Upload { state, .. } = next(&changes, |c| {
        matches!(
            c,
            ChangeSet::Upload {
                state: UploadState::Sending(_),
                ..
            }
        )
    })
    .await
    else {
        unreachable!()
    };
    let UploadState::Sending(fraction) = state else {
        unreachable!()
    };
    assert!((0.0..1.0).contains(&fraction));
    let ChangeSet::Upload { upload_id, state } = next(&changes, |c| {
        matches!(
            c,
            ChangeSet::Upload {
                state: UploadState::Done { .. },
                ..
            }
        )
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(upload_id, "u1");
    assert_eq!(
        state,
        UploadState::Done {
            file_id: "f1".into()
        }
    );

    // The server refuses the next one.
    sync.send(Command::Upload {
        upload_id: "u2".into(),
        channel_id: "c1".into(),
        file_name: "huge.bin".into(),
        bytes: vec![0; 10],
    });
    next(&changes, |c| {
        matches!(c, ChangeSet::Upload { upload_id, state: UploadState::Failed(_) } if upload_id == "u2")
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_thumbnail_comes_from_the_server_once_then_the_cache() {
    use matty_sync::Connection;

    let server = server().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/files/f1/thumbnail"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/jpeg")
                .set_body_bytes(b"jpeg".to_vec()),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let sync =
        matty_sync::spawn_at(dir.path().join("db.sqlite"), Some(dir.path().join("media"))).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::Connect {
        client: matty_api::Client::new(server.uri())
            .unwrap()
            .with_token("token"),
    });
    next(&changes, |c| {
        matches!(c, ChangeSet::Connection(Connection::Loaded))
    })
    .await;

    for _ in 0..2 {
        sync.send(Command::LoadThumbnail {
            file_id: "f1".into(),
        });
        let ChangeSet::Thumbnail {
            file_id,
            bytes,
            content_type,
        } = next(&changes, |c| matches!(c, ChangeSet::Thumbnail { .. })).await
        else {
            unreachable!()
        };
        assert_eq!(
            (file_id.as_str(), &*bytes, content_type.as_str()),
            ("f1", &b"jpeg"[..], "image/jpeg")
        );
    }
    let fetched = common::requested(&server)
        .await
        .into_iter()
        .filter(|path| path.ends_with("/thumbnail"))
        .count();
    assert_eq!(fetched, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_lands_in_the_folder_with_the_servers_bytes() {
    use matty_sync::DownloadState;

    let server = server().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/files/f1"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-1.7 notes".to_vec()))
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("notes.pdf"), b"older").unwrap();

    sync.send(Command::Download {
        file_id: "f1".into(),
        folder: folder.path().to_path_buf(),
        name: "notes.pdf".into(),
    });
    let ChangeSet::Download { file_id, state } =
        next(&changes, |c| matches!(c, ChangeSet::Download { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(file_id, "f1");
    let saved = folder.path().join("notes (1).pdf");
    assert_eq!(state, DownloadState::Done(saved.clone()));
    assert_eq!(std::fs::read(saved).unwrap(), b"%PDF-1.7 notes");
    assert_eq!(
        std::fs::read(folder.path().join("notes.pdf")).unwrap(),
        b"older"
    );
}
