//! An App form's dynamic selects (#226): a gpuicn combobox whose options
//! the App looks up as the user types. Typing asks after a short pause
//! ([`InteractiveDialogEvent::Lookup`]); sync makes the field's lookup call
//! and the options come back as [`DialogChange::Options`], for the query
//! last asked. The chosen option goes to the App as `{label, value}`.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::{App, AppContext as _, Context, Entity, SharedString, Window};
use matty_api::models::DialogElement;
use serde_json::Value;

use super::{Control, InteractiveDialogEvent, InteractiveDialogView};
use crate::ui::input::InputEvent;
use crate::ui::select::{SelectItem, SelectState};

/// How long typing pauses before the App is asked.
pub(super) const PAUSE: Duration = Duration::from_millis(250);

/// What the dialog asks sync to look up.
#[derive(Clone, Debug, PartialEq)]
pub struct DialogLookup {
    pub callback_id: String,
    pub state: String,
    pub field: String,
    pub query: String,
    /// The form's other values so far, as they would be submitted.
    pub values: HashMap<String, Value>,
}

impl DialogLookup {
    pub fn command(&self) -> matty_sync::Command {
        matty_sync::Command::App(matty_sync::AppCommand::Lookup {
            callback_id: self.callback_id.clone(),
            state: self.state.clone(),
            field: self.field.clone(),
            query: self.query.clone(),
            values: self.values.clone(),
        })
    }
}

/// Whether `element` is a select whose options an App looks up.
pub(super) fn is_dynamic(element: &DialogElement) -> bool {
    element.kind == "select" && element.data_source == matty_sync::APP_LOOKUP_SOURCE
}

/// A dynamic select's state.
pub(super) struct Lookup {
    pub(super) state: Entity<SelectState>,
    /// The query last asked about, whose answer the combobox takes.
    asked: Option<String>,
    /// The options' labels by value, for the submission.
    labels: HashMap<SharedString, SharedString>,
    _ask: Option<gpui_kit::Task<()>>,
    _typing: gpui_kit::Subscription,
}

impl Lookup {
    pub(super) fn new(
        element: &DialogElement,
        window: &mut Window,
        cx: &mut Context<InteractiveDialogView>,
    ) -> Self {
        let default: SharedString = element.default.clone().into();
        let chosen = (!default.is_empty()).then_some(default);
        let items = chosen.iter().map(|value| (value.clone(), value.clone()));
        let (state, typing) = combobox(&element.name, items.collect(), chosen, window, cx);
        Self {
            state,
            asked: None,
            labels: HashMap::new(),
            _ask: None,
            _typing: typing,
        }
    }

    /// The chosen option as the App reads it.
    pub(super) fn submitted(&self, cx: &App) -> Value {
        match self.state.read(cx).value() {
            Some(value) if !value.is_empty() => {
                let label = self.labels.get(value).unwrap_or(value);
                serde_json::json!({ "label": label.as_ref(), "value": value.as_ref() })
            }
            _ => Value::Null,
        }
    }
}

/// A combobox over `items`, `(value, label)`, with `chosen` picked, that
/// asks for options as its text changes.
fn combobox(
    field: &str,
    items: Vec<(SharedString, SharedString)>,
    chosen: Option<SharedString>,
    window: &mut Window,
    cx: &mut Context<InteractiveDialogView>,
) -> (Entity<SelectState>, gpui_kit::Subscription) {
    let items: Vec<SelectItem> = items
        .into_iter()
        .map(|(value, label)| SelectItem::new(value, label))
        .collect();
    let state = cx.new(|cx| {
        let mut state = SelectState::new(items, window, cx);
        if chosen.is_some() {
            state.set_value(chosen, window, cx);
        }
        state
    });
    cx.observe(&state, |_, _, cx| cx.notify()).detach();
    let input = state.read(cx).input().clone();
    let field = field.to_owned();
    let typing = cx.subscribe_in(
        &input,
        window,
        move |this, input, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                let query = input.read(cx).value().to_string();
                this.type_lookup(&field, query, cx);
            }
        },
    );
    (state, typing)
}

impl InteractiveDialogView {
    fn lookup_mut(&mut self, field: &str) -> Option<&mut Lookup> {
        match &mut self.field_named(field)?.control {
            Control::Lookup(lookup) => Some(lookup),
            _ => None,
        }
    }

