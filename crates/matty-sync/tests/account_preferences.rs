//! Preferences mirrors the user's display and advanced settings from the
//! server and writes them back (#156, #157).
#![allow(clippy::unwrap_used)]

mod common;

use common::{ME, connected, next, server};
use matty_api::models::Preference;
use matty_sync::{ChangeSet, Command};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

fn triples(preferences: &[Preference]) -> Vec<(&str, &str, &str)> {
    let mut all: Vec<_> = preferences
        .iter()
        .map(|p| (p.category.as_str(), p.name.as_str(), p.value.as_str()))
        .collect();
    all.sort();
    all
}

#[tokio::test(flavor = "multi_thread")]
async fn the_users_display_and_advanced_settings_arrive_at_sign_in() {
    let server = server().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/me/preferences"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"user_id": ME, "category": "display_settings", "name": "message_display", "value": "compact"},
            {"user_id": ME, "category": "display_settings", "name": "name_format", "value": "full_name"},
            {"user_id": ME, "category": "advanced_settings", "name": "send_on_ctrl_enter", "value": "true"},
            {"user_id": ME, "category": "favorite_channel", "name": "c1", "value": "true"},
        ])))
        .with_priority(1)
        .mount(&server)
        .await;
    let sync = matty_sync::spawn(matty_store::Store::open_in_memory().unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::Connect {
        client: matty_api::Client::new(server.uri())
            .unwrap()
            .with_token("token"),
    });

    let ChangeSet::AccountPreferences(preferences) =
        next(&changes, |c| matches!(c, ChangeSet::AccountPreferences(_))).await
    else {
        unreachable!()
    };
    // Favorites and other categories are not Preferences' business.
    assert_eq!(
        triples(&preferences),
        vec![
            ("advanced_settings", "send_on_ctrl_enter", "true"),
            ("display_settings", "message_display", "compact"),
            ("display_settings", "name_format", "full_name"),
        ]
    );
}

async fn saved_preferences(server: &wiremock::MockServer) -> Vec<Value> {
    let requests = server.received_requests().await.unwrap_or_default();
    requests
        .iter()
        .filter(|r| {
            r.method.as_str() == "PUT" && r.url.path() == format!("/api/v4/users/{ME}/preferences")
        })
        .flat_map(|r| serde_json::from_slice::<Vec<Value>>(&r.body).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_setting_is_saved_as_the_users_and_echoed() {
    let server = server().await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v4/users/{ME}/preferences")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    let preference = |category: &str, name: &str, value: &str| Preference {
        category: category.into(),
        name: name.into(),
        value: value.into(),
        ..Preference::default()
    };

    sync.send(Command::SetPreferences {
        preferences: vec![
            preference("advanced_settings", "send_on_ctrl_enter", "false"),
            preference("advanced_settings", "code_block_ctrl_enter", "true"),
        ],
    });
    let ChangeSet::AccountPreferences(echoed) = next(
        &changes,
        |c| matches!(c, ChangeSet::AccountPreferences(p) if !p.is_empty()),
    )
    .await
    else {
        unreachable!()
    };
    assert_eq!(
        triples(&echoed),
        vec![
            ("advanced_settings", "code_block_ctrl_enter", "true"),
            ("advanced_settings", "send_on_ctrl_enter", "false"),
        ]
    );

    for _ in 0..50 {
        if !saved_preferences(&server).await.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let saved = saved_preferences(&server).await;
    assert_eq!(saved.len(), 2, "{saved:?}");
    for (sent, name, value) in [
        (&saved[0], "send_on_ctrl_enter", "false"),
        (&saved[1], "code_block_ctrl_enter", "true"),
    ] {
        assert_eq!(sent["user_id"], ME);
        assert_eq!(sent["category"], "advanced_settings");
        assert_eq!(sent["name"], name);
        assert_eq!(sent["value"], value);
    }
}
