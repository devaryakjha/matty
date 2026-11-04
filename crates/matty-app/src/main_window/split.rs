//! Split view (#222, PLAN §3 M3): a second channel beside the open one, in
//! the main window. The second pane is a [`PopOutView`] hosted in the
//! window ([`Host::Split`](crate::popout::Host)): its own channel view,
//! composer and sync fork, as a pop-out has.
//!
//! One pane has focus at a time. Sidebar clicks open in it, the sidebar
//! marks its channel, and a channel counts as read only while it shows in
//! the focused pane of a focused window. Threads from either pane open in
//! the right pane. The second channel and the divider's place are saved per
//! server and come back at launch.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px,
};

use super::{CONTENT_MIN, MainWindow, SPLIT_HANDLE, Seen, metrics, scaled};
use crate::actions::{ClosePane, FocusOtherPane, OpenInSplitView, SwapPanes};
use crate::live::Live;
use crate::popout::{Ask, PaneEvent, PopOutView};
use crate::right_pane::RightPaneContent;
use crate::settings::SplitPlace;
use crate::ui::{
    resizable::{PaneLimits, Resizable},
    theme::UiTheme,
};

/// One of split view's two panes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Pane {
    /// The main window's own channel view, on the left.
    #[default]
    Main,
    /// The second channel, on the right.
    Second,
}

/// The main window's split view state.
pub(super) struct Split {
    pane: Option<Entity<PopOutView>>,
    focused: Pane,
    main_focus: FocusHandle,
    second_focus: FocusHandle,
    /// The server whose saved split was looked at, so it opens once.
    restored: Option<String>,
    _pane_events: Option<Subscription>,
}

impl Split {
    pub(super) fn new(window: &mut Window, cx: &mut Context<MainWindow>) -> Self {
        let main_focus = cx.focus_handle();
        let second_focus = cx.focus_handle();
        // Tab, a click or a shortcut moving focus into a pane makes it the
        // focused one.
        cx.on_focus_in(&main_focus, window, |this, window, cx| {
            this.focus_pane(Pane::Main, window, cx)
        })
        .detach();
        cx.on_focus_in(&second_focus, window, |this, window, cx| {
            this.focus_pane(Pane::Second, window, cx)
        })
        .detach();
        Self {
            pane: None,
            focused: Pane::Main,
            main_focus,
            second_focus,
            restored: None,
            _pane_events: None,
        }
    }

    /// Whether the main pane is the one with focus; always, without a split.
    pub(super) fn main_focused(&self) -> bool {
        self.pane.is_none() || self.focused == Pane::Main
    }
}

impl MainWindow {
    /// The split's second pane when it shows: not over Threads or Drafts.
    fn shown_split(&self) -> Option<&Entity<PopOutView>> {
        self.split
            .pane
            .as_ref()
            .filter(|_| !self.threads_open && !self.drafts_open && self.login.is_none())
    }

    /// Whether the second pane shows and has focus.
    fn second_focused(&self) -> bool {
        self.shown_split().is_some() && self.split.focused == Pane::Second
    }

    /// The server a split is saved under; empty in the preview.
    fn split_server(&self) -> String {
        self.live
            .as_ref()
            .map(|live| live.server().to_owned())
            .unwrap_or_default()
    }

    /// Split view's keys, on the window's root.
    pub(super) fn on_split_actions(root: gpui_kit::Div, cx: &mut Context<Self>) -> gpui_kit::Div {
        root.on_action(
            cx.listener(|this, _: &OpenInSplitView, window, cx| this.split_from_keys(window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &FocusOtherPane, window, cx| this.focus_other_pane(window, cx)),
        )
        .on_action(cx.listener(|this, _: &SwapPanes, window, cx| this.swap_panes(window, cx)))
        .on_action(
            cx.listener(|this, _: &ClosePane, window, cx| this.close_focused_pane(window, cx)),
        )
    }

    /// Cmd/Ctrl+\: with a split, focus its second pane; otherwise open the
    /// channel before this one beside it, or this one when there is none.
    fn split_from_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shown_split().is_some() {
            return self.focus_pane_composer(Pane::Second, window, cx);
        }
        let Some(current) = self.selected_channel.clone() else {
            return;
        };
        let previous = self.history[..self.history_at.min(self.history.len())]
            .iter()
            .rev()
            .find(|id| **id != current)
            .cloned();
        self.open_in_split(previous.unwrap_or(current), window, cx);
    }

