//! Interactive dialogs (#211). An integration answers a button press or a
//! slash command with a `trigger_id`, then opens a form with it: the server
//! sends `open_dialog`, and Matty shows the form as a modal dialog. Submit
//! checks what it can here (required fields, lengths, the shape of an email,
//! number or URL), then sync posts the values to `actions/dialogs/submit`;
//! the integration may answer with problems, by field or for the whole
//! form, which the dialog shows and the user can fix. Closing it tells the
//! integration when it asked to hear. An App form's dynamic selects look
//! their options up as the user types ([`lookup`]).

mod lookup;

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use matty_api::models::{DialogElement, OpenDialogRequest, SubmitDialogResponse};
use serde_json::Value;

use crate::ui::{
    alert::{Alert, AlertVariant},
    button::{Button, ButtonVariant},
    checkbox::Checkbox,
    dialog::{
        DialogChangeReason, DialogHandle, dialog, dialog_close, dialog_footer, dialog_header,
        dialog_popup, dialog_title,
    },
    field::{Field, field_content, field_title},
    input::{InputState, Textarea, TextareaState},
    markdown::markdown,
    radio_group::{RadioGroup, RadioItem},
    select::{Select, SelectItem, SelectState},
    spinner::Spinner,
    theme::UiTheme,
};

/// The people and channels a `users` or `channels` menu offers:
/// `(id, name)`, by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuSources {
    pub people: Vec<(SharedString, SharedString)>,
    pub channels: Vec<(SharedString, SharedString)>,
}

/// What the dialog asks the window to send.
#[derive(Clone, Debug, PartialEq)]
pub struct DialogSubmit {
    pub url: String,
    pub callback_id: String,
    pub state: String,
    /// The values by field name; empty when cancelled.
    pub submission: HashMap<String, Value>,
    pub cancelled: bool,
}

impl DialogSubmit {
    /// The sync command, from the channel the user is in and its team.
    pub fn command(&self, channel_id: String, team_id: String) -> matty_sync::Command {
        matty_sync::Command::SubmitDialog {
            url: self.url.clone(),
            callback_id: self.callback_id.clone(),
            state: self.state.clone(),
            channel_id,
            team_id,
            submission: self.submission.clone(),
            cancelled: self.cancelled,
        }
    }
}

pub use lookup::DialogLookup;

/// What the dialog emits.
#[derive(Clone, Debug, PartialEq)]
pub enum InteractiveDialogEvent {
    /// Send this to the integration.
    Submit(DialogSubmit),
    /// Ask an App for a dynamic select's options (#226).
    Lookup(DialogLookup),
}

impl InteractiveDialogEvent {
    /// The sync command, from the channel the user is in and its team.
    pub fn command(&self, channel_id: String, team_id: String) -> matty_sync::Command {
        match self {
            Self::Submit(submit) => submit.command(channel_id, team_id),
            Self::Lookup(lookup) => lookup.command(),
        }
    }
}

/// What sync said that concerns dialogs.
#[derive(Clone, Debug, PartialEq)]
pub enum DialogChange {
    /// An integration opened one.
    Open(Arc<OpenDialogRequest>),
    /// The integration answered a submission.
    Answered {
        callback_id: String,
        cancelled: bool,
        answer: SubmitDialogResponse,
    },
    /// The submission did not reach it.
    Failed(SharedString),
    /// An App's options for a dynamic select, `(value, label)`, for what
    /// the user had typed.
    Options {
        callback_id: String,
        field: String,
        query: String,
        options: Vec<(SharedString, SharedString)>,
    },
}

impl DialogChange {
    /// The dialog's part of a change from sync, if any.
    pub fn from_change(change: &matty_sync::ChangeSet) -> Option<Self> {
        use matty_sync::{ChangeSet, CommandError};
        Some(match change {
            ChangeSet::OpenDialog(request) => Self::Open(request.clone()),
            ChangeSet::DialogSubmitted {
                callback_id,
                cancelled,
                answer,
            } => Self::Answered {
                callback_id: callback_id.clone(),
                cancelled: *cancelled,
                answer: answer.clone(),
            },
            ChangeSet::Failed(CommandError::Server {
                action: "submit dialog",
                error,
                ..
            }) => Self::Failed(format!("The integration couldn't take that: {error}").into()),
            ChangeSet::Failed(CommandError::Offline {
                action: "submit dialog",
            }) => Self::Failed("You're offline. Try again once you're back online.".into()),
            ChangeSet::AppLookup {
                callback_id,
                field,
                query,
                options,
            } => Self::Options {
                callback_id: callback_id.clone(),
                field: field.clone(),
                query: query.clone(),
                options: options
                    .iter()
                    .map(|option| (option.value.clone().into(), option.label.clone().into()))
                    .collect(),
            },
            _ => return None,
        })
    }
}

