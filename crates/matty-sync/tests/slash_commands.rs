//! Slash commands (#210): suggestions while typing one, and running one
//! with the server's answer or refusal.
#![allow(clippy::unwrap_used)]

mod common;

use common::{connected, next, server};
use matty_sync::{ChangeSet, Command};
use serde_json::json;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn suggestions_come_back_for_the_line_typed() {
    let server = server().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/teams/t1/commands/autocomplete_suggestions"))
        .and(query_param("user_input", "/jira "))
        .and(query_param("channel_id", "c1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"Complete": "jira create", "Suggestion": "create", "Hint": "[text]",
             "Description": "Create a new Issue", "IconData": ""}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    // A server without suggestions lists its commands instead.
    Mock::given(method("GET"))
        .and(path("/api/v4/teams/t2/commands/autocomplete_suggestions"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "id": "api.context.404.app_error", "message": "Sorry, we could not find the page.",
            "status_code": 404
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/teams/t2/commands/autocomplete"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"trigger": "shrug", "auto_complete": true, "auto_complete_desc": "Shrug"},
            {"trigger": "deploy", "auto_complete": true, "auto_complete_hint": "[branch]"}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    sync.send(Command::SuggestCommands {
        team_id: "t1".into(),
        channel_id: "c1".into(),
        root_id: String::new(),
        user_input: "/jira ".into(),
    });
    let ChangeSet::CommandSuggestions { suggestions, .. } = next(&changes, |c| {
        matches!(c, ChangeSet::CommandSuggestions { user_input, channel_id, .. }
            if user_input == "/jira " && channel_id == "c1")
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].complete, "jira create");
    sync.send(Command::SuggestCommands {
        team_id: "t2".into(),
        channel_id: "c2".into(),
        root_id: "r2".into(),
        user_input: "/de".into(),
    });
    let ChangeSet::CommandSuggestions {
        suggestions,
        root_id,
        ..
    } = next(
        &changes,
        |c| matches!(c, ChangeSet::CommandSuggestions { user_input, .. } if user_input == "/de"),
    )
    .await
    else {
        unreachable!()
    };
    assert_eq!(root_id, "r2");
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].complete, "deploy");
    assert_eq!(suggestions[0].hint, "[branch]");
    server.verify().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_command_runs_and_answers_or_is_refused() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/commands/execute"))
        .and(body_json(json!({
            "command": "/me waves",
            "channel_id": "c1",
            "team_id": "t1",
            "root_id": "r1"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "response_type": "in_channel", "text": "*waves*"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/commands/execute"))
        .and(body_json(
            json!({"command": "/go", "channel_id": "c1", "team_id": "t1"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "goto_location": "/matty/channels/town-square"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/commands/execute"))
        .and(body_json(
            json!({"command": "/nope", "channel_id": "c1", "team_id": "t1"}),
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "id": "api.command.execute_command.not_found.app_error",
            "message": "Command with a trigger of 'nope' not found.",
            "status_code": 404
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    let run = |command: &str, root_id: &str| Command::ExecuteCommand {
        team_id: "t1".into(),
        channel_id: "c1".into(),
        root_id: root_id.into(),
        command: command.into(),
    };
    sync.send(run("/me waves", "r1"));
    let ChangeSet::CommandRan {
        outcome, root_id, ..
    } = next(
        &changes,
        |c| matches!(c, ChangeSet::CommandRan { command, .. } if command == "/me waves"),
    )
    .await
    else {
        unreachable!()
    };
    assert_eq!(root_id, "r1");
    let answer = outcome.unwrap();
    assert!(!answer.is_ephemeral());
    sync.send(run("/go", ""));
    let ChangeSet::CommandRan { outcome, .. } = next(
        &changes,
        |c| matches!(c, ChangeSet::CommandRan { command, .. } if command == "/go"),
    )
    .await
    else {
        unreachable!()
    };
    let goto = outcome.unwrap().goto_location;
    assert_eq!(
        goto,
        format!("{}/matty/channels/town-square", server.uri()),
        "a path is on the server"
    );
    sync.send(run("/nope", ""));
    let ChangeSet::CommandRan { outcome, .. } = next(
        &changes,
        |c| matches!(c, ChangeSet::CommandRan { command, .. } if command == "/nope"),
    )
    .await
    else {
        unreachable!()
    };
    assert_eq!(
        outcome.unwrap_err(),
        "Command with a trigger of 'nope' not found."
    );
    server.verify().await;
}
