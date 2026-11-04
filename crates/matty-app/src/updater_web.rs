//! The updater on the web: there is nothing to install, because the page
//! always loads the latest deployed build. Same API as `updater.rs`, so the
//! windows that start it compile unchanged.

use gpui_kit::{App, AppContext as _, Entity, Window};

use crate::ui::{composer::ComposerState, toast::ToastState};

/// Stands in for the desktop's update prompt and never shows.
pub(crate) struct UpdatePrompt;

impl UpdatePrompt {
    pub(crate) fn start(
        _toasts: Entity<ToastState>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|_| Self)
    }
}

/// Nothing waits on composers on the web.
pub(crate) fn watch_composer(_composer: &Entity<ComposerState>, _cx: &mut App) {}
