//! Preferences' notification sound (#142): gpuicn's select over the web
//! app's sound names, saved to the user's `desktop_notification_sound` so
//! the web app and Matty agree. Choosing one plays it.

use std::collections::HashMap;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement as _, ParentElement as _, SharedString,
    Styled as _, Window, div,
};
use matty_notify::Sound;

use super::MainWindow;
use crate::style::{metrics, scaled};
use crate::ui::select::{Select, SelectEvent, SelectItem, SelectState};
use crate::ui::theme::UiTheme;

/// The prop that names the sound.
const PROP: &str = "desktop_notification_sound";

/// The sound `props` names, as the select's value.
fn chosen(props: Option<&HashMap<String, String>>) -> SharedString {
    let value = props.and_then(|props| props.get(PROP)).map(String::as_str);
    Sound::from_prop(value).prop().into()
}

/// The picker, showing what `props` chose, and saving and playing what the
/// user picks.
pub(super) fn picker(
    props: Option<&HashMap<String, String>>,
    window: &mut Window,
    cx: &mut Context<MainWindow>,
) -> Entity<SelectState> {
    let picker = cx.new(|cx| {
        let items = Sound::ALL.map(|sound| SelectItem::new(sound.prop(), sound.prop()));
        let mut state = SelectState::new(items, window, cx);
        state.set_value(Some(chosen(props)), window, cx);
        state
    });
    cx.subscribe(&picker, |this, _, event: &SelectEvent, cx| {
        let SelectEvent::Change(Some(value)) = event else {
            return;
        };
        // The server's own value coming back is not a choice.
        if chosen(this.notify_props.as_ref()) == *value {
            return;
        }
        let sound = Sound::from_prop(Some(value));
        matty_notify::play_sound(sound);
        this.set_notify_prop(PROP, sound.prop(), cx);
    })
    .detach();
    picker
}

impl MainWindow {
    /// Shows the sound the server last sent, as when the web app changed it.
    pub(super) fn show_sound(&self, window: &mut Window, cx: &mut Context<Self>) {
        let value = chosen(self.notify_props.as_ref());
        self.sound.update(cx, |state, cx| {
            if state.value() != Some(&value) {
                state.set_value(Some(value), window, cx);
            }
        });
    }

    /// The row under the Sound switch; off while sounds are.
    pub(super) fn sound_row(
        &self,
        sounds_on: bool,
        labelled_row: &dyn Fn(SharedString, SharedString, gpui_kit::AnyElement) -> gpui_kit::Div,
        cx: &gpui_kit::App,
    ) -> gpui_kit::Div {
        let theme = UiTheme::read(cx);
        labelled_row(
            "Notification sound".into(),
            "Choosing one plays it.".into(),
            div()
                .flex_shrink_0()
                .w(scaled(theme, metrics::FIELD_NARROW_WIDTH))
                .child(
                    Select::new(&self.sound)
                        .aria_label("Notification sound")
                        .disabled(!sounds_on),
                )
                .into_any_element(),
        )
    }
}