    /// The user typed `query` in a dynamic select: asks after a pause,
    /// unless that is what was asked last.
    fn type_lookup(&mut self, field: &str, query: String, cx: &mut Context<Self>) {
        let Some(lookup) = self.lookup_mut(field) else {
            return;
        };
        if lookup.asked.as_deref() == Some(query.as_str()) {
            lookup._ask = None;
            return;
        }
        let field = field.to_owned();
        lookup._ask = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PAUSE).await;
            this.update(cx, |this, cx| this.ask_lookup(&field, query, cx))
                .ok();
        }));
    }

    /// Asks sync for `field`'s options for `query`.
    pub(super) fn ask_lookup(&mut self, field: &str, query: String, cx: &mut Context<Self>) {
        let values = self.values_so_far(field, cx);
        let Some(shown) = &self.shown else {
            return;
        };
        let dialog = &shown.request.dialog;
        let (callback_id, state) = (dialog.callback_id.clone(), dialog.state.clone());
        let Some(lookup) = self.lookup_mut(field) else {
            return;
        };
        lookup.asked = Some(query.clone());
        lookup._ask = None;
        lookup
            .state
            .update(cx, |state, cx| state.set_loading(true, cx));
        cx.emit(InteractiveDialogEvent::Lookup(DialogLookup {
            callback_id,
            state,
            field: field.to_owned(),
            query,
            values,
        }));
    }

    /// The other fields' values, as a submission would send them.
    fn values_so_far(&self, except: &str, cx: &App) -> HashMap<String, Value> {
        self.shown
            .iter()
            .flat_map(|shown| &shown.fields)
            .filter(|field| field.element.name != except)
            .map(|field| (field.element.name.clone(), field.submitted(cx)))
            .filter(|(_, value)| !value.is_null())
            .collect()
    }

    /// The App's options for `field` and `query`. Only the answer to the
    /// query last asked shows. The options replace the combobox's in place
    /// (gpuicn#141), so its text, its choice, its highlight and its focus
    /// stay as they are.
    pub(super) fn lookup_options(
        &mut self,
        callback_id: &str,
        field: &str,
        query: &str,
        options: Vec<(SharedString, SharedString)>,
        cx: &mut Context<Self>,
    ) {
        if self
            .shown
            .as_ref()
            .is_none_or(|shown| shown.request.dialog.callback_id != callback_id)
        {
            return;
        }
        let Some(lookup) = self.lookup_mut(field) else {
            return;
        };
        if lookup.asked.as_deref() != Some(query) {
            return;
        }
        let items: Vec<SelectItem> = options
            .iter()
            .map(|(value, label)| SelectItem::new(value.clone(), label.clone()))
            .collect();
        lookup.labels.extend(options);
        lookup.state.update(cx, |state, cx| {
            state.set_items(items, cx);
            state.set_loading(false, cx);
        });
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    struct Host {
        dialog: Entity<InteractiveDialogView>,
    }

    impl gpui_kit::Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui_kit::IntoElement {
            use gpui_kit::{ParentElement as _, Styled as _};
            gpui_kit::div().size_full().child(self.dialog.clone())
        }
    }

    fn request() -> Arc<OpenDialogRequest> {
        Arc::new(OpenDialogRequest {
            trigger_id: String::new(),
            url: format!("{}hello", matty_sync::APP_DIALOG_URL),
            dialog: matty_api::models::Dialog {
                callback_id: "9".into(),
                title: "File an issue".into(),
                elements: vec![
                    DialogElement {
                        name: "title".into(),
                        display_name: "Title".into(),
                        kind: "text".into(),
                        default: "Crash".into(),
                        ..DialogElement::default()
                    },
                    DialogElement {
                        name: "issue".into(),
                        display_name: "Related issue".into(),
                        kind: "select".into(),
                        data_source: matty_sync::APP_LOOKUP_SOURCE.into(),
                        ..DialogElement::default()
                    },
                ],
                state: "{}".into(),
                ..Default::default()
            },
        })
    }

    /// Opening the form asks for the field's first options; typing asks
    /// again after a pause, the answer fills the combobox, and the choice
    /// goes to the App with its label.
    #[gpui_kit::test]
    fn a_dynamic_select_asks_as_the_user_types(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let window = cx.add_window({
            let events = events.clone();
            move |window, cx| {
                let dialog = cx.new(|_| InteractiveDialogView::new());
                cx.subscribe(&dialog, move |_, _, event: &InteractiveDialogEvent, _| {
                    events.borrow_mut().push(event.clone());
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
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let lookups = |events: &Rc<RefCell<Vec<InteractiveDialogEvent>>>| -> Vec<DialogLookup> {
            events
                .borrow()
                .iter()
                .filter_map(|event| match event {
                    InteractiveDialogEvent::Lookup(lookup) => Some(lookup.clone()),
                    InteractiveDialogEvent::Submit(_) => None,
                })
                .collect()
        };
        let asked = lookups(&events);
        assert_eq!(asked.len(), 1, "the first options are asked on opening");
        assert_eq!(
            (asked[0].field.as_str(), asked[0].query.as_str()),
            ("issue", "")
        );
        assert_eq!(asked[0].values["title"], serde_json::json!("Crash"));
        assert!(matches!(
            asked[0].command(),
            matty_sync::Command::App(matty_sync::AppCommand::Lookup { .. })
        ));

        // Typing asks once the user pauses.
        let state = |cx: &mut gpui_kit::VisualTestContext| {
            dialog.read_with(cx, |dialog, _| {
                let Some(Control::Lookup(lookup)) = dialog
                    .shown
                    .as_ref()
                    .and_then(|shown| shown.fields.get(1))
                    .map(|field| &field.control)
                else {
                    panic!("a dynamic select");
                };
                lookup.state.clone()
            })
        };
        let input = state(&mut cx).read_with(&cx, |state, _| state.input().clone());
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.focus(window, cx);
                input.replace_all("cra", window, cx);
            })
        });
        assert_eq!(lookups(&events).len(), 1, "not before the pause");
        cx.executor().advance_clock(PAUSE * 2);
        cx.run_until_parked();
        let asked = lookups(&events);
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[1].query, "cra");

        // An answer to an older query is dropped; the latest fills it.
        let options = vec![
            ("MM-1".into(), "MM-1 Crash on start".into()),
            ("MM-7".into(), "MM-7 Crash on quit".into()),
        ];
        let change = |query: &str, options: Vec<(SharedString, SharedString)>| {
            DialogChange::from_change(&matty_sync::ChangeSet::AppLookup {
                callback_id: "9".into(),
                field: "issue".into(),
                query: query.into(),
                options: options
                    .into_iter()
                    .map(|(value, label)| matty_api::models::AppSelectOption {
                        label: label.to_string(),
                        value: value.to_string(),
                    })
                    .collect(),
            })
            .unwrap()
        };
        let before = state(&mut cx);
        assert!(
            before.read_with(&cx, |state, _| state.is_loading()),
            "the list says it is loading while the App is asked"
        );
        cx.update(|window, cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.apply(
                    change("", options.clone()),
                    true,
                    Default::default,
                    window,
                    cx,
                )
            })
        });
        assert!(
            before.read_with(&cx, |state, _| state.items().is_empty()),
            "a stale answer changes nothing"
        );
        cx.update(|window, cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.apply(change("cra", options), true, Default::default, window, cx)
            })
        });
        cx.run_until_parked();
        let after = state(&mut cx);
        assert_eq!(after, before, "the options replace the list in place");
        after.read_with(&cx, |state, _| {
            assert_eq!(state.items().len(), 2, "the options came");
            assert!(!state.is_loading());
        });
        cx.update(|window, cx| {
            let input = after.read(cx).input().clone();
            assert_eq!(input.read(cx).value().as_ref(), "cra", "the text stays");
            assert!(input.read(cx).focus_handle(cx).is_focused(window));
            assert!(after.read(cx).is_open(), "the list shows the options");
        });
        assert_eq!(
            lookups(&events).len(),
            2,
            "the same query is not asked again"
        );

        // Picking one sends it with its label.
        cx.update(|window, cx| {
            after.update(cx, |state, cx| {
                state.set_value(Some("MM-7".into()), window, cx)
            });
            dialog.update(cx, |dialog, cx| dialog.submit(cx));
        });
        let submitted = events
            .borrow()
            .iter()
            .find_map(|event| match event {
                InteractiveDialogEvent::Submit(submit) => Some(submit.clone()),
                InteractiveDialogEvent::Lookup(_) => None,
            })
            .expect("submitted");
        assert_eq!(
            submitted.submission["issue"],
            serde_json::json!({"label": "MM-7 Crash on quit", "value": "MM-7"})
        );
    }
}