/// A field's value as the user has it.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Text(String),
    Bool(bool),
    /// A select or radio choice; `None` for none.
    Choice(Option<SharedString>),
}

/// What is wrong with `value` for `element`, in the web app's words, or
/// `None` when it can go.
pub fn problem(element: &DialogElement, value: &FieldValue) -> Option<String> {
    let empty = match value {
        FieldValue::Text(text) => text.trim().is_empty(),
        FieldValue::Bool(checked) => !checked,
        FieldValue::Choice(choice) => choice.as_ref().is_none_or(|value| value.is_empty()),
    };
    if empty {
        return (!element.optional).then(|| "This field is required.".to_owned());
    }
    match value {
        FieldValue::Text(text) => {
            let length = i64::try_from(text.chars().count()).unwrap_or(i64::MAX);
            if element.min_length > 0 && length < element.min_length {
                return Some(format!(
                    "Must be at least {} characters.",
                    element.min_length
                ));
            }
            if element.max_length > 0 && length > element.max_length {
                return Some(format!(
                    "Must be at most {} characters.",
                    element.max_length
                ));
            }
            let text = text.trim();
            match element.subtype.as_str() {
                "email" if !text.contains('@') => Some("Must be a valid email address.".into()),
                "number" if text.parse::<f64>().is_err() => Some("Must be a number.".into()),
                "url" if !(text.starts_with("http://") || text.starts_with("https://")) => {
                    Some("Must be a link starting with http:// or https://.".into())
                }
                _ => None,
            }
        }
        FieldValue::Choice(Some(choice)) if element.kind == "radio" => (!element
            .options
            .iter()
            .any(|option| option.value == choice.as_ref()))
        .then(|| "Choose one of the options.".to_owned()),
        _ => None,
    }
}

/// `value` as the integration reads it: a number for a `number` field, a
/// bool for a checkbox, `null` for nothing.
pub fn submitted(element: &DialogElement, value: &FieldValue) -> Value {
    match value {
        FieldValue::Bool(checked) => Value::Bool(*checked),
        FieldValue::Text(text) if text.trim().is_empty() => Value::Null,
        FieldValue::Text(text) if element.subtype == "number" => text
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(|number| {
                if number.fract() == 0.0 && number.abs() < 9e15 {
                    #[allow(clippy::cast_possible_truncation)]
                    Some(Value::from(number as i64))
                } else {
                    serde_json::Number::from_f64(number).map(Value::Number)
                }
            })
            .unwrap_or_else(|| Value::String(text.clone())),
        FieldValue::Text(text) => Value::String(text.clone()),
        FieldValue::Choice(Some(choice)) if !choice.is_empty() => Value::String(choice.to_string()),
        FieldValue::Choice(_) => Value::Null,
    }
}

/// A field's control and its retained state.
enum Control {
    Text(Entity<InputState>),
    Textarea(Entity<TextareaState>),
    /// A menu; `true` when it is searchable, for people and channels.
    Select(Entity<SelectState>, bool),
    Bool(bool),
    Radio(Option<SharedString>),
    /// A dynamic select, whose options an App looks up (#226).
    Lookup(lookup::Lookup),
}

struct DialogField {
    element: DialogElement,
    control: Control,
}

