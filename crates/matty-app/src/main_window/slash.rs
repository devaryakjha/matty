//! The main window's side of slash commands (#210): it asks sync for
//! suggestions and runs commands for its channel and thread composers,
//! does what Matty's own commands ask of the window, and hands sync's
//! answers back to the composers.

use std::sync::Arc;

use gpui_kit::{Context, Window};

use super::{MainWindow, preferences::collapse_previews_preference};
use crate::slash::Request;

impl MainWindow {
    /// Does what a composer's slash command asks.
    pub(super) fn slash(&mut self, request: &Request, window: &mut Window, cx: &mut Context<Self>) {
        match request {
            Request::Search(terms) => self.search(terms.clone().into(), window, cx),
            Request::SignOut => self.sign_out(window, cx),
            Request::CollapsePreviews(collapsed) => {
                let collapsed = *collapsed;
                self.set_synced(
                    |settings| settings.collapse_previews = collapsed,
                    |settings| collapse_previews_preference(settings.collapse_previews),
                    cx,
                );
            }
            request => {
                if let Some(live) = &self.live
                    && let Some(command) = crate::slash::sync_command(live, request)
                {
                    live.send(command);
                }
            }
        }
    }

    /// Hands sync's answers about commands to the composers, and keeps an
    /// answer's ephemeral text as an ephemeral message.
    pub(super) fn slash_change(
        &mut self,
        change: &matty_sync::ChangeSet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(
            change,
            matty_sync::ChangeSet::CommandSuggestions { .. }
                | matty_sync::ChangeSet::CommandRan { .. }
                | matty_sync::ChangeSet::Failed(matty_sync::CommandError::Offline {
                    action: "run command"
                })
        ) {
            return;
        }
        self.channel
            .update(cx, |view, cx| view.slash_change(change, window, cx));
        self.right_pane
            .update(cx, |pane, cx| pane.slash_change(change, window, cx));
        if let Some(post) = crate::slash::ephemeral(change)
            && let Some(live) = self.live.as_mut()
        {
            let update = live.apply(&matty_sync::ChangeSet::Ephemeral(Arc::new(post)));
            self.apply_update(update, window, cx);
        }
    }
}
