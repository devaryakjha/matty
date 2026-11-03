//! An open channel's header and counts reach the app.
#![allow(clippy::unwrap_used)]

mod common;

use common::{next, server};
use matty_api::Client;
use matty_store::Store;
use matty_sync::{ChangeSet, Command};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn an_open_channel_sends_itself_and_its_counts() {
    let server = server().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/stats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "channel_id": "c1", "member_count": 12, "guest_count": 0,
            "pinnedpost_count": 3, "files_count": 0
        })))
        .expect(1)
        .mount(&server)
        .await;

    let sync = matty_sync::spawn(Store::open_in_memory().unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::OpenChannel {
        channel_id: "c1".into(),
    });
    sync.send(Command::Connect {
        client: Client::new(server.uri()).unwrap().with_token("token"),
    });
    let ChangeSet::Channel(channel) = next(&changes, |c| matches!(c, ChangeSet::Channel(_))).await
    else {
        unreachable!()
    };
    assert_eq!(channel.display_name, "C1");
    let ChangeSet::Stats(stats) = next(&changes, |c| matches!(c, ChangeSet::Stats(_))).await else {
        unreachable!()
    };
    assert_eq!((stats.member_count, stats.pinnedpost_count), (12, 3));
}
