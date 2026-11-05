//! The dialogs behind the sidebar's category actions (#208): name a new
//! category, rename one, or confirm deleting one.

use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, Focusable as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
};

use crate::t;
use crate::ui::{
    alert_dialog::{alert_dialog, alert_dialog_action, alert_dialog_cancel},
    button::{Button, ButtonVariant},
    dialog::{DialogHandle, dialog, dialog_description, dialog_popup, dialog_title},
    field::Field,
    input::{InputEvent, InputState},
    theme::UiTheme,
};

/// Mattermost's limit on a category name.
pub const MAX_NAME: usize = 22;

/// What the user confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CategoryEvent {
    /// A new category named `name`, with `channel_id` moved into it.
    Create {
        name: String,
        channel_id: Option<SharedString>,
    },
    Rename {
        category_id: SharedString,
        name: String,
    },
    Delete {
        category_id: SharedString,
    },
}

/// What the name dialog is for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Naming {
    Create { channel_id: Option<SharedString> },
    Rename { category_id: SharedString },
}

pub struct CategoryDialogs {
    name: DialogHandle,
    input: Entity<InputState>,
    naming: Option<Naming>,
    delete: DialogHandle,
    /// The category to delete and its name, while the confirmation shows.
    deleting: Option<(SharedString, SharedString)>,
    focus_input: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CategoryEvent> for CategoryDialogs {}

impl CategoryDialogs {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("category-name-placeholder")));
        let subscriptions =
            vec![
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::PressEnter { .. } => this.save(window, cx),
                        // The Create button follows whether the name is usable.
                        _ => cx.notify(),
                    }
                }),
            ];
        Self {
            name: DialogHandle::new(false),
            input,
            naming: None,
            delete: DialogHandle::new(false),
            deleting: None,
            focus_input: false,
            _subscriptions: subscriptions,
        }
    }

    /// "Create new category", from the sidebar's plus menu or a row's
    /// "Move to" menu (then the row's channel moves into it).
    pub fn create(
        &mut self,
        channel_id: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_name(Naming::Create { channel_id }, "", window, cx);
    }

    pub fn rename(
        &mut self,
        category_id: SharedString,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_name(Naming::Rename { category_id }, name, window, cx);
    }

    pub fn delete(
        &mut self,
        category_id: SharedString,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.deleting = Some((category_id, name));
        self.delete.open(window, cx);
        cx.notify();
    }

    fn open_name(
        &mut self,
        naming: Naming,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.naming = Some(naming);
        self.input.update(cx, |input, cx| {
            input.set_value(name.to_string(), window, cx)
        });
        self.name.open(window, cx);
        self.focus_input = true;
        cx.notify();
    }

    /// The typed name, trimmed, when it is one the server takes.
    fn typed(&self, cx: &gpui_kit::App) -> Option<String> {
        valid_name(&self.input.read(cx).value())
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.name.is_open() {
            return;
        }
        let (Some(name), Some(naming)) = (self.typed(cx), self.naming.clone()) else {
            return;
        };
        self.name.close(window, cx);
        cx.emit(match naming {
            Naming::Create { channel_id } => CategoryEvent::Create { name, channel_id },
            Naming::Rename { category_id } => CategoryEvent::Rename { category_id, name },
        });
    }

    fn name_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let creating = matches!(self.naming, Some(Naming::Create { .. }));
        let (title, action) = if creating {
            (t!("category-create-title"), t!("category-create"))
        } else {
            (t!("category-rename-title"), t!("category-rename"))
        };
        let usable = self.typed(cx).is_some();
        let popup = dialog_popup("category.name.popup", title.clone(), cx)
            .child(dialog_title("category.name.title", cx).child(title))
            .child(
                div().pt(theme.space(2.)).child(
                    Field::new("category.name.input", &self.input)
                        .label(t!("category-name"))
                        .description(t!("category-name-limit", max = MAX_NAME)),
                ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        Button::new("category.name.cancel")
                            .variant(ButtonVariant::Outline)
                            .label(t!("category-cancel"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.name.close(window, cx);
                            })),
                    )
                    .child(
                        Button::new("category.name.save")
                            .variant(ButtonVariant::Default)
                            .label(action)
                            .disabled(!usable)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            );
        let element = dialog("category.name", &self.name, popup, window, cx);
        if self.focus_input && self.name.is_open() {
            self.focus_input = false;
            let focus = self.input.read(cx).focus_handle(cx);
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        element
    }

    fn delete_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let name = self
            .deleting
            .as_ref()
            .map(|(_, name)| name.clone())
            .unwrap_or_default();
        let popup = dialog_popup("category.delete.popup", t!("category-delete-title"), cx)
            .child(
                dialog_title("category.delete.title", cx)
                    .child(t!("category-delete-heading", name = name.to_string())),
            )
            .child(
                dialog_description("category.delete.description", cx)
                    .child(t!("category-delete-description")),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        alert_dialog_cancel("category.delete.cancel")
                            .variant(ButtonVariant::Outline)
                            .label(t!("category-cancel")),
                    )
                    .child(
                        alert_dialog_action("category.delete.confirm")
                            .variant(ButtonVariant::Destructive)
                            .label(t!("category-delete")),
                    ),
            );
        let view = cx.entity().downgrade();
        alert_dialog("category.delete", &self.delete, popup, window, cx).on_ok(move |_, _, cx| {
            view.update(cx, |this, cx| {
                if let Some((category_id, _)) = this.deleting.take() {
                    cx.emit(CategoryEvent::Delete { category_id });
                }
            })
            .ok();
            true
        })
    }
}

impl Render for CategoryDialogs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(self.name_dialog(window, cx))
            .child(self.delete_dialog(window, cx))
    }
}

/// `typed` trimmed, when it is a name the server takes: not empty and at
/// most [`MAX_NAME`] characters.
pub fn valid_name(typed: &str) -> Option<String> {
    let name = typed.trim();
    (!name.is_empty() && name.chars().count() <= MAX_NAME).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_capped() {
        assert_eq!(valid_name("  Projects "), Some("Projects".into()));
        assert_eq!(valid_name("   "), None);
        assert_eq!(
            valid_name(&"x".repeat(MAX_NAME)),
            Some("x".repeat(MAX_NAME))
        );
        assert_eq!(valid_name(&"x".repeat(MAX_NAME + 1)), None);
    }
}
