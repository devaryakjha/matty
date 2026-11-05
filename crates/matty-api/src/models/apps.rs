//! The Mattermost Apps framework (#226): what installed Apps add to the
//! post menu, the channel header and slash commands (their bindings), and
//! the calls those make. Served by the `com.mattermost.apps` plugin under
//! `plugins/com.mattermost.apps/api/v1`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::null_as_default;

/// Where a binding shows: the top-level locations the server answers with.
pub mod location {
    /// A post's menu.
    pub const POST_MENU: &str = "/post_menu";
    /// The channel header.
    pub const CHANNEL_HEADER: &str = "/channel_header";
    /// Slash commands.
    pub const COMMAND: &str = "/command";
}

/// One binding: a menu item, a header button, or a command, possibly with
/// sub-bindings (a submenu, or subcommands).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct AppBinding {
    /// The App it belongs to.
    pub app_id: String,
    /// Its location: a top-level one such as `/post_menu`, or a name
    /// unique among its siblings.
    pub location: String,
    /// An icon URL.
    pub icon: String,
    /// The label shown, or the command's name.
    pub label: String,
    /// A command's argument hint.
    pub hint: String,
    /// A longer description, for a tooltip or autocomplete.
    pub description: String,
    /// Sub-bindings.
    #[serde(deserialize_with = "null_as_default")]
    pub bindings: Vec<AppBinding>,
    /// A form to show before calling.
    pub form: Option<AppForm>,
    /// The call to make when chosen.
    pub submit: Option<AppCall>,
}

impl AppBinding {
    /// The name this binding adds to its parent's location.
    pub fn name(&self) -> &str {
        if self.location.is_empty() {
            &self.label
        } else {
            &self.location
        }
    }
}

/// A call to an App: the path on the App and what the server should add.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct AppCall {
    /// The App's path, such as `/send`.
    pub path: String,
    /// What the server adds to the context, by name (`all`, `summary`,
    /// `id` ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expand: Option<Value>,
    /// The App's own data, sent back unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
}

/// A form an App shows before a call, or answers a call with.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct AppForm {
    /// The heading.
    pub title: String,
    /// Markdown above the fields.
    pub header: String,
    /// Markdown below the fields.
    pub footer: String,
    /// An icon URL.
    pub icon: String,
    /// The fields, in order.
    #[serde(deserialize_with = "null_as_default")]
    pub fields: Vec<AppField>,
    /// The call the values go to.
    pub submit: Option<AppCall>,
    /// A call that answers with the form itself, when the form is not
    /// given whole.
    pub source: Option<AppCall>,
    /// Whether the App hears when the user cancels.
    pub submit_on_cancel: bool,
}

/// One field of an [`AppForm`].
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct AppField {
    /// The key its value goes under.
    pub name: String,
    /// `text`, `static_select`, `dynamic_select`, `bool`, `user`,
    /// `channel`, `markdown` or `radio`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Whether it must be filled.
    pub is_required: bool,
    /// Whether the user may change it.
    pub readonly: bool,
    /// Its starting value: text, a bool, or an option `{label, value}`.
    pub value: Value,
    /// Shown under the field; for `markdown`, the text itself.
    pub description: String,
    /// The label.
    pub label: String,
    /// The placeholder.
    pub hint: String,
    /// The label in a modal, when it differs from [`label`](Self::label).
    pub modal_label: String,
    /// For selects and radios.
    #[serde(deserialize_with = "null_as_default")]
    pub options: Vec<AppSelectOption>,
    /// Whether several options may be chosen.
    pub multiselect: bool,
    /// For `text`: `input`, `textarea`, `email`, `number`, `password`,
    /// `tel` or `url`.
    pub subtype: String,
    /// Text length limits; 0 for none.
    pub min_length: i64,
    /// See [`min_length`](Self::min_length).
    pub max_length: i64,
    /// For `dynamic_select`: the call that finds its options as the user
    /// types. Without it, the form's submit call answers at its path
    /// plus `/lookup`.
    pub lookup: Option<AppCall>,
}

/// An option of a select or radio field, and how a chosen option is sent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AppSelectOption {
    /// What the user sees.
    pub label: String,
    /// What the App gets.
    pub value: String,
}

/// Where a call comes from, for the App.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct AppContext {
    /// The App.
    pub app_id: String,
    /// The binding's full location, such as `/post_menu/send`.
    pub location: String,
    /// The client, for the App to tailor its answer.
    pub user_agent: String,
    /// The team the user is in.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub team_id: String,
    /// The channel.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub channel_id: String,
    /// The post, for a post menu binding.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub post_id: String,
    /// The post's thread root, for a reply.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub root_id: String,
}

/// `POST plugins/com.mattermost.apps/api/v1/call`'s body.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct AppCallRequest {
    /// The App's path.
    pub path: String,
    /// What the server adds to the context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expand: Option<Value>,
    /// The App's own data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
    /// Where it comes from.
    pub context: AppContext,
    /// A form's values by field name.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub values: HashMap<String, Value>,
    /// A command's text as typed.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub raw_command: String,
    /// For a lookup: the field whose options are wanted.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub selected_field: String,
    /// For a lookup: what the user typed in it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub query: String,
}

impl AppCallRequest {
    /// A request for `call` from `context`.
    pub fn new(call: &AppCall, context: AppContext) -> Self {
        Self {
            path: call.path.clone(),
            expand: call.expand.clone(),
            state: call.state.clone(),
            context,
            ..Self::default()
        }
    }
}

/// What kind of answer an App gave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AppResponseKind {
    /// Done; [`AppCallResponse::text`] may say something.
    #[default]
    Ok,
    /// Failed; [`AppCallResponse::text`] says why, and `data.errors` may
    /// name fields.
    Error,
    /// Show [`AppCallResponse::form`].
    Form,
    /// Make [`AppCallResponse::call`] next.
    Call,
    /// Open [`AppCallResponse::navigate_to_url`].
    Navigate,
}

/// The App's bot, which says the App's messages.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AppMetadata {
    /// The bot's user id.
    pub bot_user_id: String,
    /// The bot's username.
    pub bot_username: String,
}

/// What a call answers.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct AppCallResponse {
    /// What kind of answer.
    #[serde(rename = "type")]
    pub kind: AppResponseKind,
    /// Markdown for the user: a result, or what went wrong.
    pub text: String,
    /// For `error`: `{"errors": {field: problem}}`.
    pub data: Value,
    /// For `form`.
    pub form: Option<AppForm>,
    /// For `call`.
    pub call: Option<AppCall>,
    /// For `navigate`.
    pub navigate_to_url: String,
    /// For `navigate`: open outside Mattermost even for a Mattermost link.
    pub use_external_browser: bool,
    /// Who the App posts as.
    pub app_metadata: Option<AppMetadata>,
}

impl AppCallResponse {
    /// For a lookup's `ok`: the options in `data.items`.
    pub fn lookup_items(&self) -> Vec<AppSelectOption> {
        self.data
            .get("items")
            .cloned()
            .and_then(|items| serde_json::from_value(items).ok())
            .unwrap_or_default()
    }

    /// For an `error`: problems by field name.
    pub fn field_errors(&self) -> HashMap<String, String> {
        self.data
            .get("errors")
            .and_then(Value::as_object)
            .map(|errors| {
                errors
                    .iter()
                    .filter_map(|(field, text)| Some((field.clone(), text.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default()
    }
}
