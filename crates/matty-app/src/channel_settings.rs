//! Editing a channel from its header menu (PLAN §3 M1 channel management):
//! rename, header and purpose share one small dialog.

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div,
};

use crate::style::{self, TypeStyleExt as _};
use crate::ui::{
    button::{Button, ButtonVariant},
    dialog::{DialogHandle, dialog_description, dialog_title},
    field::Field,
    input::{InputEvent, InputState},
    theme::UiTheme,
};

/// Which channel text the dialog edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelField {
    Name,
    Header,
    Purpose,
}

impl ChannelField {
    pub fn title(self) -> &'static str {
        match self {
            Self::Name => "Rename channel",
            Self::Header => "Edit header",
            Self::Purpose => "Edit purpose",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Name => "Everyone sees the new name; the address stays the same.",
            Self::Header => "Shown next to the channel name, for a topic or useful links.",
            Self::Purpose => "Shown in Browse channels and when the channel is empty.",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Header => "Header",
            Self::Purpose => "Purpose",
        }
    }

    /// Mattermost's limits.
    pub fn max_len(self) -> usize {
        match self {
            Self::Name => 64,
            Self::Header => 1024,
            Self::Purpose => 250,
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::Name => "Channel name",
            Self::Header => "What is going on this week",
            Self::Purpose => "What this channel is for",
        }
    }
}

/// What the dialog decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelEditEvent {
    Save {
        field: ChannelField,
        value: SharedString,
    },
}

/// The rename / header / purpose dialog body.
pub struct ChannelEditView {
    field: ChannelField,
    input: Entity<InputState>,
    handle: DialogHandle,
    focus_input: bool,
    _subscription: gpui_kit::Subscription,
}

impl EventEmitter<ChannelEditEvent> for ChannelEditView {}

impl ChannelEditView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.save(window, cx);
                } else {
                    cx.notify();
                }
            });
        Self {
            field: ChannelField::Header,
            input,
            handle: DialogHandle::new(false),
            focus_input: false,
            _subscription: subscription,
        }
    }

    pub fn handle(&self) -> &DialogHandle {
        &self.handle
    }

    pub fn field(&self) -> ChannelField {
        self.field
    }

    /// Opens the dialog on `field` with its current value selected for
    /// typing over.
    pub fn open(
        &mut self,
        field: ChannelField,
        current: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.field = field;
        self.input.update(cx, |input, cx| {
            input.set_value(current, window, cx);
            input.set_placeholder(field.placeholder(), window, cx);
        });
        self.handle.open(window, cx);
        self.focus_input = true;
        cx.notify();
    }

    fn value(&self, cx: &App) -> String {
        self.input.read(cx).value().trim().to_string()
    }

    fn can_save(&self, cx: &App) -> bool {
        let value = self.value(cx);
        let len = value.chars().count();
        len <= self.field.max_len() && (self.field != ChannelField::Name || len >= 2)
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_save(cx) {
            return;
        }
        let value: SharedString = self.value(cx).into();
        self.handle.close(window, cx);
        cx.emit(ChannelEditEvent::Save {
            field: self.field,
            value,
        });
    }
}

impl Render for ChannelEditView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        if self.focus_input {
            self.focus_input = false;
            let focus = self.input.read(cx).focus_handle(cx);
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        let field = self.field;
        let len = self.value(cx).chars().count();
        let over = len > field.max_len();
        let count = format!("{len} / {}", field.max_len());
        let mut input = Field::new("channel.edit.input", &self.input).label(field.label());
        if over {
            input = input.error(format!("At most {} characters.", field.max_len()));
        } else {
            input = input.description(count);
        }
        div()
            .flex()
            .flex_col()
            .gap(theme.space(4.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space(1.))
                    .child(dialog_title("channel.edit.title", cx).child(field.title()))
                    .child(
                        dialog_description("channel.edit.description", cx)
                            .child(field.description()),
                    ),
            )
            .child(input)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pt(theme.space(2.))
                    .child(
                        div()
                            .type_style(style::META, &theme)
                            .text_color(theme.colors.muted_foreground)
                            .child("Enter to save"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(theme.space(2.))
                            .child(
                                Button::new("channel.edit.cancel")
                                    .variant(ButtonVariant::Outline)
                                    .child("Cancel")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.handle.close(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("channel.edit.save")
                                    .variant(ButtonVariant::Default)
                                    .disabled(!self.can_save(cx))
                                    .child("Save")
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.save(window, cx)),
                                    ),
                            ),
                    ),
            )
    }
}
