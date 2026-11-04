//! Interactive dialogs (#211): the form an integration opens with the
//! `trigger_id` a button press or slash command gave it, and what the user
//! sends back.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{PostActionOption, null_as_default};

/// What the `open_dialog` event carries: the dialog and where it goes.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct OpenDialogRequest {
    /// The trigger the integration answered with.
    pub trigger_id: String,
    /// The integration's URL; the server posts the submission there.
    pub url: String,
    /// The form.
    pub dialog: Dialog,
}

/// An interactive dialog's form.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Dialog {
    /// Sent back with the submission, for the integration to tell its
    /// dialogs apart.
    pub callback_id: String,
    /// The heading.
    pub title: String,
    /// Markdown shown above the fields.
    pub introduction_text: String,
    /// A picture beside the title.
    pub icon_url: String,
    /// The fields, in order.
    #[serde(deserialize_with = "null_as_default")]
    pub elements: Vec<DialogElement>,
    /// The submit button's label; `Submit` when empty.
    pub submit_label: String,
    /// Whether the integration hears when the user cancels.
    pub notify_on_cancel: bool,
    /// The integration's own data, sent back unchanged.
    pub state: String,
}

/// One field of a [`Dialog`].
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct DialogElement {
    /// The label.
    pub display_name: String,
    /// The key its value is submitted under.
    pub name: String,
    /// `text`, `textarea`, `select`, `bool` or `radio`.
    #[serde(rename = "type")]
    pub kind: String,
    /// For `text` and `textarea`: `email`, `number`, `password`, `tel`,
    /// `url` or `text`.
    pub subtype: String,
    /// The value it starts with; `true` or `false` for `bool`.
    #[serde(deserialize_with = "text_or_default")]
    pub default: String,
    /// Shown while empty; for `bool`, the checkbox's own label.
    pub placeholder: String,
    /// Shown under the field.
    pub help_text: String,
    /// Whether it may be left empty.
    pub optional: bool,
    /// Text length limits; 0 for none.
    pub min_length: i64,
    /// See [`min_length`](Self::min_length).
    pub max_length: i64,
    /// For `select`: empty for [`options`](Self::options), `users` or
    /// `channels` for the server's own lists.
    pub data_source: String,
    /// For `select` and `radio`.
    #[serde(deserialize_with = "null_as_default")]
    pub options: Vec<PostActionOption>,
}

/// A `default` the integration may send as a string, a number or a bool.
fn text_or_default<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text,
        Some(other) => other.to_string(),
    })
}

/// `POST actions/dialogs/submit`'s body.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct SubmitDialogRequest {
    /// Always `dialog_submission`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The dialog's [`OpenDialogRequest::url`].
    pub url: String,
    /// The dialog's [`Dialog::callback_id`].
    pub callback_id: String,
    /// The dialog's [`Dialog::state`].
    pub state: String,
    /// Who submits.
    pub user_id: String,
    /// Where the user was.
    pub channel_id: String,
    /// The team of that channel.
    pub team_id: String,
    /// The values by field name: text, or a bool for a `bool` field, or a
    /// number for a `number` field.
    pub submission: HashMap<String, Value>,
    /// Set, with no submission, when the user cancelled.
    pub cancelled: bool,
}

/// What `actions/dialogs/submit` answers: problems the integration found.
/// Both empty means it took the submission.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct SubmitDialogResponse {
    /// A problem with the whole form.
    pub error: String,
    /// Problems by field name.
    #[serde(deserialize_with = "null_as_default")]
    pub errors: HashMap<String, String>,
}

impl SubmitDialogResponse {
    /// Whether the integration took the submission.
    pub fn accepted(&self) -> bool {
        self.error.is_empty() && self.errors.is_empty()
    }
}
