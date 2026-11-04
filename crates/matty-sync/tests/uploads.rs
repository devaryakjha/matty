//! Files go up ahead of the post that carries them (PLAN §4.6).
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
