//! Slash commands (#210): listing, suggestions and running one send what
//! the server expects and read its answers, from fixtures written in
//! Mattermost's shapes (see `tests/fixtures/README.md`).

#![allow(clippy::unwrap_used)]

use matty_api::{
    ApiError, Client,
    models::{CommandResponse, CommandSuggestion, ExecuteCommand, SlashCommand},
};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path, query_param, query_param_is_missing},
};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn command_fixtures_decode() {
    let commands: Vec<SlashCommand> =
        serde_json::from_value(fixture("commands_autocomplete")).unwrap();
    let triggers: Vec<&str> = commands.iter().map(|c| c.trigger.as_str()).collect();
    assert_eq!(triggers, ["shrug", "me", "jira", "deploy"]);
    assert_eq!(commands[0].auto_complete_hint, "[message]");
    assert!(commands.iter().all(|c| c.auto_complete));
    assert_eq!(commands[2].plugin_id, "jira");
    let suggestions: Vec<CommandSuggestion> =
        serde_json::from_value(fixture("command_suggestions")).unwrap();
    assert_eq!(suggestions[1].complete, "jira create");
    assert_eq!(suggestions[1].suggestion, "create");
    assert_eq!(suggestions[1].hint, "[text]");
    assert_eq!(suggestions[1].description, "Create a new Issue");
    let away: CommandResponse =
        serde_json::from_value(fixture("command_response_ephemeral")).unwrap();
    assert!(away.is_ephemeral());
    assert_eq!(away.text, "You are now away");
    let goto: CommandResponse = serde_json::from_value(fixture("command_response_goto")).unwrap();
    assert!(goto.is_ephemeral(), "an empty response type is ephemeral");
    assert_eq!(goto.goto_location, "/matty/channels/town-square");
}

#[tokio::test]
async fn commands_list_for_a_team() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/teams/t1/commands/autocomplete"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("commands_autocomplete")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/commands"))
        .and(query_param("team_id", "t1"))
        .and(query_param("custom_only", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_string("null"))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    let commands = client.autocomplete_commands("t1").await.unwrap();
    assert_eq!(commands.len(), 4);
    assert!(
        client.list_commands("t1", false).await.unwrap().is_empty(),
        "null is no commands"
    );
}

#[tokio::test]
async fn suggestions_send_the_line_and_where_it_is_typed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/teams/t1/commands/autocomplete_suggestions"))
        .and(query_param("user_input", "/jira "))
        .and(query_param("channel_id", "c1"))
        .and(query_param("root_id", "r1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("command_suggestions")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/teams/t1/commands/autocomplete_suggestions"))
        .and(query_param("user_input", "/zz"))
        .and(query_param_is_missing("root_id"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    let found = client
        .command_suggestions("t1", "/jira ", "c1", "r1")
        .await
        .unwrap();
    assert_eq!(found.len(), 3);
    assert_eq!(found[0].complete, "jira connect");
    assert!(
        client
            .command_suggestions("t1", "/zz", "c1", "")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn running_a_command_sends_the_line_and_reads_the_answer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/commands/execute"))
        .and(body_json(json!({
            "command": "/away",
            "channel_id": "c1",
            "team_id": "t1",
            "root_id": "r1"
        })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(fixture("command_response_ephemeral")),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/commands/execute"))
        .and(body_json(json!({"command": "/nope", "channel_id": "d1"})))
        .respond_with(ResponseTemplate::new(404).set_body_json(fixture("command_not_found")))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    let answer = client
        .execute_command(&ExecuteCommand {
            command: "/away".into(),
            channel_id: "c1".into(),
            team_id: "t1".into(),
            root_id: "r1".into(),
        })
        .await
        .unwrap();
    assert_eq!(answer.text, "You are now away");
    // A direct message has no team, and the channel is no thread.
    let error = client
        .execute_command(&ExecuteCommand {
            command: "/nope".into(),
            channel_id: "d1".into(),
            ..ExecuteCommand::default()
        })
        .await
        .unwrap_err();
    assert!(
        matches!(&error, ApiError::Server { status: 404, message, .. }
            if message == "Command with a trigger of 'nope' not found."),
        "{error}"
    );
}
