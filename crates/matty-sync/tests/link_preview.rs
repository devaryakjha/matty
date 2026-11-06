//! The link preview while composing asks the server once per link (#188).
#![allow(clippy::unwrap_used)]

mod common;

use common::{connected, next, server};
use matty_sync::{ChangeSet, Command};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn a_link_is_asked_about_once() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/opengraph"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "type": "website",
            "url": "https://github.com/karyalabs/matty",
            "title": "karyalabs/matty",
            "description": "A native Mattermost client",
            "site_name": "GitHub",
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    let url = "https://github.com/karyalabs/matty".to_owned();

    for _ in 0..3 {
        sync.send(Command::LoadLinkPreview { url: url.clone() });
        let ChangeSet::LinkPreview {
            url: answered,
            graph,
        } = next(&changes, |c| matches!(c, ChangeSet::LinkPreview { .. })).await
        else {
            unreachable!()
        };
        assert_eq!(answered, url);
        let graph = graph.unwrap();
        assert_eq!(graph.title, "karyalabs/matty");
        assert_eq!(graph.site_name, "GitHub");
    }
    // The mock's expect(1) checks there was one request when it drops.
}

#[tokio::test(flavor = "multi_thread")]
async fn previews_follow_the_servers_setting() {
    for (setting, shown) in [("true", true), ("false", false)] {
        let server = common::server_with_config(json!({"EnableLinkPreviews": setting})).await;
        let sync = matty_sync::spawn(matty_store::Store::open_in_memory().unwrap()).unwrap();
        let changes = sync.subscribe();
        sync.send(Command::Connect {
            client: matty_api::Client::new(server.uri())
                .unwrap()
                .with_token("token"),
        });
        next(
            &changes,
            |c| matches!(c, ChangeSet::LinkPreviews(on) if *on == shown),
        )
        .await;
    }
}
