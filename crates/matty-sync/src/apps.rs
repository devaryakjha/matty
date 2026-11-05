//! The Mattermost Apps framework (#226): installed Apps' bindings for a
//! channel, and the calls they make.
//!
//! Bindings are fetched whenever a channel opens and published as
//! [`ChangeSet::AppBindings`]. Choosing one runs [`AppCommand::Run`]: its
//! call goes to the Apps plugin with the context (App, location, team,
//! channel, post), and the answer comes back as [`ChangeSet::AppAnswered`]
//! for the window that asked. A form, given whole or answered, is shown as
//! an interactive dialog ([`ChangeSet::OpenDialog`]) whose URL starts with
//! [`DIALOG_URL`]; its submission comes back here instead of going to
//! `actions/dialogs/submit`, and the App's field errors show on the form.
//! An App's text after a call shows as an ephemeral message, as the web
//! app shows it. Servers without the Apps plugin answer 404, which is
//! remembered for a while so channels open without asking again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use matty_api::models::{
    AppBinding, AppCall, AppCallRequest, AppCallResponse, AppContext, AppField, AppForm,
    AppResponseKind, AppSelectOption, Dialog, DialogElement, OpenDialogRequest, Post,
    PostActionOption, SubmitDialogRequest, SubmitDialogResponse,
};
use matty_api::{ApiError, Client};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;
use web_time::{Duration, Instant};

use crate::change::ChangeSet;
use crate::load::Loaded;

/// How a dialog that shows an App's form is told apart: its URL starts
/// with this, followed by the App's id.
pub const DIALOG_URL: &str = "matty-apps:";

/// The `data_source` of a dialog select whose options an App looks up.
pub const DYNAMIC: &str = "app_lookup";

/// How long a server's missing Apps plugin is remembered.
const MISSING_FOR: Duration = Duration::from_secs(10 * 60);

/// A call that answers with another call is followed this many times.
const MAX_HOPS: usize = 3;

/// Servers whose Apps plugin answered 404, and when.
static MISSING: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

/// What the window asks of the Apps framework.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum AppCommand {
    /// Fetches the bindings for a channel;
    /// [`ChangeSet::AppBindings`] answers unless the server has no Apps.
    Bindings {
        /// The channel.
        channel_id: String,
        /// The team the user is in.
        team_id: String,
    },
    /// The user chose a binding: shows its form, or makes its call.
    /// [`ChangeSet::AppAnswered`] answers with `call_id`.
    Run {
        /// The caller's id for the call, echoed in the answer.
        call_id: String,
        /// The binding chosen.
        binding: Box<AppBinding>,
        /// Where it was chosen, with its full location.
        context: Box<AppContext>,
    },
    /// A dynamic select on an App's form wants options for what the user
    /// typed; [`ChangeSet::AppLookup`] answers.
    Lookup {
        /// The form's dialog callback id, echoed in the answer.
        callback_id: String,
        /// The form's dialog state, which carries its call and context.
        state: String,
        /// The field.
        field: String,
        /// What the user typed.
        query: String,
        /// The form's other values so far, by field name.
        values: HashMap<String, Value>,
    },
}

/// How an App's call ended, for the window that made it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppAnswer {
    /// Done. Any text shows as an ephemeral message.
    Done,
    /// The App or the server refused; what to tell the user.
    Failed(String),
    /// Open this link, through Matty's link routing.
    Navigate {
        /// The link.
        url: String,
        /// The App asked for the browser even for a Mattermost link.
        external: bool,
    },
    /// A form is showing.
    Form,
}

/// Whether `url` is a dialog showing an App's form.
pub fn is_app_dialog(url: &str) -> bool {
    url.starts_with(DIALOG_URL)
}

fn server_key(client: &Client) -> String {
    client.base_url().to_string()
}

fn is_missing(client: &Client) -> bool {
    let key = server_key(client);
    let mut missing = MISSING.lock().unwrap_or_else(|p| p.into_inner());
    missing.retain(|(_, at)| at.elapsed() < MISSING_FOR);
    missing.iter().any(|(server, _)| *server == key)
}