    /// Opens `id` in the second pane, starting split view when there is
    /// none, and puts focus there.
    pub(super) fn open_in_split(
        &mut self,
        id: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.login.is_some() {
            return;
        }
        // Split view sits beside a channel, not Threads or Drafts.
        if (self.threads_open || self.drafts_open)
            && let Some(main) = self.selected_channel.clone()
        {
            self.show_channel(main, window, cx);
        }
        let same = self
            .split
            .pane
            .as_ref()
            .is_some_and(|pane| *pane.read(cx).channel_id() == id);
        if !same {
            self.mount_split(id.clone(), window, cx);
            self.save_split(cx);
            // Opening a channel views it, as the main pane's does.
            self.view_channels(std::slice::from_ref(&id));
            self.notifier.withdraw(&id);
        }
        self.focus_pane_composer(Pane::Second, window, cx);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.select(Some(id), cx));
    }

    /// A sidebar click, a link or the switcher: the channel opens in the
    /// focused pane.
    pub(super) fn open_in_focused_pane(
        &mut self,
        id: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.second_focused() {
            self.open_in_split(id, window, cx);
        } else {
            self.open_channel(id, window, cx);
        }
    }

    /// Puts the caret in the focused pane's composer (Cmd/Ctrl+Shift+L, or
    /// after a sidebar click).
    pub(super) fn focus_composer_in_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = if self.second_focused() {
            Pane::Second
        } else {
            Pane::Main
        };
        self.focus_pane_composer(pane, window, cx);
    }

    fn focus_pane_composer(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        match (pane, self.split.pane.clone()) {
            (Pane::Second, Some(second)) => {
                second.update(cx, |second, cx| second.focus_composer(window, cx))
            }
            _ => self
                .channel
                .update(cx, |channel, cx| channel.focus_composer(window, cx)),
        }
        self.focus_pane(pane, window, cx);
    }

    /// Makes `id` the second pane's channel, with a fresh fork of sync.
    fn mount_split(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let fork = self.live.as_ref().map(Live::fork);
        let server = self.split_server();
        let pane = cx.new(|cx| PopOutView::in_split(id, server, fork, window, cx));
        pane.update(cx, |pane, cx| pane.set_split_controls(true, cx));
        self.channel
            .update(cx, |channel, cx| channel.set_split_controls(true, cx));
        self.split._pane_events = Some(cx.subscribe_in(&pane, window, Self::split_pane_event));
        // The old pane, if any, closes its channel with sync as it goes.
        self.split.pane = Some(pane);
        cx.notify();
    }

    /// Ends split view. `forget` drops its saved place, as the user closing
    /// it does; signing out keeps it for next time.
    fn close_split(&mut self, forget: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.split.pane.take() else {
            return;
        };
        self.split._pane_events = None;
        let had_focus = pane.read(cx).contains_focus(window, cx);
        drop(pane);
        self.split.focused = Pane::Main;
        self.channel
            .update(cx, |channel, cx| channel.set_split_controls(false, cx));
        if forget {
            self.save_split(cx);
        }
        if !self.threads_open && !self.drafts_open {
            let selected = self.selected_channel.clone();
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.highlight(selected, cx));
        }
        if had_focus {
            self.channel
                .update(cx, |channel, cx| channel.focus_composer(window, cx));
        }
        cx.notify();
    }

    /// Cmd/Ctrl+Alt+W: closes the focused pane, and the other one fills
    /// the window.
    fn close_focused_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shown_split().is_none() {
            return;
        }
        match self.split.focused {
            Pane::Second => self.close_split(true, window, cx),
            Pane::Main => self.close_main_pane(window, cx),
        }
    }

    /// The main pane's "Close pane": the second channel moves into it.
    pub(super) fn close_main_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(second) = self
            .split
            .pane
            .as_ref()
            .map(|pane| pane.read(cx).channel_id().clone())
        else {
            return;
        };
        self.close_split(true, window, cx);
        self.open_channel(second, window, cx);
        self.channel
            .update(cx, |channel, cx| channel.focus_composer(window, cx));
    }

    /// Cmd/Ctrl+Alt+\ or the header's "Swap panes": the two channels trade
    /// places, and focus stays on the same side.
    pub(super) fn swap_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(second), Some(main)) = (
            self.shown_split()
                .map(|pane| pane.read(cx).channel_id().clone()),
            self.selected_channel.clone(),
        ) else {
            return;
        };
        if second == main {
            return;
        }
        let focused = self.split.focused;
        self.mount_split(main, window, cx);
        self.open_channel(second, window, cx);
        self.save_split(cx);
        self.split.focused = Pane::Main;
        self.focus_pane_composer(focused, window, cx);
    }

    /// F6: focus moves to the other pane's composer.
    fn focus_other_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shown_split().is_none() {
            return;
        }
        let other = match self.split.focused {
            Pane::Main => Pane::Second,
            Pane::Second => Pane::Main,
        };
        self.focus_pane_composer(other, window, cx);
    }

    /// Makes `pane` the focused one: the sidebar marks its channel, and
    /// its channel counts as read if the user can see its newest post.
    fn focus_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        if self.shown_split().is_none() || self.split.focused == pane {
            return;
        }
        self.split.focused = pane;
        let shown = match pane {
            Pane::Main => self.selected_channel.clone(),
            Pane::Second => self
                .split
                .pane
                .as_ref()
                .map(|pane| pane.read(cx).channel_id().clone()),
        };
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.highlight(shown, cx));
        match pane {
            Pane::Main => self.view_if_seen(window, cx),
            Pane::Second => self.view_split_if_seen(window, cx),
        }
        cx.notify();
    }

    /// Views the second pane's channel when it has unread posts the user
    /// can see: the window and the pane have focus, nothing covers it, and
    /// it shows its newest post (#145).
    pub(super) fn view_split_if_seen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.split.pane.as_ref() else {
            return;
        };
        let pane = pane.read(cx);
        let id = pane.channel_id().clone();
        let seen = Seen {
            focused: window.is_window_active() && self.split.focused == Pane::Second,
            channel_shown: !self.threads_open && !self.drafts_open,
            covered: self.right_drawer_shown || self.sidebar_drawer_shown,
            at_newest: pane.at_newest(cx),
            unread: self.sidebar.read(cx).is_unread(&id),
            kept_unread: pane.kept_unread(),
        };
        if seen.views() {
            self.view_channels(std::slice::from_ref(&id));
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.mark_read(&id, cx));
        }
    }

    /// The notification policy's view of what the user sees: the focused
    /// pane's channel.
    pub(super) fn split_seen(
        &self,
        app: matty_notify::AppState,
        cx: &gpui_kit::App,
    ) -> matty_notify::AppState {
        match self.shown_split().filter(|_| self.second_focused()) {
            Some(pane) => matty_notify::AppState {
                visible_channel: Some(pane.read(cx).channel_id().to_string()),
                ..app
            },
            None => app,
        }
    }

    /// Opens again the split saved for the shown server, once, when its
    /// sidebar is in (or at start in the preview). A split of another
    /// server goes, keeping its place.
    pub(super) fn restore_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.live.is_none() && !crate::preview::enabled() {
            return;
        }
        let server = self.split_server();
        if let Some(pane) = &self.split.pane {
            if pane.read(cx).server() == server {
                return;
            }
            self.close_split(false, window, cx);
        }
        if self.split.restored.as_deref() == Some(server.as_str()) {
            return;
        }
        self.split.restored = Some(server.clone());
        let saved = self
            .settings
            .read(cx)
            .get()
            .splits
            .iter()
            .find(|place| place.server == server)
            .map(|place| SharedString::from(place.channel_id.clone()));
        if let Some(id) = saved {
            self.mount_split(id, window, cx);
        }
    }

    /// Saves the second pane's channel for this server, or forgets it.
    fn save_split(&mut self, cx: &mut Context<Self>) {
        let server = self.split_server();
        let channel = self
            .split
            .pane
            .as_ref()
            .map(|pane| pane.read(cx).channel_id().to_string());
        self.settings.update(cx, |store, cx| {
            store.update(cx, |settings| {
                let at = settings
                    .splits
                    .iter()
                    .position(|place| place.server == server);
                match (channel, at) {
                    (Some(channel_id), Some(at)) => settings.splits[at].channel_id = channel_id,
                    (Some(channel_id), None) => settings.splits.push(SplitPlace {
                        server,
                        channel_id,
                        ratio: 0.5,
                    }),
                    (None, Some(at)) => {
                        settings.splits.remove(at);
                    }
                    (None, None) => {}
                }
            })
        });
    }

    /// What the second pane asks of the main window.
    fn split_pane_event(
        &mut self,
        pane: &Entity<PopOutView>,
        event: &PaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let channel = pane.read(cx).channel_id().clone();
        match event {
            PaneEvent::Ask(Ask::Show {
                channel: shown,
                post,
            }) if *shown == channel => {
                if let Some(post) = post.clone() {
                    pane.update(cx, |pane, cx| pane.jump_to(post, cx));
                }
            }
            // Its channel's pinned list and info, not the main pane's.
            PaneEvent::Ask(Ask::Pinned(id)) if *id == channel => self.show_split_pinned(pane, cx),
            PaneEvent::Ask(Ask::Info(id)) if *id == channel => self.show_split_info(pane, cx),
            PaneEvent::Ask(Ask::Show { channel, post }) => {
                // Elsewhere in Matty opens where the user is: this pane.
                self.open_in_split(channel.clone(), window, cx);
                if let (Some(post), Some(pane)) = (post.clone(), self.split.pane.clone()) {
                    pane.update(cx, |pane, cx| pane.jump_to(post, cx));
                }
            }
            PaneEvent::Ask(Ask::Forward { post, .. }) => {
                let post = post.clone();
                let switcher = self.switcher.clone();
                window.on_next_frame(move |window, cx| {
                    switcher.update(cx, |switcher, cx| switcher.open_forward(post, window, cx));
                });
            }
            PaneEvent::Ask(ask) => self.answer_ask(ask.clone(), window, cx),
            PaneEvent::OpenThread(root) => self.open_split_thread(pane, root.clone(), cx),
            PaneEvent::CheckSeen => self.view_split_if_seen(window, cx),
            PaneEvent::Swap => self.swap_panes(window, cx),
            PaneEvent::ClosePane => self.close_split(true, window, cx),
            PaneEvent::Closed => {
                // Signed out: the split comes back with the next sign-in.
                self.close_split(false, window, cx);
                self.split.restored = None;
            }
            PaneEvent::MarkRead => {
                if self.right_pane_open {
                    self.right_pane_open = false;
                    return cx.notify();
                }
                self.view_channels(std::slice::from_ref(&channel));
                self.sidebar
                    .update(cx, |sidebar, cx| sidebar.mark_read(&channel, cx));
            }
        }
    }

    /// The thread under `root` from the second pane, in the right pane.
    fn open_split_thread(
        &mut self,
        pane: &Entity<PopOutView>,
        root: SharedString,
        cx: &mut Context<Self>,
    ) {
        let pane = pane.read(cx);
        let channel_id = pane.channel_id().clone();
        let profiles = pane
            .channel_view()
            .and_then(|view| view.read(cx).model().map(|model| model.profiles.clone()))
            .unwrap_or_default();
        self.right_pane.update(cx, |right, cx| {
            right.set_profiles(profiles, cx);
            right.set_typing(Vec::new(), cx);
        });
        if let Some(live) = &mut self.live {
            live.open_thread(&root);
            self.right_pane
                .update(cx, |right, cx| right.set_content(None, cx));
        } else {
            let thread = crate::preview::enabled()
                .then(|| crate::preview::thread(&channel_id, &root))
                .flatten();
            self.right_pane.update(cx, |right, cx| {
                right.set_content(
                    thread.map(|thread| RightPaneContent::Thread(Box::new(thread))),
                    cx,
                );
            });
        }
        self.right_pane_open = true;
        cx.notify();
    }

    fn show_split_pinned(&mut self, pane: &Entity<PopOutView>, cx: &mut Context<Self>) {
        let Some(channel) = pane
            .read(cx)
            .channel_view()
            .and_then(|view| view.read(cx).model().cloned())
        else {
            return;
        };
        let posts = if crate::preview::enabled() {
            crate::preview::pinned(&channel)
        } else {
            vec![]
        };
        self.show_listing(
            RightPaneContent::Pinned {
                channel_name: channel.name.clone(),
                posts,
            },
            cx,
        );
        if let Some(live) = self.live.as_mut() {
            live.show_pinned(&channel.id);
            self.right_pane
                .update(cx, |right, cx| right.set_loading(true, cx));
        }
    }

    fn show_split_info(&mut self, pane: &Entity<PopOutView>, cx: &mut Context<Self>) {
        self.leave_thread();
        let info = pane
            .read(cx)
            .channel_view()
            .and_then(|view| view.read(cx).model().cloned());
        self.right_pane.update(cx, |right, cx| {
            right.set_content(info.map(RightPaneContent::ChannelInfo), cx)
        });
        self.right_pane_open = true;
        cx.notify();
    }

    /// The channel view, or both panes side by side with gpuicn's
    /// Resizable between them. The divider's place is kept as the first
    /// pane's share, so it holds as the window changes size.
    pub(super) fn channel_area(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(second) = self.shown_split().cloned() else {
            return self.channel.clone().into_any_element();
        };
        let theme = UiTheme::read(cx).clone();
        let settings = self.settings.read(cx).get().clone();
        let server = self.split_server();
        let ratio = settings
            .splits
            .iter()
            .find(|place| place.server == server)
            .map_or(0.5, |place| place.ratio);
        let rail = if self.rail.read(cx).is_multi() {
            f32::from(scaled(&theme, metrics::RAIL_WIDTH))
        } else {
            0.
        };
        let sidebar = if self.sidebar_visible && !Self::narrow_sidebar(window) {
            settings.sidebar_width + SPLIT_HANDLE
        } else {
            0.
        };
        let right = if self.right_pane_open && !Self::narrow_right_pane(window) {
            settings.right_pane_width + SPLIT_HANDLE
        } else {
            0.
        };
        let extent =
            (f32::from(window.viewport_size().width) - rail - sidebar - right - SPLIT_HANDLE)
                .max(2. * CONTENT_MIN);
        let main_name = self
            .channel
            .read(cx)
            .model()
            .map(|model| model.name.clone());
        let second_name = second
            .read(cx)
            .channel_view()
            .and_then(|view| view.read(cx).model().map(|model| model.name.clone()));
        let main = self.pane_frame(
            Pane::Main,
            main_name,
            self.channel.clone().into_any_element(),
            cx,
        );
        let second = self.pane_frame(Pane::Second, second_name, second.into_any_element(), cx);
        let store = self.settings.clone();
        Resizable::new(
            "main.channel-split",
            "Resize split view",
            px(extent * ratio),
        )
        .first(main)
        .second(second)
        .first_limits(PaneLimits::new(px(CONTENT_MIN), px(100_000.)))
        .second_limits(PaneLimits::new(px(CONTENT_MIN), px(100_000.)))
        .on_resize(move |width: &Pixels, _, cx| {
            let ratio = (f32::from(*width) / extent).clamp(0.1, 0.9);
            let server = server.clone();
            store.update(cx, |store, cx| {
                store.update(cx, |settings| {
                    if let Some(place) = settings
                        .splits
                        .iter_mut()
                        .find(|place| place.server == server)
                    {
                        place.ratio = ratio;
                    }
                })
            });
        })
        .into_any_element()
    }

    /// One pane of the split: a line along its top in the focus color
    /// while it has focus, and a name that says so to screen readers.
    fn pane_frame(
        &self,
        which: Pane,
        name: Option<SharedString>,
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = UiTheme::read(cx);
        let focused = self.split.focused == which;
        let (id, focus, side) = match which {
            Pane::Main => ("split.main", &self.split.main_focus, "Left pane"),
            Pane::Second => ("split.second", &self.split.second_focus, "Right pane"),
        };
        let label = match (name, focused) {
            (Some(name), true) => format!("{side}, {name}, focused"),
            (Some(name), false) => format!("{side}, {name}"),
            (None, true) => format!("{side}, focused"),
            (None, false) => side.to_string(),
        };
        let line = if focused {
            theme.colors.ring
        } else {
            gpui_kit::Rgba {
                a: 0.,
                ..theme.colors.ring
            }
        };
        div()
            .id(id)
            .test_support()
            .track_focus(focus)
            .role(gpui_kit::Role::Group)
            .aria_label(label)
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .border_t_2()
            .border_color(line)
            .capture_any_mouse_down(
                cx.listener(move |this, _, window, cx| this.focus_pane(which, window, cx)),
            )
            .child(div().flex_1().min_h_0().w_full().child(content))
            .into_any_element()
    }
}

/// What the crate's integration tests read of split view.
impl MainWindow {
    /// Whether the sidebar marks `id` unread.
    pub(crate) fn is_unread_for_tests(&self, id: &str, cx: &gpui_kit::App) -> bool {
        self.sidebar.read(cx).is_unread(id)
    }

    /// The second pane's channel and the bodies of its posts.
    pub(crate) fn split_for_tests(&self, cx: &gpui_kit::App) -> Option<(String, Vec<String>)> {
        let pane = self.split.pane.as_ref()?.read(cx);
        let posts = pane
            .channel_view()
            .and_then(|view| view.read(cx).model())
            .map(|model| {
                model
                    .posts
                    .iter()
                    .map(|post| post.body.to_string())
                    .collect()
            })
            .unwrap_or_default();
        Some((pane.channel_id().to_string(), posts))
    }
}
