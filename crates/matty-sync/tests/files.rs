//! Files: uploads ahead of the post that carries them, and thumbnails.
#![allow(clippy::unwrap_used)]

mod common;

use common::{connected, next, server};
use matty_sync::{ChangeSet, Command, ImageSize, UploadState};
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
        sync.send(Command::LoadImage {
            file_id: "f1".into(),
            size: ImageSize::Thumbnail,
        });
        let ChangeSet::Image {
            file_id,
            size,
            bytes,
            content_type,
        } = next(&changes, |c| matches!(c, ChangeSet::Image { .. })).await
        else {
            unreachable!()
        };
        assert_eq!(
            (file_id.as_str(), &*bytes, content_type.as_str()),
            ("f1", &b"jpeg"[..], "image/jpeg")
        );
        assert_eq!(size, ImageSize::Thumbnail);
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
    // Progress comes first, counting up to the whole file.
    assert_eq!(file_id, "f1");
    assert_eq!(
        state,
        DownloadState::Progress {
            received: 14,
            total: Some(14)
        }
    );
    let ChangeSet::Download { state, .. } = next(&changes, |c| {
        matches!(
            c,
            ChangeSet::Download {
                state: DownloadState::Done(_) | DownloadState::Failed(_),
                ..
            }
        )
    })
    .await
    else {
        unreachable!()
    };
    let saved = folder.path().join("notes (1).pdf");
    assert_eq!(state, DownloadState::Done(saved.clone()));
    assert_eq!(std::fs::read(saved).unwrap(), b"%PDF-1.7 notes");
    assert_eq!(
        std::fs::read(folder.path().join("notes.pdf")).unwrap(),
        b"older"
    );
    // Nothing half-written is left beside them.
    let mut names: Vec<_> = std::fs::read_dir(folder.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["notes (1).pdf", "notes.pdf"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_download_says_why_and_leaves_nothing_behind() {
    use matty_sync::DownloadState;

    let server = server().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/files/f2"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "id": "app.file_info.get.app_error",
            "message": "Unable to get the file.",
            "status_code": 404
        })))
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    let folder = tempfile::tempdir().unwrap();
    sync.send(Command::Download {
        file_id: "f2".into(),
        folder: folder.path().to_path_buf(),
        name: "gone.pdf".into(),
    });
    let ChangeSet::Download { file_id, state } =
        next(&changes, |c| matches!(c, ChangeSet::Download { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(file_id, "f2");
    assert!(
        matches!(&state, DownloadState::Failed(why) if why.contains("Unable to get the file")),
        "{state:?}"
    );
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_outside_image_is_fetched_directly_once_then_cached() {
    use matty_sync::Connection;

    let server = server().await;
    let outside = wiremock::MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/og.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(b"png".to_vec()),
        )
        .mount(&outside)
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

    let url = format!("{}/og.png", outside.uri());
    for _ in 0..2 {
        sync.send(Command::LoadRemoteImage { url: url.clone() });
        let ChangeSet::RemoteImage {
            url: answered,
            bytes,
            content_type,
        } = next(&changes, |c| matches!(c, ChangeSet::RemoteImage { .. })).await
        else {
            unreachable!()
        };
        assert_eq!(
            (answered.as_str(), &*bytes, content_type.as_str()),
            (url.as_str(), &b"png"[..], "image/png")
        );
    }
    // The server has no image proxy, so it never saw the request.
    assert_eq!(outside.received_requests().await.unwrap().len(), 1);
    assert!(
        !common::requested(&server)
            .await
            .iter()
            .any(|path| path.contains("/image"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_picture_is_fetched_once_per_version_and_kept() {
    use matty_sync::Connection;

    let server = server().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/u2/image"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(b"\x89PNGface".to_vec()),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/gone/image"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("media");
    let connect = |sync: &matty_sync::SyncHandle| {
        sync.send(Command::Connect {
            client: matty_api::Client::new(server.uri())
                .unwrap()
                .with_token("token"),
        })
    };
    let fetches = || async {
        common::requested(&server)
            .await
            .iter()
            .filter(|path| path.ends_with("/u2/image"))
            .count()
    };
    let avatar = |user_id: &str, version| Command::LoadAvatar {
        user_id: user_id.into(),
        last_picture_update: Some(version),
    };
    type Answer = (String, i64, Option<(std::sync::Arc<[u8]>, String)>);
    async fn answer(changes: &async_channel::Receiver<ChangeSet>) -> Answer {
        let ChangeSet::Avatar {
            user_id,
            last_picture_update,
            image,
        } = next(changes, |c| matches!(c, ChangeSet::Avatar { .. })).await
        else {
            unreachable!()
        };
        (user_id, last_picture_update, image)
    }

    {
        let sync = matty_sync::spawn_at(dir.path().join("db.sqlite"), Some(media.clone())).unwrap();
        let changes = sync.subscribe();
        connect(&sync);
        next(&changes, |c| {
            matches!(c, ChangeSet::Connection(Connection::Loaded))
        })
        .await;
        for _ in 0..2 {
            sync.send(avatar("u2", 10));
            let (user_id, version, image) = answer(&changes).await;
            assert_eq!((user_id.as_str(), version), ("u2", 10));
            let (bytes, content_type) = image.unwrap();
            assert_eq!(
                (&*bytes, content_type.as_str()),
                (&b"\x89PNGface"[..], "image/png")
            );
        }
        assert_eq!(fetches().await, 1, "the second ask came from the cache");
        // A new picture time is a new picture.
        sync.send(avatar("u2", 20));
        assert_eq!(answer(&changes).await.1, 20);
        assert_eq!(fetches().await, 2);
        // No picture: an empty answer, so the app keeps the initials.
        sync.send(avatar("gone", 1));
        let (user_id, _, image) = answer(&changes).await;
        assert_eq!((user_id.as_str(), image), ("gone", None));
    }

    // Offline, with no session at all, the kept picture still shows.
    let sync = matty_sync::spawn_at(dir.path().join("db.sqlite"), Some(media)).unwrap();
    let changes = sync.subscribe();
    sync.send(avatar("u2", 20));
    let (_, version, image) = answer(&changes).await;
    assert_eq!(version, 20);
    assert!(image.is_some());
    sync.send(avatar("u3", 1));
    assert_eq!(answer(&changes).await.2, None);
    assert_eq!(fetches().await, 2);
}