fn note_missing(client: &Client) {
    let mut missing = MISSING.lock().unwrap_or_else(|p| p.into_inner());
    missing.push((server_key(client), Instant::now()));
}

/// Runs a command on its own task.
pub(crate) fn command(
    client: Client,
    command: AppCommand,
    results: UnboundedSender<Loaded>,
    dirty: &mut crate::engine::Dirty,
) {
    match command {
        AppCommand::Bindings {
            channel_id,
            team_id,
        } => {
            if is_missing(&client) {
                return;
            }
            matty_api::rt::spawn(bindings(client, channel_id, team_id, results));
        }
        AppCommand::Run {
            call_id,
            binding,
            context,
        } => match plan(&binding, &context, &call_id) {
            Plan::Show(request) => dirty.direct.extend([
                ChangeSet::OpenDialog(Arc::new(request)),
                ChangeSet::AppAnswered {
                    call_id,
                    answer: AppAnswer::Form,
                },
            ]),
            Plan::Call(call) => {
                let request = AppCallRequest::new(&call, *context);
                matty_api::rt::spawn(async move {
                    let changes = call_app(&client, request, &call_id).await;
                    let _ = results.send(Loaded::Changes(changes));
                });
            }
            Plan::Nothing => dirty.direct.push(ChangeSet::AppAnswered {
                call_id,
                answer: AppAnswer::Failed("This App action has nothing to do.".into()),
            }),
        },
        AppCommand::Lookup {
            callback_id,
            state,
            field,
            query,
            values,
        } => {
            let request = lookup_call(&state, &field, &query, &values);
            matty_api::rt::spawn(async move {
                let options = match request {
                    Some(request) => match client.app_call(&request).await {
                        Ok(response) if response.kind == AppResponseKind::Ok => {
                            response.lookup_items()
                        }
                        Ok(response) => {
                            tracing::warn!(text = %response.text, "an App lookup was refused");
                            Vec::new()
                        }
                        Err(error) => {
                            tracing::warn!(%error, "an App lookup failed");
                            Vec::new()
                        }
                    },
                    None => Vec::new(),
                };
                let _ = results.send(Loaded::Changes(vec![ChangeSet::AppLookup {
                    callback_id,
                    field,
                    query,
                    options,
                }]));
            });
        }
    }
}

/// The lookup call for `field` of the form whose dialog state is `state`:
/// the field's own lookup call, or the form's submit call at its path plus
/// `/lookup`, with the form's values, the field and the query. `None` when
/// the state is not an App form's or has no such field.
fn lookup_call(
    state: &str,
    field: &str,
    query: &str,
    values: &HashMap<String, Value>,
) -> Option<AppCallRequest> {
    let state: Value = serde_json::from_str(state).ok()?;
    let context: AppContext = serde_json::from_value(state.get("context")?.clone()).ok()?;
    let spec = state
        .get("fields")?
        .as_array()?
        .iter()
        .find(|spec| spec.get("name").and_then(Value::as_str) == Some(field))?;
    let call = match spec.get("lookup").filter(|lookup| !lookup.is_null()) {
        Some(lookup) => serde_json::from_value::<AppCall>(lookup.clone()).ok()?,
        None => {
            let mut call: AppCall = serde_json::from_value(state.get("call")?.clone()).ok()?;
            call.path = format!("{}/lookup", call.path.trim_end_matches('/'));
            call
        }
    };
    let mut request = AppCallRequest::new(&call, context);
    request.values = values
        .iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    request.selected_field = field.to_owned();
    request.query = query.to_owned();
    Some(request)
}

async fn bindings(
    client: Client,
    channel_id: String,
    team_id: String,
    results: UnboundedSender<Loaded>,
) {
    match client.app_bindings(&channel_id, &team_id).await {
        Ok(bindings) => {
            let _ = results.send(Loaded::Changes(vec![ChangeSet::AppBindings {
                channel_id,
                bindings: Arc::from(bindings),
            }]));
        }
        // No Apps plugin: nothing to show, and nothing to say.
        Err(error) if error.is_not_found() => note_missing(&client),
        Err(error) => tracing::warn!(%error, "could not load App bindings"),
    }
}

