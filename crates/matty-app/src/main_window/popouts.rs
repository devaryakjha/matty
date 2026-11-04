//! Channels and threads in windows of their own, from the main window
//! (#222): what opens one, and what the pop-outs ask of the main window.
//! The pop-outs themselves are [`crate::popout`].

use gpui_kit::{Context, Entity, Focusable as _, SharedString, Window};

use super::MainWindow;
use crate::popout::{Ask, Request, Requests, Target};
use crate::right_pane::{RightPaneContent, RightPaneView};

impl MainWindow {
    /// "Open in new window" on a sidebar row.
    pub(super) fn pop_out_channel(&mut self, id: SharedString, cx: &mut Context<Self>) {
        crate::popout::open(self.live.as_ref(), Target::Channel(id), cx);
    }

    /// The thread under `root` in a window of its own; `channel` is the
    /// thread's, else the open channel.
    pub(super) fn pop_out_thread(
        &mut self,
        root: SharedString,
        channel: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let Some(channel) = channel.or_else(|| self.selected_channel.clone()) else {
            return;
        };
        crate::popout::open(self.live.as_ref(), Target::Thread { root, channel }, cx);
    }

    /// The thread pane's "Open in new window": the thread moves out of the
    /// right pane into its own window.
    pub(super) fn pop_out_pane_thread(
        &mut self,
        pane: &Entity<RightPaneView>,
        root: &SharedString,
        cx: &mut Context<Self>,
    ) {
        let channel = match pane.read(cx).content() {
            Some(RightPaneContent::Thread(thread)) => Some(thread.channel_id.clone()),
            _ => None,
        };
        if *pane == self.right_pane {
            self.leave_thread();
            self.right_pane_open = false;
            cx.notify();
        }
        self.pop_out_thread(root.clone(), channel, cx);
    }

    /// Cmd/Ctrl+Shift+O: the thread when focus is in the thread pane, else
    /// the open channel.
    pub(super) fn open_in_new_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.right_pane.clone();
        if self.right_pane_open && pane.focus_handle(cx).contains_focused(window, cx) {
            let root = match pane.read(cx).content() {
                Some(RightPaneContent::Thread(thread)) => Some(thread.root.id.clone()),
                _ => None,
            };
            if let Some(root) = root {
                return self.pop_out_pane_thread(&pane, &root, cx);
            }
        }
        if self.threads_open || self.drafts_open {
            return;
        }
        if let Some(id) = self.selected_channel.clone() {
            self.pop_out_channel(id, cx);
        }
    }

    /// What a pop-out asks of the main window: going somewhere else in
    /// Matty brings this window forward there.
    pub(super) fn popout_request(
        &mut self,
        _: &Entity<Requests>,
        request: &Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shown = self.live.as_ref().map(|live| live.server().to_owned());
        if shown.unwrap_or_default() != request.server {
            tracing::info!("a pop-out of a server not shown asked the main window");
            return;
        }
        if !matches!(request.ask, Ask::Offline(_)) {
            cx.activate(true);
            window.activate_window();
        }
        match request.ask.clone() {
            Ask::Show { channel, post } => {
                self.open_channel(channel, window, cx);
                if let Some(post) = post {
                    self.channel
                        .update(cx, |channel, cx| channel.jump_to(post, cx));
                }
            }
            Ask::OpenLink(url) => self.open_message_link(url, window, cx),
            Ask::OpenDirect(display_name) => self.open_direct(display_name, window, cx),
            Ask::ViewProfile(profile) => self.show_listing(RightPaneContent::Profile(profile), cx),
            Ask::Pinned(channel) => {
                self.open_channel(channel, window, cx);
                self.show_pinned(cx);
            }
            Ask::Info(channel) => {
                self.open_channel(channel, window, cx);
                self.show_channel_info(cx);
            }
            Ask::Forward { channel, post } => {
                self.open_channel(channel, window, cx);
                let switcher = self.switcher.clone();
                window.on_next_frame(move |window, cx| {
                    switcher.update(cx, |switcher, cx| switcher.open_forward(post, window, cx));
                });
            }
            Ask::Leave(channel) => self.leave_channel(channel, window, cx),
            Ask::Archive(channel) => self.archive_channel(channel, window, cx),
            Ask::Offline(action) => self.say_offline(action, cx),
        }
    }
}
