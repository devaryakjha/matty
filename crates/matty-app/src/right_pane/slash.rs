//! The reply composer's slash commands (#210); see [`crate::slash`].

use gpui_kit::{Context, SharedString, Window};

use super::{RightPaneContent, RightPaneEvent, RightPaneView};
use crate::actions::{OpenPreferences, ShowShortcuts};
use crate::slash::{Change, Line, Local, Place, Request};

impl RightPaneView {
    /// The open thread's channel and root.
    fn command_place(&self) -> Option<(SharedString, SharedString)> {
        match self.content.as_ref()? {
            RightPaneContent::Thread(thread) => {
                Some((thread.channel_id.clone(), thread.root.id.clone()))
            }
            _ => None,
        }
    }

    /// The composer shows `query` after `/`: the commands that match now,
    /// and the server is asked for its own.
    pub(super) fn slash_typing(&mut self, query: &str, cx: &mut Context<Self>) {
        let asks = self.slash.typing(query, !self.offline);
        if let Some((channel_id, root_id)) = self.command_place() {
            for user_input in asks {
                cx.emit(RightPaneEvent::Slash(Request::Suggest {
                    channel_id: channel_id.clone(),
                    root_id: root_id.clone(),
                    user_input,
                }));
            }
        }
        self.show_commands(query, cx);
    }

    fn show_commands(&self, query: &str, cx: &mut Context<Self>) {
        let rows = self.slash.suggestions(query);
        self.composer
            .update(cx, |composer, cx| composer.set_suggestions(rows, cx));
    }

    /// Runs `text` when it is a slash command; `true` when it was, and the
    /// reply is not sent.
    pub(super) fn run_slash(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match crate::slash::parse(text, Place::Thread) {
            Line::Message => false,
            Line::Local(Local::Code(block)) => {
                // A reply like any other, so it waits in the outbox offline.
                self.reply(block.into(), window, cx);
                true
            }
            Line::Local(local) => {
                self.clear_command(window, cx);
                let request = match local {
                    Local::Shortcuts => {
                        window.dispatch_action(Box::new(ShowShortcuts), cx);
                        return true;
                    }
                    Local::Settings => {
                        window.dispatch_action(Box::new(OpenPreferences), cx);
                        return true;
                    }
                    Local::Search(terms) => Request::Search(terms),
                    Local::Status(presence) => Request::Status(presence),
                    Local::CollapsePreviews(collapsed) => Request::CollapsePreviews(collapsed),
                    Local::SignOut => Request::SignOut,
                    // The channel's own dialogs: `parse` leaves these to
                    // the server in a thread.
                    Local::Leave
                    | Local::Mute
                    | Local::Header
                    | Local::Purpose
                    | Local::Code(_) => {
                        return true;
                    }
                };
                cx.emit(RightPaneEvent::Slash(request));
                true
            }
            Line::Server(command) => {
                // One at a time: Enter again while it runs does nothing.
                if self.slash.running().is_some() || self.refused_offline("run command", cx) {
                    return true;
                }
                let Some((channel_id, root_id)) = self.command_place() else {
                    return true;
                };
                self.slash.started(&command);
                cx.emit(RightPaneEvent::Slash(Request::Execute {
                    channel_id,
                    root_id,
                    command,
                }));
                cx.notify();
                true
            }
        }
    }

    /// The composer empties, as after replying.
    fn clear_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._draft_task = None;
        if let Some(cleared) = self.draft.sent(cx) {
            cx.emit(RightPaneEvent::SaveDraft(cleared));
        }
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.draft_len = 0;
        cx.notify();
    }

    /// Takes sync's answers about the open thread's commands: suggestions,
    /// and the result of the one running.
    pub fn slash_change(
        &mut self,
        change: &matty_sync::ChangeSet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((channel_id, root_id)) = self.command_place() else {
            return;
        };
        match Change::of(change, &channel_id, &root_id) {
            Some(Change::Suggested {
                user_input,
                suggestions,
            }) => {
                let (shown, next) = self.slash.suggested(user_input, suggestions);
                if let Some(user_input) = next {
                    cx.emit(RightPaneEvent::Slash(Request::Suggest {
                        channel_id,
                        root_id,
                        user_input,
                    }));
                }
                if shown && let Some(query) = self.slash.query().map(str::to_owned) {
                    self.show_commands(&query, cx);
                }
            }
            Some(Change::Ran { command, outcome }) => {
                let Some(ran) = self.slash.ran(command, outcome) else {
                    return;
                };
                let text = self.composer.read(cx).value(cx);
                if ran.done.is_some_and(|done| text.trim() == done.trim()) {
                    self.clear_command(window, cx);
                }
                if let Some(goto) = ran.goto {
                    cx.emit(RightPaneEvent::OpenLink(goto));
                }
                cx.notify();
            }
            Some(Change::Lost) if self.slash.lost() => cx.notify(),
            Some(Change::Lost) | None => {}
        }
    }
}