/// What choosing a binding does.
enum Plan {
    Show(OpenDialogRequest),
    Call(AppCall),
    Nothing,
}

fn plan(binding: &AppBinding, context: &AppContext, call_id: &str) -> Plan {
    if let Some(form) = &binding.form {
        if let (false, Some(submit)) = (form.fields.is_empty(), &form.submit) {
            return Plan::Show(form_dialog(form, submit, context, call_id, &binding.label));
        }
        if let Some(source) = &form.source {
            return Plan::Call(source.clone());
        }
        if let Some(submit) = &form.submit {
            return Plan::Call(submit.clone());
        }
    }
    binding.submit.clone().map_or(Plan::Nothing, Plan::Call)
}

/// Makes a call and turns the App's answer into change sets.
async fn call_app(client: &Client, mut request: AppCallRequest, call_id: &str) -> Vec<ChangeSet> {
    for _ in 0..MAX_HOPS {
        let response = match client.app_call(&request).await {
            Ok(response) => response,
            Err(error) => return vec![failed(call_id, &error)],
        };
        match response.kind {
            AppResponseKind::Call => match &response.call {
                Some(next) => {
                    request = AppCallRequest::new(next, request.context.clone());
                    continue;
                }
                None => return vec![done(call_id)],
            },
            _ => return answer(&request, response, call_id),
        }
    }
    vec![ChangeSet::AppAnswered {
        call_id: call_id.to_owned(),
        answer: AppAnswer::Failed("The App kept asking for more calls.".into()),
    }]
}

fn failed(call_id: &str, error: &ApiError) -> ChangeSet {
    let text = match error {
        ApiError::Server { message, .. } => message.clone(),
        other => other.to_string(),
    };
    ChangeSet::AppAnswered {
        call_id: call_id.to_owned(),
        answer: AppAnswer::Failed(text),
    }
}

fn done(call_id: &str) -> ChangeSet {
    ChangeSet::AppAnswered {
        call_id: call_id.to_owned(),
        answer: AppAnswer::Done,
    }
}

/// The change sets for an App's answer other than `call`.
fn answer(request: &AppCallRequest, response: AppCallResponse, call_id: &str) -> Vec<ChangeSet> {
    match response.kind {
        AppResponseKind::Ok | AppResponseKind::Call => {
            let mut changes = Vec::new();
            if !response.text.trim().is_empty() {
                changes.push(ChangeSet::Ephemeral(Arc::new(ephemeral(
                    &request.context,
                    &response,
                    call_id,
                ))));
            }
            changes.push(done(call_id));
            changes
        }
        AppResponseKind::Error => vec![ChangeSet::AppAnswered {
            call_id: call_id.to_owned(),
            answer: AppAnswer::Failed(if response.text.trim().is_empty() {
                "The App could not do that.".into()
            } else {
                response.text
            }),
        }],
        AppResponseKind::Form => match response.form {
            Some(form) => {
                let Some(submit) = form.submit.clone() else {
                    return vec![ChangeSet::AppAnswered {
                        call_id: call_id.to_owned(),
                        answer: AppAnswer::Failed("The App's form cannot be sent.".into()),
                    }];
                };
                vec![
                    ChangeSet::OpenDialog(Arc::new(form_dialog(
                        &form,
                        &submit,
                        &request.context,
                        call_id,
                        "",
                    ))),
                    ChangeSet::AppAnswered {
                        call_id: call_id.to_owned(),
                        answer: AppAnswer::Form,
                    },
                ]
            }
            None => vec![done(call_id)],
        },
        AppResponseKind::Navigate => vec![ChangeSet::AppAnswered {
            call_id: call_id.to_owned(),
            answer: AppAnswer::Navigate {
                url: response.navigate_to_url,
                external: response.use_external_browser,
            },
        }],
    }
}