impl DialogField {
    fn new(
        element: DialogElement,
        sources: &MenuSources,
        window: &mut Window,
        cx: &mut Context<InteractiveDialogView>,
    ) -> Self {
        let default: SharedString = element.default.clone().into();
        let control = match element.kind.as_str() {
            "textarea" => Control::Textarea(cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder(element.placeholder.clone())
                    .default_value(default)
                    .rows(4)
            })),
            "select" if lookup::is_dynamic(&element) => {
                Control::Lookup(lookup::Lookup::new(&element, window, cx))
            }
            "select" => {
                let (options, searchable) = match element.data_source.as_str() {
                    "users" => (sources.people.clone(), true),
                    "channels" => (sources.channels.clone(), true),
                    _ => (
                        element
                            .options
                            .iter()
                            .map(|option| (option.value.clone().into(), option.text.clone().into()))
                            .collect(),
                        false,
                    ),
                };
                let items: Vec<SelectItem> = options
                    .into_iter()
                    .map(|(value, label)| SelectItem::new(value, label))
                    .collect();
                let state = cx.new(|cx| {
                    let mut state = SelectState::new(items, window, cx);
                    if !default.is_empty() {
                        state.set_value(Some(default.clone()), window, cx);
                    }
                    state
                });
                cx.observe(&state, |_, _, cx| cx.notify()).detach();
                Control::Select(state, searchable)
            }
            "bool" => Control::Bool(matches!(
                element.default.trim().to_lowercase().as_str(),
                "true" | "1" | "yes" | "on"
            )),
            "radio" => Control::Radio((!default.is_empty()).then_some(default)),
            // `text`, and any type this Matty does not know yet.
            _ => Control::Text(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(element.placeholder.clone())
                    .masked(element.subtype == "password")
                    .default_value(default)
            })),
        };
        Self { element, control }
    }

    fn value(&self, cx: &App) -> FieldValue {
        match &self.control {
            Control::Text(state) => FieldValue::Text(state.read(cx).value().to_string()),
            Control::Textarea(state) => FieldValue::Text(state.read(cx).value().to_string()),
            Control::Select(state, _) => FieldValue::Choice(state.read(cx).value().cloned()),
            Control::Bool(checked) => FieldValue::Bool(*checked),
            Control::Radio(choice) => FieldValue::Choice(choice.clone()),
            Control::Lookup(lookup) => FieldValue::Choice(lookup.state.read(cx).value().cloned()),
        }
    }

    /// The value as the integration reads it.
    fn submitted(&self, cx: &App) -> Value {
        match &self.control {
            Control::Lookup(lookup) => lookup.submitted(cx),
            _ => submitted(&self.element, &self.value(cx)),
        }
    }
}

/// An open dialog.
struct Shown {
    request: Arc<OpenDialogRequest>,
    fields: Vec<DialogField>,
    /// Problems by field name, ours or the integration's.
    errors: HashMap<String, SharedString>,
    /// The integration's problem with the whole form, or why it did not
    /// get there.
    error: Option<SharedString>,
    /// Sent, and waiting for the integration.
    submitting: bool,
}

/// The modal that shows an integration's dialog. Each window keeps one.
pub struct InteractiveDialogView {
    handle: DialogHandle,
    shown: Option<Shown>,
    focus_first: bool,
}

impl EventEmitter<InteractiveDialogEvent> for InteractiveDialogView {}

impl Default for InteractiveDialogView {
    fn default() -> Self {
        Self::new()
    }
}

impl InteractiveDialogView {
    pub fn new() -> Self {
        Self {
            handle: DialogHandle::new(false),
            shown: None,
            focus_first: false,
        }
    }

    /// Whether a dialog is open.
    pub fn is_open(&self) -> bool {
        self.shown.is_some()
    }

    /// Takes a change from sync. `here` says this window opens a new
    /// dialog; `sources` lists the people and channels its menus offer.
    pub fn apply(
        &mut self,
        change: DialogChange,
        here: bool,
        sources: impl FnOnce() -> MenuSources,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match change {
            DialogChange::Open(request) if here => self.open(request, &sources(), window, cx),
            DialogChange::Open(_) => {}
            DialogChange::Answered {
                callback_id,
                cancelled: false,
                answer,
            } => self.answered(&callback_id, answer, window, cx),
            DialogChange::Answered { .. } => {}
            DialogChange::Failed(error) => {
                if let Some(shown) = self.shown.as_mut().filter(|shown| shown.submitting) {
                    shown.submitting = false;
                    shown.error = Some(error);
                    cx.notify();
                }
            }
            DialogChange::Options {
                callback_id,
                field,
                query,
                options,
            } => self.lookup_options(&callback_id, &field, &query, options, window, cx),
        }
    }

