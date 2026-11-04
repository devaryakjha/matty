//! A pop-out's or split pane's side of slash commands (#210): sync is its
//! own fork, and what needs the main window (search, signing out,
//! preferences) is asked of it.

use std::sync::Arc;

use gpui_kit::{Context, Window};

use super::{Ask, Content, PopOutView};
use crate::slash::Request;

impl PopOutView {
    /// Does what the composer's slash command asks.
    pub(super) fn slash(&mut self, request: &Request, cx: &mut Context<Self>) {
        match self
            .live
            .as_ref()
            .map(|live| (live, crate::slash::sync_command(live, request)))
        {
            Some((live, Some(command))) => live.send(command),
            Some((_, None)) => {
                if matches!(
                    request,
                    Request::Search(_) | Request::SignOut | Request::CollapsePreviews(_)
                ) {
                    self.ask(Ask::Slash(request.clone()), cx);
                }
            }
            None => {}
        }
    }

    /// Hands sync's answers about commands to the composer, and keeps an
    /// answer's ephemeral text as an ephemeral message.
    pub(super) fn slash_change(
        &mut self,
        change: &matty_sync::ChangeSet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &self.content {
            Content::Channel(view) => {
                view.update(cx, |view, cx| view.slash_change(change, window, cx))
            }
            Content::Thread(pane) => {
                pane.update(cx, |pane, cx| pane.slash_change(change, window, cx))
            }
        }
        if let Some(post) = crate::slash::ephemeral(change)
            && let Some(live) = self.live.as_mut()
        {
            let update = live.apply(&matty_sync::ChangeSet::Ephemeral(Arc::new(post)));
            self.apply_update(update, window, cx);
        }
    }
}