/// The App's text as an ephemeral message where the call came from, from
/// the App's bot when the App names it.
fn ephemeral(context: &AppContext, response: &AppCallResponse, call_id: &str) -> Post {
    let bot = response.app_metadata.clone().unwrap_or_default();
    let mut props = HashMap::new();
    props.insert("from_bot".to_owned(), json!("true"));
    if !bot.bot_username.is_empty() {
        props.insert("override_username".to_owned(), json!(bot.bot_username));
    }
    Post {
        id: format!("app-{call_id}"),
        create_at: 0,
        user_id: bot.bot_user_id,
        channel_id: context.channel_id.clone(),
        root_id: context.root_id.clone(),
        message: response.text.clone(),
        kind: "system_ephemeral".into(),
        props,
        ..Post::default()
    }
}

/// An App form as an interactive dialog. The dialog's state carries the
/// call, the context and the fields' types, so the submission can be sent
/// back to the App as it expects.
pub fn form_dialog(
    form: &AppForm,
    submit: &AppCall,
    context: &AppContext,
    call_id: &str,
    fallback_title: &str,
) -> OpenDialogRequest {
    let mut introduction = form.header.trim().to_owned();
    let mut elements = Vec::new();
    for field in &form.fields {
        if field.kind == "markdown" {
            if !field.description.trim().is_empty() {
                if !introduction.is_empty() {
                    introduction.push_str("\n\n");
                }
                introduction.push_str(field.description.trim());
            }
            continue;
        }
        if field.readonly {
            continue;
        }
        elements.push(element(field));
    }
    if !form.footer.trim().is_empty() {
        if !introduction.is_empty() {
            introduction.push_str("\n\n");
        }
        introduction.push_str(form.footer.trim());
    }
    let fields: Vec<Value> = form
        .fields
        .iter()
        .map(|field| {
            json!({
                "name": field.name,
                "type": field.kind,
                "options": field.options,
                "readonly": field.readonly,
                "value": field.value,
                "lookup": field.lookup,
            })
        })
        .collect();
    let state = json!({
        "call_id": call_id,
        "call": submit,
        "context": context,
        "fields": fields,
    });
    let title = if form.title.trim().is_empty() {
        fallback_title.to_owned()
    } else {
        form.title.clone()
    };
    OpenDialogRequest {
        trigger_id: String::new(),
        url: format!("{DIALOG_URL}{}", context.app_id),
        dialog: Dialog {
            callback_id: call_id.to_owned(),
            title,
            introduction_text: introduction,
            icon_url: form.icon.clone(),
            elements,
            submit_label: String::new(),
            notify_on_cancel: false,
            state: state.to_string(),
        },
    }
}

/// The text of a field's starting value: a string, a bool, or an option's
/// value.
fn value_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Object(option) => option
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        other => other.to_string(),
    }
}

/// A dialog field for an App field. A dynamic select is a select whose
/// options come from [`AppCommand::Lookup`] as the user types.
fn element(field: &AppField) -> DialogElement {
    let label = if field.modal_label.is_empty() {
        if field.label.is_empty() {
            field.name.clone()
        } else {
            field.label.clone()
        }
    } else {
        field.modal_label.clone()
    };
    let options: Vec<PostActionOption> = field
        .options
        .iter()
        .map(|option| PostActionOption {
            text: option.label.clone(),
            value: option.value.clone(),
        })
        .collect();
    let (kind, subtype, data_source) = match field.kind.as_str() {
        "static_select" => ("select", "", ""),
        "radio" => ("radio", "", ""),
        "user" => ("select", "", "users"),
        "channel" => ("select", "", "channels"),
        "dynamic_select" => ("select", "", DYNAMIC),
        "bool" => ("bool", "", ""),
        _ => match field.subtype.as_str() {
            "textarea" => ("textarea", "", ""),
            "email" | "number" | "password" | "tel" | "url" => ("text", field.subtype.as_str(), ""),
            _ => ("text", "", ""),
        },
    };
    DialogElement {
        display_name: label,
        name: field.name.clone(),
        kind: kind.to_owned(),
        subtype: subtype.to_owned(),
        default: value_text(&field.value),
        placeholder: field.hint.clone(),
        help_text: field.description.clone(),
        optional: !field.is_required,
        min_length: field.min_length,
        max_length: field.max_length,
        data_source: data_source.to_owned(),
        options,
    }
}