    /// Shows `request`, in place of any dialog already open.
    pub fn open(
        &mut self,
        request: Arc<OpenDialogRequest>,
        sources: &MenuSources,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancelled(cx);
        let fields = request
            .dialog
            .elements
            .iter()
            .filter(|element| !element.name.is_empty())
            .map(|element| DialogField::new(element.clone(), sources, window, cx))
            .collect();
        self.shown = Some(Shown {
            request,
            fields,
            errors: HashMap::new(),
            error: None,
            submitting: false,
        });
        // Dynamic selects show the App's first options when opened.
        let dynamic: Vec<String> = self
            .shown
            .iter()
            .flat_map(|shown| &shown.fields)
            .filter(|field| matches!(field.control, Control::Lookup(_)))
            .map(|field| field.element.name.clone())
            .collect();
        for field in dynamic {
            self.ask_lookup(&field, String::new(), cx);
        }
        self.focus_first = true;
        self.handle.open(window, cx);
        cx.notify();
    }

    /// The integration's answer to the submission.
    fn answered(
        &mut self,
        callback_id: &str,
        answer: SubmitDialogResponse,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(shown) = self
            .shown
            .as_mut()
            .filter(|shown| shown.submitting && shown.request.dialog.callback_id == callback_id)
        else {
            return;
        };
        shown.submitting = false;
        if answer.accepted() {
            self.shown = None;
            self.handle.close(window, cx);
        } else {
            shown.errors = answer
                .errors
                .into_iter()
                .map(|(name, error)| (name, error.into()))
                .collect();
            shown.error = (!answer.error.is_empty()).then(|| answer.error.into());
        }
        cx.notify();
    }

