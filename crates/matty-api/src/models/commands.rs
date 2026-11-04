//! Slash commands (#210): the commands a team offers, the server's
//! suggestions for what the user is typing, and what running one answers.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::null_as_default;

/// A slash command: one of the server's own, a plugin's, or an
/// integration's, as `teams/{team_id}/commands/autocomplete` and
/// `commands?team_id=` list them.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct SlashCommand {
    /// Empty for the server's own commands.
    pub id: String,
    /// Empty for the server's own commands.
    pub team_id: String,
    /// What follows the `/`, such as `shrug`.
    pub trigger: String,
    /// Whether autocomplete offers it.
    pub auto_complete: bool,
    /// What it does, for the autocomplete row.
    pub auto_complete_desc: String,
    /// Its arguments, such as `[message]`.
    pub auto_complete_hint: String,
    /// The integration's name for it.
    pub display_name: String,
    /// The integration's description.
    pub description: String,
    /// Set when a plugin registered it.
    pub plugin_id: String,
    /// Set when it was deleted.
    pub delete_at: i64,
}

/// One row of `teams/{team_id}/commands/autocomplete_suggestions`: a
/// command, a subcommand or an argument value. The server spells these
/// fields with capitals.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct CommandSuggestion {
    /// The whole command so far with this suggestion, without the `/`,
    /// such as `jira create`.
    #[serde(rename = "Complete")]
    pub complete: String,
    /// The part this suggestion adds, such as `create`.
    #[serde(rename = "Suggestion")]
    pub suggestion: String,
    /// What comes after it, such as `[issue type]`.
    #[serde(rename = "Hint")]
    pub hint: String,
    /// What it does.
    #[serde(rename = "Description")]
    pub description: String,
    /// An icon as a URL or a data URL; Matty does not draw it.
    #[serde(rename = "IconData")]
    pub icon_data: String,
}

/// The body of `commands/execute`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ExecuteCommand {
    /// The whole line, starting with `/`, such as `/me waves`.
    pub command: String,
    /// The channel it runs in.
    pub channel_id: String,
    /// The channel's team; empty for a direct or group message.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub team_id: String,
    /// The thread when typed in a reply; empty in the channel.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub root_id: String,
}

/// What `commands/execute` answers.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct CommandResponse {
    /// `ephemeral` (only the user sees the text; the server also sends it
    /// as an `ephemeral_message` event) or `in_channel` (the server posted
    /// it). Empty means `ephemeral`.
    pub response_type: String,
    /// The answer's text.
    pub text: String,
    /// The channel the answer went to, when not the one typed in.
    pub channel_id: String,
    /// A place to go: a URL, or a path on this server such as
    /// `/team/channels/town-square`.
    pub goto_location: String,
    /// Set when the integration opens an interactive dialog; the server
    /// then sends `open_dialog`.
    pub trigger_id: String,
    /// Text only the user sees that the server does not send as an event,
    /// as Apps framework commands answer.
    pub ephemeral_text: String,
    /// Further answers; Matty reads none of them.
    #[serde(deserialize_with = "null_as_default")]
    pub extra_responses: Vec<Value>,
}

impl CommandResponse {
    /// Whether only the user sees the answer.
    pub fn is_ephemeral(&self) -> bool {
        self.response_type.is_empty() || self.response_type == "ephemeral"
    }
}