/// A submitted value as the App reads it: options as `{label, value}`,
/// bools as bools, everything else as text. A dynamic select's choice
/// comes as `{label, value}` already, since only the dialog knows its
/// label.
fn app_value(kind: &str, options: &[AppSelectOption], value: &Value) -> Option<Value> {
    let text = match value {
        Value::Null => return None,
        Value::Object(option) if kind == "dynamic_select" => {
            return Some(Value::Object(option.clone()));
        }
        Value::Bool(checked) => return Some(Value::Bool(*checked)),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    Some(match kind {
        "static_select" | "radio" | "user" | "channel" | "dynamic_select" => {
            let label = options
                .iter()
                .find(|option| option.value == text)
                .map_or_else(|| text.clone(), |option| option.label.clone());
            json!({ "label": label, "value": text })
        }
        "bool" => Value::Bool(matches!(text.as_str(), "true" | "1" | "yes" | "on")),
        _ => Value::String(text),
    })
}

/// The call a submitted App form makes, from its dialog's state.
fn submission_call(submit: &SubmitDialogRequest) -> Option<(String, AppCallRequest)> {
    let state: Value = serde_json::from_str(&submit.state).ok()?;
    let call_id = state.get("call_id")?.as_str()?.to_owned();
    let call: AppCall = serde_json::from_value(state.get("call")?.clone()).ok()?;
    let context: AppContext = serde_json::from_value(state.get("context")?.clone()).ok()?;
    let mut request = AppCallRequest::new(&call, context);
    for field in state.get("fields")?.as_array()? {
        let name = field
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let kind = field
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let options: Vec<AppSelectOption> = field
            .get("options")
            .cloned()
            .and_then(|options| serde_json::from_value(options).ok())
            .unwrap_or_default();
        let readonly = field.get("readonly").and_then(Value::as_bool) == Some(true);
        let value = if readonly {
            field.get("value").cloned().filter(|value| !value.is_null())
        } else {
            submit
                .submission
                .get(name)
                .and_then(|value| app_value(kind, &options, value))
        };
        if let Some(value) = value {
            request.values.insert(name.to_owned(), value);
        }
    }
    Some((call_id, request))
}

/// Sends a submitted App form to the App. Its field errors come back to
/// the form; anything else closes it and answers like a call.
pub(crate) async fn submit(
    client: Client,
    submit: SubmitDialogRequest,
    results: UnboundedSender<Loaded>,
) {
    let closed = |answer: SubmitDialogResponse| ChangeSet::DialogSubmitted {
        callback_id: submit.callback_id.clone(),
        cancelled: submit.cancelled,
        answer,
    };
    if submit.cancelled {
        let _ = results.send(Loaded::Changes(vec![closed(
            SubmitDialogResponse::default(),
        )]));
        return;
    }
    let Some((call_id, request)) = submission_call(&submit) else {
        let _ = results.send(Loaded::Changes(vec![closed(SubmitDialogResponse {
            error: "Matty could not read this form.".into(),
            errors: HashMap::new(),
        })]));
        return;
    };
    let response = match client.app_call(&request).await {
        Ok(response) => response,
        Err(error) => {
            let _ = results.send(Loaded::Changes(vec![closed(SubmitDialogResponse {
                error: error.to_string(),
                errors: HashMap::new(),
            })]));
            return;
        }
    };
    let changes = if response.kind == AppResponseKind::Error {
        let errors = response.field_errors();
        let error = if response.text.trim().is_empty() && errors.is_empty() {
            "The App could not take that.".to_owned()
        } else {
            response.text
        };
        vec![closed(SubmitDialogResponse { error, errors })]
    } else {
        let mut changes = vec![closed(SubmitDialogResponse::default())];
        if response.kind == AppResponseKind::Call {
            changes.extend(call_app(&client, request, &call_id).await);
        } else {
            changes.extend(answer(&request, response, &call_id));
        }
        changes
    };
    let _ = results.send(Loaded::Changes(changes));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> AppForm {
        serde_json::from_value(json!({
            "title": "Send a message",
            "header": "Pick who gets it.",
            "submit": {"path": "/send", "expand": {"post": "all"}},
            "fields": [
                {"name": "text", "type": "text", "label": "Text", "is_required": true},
                {"name": "notes", "type": "text", "subtype": "textarea"},
                {"name": "level", "type": "static_select", "options": [
                    {"label": "High", "value": "high"}, {"label": "Low", "value": "low"}
                ], "value": {"label": "Low", "value": "low"}},
                {"name": "who", "type": "user"},
                {"name": "loud", "type": "bool", "value": true},
                {"name": "info", "type": "markdown", "description": "**Read this**"},
                {"name": "fixed", "type": "text", "readonly": true, "value": "kept"}
            ]
        }))
        .unwrap()
    }

    fn context() -> AppContext {
        AppContext {
            app_id: "hello".into(),
            location: "/post_menu/send".into(),
            user_agent: "desktop".into(),
            channel_id: "c".into(),
            team_id: "t".into(),
            post_id: "p".into(),
            ..AppContext::default()
        }
    }

    #[test]
    fn a_form_becomes_a_dialog_and_its_values_go_back_as_the_app_reads_them() {
        let form = form();
        let request = form_dialog(&form, form.submit.as_ref().unwrap(), &context(), "1", "");
        assert!(is_app_dialog(&request.url));
        assert_eq!(request.dialog.title, "Send a message");
        assert_eq!(
            request.dialog.introduction_text,
            "Pick who gets it.\n\n**Read this**"
        );
        let kinds: Vec<_> = request
            .dialog
            .elements
            .iter()
            .map(|e| (e.name.as_str(), e.kind.as_str(), e.data_source.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("text", "text", ""),
                ("notes", "textarea", ""),
                ("level", "select", ""),
                ("who", "select", "users"),
                ("loud", "bool", ""),
            ]
        );
        assert_eq!(request.dialog.elements[2].default, "low");
        assert_eq!(request.dialog.elements[4].default, "true");
        assert!(!request.dialog.elements[0].optional);

        let submit = SubmitDialogRequest {
            url: request.url.clone(),
            callback_id: request.dialog.callback_id.clone(),
            state: request.dialog.state.clone(),
            submission: HashMap::from([
                ("text".to_owned(), json!("hi")),
                ("level".to_owned(), json!("high")),
                ("who".to_owned(), json!("u1")),
                ("loud".to_owned(), json!(false)),
                ("notes".to_owned(), Value::Null),
            ]),
            ..SubmitDialogRequest::default()
        };
        let (call_id, call) = submission_call(&submit).unwrap();
        assert_eq!(call_id, "1");
        assert_eq!(call.path, "/send");
        assert_eq!(call.expand, Some(json!({"post": "all"})));
        assert_eq!(call.context, context());
        assert_eq!(call.values["text"], json!("hi"));
        assert_eq!(
            call.values["level"],
            json!({"label": "High", "value": "high"})
        );
        assert_eq!(call.values["who"], json!({"label": "u1", "value": "u1"}));
        assert_eq!(call.values["loud"], json!(false));
        assert_eq!(call.values["fixed"], json!("kept"));
        assert!(!call.values.contains_key("notes"));
    }

    /// A dynamic select becomes a looked-up select; its lookup goes to the
    /// field's own call, or the submit call's path plus `/lookup`, with the
    /// query, and its chosen `{label, value}` goes back as it is.
    #[test]
    fn a_dynamic_select_looks_its_options_up() {
        let mut form = form();
        form.fields.push(
            serde_json::from_value(json!({
                "name": "issue", "type": "dynamic_select", "label": "Issue",
                "lookup": {"path": "/issues/lookup", "expand": {"user": "summary"}}
            }))
            .unwrap(),
        );
        form.fields.push(
            serde_json::from_value(json!({"name": "project", "type": "dynamic_select"})).unwrap(),
        );
        let request = form_dialog(&form, form.submit.as_ref().unwrap(), &context(), "1", "");
        let issue = request
            .dialog
            .elements
            .iter()
            .find(|e| e.name == "issue")
            .unwrap();
        assert_eq!(
            (issue.kind.as_str(), issue.data_source.as_str()),
            ("select", DYNAMIC)
        );

        let values = HashMap::from([
            ("text".to_owned(), json!("hi")),
            ("notes".to_owned(), Value::Null),
        ]);
        let own = lookup_call(&request.dialog.state, "issue", "MM-1", &values).unwrap();
        assert_eq!(own.path, "/issues/lookup");
        assert_eq!(own.expand, Some(json!({"user": "summary"})));
        assert_eq!(
            (own.selected_field.as_str(), own.query.as_str()),
            ("issue", "MM-1")
        );
        assert_eq!(own.context, context());
        assert_eq!(
            own.values,
            HashMap::from([("text".to_owned(), json!("hi"))])
        );
        let body = serde_json::to_value(&own).unwrap();
        assert_eq!(body["selected_field"], "issue");
        assert_eq!(body["query"], "MM-1");

        let fallback = lookup_call(&request.dialog.state, "project", "", &values).unwrap();
        assert_eq!(fallback.path, "/send/lookup");
        assert!(lookup_call(&request.dialog.state, "nope", "", &values).is_none());
        assert!(lookup_call("not json", "issue", "", &values).is_none());

        let submit = SubmitDialogRequest {
            state: request.dialog.state.clone(),
            submission: HashMap::from([(
                "issue".to_owned(),
                json!({"label": "MM-1 Crash", "value": "MM-1"}),
            )]),
            ..SubmitDialogRequest::default()
        };
        let (_, call) = submission_call(&submit).unwrap();
        assert_eq!(
            call.values["issue"],
            json!({"label": "MM-1 Crash", "value": "MM-1"})
        );
    }

    #[test]
    fn a_binding_shows_its_form_or_makes_its_call() {
        let mut binding = AppBinding {
            label: "Send".into(),
            submit: Some(AppCall {
                path: "/go".into(),
                ..AppCall::default()
            }),
            ..AppBinding::default()
        };
        assert!(matches!(plan(&binding, &context(), "1"), Plan::Call(call) if call.path == "/go"));
        binding.submit = None;
        binding.form = Some(form());
        assert!(matches!(plan(&binding, &context(), "1"), Plan::Show(_)));
        binding.form = Some(AppForm {
            source: Some(AppCall {
                path: "/form".into(),
                ..AppCall::default()
            }),
            ..AppForm::default()
        });
        assert!(
            matches!(plan(&binding, &context(), "1"), Plan::Call(call) if call.path == "/form")
        );
        binding.form = None;
        assert!(matches!(plan(&binding, &context(), "1"), Plan::Nothing));
    }

    #[test]
    fn answers_become_change_sets() {
        let request = AppCallRequest::new(&AppCall::default(), context());
        let ok: AppCallResponse = serde_json::from_value(json!({
            "type": "ok", "text": "Sent!",
            "app_metadata": {"bot_user_id": "bot", "bot_username": "hello-bot"}
        }))
        .unwrap();
        let changes = answer(&request, ok, "7");
        let ChangeSet::Ephemeral(post) = &changes[0] else {
            panic!("{changes:?}")
        };
        assert_eq!(post.message, "Sent!");
        assert_eq!(post.user_id, "bot");
        assert_eq!(post.channel_id, "c");
        assert_eq!(changes[1], done("7"));

        let error: AppCallResponse =
            serde_json::from_value(json!({"type": "error", "text": "Nope"})).unwrap();
        assert_eq!(
            answer(&request, error, "7"),
            [ChangeSet::AppAnswered {
                call_id: "7".into(),
                answer: AppAnswer::Failed("Nope".into())
            }]
        );
        let go: AppCallResponse = serde_json::from_value(
            json!({"type": "navigate", "navigate_to_url": "https://example.com"}),
        )
        .unwrap();
        assert_eq!(
            answer(&request, go, "7"),
            [ChangeSet::AppAnswered {
                call_id: "7".into(),
                answer: AppAnswer::Navigate {
                    url: "https://example.com".into(),
                    external: false
                }
            }]
        );
    }
}