    /// Checks the fields and, when they can go, sends them.
    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        if shown.submitting {
            return;
        }
        let values: Vec<(&DialogField, FieldValue)> = shown
            .fields
            .iter()
            .map(|field| (field, field.value(cx)))
            .collect();
        let errors: HashMap<String, SharedString> = values
            .iter()
            .filter_map(|(field, value)| {
                problem(&field.element, value)
                    .map(|error| (field.element.name.clone(), error.into()))
            })
            .collect();
        let submission: HashMap<String, Value> = values
            .iter()
            .map(|(field, _)| (field.element.name.clone(), field.submitted(cx)))
            .collect();
        shown.error = None;
        shown.errors = errors;
        if shown.errors.is_empty() {
            shown.submitting = true;
            let dialog = &shown.request.dialog;
            cx.emit(InteractiveDialogEvent::Submit(DialogSubmit {
                url: shown.request.url.clone(),
                callback_id: dialog.callback_id.clone(),
                state: dialog.state.clone(),
                submission,
                cancelled: false,
            }));
        }
        cx.notify();
    }

    /// The user closed the dialog: forget it, and tell the integration
    /// when it asked.
    fn cancelled(&mut self, cx: &mut Context<Self>) {
        let Some(shown) = self.shown.take() else {
            return;
        };
        let dialog = &shown.request.dialog;
        if dialog.notify_on_cancel {
            cx.emit(InteractiveDialogEvent::Submit(DialogSubmit {
                url: shown.request.url.clone(),
                callback_id: dialog.callback_id.clone(),
                state: dialog.state.clone(),
                submission: HashMap::new(),
                cancelled: true,
            }));
        }
        cx.notify();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancelled(cx);
        self.handle.close(window, cx);
    }

    fn field(
        &self,
        index: usize,
        field: &DialogField,
        error: Option<SharedString>,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = UiTheme::read(cx).clone();
        let element = &field.element;
        let id = |part: &str| {
            gpui_kit::ElementId::Name(format!("interactive-dialog.field.{index}.{part}").into())
        };
        let label: SharedString = if element.display_name.is_empty() {
            element.name.clone().into()
        } else {
            element.display_name.clone().into()
        };
        let help: Option<SharedString> =
            (!element.help_text.is_empty()).then(|| element.help_text.clone().into());
        let required = !element.optional;
        let selector = format!("interactive-dialog-field-{}", element.name);
        let field_element = match &field.control {
            Control::Text(state) => {
                let mut field = Field::new(id("input"), state)
                    .label(label)
                    .required(required)
                    .disabled(disabled);
                field = match (error, help) {
                    (Some(error), _) => field.error(error),
                    (None, Some(help)) => field.description(help),
                    (None, None) => field,
                };
                field.into_any_element()
            }
            Control::Textarea(state) => {
                let mut field = Field::from_control(
                    id("input"),
                    // gpuicn's focus ring shows through a textarea's
                    // see-through fill; the same colour, made opaque, keeps
                    // it to the edge as on a single-line field.
                    Textarea::new(state).bg(theme.colors.popover.blend(theme.control().background)),
                )
                .label(label)
                .required(required)
                .disabled(disabled);
                field = match (error, help) {
                    (Some(error), _) => field.error(error),
                    (None, Some(help)) => field.description(help),
                    (None, None) => field,
                };
                field.into_any_element()
            }
            Control::Lookup(lookup) => {
                let placeholder: SharedString = if element.placeholder.is_empty() {
                    "Type to search".into()
                } else {
                    element.placeholder.clone().into()
                };
                let combobox =
                    crate::ui::combobox::combobox(&lookup.state).placeholder(placeholder);
                let mut field = Field::from_control(id("select"), combobox)
                    .label(label)
                    .required(required)
                    .disabled(disabled);
                field = match (error, help) {
                    (Some(error), _) => field.error(error),
                    (None, Some(help)) => field.description(help),
                    (None, None) => field,
                };
                field.into_any_element()
            }
            Control::Select(state, searchable) => {
                let select = if *searchable {
                    crate::ui::combobox::combobox(state)
                } else {
                    Select::new(state)
                };
                let placeholder: SharedString = if element.placeholder.is_empty() {
                    "Choose an option".into()
                } else {
                    element.placeholder.clone().into()
                };
                let mut field = Field::from_control(id("select"), select.placeholder(placeholder))
                    .label(label)
                    .required(required)
                    .disabled(disabled);
                field = match (error, help) {
                    (Some(error), _) => field.error(error),
                    (None, Some(help)) => field.description(help),
                    (None, None) => field,
                };
                field.into_any_element()
            }
            Control::Bool(checked) => {
                let name = element.name.clone();
                let view = cx.weak_entity();
                let checkbox = Checkbox::new(id("checkbox")).checked(*checked).on_change(
                    move |next, _, _, cx| {
                        let name = name.clone();
                        view.update(cx, |this, cx| this.set_bool(&name, next, cx))
                            .ok();
                    },
                );
                // The placeholder is the text beside the box; with one, the
                // field's name heads it.
                let beside: Option<SharedString> =
                    (!element.placeholder.is_empty()).then(|| element.placeholder.clone().into());
                let field = Field::from_control(id("bool"), checkbox)
                    .label(beside.clone().unwrap_or_else(|| label.clone()))
                    .required(required && beside.is_none())
                    .disabled(disabled);
                let field = match (error, help) {
                    (Some(error), _) => field.error(error),
                    (None, Some(help)) => field.description(help),
                    (None, None) => field,
                };
                match beside {
                    None => field.into_any_element(),
                    Some(_) => field_content(cx)
                        .child(
                            field_title(cx)
                                .child(label)
                                .when(required, |title| title.child(" *")),
                        )
                        .child(field)
                        .into_any_element(),
                }
            }
            Control::Radio(choice) => {
                let name = element.name.clone();
                let view = cx.weak_entity();
                let mut group = RadioGroup::new(id("radio"));
                if let Some(choice) = choice {
                    group = group.value(choice.clone());
                }
                for (option_index, option) in element.options.iter().enumerate() {
                    group = group.item(
                        RadioItem::new(id(&format!("radio.{option_index}")), option.value.clone())
                            .label(option.text.clone()),
                    );
                }
                let group = group.on_change(move |next, _, cx| {
                    let name = name.clone();
                    view.update(cx, |this, cx| this.set_choice(&name, next, cx))
                        .ok();
                });
                let field = Field::from_control(id("radio-field"), group)
                    .label(label)
                    .required(required)
                    .disabled(disabled);
                match (error, help) {
                    (Some(error), _) => field.error(error),
                    (None, Some(help)) => field.description(help),
                    (None, None) => field,
                }
                .into_any_element()
            }
        };
        div()
            .debug_selector(move || selector.clone())
            .child(field_element)
            .into_any_element()
    }

    fn set_bool(&mut self, name: &str, checked: bool, cx: &mut Context<Self>) {
        if let Some(field) = self.field_named(name) {
            field.control = Control::Bool(checked);
            cx.notify();
        }
    }

    fn set_choice(&mut self, name: &str, choice: SharedString, cx: &mut Context<Self>) {
        if let Some(field) = self.field_named(name) {
            field.control = Control::Radio(Some(choice));
            cx.notify();
        }
    }

    fn field_named(&mut self, name: &str) -> Option<&mut DialogField> {
        self.shown
            .as_mut()?
            .fields
            .iter_mut()
            .find(|field| field.element.name == name)
    }

    /// Focuses the first text field once it is on screen.
    fn focus_first_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.focus_first) {
            return;
        }
        let focus =
            self.shown
                .iter()
                .flat_map(|shown| &shown.fields)
                .find_map(|field| match &field.control {
                    Control::Text(state) => Some(state.read(cx).focus_handle(cx)),
                    Control::Textarea(state) => Some(state.read(cx).focus_handle(cx)),
                    _ => None,
                });
        if let Some(focus) = focus {
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
    }
}

