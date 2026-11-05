//! "Forward to …" (#213): after the switcher picks where a message goes,
//! an optional comment to send above the quoted card, as the webapp asks.

use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, Focusable as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div,
};

use crate::t;
use crate::ui::{
    button::{Button, ButtonVariant},
    dialog::{DialogHandle, dialog_description, dialog_title},
    input::{Input, InputEvent, InputState},
    theme::UiTheme,
};

/// The forward the user confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardEvent {
    pub post: SharedString,
    pub to: SharedString,
    pub comment: String,
}

/// Where a forward goes, while the dialog asks for a comment.
struct Target {
    post: SharedString,
    to: SharedString,
    name: SharedString,
}

/// The forward dialog's body; the window wraps it in `dialog`.
pub struct ForwardView {
    comment: Entity<InputState>,
    handle: DialogHandle,
    target: Option<Target>,
    focus_comment: bool,
    _subscription: gpui_kit::Subscription,
}

impl EventEmitter<ForwardEvent> for ForwardView {}

impl ForwardView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let comment =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("forward-comment-placeholder")));
        let subscription = cx.subscribe_in(
            &comment,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.send(window, cx);
                }
                cx.notify();
            },
        );
        Self {
            comment,
            handle: DialogHandle::new(false),
            target: None,
            focus_comment: false,
            _subscription: subscription,
        }
    }

    /// The dialog's open state, for the window's `dialog` wrapper.
    pub fn handle(&self) -> &DialogHandle {
        &self.handle
    }

    /// Opens the dialog for forwarding `post` to the channel `to`, called
    /// `name`, with an empty comment.
    pub fn open(
        &mut self,
        post: SharedString,
        to: SharedString,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.target = Some(Target { post, to, name });
        self.comment
            .update(cx, |comment, cx| comment.set_value("", window, cx));
        self.handle.open(window, cx);
        self.focus_comment = true;
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Target { post, to, .. }) = self.target.take() else {
            return;
        };
        let comment = self.comment.read(cx).value().trim().to_owned();
        self.handle.close(window, cx);
        cx.emit(ForwardEvent { post, to, comment });
    }
}

/// The post a forward sends: the comment, then the permalink the server
/// turns into the quoted card, as the webapp writes it.
pub fn message(comment: &str, link: &str) -> String {
    let comment = comment.trim();
    if comment.is_empty() {
        link.to_owned()
    } else {
        format!("{comment}\n{link}")
    }
}

impl Render for ForwardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        if self.focus_comment {
            self.focus_comment = false;
            let focus = self.comment.read(cx).focus_handle(cx);
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        let title = self.target.as_ref().map_or_else(
            || t!("forward-title-unknown"),
            |target| t!("forward-title", name = target.name.to_string()),
        );
        div()
            .flex()
            .flex_col()
            .gap(theme.space(2.))
            .child(dialog_title("forward.title", cx).child(title))
            .child(dialog_description("forward.description", cx).child(t!("forward-description")))
            .child(Input::new(&self.comment).aria_label(t!("forward-comment")))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        // A plain button: `dialog_close` is the corner X,
                        // which drew nothing here, so there was no Cancel.
                        Button::new("forward.cancel")
                            .variant(ButtonVariant::Outline)
                            .label(t!("forward-cancel"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.handle.close(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("forward.send")
                            .label(t!("forward-send"))
                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comment_goes_above_the_link() {
        assert_eq!(message("", "https://x/pl/1"), "https://x/pl/1");
        assert_eq!(message("  ", "https://x/pl/1"), "https://x/pl/1");
        assert_eq!(message(" Look ", "https://x/pl/1"), "Look\nhttps://x/pl/1");
    }
}