impl Render for InteractiveDialogView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.focus_first_field(window, cx);
        let theme = UiTheme::read(cx).clone();
        let view = cx.weak_entity();
        let popup = match &self.shown {
            None => div().into_any_element(),
            Some(shown) => {
                let dialog = &shown.request.dialog;
                let title: SharedString = if dialog.title.is_empty() {
                    "Integration".into()
                } else {
                    dialog.title.clone().into()
                };
                let submit_label: SharedString = if dialog.submit_label.is_empty() {
                    "Submit".into()
                } else {
                    dialog.submit_label.clone().into()
                };
                let submitting = shown.submitting;
                let fields: Vec<gpui_kit::AnyElement> = shown
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(index, field)| {
                        let error = shown.errors.get(&field.element.name).cloned();
                        self.field(index, field, error, submitting, cx)
                    })
                    .collect();
                // An error for a field the form does not have still shows.
                let stray: Vec<SharedString> = shown
                    .errors
                    .iter()
                    .filter(|(name, _)| !shown.fields.iter().any(|f| f.element.name == **name))
                    .map(|(_, error)| error.clone())
                    .collect();
                let error = shown.error.iter().cloned().chain(stray);
                dialog_popup("interactive-dialog.popup", title.clone(), cx)
                    .child(
                        dialog_header(cx)
                            .pr(theme.space(8.))
                            .child(dialog_title("interactive-dialog.title", cx).child(title))
                            .when(!dialog.introduction_text.is_empty(), |header| {
                                header.child(
                                    Alert::new("interactive-dialog.introduction")
                                        .variant(AlertVariant::Info)
                                        .child(markdown(
                                            "interactive-dialog.introduction.text",
                                            dialog.introduction_text.clone(),
                                            cx,
                                        )),
                                )
                            }),
                    )
                    .children(error.enumerate().map(|(index, error)| {
                        div()
                            .debug_selector(|| "interactive-dialog-error".into())
                            .child(
                                Alert::new(("interactive-dialog.error", index))
                                    .variant(AlertVariant::Destructive)
                                    .description(error),
                            )
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.space(5.))
                            .children(fields),
                    )
                    .child(
                        dialog_footer(cx)
                            .child(
                                Button::new("interactive-dialog.cancel")
                                    .variant(ButtonVariant::Outline)
                                    .label("Cancel")
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.cancel(window, cx)),
                                    ),
                            )
                            .child(
                                div()
                                    .debug_selector(|| "interactive-dialog-submit".into())
                                    .child(
                                        Button::new("interactive-dialog.submit")
                                            .aria_label(if submitting {
                                                format!("{submit_label} (sending)").into()
                                            } else {
                                                submit_label.clone()
                                            })
                                            .disabled(submitting)
                                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(theme.space(1.5))
                                                    .when(submitting, |label| {
                                                        label.child(Spinner::new(
                                                            "interactive-dialog.spinner",
                                                        ))
                                                    })
                                                    .child(submit_label),
                                            ),
                                    ),
                            ),
                    )
                    .child(dialog_close("interactive-dialog.close", cx))
                    .into_any_element()
            }
        };
        dialog("interactive-dialog", &self.handle, popup, window, cx).on_open_change(
            move |open, reason, _, cx| {
                // Our own closing has already dealt with the dialog.
                if !open && reason != DialogChangeReason::Imperative {
                    view.update(cx, |this, cx| this.cancelled(cx)).ok();
                }
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(kind: &str) -> DialogElement {
        DialogElement {
            name: "f".into(),
            display_name: "F".into(),
            kind: kind.into(),
            ..DialogElement::default()
        }
    }

    #[test]
    fn required_fields_and_lengths_are_checked() {
        let text = DialogElement {
            min_length: 3,
            max_length: 5,
            ..element("text")
        };
        let check = |value: &str| problem(&text, &FieldValue::Text(value.into()));
        assert_eq!(check("  ").as_deref(), Some("This field is required."));
        assert_eq!(
            check("ab").as_deref(),
            Some("Must be at least 3 characters.")
        );
        assert_eq!(
            check("abcdef").as_deref(),
            Some("Must be at most 5 characters.")
        );
        assert_eq!(check("abcd"), None);
        let optional = DialogElement {
            optional: true,
            ..text.clone()
        };
        assert_eq!(problem(&optional, &FieldValue::Text(String::new())), None);
        // A required checkbox must be ticked, as in the web app.
        assert!(problem(&element("bool"), &FieldValue::Bool(false)).is_some());
        assert!(problem(&element("select"), &FieldValue::Choice(None)).is_some());
    }

    #[test]
    fn subtypes_and_options_are_checked() {
        let of = |subtype: &str, value: &str| {
            let element = DialogElement {
                subtype: subtype.into(),
                ..element("text")
            };
            problem(&element, &FieldValue::Text(value.into()))
        };
        assert!(of("email", "ada").is_some());
        assert!(of("email", "ada@example.com").is_none());
        assert!(of("number", "12a").is_some());
        assert!(of("number", "12.5").is_none());
        assert!(of("url", "example.com").is_some());
        assert!(of("url", "https://example.com").is_none());
        let radio = DialogElement {
            options: vec![matty_api::models::PostActionOption {
                text: "Now".into(),
                value: "now".into(),
            }],
            ..element("radio")
        };
        assert!(problem(&radio, &FieldValue::Choice(Some("later".into()))).is_some());
        assert!(problem(&radio, &FieldValue::Choice(Some("now".into()))).is_none());
    }

    #[test]
    fn values_go_as_the_integration_reads_them() {
        let number = DialogElement {
            subtype: "number".into(),
            ..element("text")
        };
        assert_eq!(
            submitted(&number, &FieldValue::Text("42".into())),
            Value::from(42)
        );
        assert_eq!(
            submitted(&number, &FieldValue::Text("1.5".into())),
            Value::from(1.5)
        );
        assert_eq!(
            submitted(&element("text"), &FieldValue::Text(" ".into())),
            Value::Null
        );
        assert_eq!(
            submitted(&element("bool"), &FieldValue::Bool(false)),
            Value::Bool(false)
        );
        assert_eq!(
            submitted(&element("select"), &FieldValue::Choice(Some("u1".into()))),
            Value::from("u1")
        );
    }

    #[test]
    fn sync_changes_reach_the_dialog() {
        use matty_sync::{ChangeSet, CommandError};
        let failed = ChangeSet::Failed(CommandError::Server {
            action: "submit dialog",
            target: "deploy".into(),
            error: "Bad gateway".into(),
        });
        assert!(matches!(
            DialogChange::from_change(&failed),
            Some(DialogChange::Failed(error)) if error.contains("Bad gateway")
        ));
        let other = ChangeSet::Failed(CommandError::Offline {
            action: "run action",
        });
        assert_eq!(DialogChange::from_change(&other), None);
    }

    /// The dialog under test and what it sent.
    struct Host {
        dialog: Entity<InteractiveDialogView>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.dialog.clone())
        }
    }

    fn request() -> Arc<OpenDialogRequest> {
        Arc::new(OpenDialogRequest {
            trigger_id: "t".into(),
            url: "https://hook.example.com".into(),
            dialog: matty_api::models::Dialog {
                callback_id: "deploy".into(),
                title: "Approve deploy".into(),
                elements: vec![
                    DialogElement {
                        name: "reason".into(),
                        display_name: "Reason".into(),
                        kind: "text".into(),
                        min_length: 3,
                        ..DialogElement::default()
                    },
                    DialogElement {
                        name: "notify".into(),
                        display_name: "Notify".into(),
                        kind: "bool".into(),
                        default: "true".into(),
                        optional: true,
                        ..DialogElement::default()
                    },
                ],
                notify_on_cancel: true,
                state: "s".into(),
                ..Default::default()
            },
        })
    }

    /// Submit checks the fields first; the integration's own complaint
    /// about a field shows under it, and its yes closes the dialog.
    #[gpui_kit::test]
    fn submitting_shows_field_errors_and_closes_when_taken(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let window = cx.add_window({
            let sent = sent.clone();
            move |window, cx| {
                let dialog = cx.new(|_| InteractiveDialogView::new());
                let record = sent.clone();
                cx.subscribe(&dialog, move |_, _, event: &InteractiveDialogEvent, _| {
                    if let InteractiveDialogEvent::Submit(submit) = event {
                        record.borrow_mut().push(submit.clone());
                    }
                })
                .detach();
                dialog.update(cx, |dialog, cx| {
                    dialog.open(request(), &MenuSources::default(), window, cx)
                });
                Host { dialog }
            }
        });
        let mut cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let dialog = window
            .read_with(&cx, |host, _| host.dialog.clone())
            .unwrap();
        let press_submit = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let bounds = cx
                .debug_bounds("interactive-dialog-submit")
                .expect("submit drawn");
            cx.simulate_click(bounds.center(), Default::default());
        };
        // Empty and required: nothing goes, and the field says why.
        press_submit(&mut cx);
        assert!(sent.borrow().is_empty());
        dialog.read_with(&cx, |dialog, _| {
            let shown = dialog.shown.as_ref().unwrap();
            assert_eq!(
                shown.errors.get("reason").map(SharedString::as_ref),
                Some("This field is required.")
            );
        });
        let reason = dialog.read_with(&cx, |dialog, _| {
            match &dialog.shown.as_ref().unwrap().fields[0].control {
                Control::Text(state) => state.clone(),
                _ => panic!("a text field"),
            }
        });
        cx.update(|window, cx| {
            reason.update(cx, |state, cx| state.set_value("ok", window, cx));
        });
        // Too short is the integration's to hear about only once it fits.
        press_submit(&mut cx);
        assert!(sent.borrow().is_empty());
        cx.update(|window, cx| {
            reason.update(cx, |state, cx| state.set_value("ship it", window, cx));
        });
        press_submit(&mut cx);
        {
            let sent = sent.borrow();
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].callback_id, "deploy");
            assert_eq!(sent[0].submission["reason"], Value::from("ship it"));
            assert_eq!(sent[0].submission["notify"], Value::Bool(true));
            assert!(!sent[0].cancelled);
        }
        // The integration refuses the reason: it shows under the field, and
        // the form can go again.
        cx.update(|window, cx| {
            dialog.update(cx, |dialog, cx| {
                let answer = SubmitDialogResponse {
                    error: String::new(),
                    errors: [("reason".to_owned(), "Name the ticket.".to_owned())].into(),
                };
                let change = DialogChange::Answered {
                    callback_id: "deploy".into(),
                    cancelled: false,
                    answer,
                };
                dialog.apply(change, true, MenuSources::default, window, cx);
            })
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        dialog.read_with(&cx, |dialog, _| {
            let shown = dialog.shown.as_ref().unwrap();
            assert!(!shown.submitting);
            assert_eq!(
                shown.errors.get("reason").map(SharedString::as_ref),
                Some("Name the ticket.")
            );
        });
        press_submit(&mut cx);
        assert_eq!(sent.borrow().len(), 2);
        cx.update(|window, cx| {
            dialog.update(cx, |dialog, cx| {
                let change = DialogChange::Answered {
                    callback_id: "deploy".into(),
                    cancelled: false,
                    answer: SubmitDialogResponse::default(),
                };
                dialog.apply(change, true, MenuSources::default, window, cx);
            })
        });
        dialog.read_with(&cx, |dialog, _| assert!(!dialog.is_open()));
        // Closed after it was taken: no cancellation goes.
        assert_eq!(sent.borrow().len(), 2);
    }

    /// Cancel tells an integration that asked to hear.
    #[gpui_kit::test]
    fn cancel_notifies_when_asked(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let window = cx.add_window({
            let sent = sent.clone();
            move |window, cx| {
                let dialog = cx.new(|_| InteractiveDialogView::new());
                let record = sent.clone();
                cx.subscribe(&dialog, move |_, _, event: &InteractiveDialogEvent, _| {
                    if let InteractiveDialogEvent::Submit(submit) = event {
                        record.borrow_mut().push(submit.clone());
                    }
                })
                .detach();
                dialog.update(cx, |dialog, cx| {
                    dialog.open(request(), &MenuSources::default(), window, cx)
                });
                Host { dialog }
            }
        });
        let mut cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let dialog = window
            .read_with(&cx, |host, _| host.dialog.clone())
            .unwrap();
        cx.update(|window, cx| dialog.update(cx, |dialog, cx| dialog.cancel(window, cx)));
        let sent = sent.borrow();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].cancelled);
        assert!(sent[0].submission.is_empty());
        assert_eq!(sent[0].state, "s");
    }
}
