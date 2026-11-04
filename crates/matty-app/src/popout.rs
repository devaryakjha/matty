//! Channels and threads in windows of their own (#222, PLAN §3 M3).
//!
//! "Open in new window" on a sidebar row, a post or the thread pane, or
//! Cmd/Ctrl+Shift+O, opens a [`PopOutView`]: the same channel view or
//! thread pane the main window uses, with its own composer, in a second
//! window. Each one follows sync through a [`Live::fork`], so it shares
//! the main window's engine, cache and connection and sees new posts,
//! edits, reactions and typing as they land.
//!
//! A pop-out's channel counts as read only while that window has focus
//! and shows its newest post, as the main window's does. A notification
//! about what a pop-out shows brings that window forward. Open pop-outs
//! are saved with their place and size and open again at launch, once
//! their server's sidebar is in.
//!
//! What only the main window can do (open a link, show a profile, the
//! pinned list or channel info, forward, leave) goes there as a
//! [`Request`].

use std::collections::HashSet;

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle,
    Global, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, Window, WindowBounds, WindowHandle, WindowOptions, div, point, px, size,
};

use crate::actions::{self, CloseWindow};
use crate::channel::{ChannelEvent, ChannelView};
use crate::live::{ChannelUpdate, Live};
use crate::profile::UserProfile;
use crate::right_pane::{RightPaneContent, RightPaneEvent, RightPaneView};
use crate::settings::{PopOutPlace, SettingsChanged, SettingsStore, WindowGeometry};
use crate::sidebar::ChannelKind;
use crate::style::{self, TypeStyleExt as _};
use crate::ui::{
    markdown::selection_root,
    theme::UiTheme,
    title_bar::{TitleBar, title_bar_window_options},
    toast::{Notice, ToastState},
};

/// A new pop-out's size.
const DEFAULT_SIZE: (f32, f32) = (720., 760.);
/// The smallest a pop-out gets: one column of messages and a composer.
const MIN_SIZE: (f32, f32) = (400., 360.);
/// How long a pop-out waits after a post before it checks whether the
/// user saw it, as the main window does (#145).
const VIEW_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(1);

/// What a pop-out shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Channel(SharedString),
    /// The thread under `root`, in `channel`.
    Thread {
        root: SharedString,
        channel: SharedString,
    },
}

impl Target {
    fn channel(&self) -> &SharedString {
        match self {
            Self::Channel(channel) | Self::Thread { channel, .. } => channel,
        }
    }

    fn root(&self) -> Option<&SharedString> {
        match self {
            Self::Channel(_) => None,
            Self::Thread { root, .. } => Some(root),
        }
    }

    fn of(place: &PopOutPlace) -> Self {
        match &place.root_id {
            Some(root) => Self::Thread {
                root: root.clone().into(),
                channel: place.channel_id.clone().into(),
            },
            None => Self::Channel(place.channel_id.clone().into()),
        }
    }

    /// Whether `place` saves this target on `server`.
    fn is(&self, server: &str, place: &PopOutPlace) -> bool {
        place.server == server && Self::of(place) == *self
    }
}

/// Something a pop-out asks of the main window, for the server it shows.
#[derive(Clone, Debug)]
pub struct Request {
    pub server: String,
    pub ask: Ask,
}

#[derive(Clone, Debug)]
pub enum Ask {
    /// Show the channel, at `post` when given.
    Show {
        channel: SharedString,
        post: Option<SharedString>,
    },
    OpenLink(SharedString),
    OpenDirect(SharedString),
    ViewProfile(UserProfile),
    Pinned(SharedString),
    Info(SharedString),
    Forward {
        channel: SharedString,
        post: SharedString,
    },
    Leave(SharedString),
    Archive(SharedString),
    /// Offline, the user asked for this action (as sync names it).
    Offline(&'static str),
}

/// Where pop-outs send their [`Request`]s; the main window subscribes.
pub struct Requests;

impl EventEmitter<Request> for Requests {}

/// The open pop-outs and what the app knows about them.
#[derive(Default)]
struct PopOuts {
    windows: Vec<WindowHandle<PopOutView>>,
    /// Servers whose saved pop-outs opened again this run.
    restored: HashSet<String>,
    requests: Option<Entity<Requests>>,
    /// Pop-outs close with the last main window, keeping their places.
    _closed: Option<gpui_kit::Subscription>,
}

impl Global for PopOuts {}

fn pop_outs(cx: &mut App) -> &mut PopOuts {
    if !cx.has_global::<PopOuts>() {
        let closed = cx.on_window_closed(|cx, _| close_with_main_window(cx));
        cx.set_global(PopOuts {
            _closed: Some(closed),
            ..PopOuts::default()
        });
    }
    cx.global_mut::<PopOuts>()
}

/// The entity the main window listens to for [`Request`]s.
pub fn requests(cx: &mut App) -> Entity<Requests> {
    if let Some(requests) = pop_outs(cx).requests.clone() {
        return requests;
    }
    let requests = cx.new(|_| Requests);
    pop_outs(cx).requests = Some(requests.clone());
    requests
}

/// The open pop-outs, each with what it shows and on which server.
fn open_windows(cx: &App) -> Vec<(WindowHandle<PopOutView>, String, Target)> {
    let Some(pop_outs) = cx.try_global::<PopOuts>() else {
        return Vec::new();
    };
    pop_outs
        .windows
        .iter()
        .filter_map(|handle| {
            let view = handle.read(cx).ok()?;
            Some((*handle, view.server.clone(), view.target.clone()))
        })
        .collect()
}

/// Brings forward the pop-out showing `target` on `server`; `false` when
/// there is none.
fn activate(server: &str, target: &Target, cx: &mut App) -> bool {
    let Some((handle, ..)) = open_windows(cx)
        .into_iter()
        .find(|(_, shown, open)| shown == server && open == target)
    else {
        return false;
    };
    handle
        .update(cx, |_, window, _| window.activate_window())
        .is_ok()
}

/// Opens `target` in a window of its own, or brings forward the one that
/// already shows it. `live` is the main window's sync; without one only
/// the preview's fixtures can pop out.
pub fn open(live: Option<&Live>, target: Target, cx: &mut App) {
    if live.is_none() && !crate::preview::enabled() {
        return;
    }
    let server = live
        .map(|live| live.server().to_owned())
        .unwrap_or_default();
    if activate(&server, &target, cx) {
        return;
    }
    let fork = live.map(Live::fork);
    cx.defer(move |cx| open_window(fork, server, target, None, cx));
}

/// Opens again the pop-outs saved for `live`'s server, once a run, when
/// its sidebar is in.
pub fn restore(live: &Live, cx: &mut App) {
    let server = live.server().to_owned();
    if !pop_outs(cx).restored.insert(server.clone()) {
        return;
    }
    let places: Vec<PopOutPlace> = SettingsStore::global(cx)
        .read(cx)
        .get()
        .popouts
        .iter()
        .filter(|place| place.server == server)
        .cloned()
        .collect();
    for place in places {
        let target = Target::of(&place);
        if activate(&server, &target, cx) {
            continue;
        }
        let fork = live.fork();
        let server = server.clone();
        cx.defer(move |cx| open_window(Some(fork), server, target, Some(place.bounds), cx));
    }
}

/// The notification policy's view of what the user sees, when a pop-out
/// has focus: its channel or thread, not the main window's.
pub fn seen_by_focused(app: matty_notify::AppState, cx: &App) -> matty_notify::AppState {
    let Some(active) = cx.active_window() else {
        return app;
    };
    let focused = open_windows(cx)
        .into_iter()
        .find(|(handle, ..)| AnyWindowHandle::from(*handle).window_id() == active.window_id());
    match focused {
        Some((_, _, target)) => matty_notify::AppState {
            focused: true,
            visible_channel: match &target {
                Target::Channel(channel) => Some(channel.to_string()),
                Target::Thread { .. } => None,
            },
            visible_thread: target.root().map(ToString::to_string),
            ..app
        },
        None => app,
    }
}

/// Brings forward the pop-out a clicked notification is about: the
/// thread's window for a reply, else the channel's, at the post. `false`
/// when no pop-out shows it and the main window should.
pub fn show_notified(clicked: &matty_notify::Clicked, cx: &mut App) -> bool {
    let windows = open_windows(cx);
    let thread = windows.iter().find(|(_, _, target)| {
        !clicked.root_id.is_empty() && target.root().is_some_and(|root| *root == clicked.root_id)
    });
    let channel = windows.iter().find(|(_, _, target)| {
        matches!(target, Target::Channel(channel) if *channel == clicked.channel_id)
    });
    let Some((handle, _, target)) = thread.or(channel) else {
        return false;
    };
    let post: SharedString = match target {
        // With collapsed threads a reply is only in its thread; the
        // channel shows its root.
        Target::Channel(_) if !clicked.root_id.is_empty() => clicked.root_id.clone().into(),
        _ => clicked.post_id.clone().into(),
    };
    cx.activate(true);
    handle
        .update(cx, |view, window, cx| {
            window.activate_window();
            view.jump_to(post, cx);
        })
        .is_ok()
}

/// Closes every pop-out once no other window is left, keeping their
/// places for the next launch.
fn close_with_main_window(cx: &mut App) {
    let popped: Vec<_> = open_windows(cx)
        .into_iter()
        .map(|(handle, ..)| AnyWindowHandle::from(handle))
        .collect();
    let others = cx.windows().into_iter().any(|window| {
        popped
            .iter()
            .all(|pop| pop.window_id() != window.window_id())
    });
    if others || popped.is_empty() {
        return;
    }
    for handle in popped {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

fn window_options(saved: Option<WindowGeometry>, cx: &App) -> WindowOptions {
    let bounds = match saved {
        Some(geometry) => {
            let bounds = Bounds::new(
                point(px(geometry.x), px(geometry.y)),
                size(
                    px(geometry.width.max(MIN_SIZE.0)),
                    px(geometry.height.max(MIN_SIZE.1)),
                ),
            );
            if geometry.maximized {
                WindowBounds::Maximized(bounds)
            } else {
                WindowBounds::Windowed(bounds)
            }
        }
        None => WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)),
            cx,
        )),
    };
    WindowOptions {
        window_bounds: Some(bounds),
        window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
        app_id: Some("app.matty.Matty".into()),
        ..title_bar_window_options()
    }
}

fn open_window(
    live: Option<(Live, async_channel::Receiver<matty_sync::ChangeSet>)>,
    server: String,
    target: Target,
    saved: Option<WindowGeometry>,
    cx: &mut App,
) {
    let options = window_options(saved, cx);
    let shown = target.clone();
    let opened = cx.open_window(options, |window, cx| {
        crate::apply_theme(window, cx);
        cx.new(|cx| PopOutView::new(shown, server, live, window, cx))
    });
    match opened {
        Ok(handle) => {
            let windows = &mut pop_outs(cx).windows;
            windows.push(handle);
            // Closed windows' handles go as new ones come.
            let open: Vec<_> = std::mem::take(windows);
            let open = open
                .into_iter()
                .filter(|window| window.read(cx).is_ok())
                .collect();
            pop_outs(cx).windows = open;
        }
        Err(error) => tracing::warn!(%error, "could not open a window"),
    }
}

/// What a pop-out draws under its title bar.
enum Content {
    Channel(Entity<ChannelView>),
    Thread(Entity<RightPaneView>),
}

/// A channel or thread in a window of its own.
pub struct PopOutView {
    target: Target,
    /// The server it is on; empty in the preview.
    server: String,
    /// Its own view of sync; `None` in the preview.
    live: Option<Live>,
    content: Content,
    toasts: Entity<ToastState>,
    focus: FocusHandle,
    /// The channel opened with sync: its sidebar row was known.
    opened: bool,
    /// The user marked the channel unread here; it is not viewed again
    /// until the window closes (#146).
    kept_unread: bool,
    /// The connection is down for good; actions that need it are refused.
    offline: bool,
    _changes: Option<Task<()>>,
    _typing_task: Option<Task<()>>,
    _view_task: Option<Task<()>>,
}

impl PopOutView {
    fn new(
        target: Target,
        server: String,
        live: Option<(Live, async_channel::Receiver<matty_sync::ChangeSet>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (live, changes) = match live {
            Some((live, changes)) => (Some(live), Some(changes)),
            None => (None, None),
        };
        let me: SharedString = match live.as_ref().and_then(Live::me_user) {
            Some(user) => crate::transcript::display_name(user).into(),
            None if crate::preview::enabled() => "Arya".into(),
            None => "You".into(),
        };
        let directory = match &live {
            Some(live) => live.directory(),
            None if crate::preview::enabled() => crate::preview::sidebar()
                .categories
                .into_iter()
                .flat_map(|category| category.rows)
                .collect(),
            None => Vec::new(),
        };
        let content = match &target {
            Target::Channel(id) => {
                let model = (live.is_none())
                    .then(|| crate::preview::channel(id))
                    .flatten();
                let view = cx.new(|cx| ChannelView::new(model, me, window, cx));
                view.update(cx, |view, cx| view.set_directory(directory, cx));
                cx.subscribe_in(&view, window, Self::channel_event).detach();
                Content::Channel(view)
            }
            Target::Thread { root, channel } => {
                let pane = cx.new(|cx| {
                    let mut pane = RightPaneView::new(me, window, cx);
                    pane.set_directory(directory);
                    pane
                });
                pane.update(cx, |pane, cx| {
                    pane.set_popped_out(true, cx);
                    if live.is_none() {
                        let thread = crate::preview::thread(channel, root);
                        pane.set_content(
                            thread.map(|thread| RightPaneContent::Thread(Box::new(thread))),
                            cx,
                        );
                    }
                });
                cx.subscribe_in(&pane, window, Self::pane_event).detach();
                Content::Thread(pane)
            }
        };
        cx.observe_window_appearance(window, |_, window, cx| crate::apply_theme(window, cx))
            .detach();
        cx.subscribe(
            &SettingsStore::global(cx),
            |_, _, _: &SettingsChanged, cx| cx.notify(),
        )
        .detach();
        cx.observe_window_bounds(window, |this, window, cx| this.remember_place(window, cx))
            .detach();
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.view_if_seen(window, cx);
            }
        })
        .detach();
        // The OS's close button; the window's own forget their place too.
        window.on_window_should_close(cx, {
            let view = cx.weak_entity();
            move |_, cx| {
                view.update(cx, |this, cx| this.forget_place(cx)).ok();
                true
            }
        });
        // Closing stops what this window held open with sync.
        cx.on_release(|this, _| {
            if let Some(live) = this.live.as_mut() {
                live.close_view();
            }
        })
        .detach();
        let toasts = cx.new(ToastState::new);
        let focus = cx.focus_handle();
        let mut this = Self {
            target,
            server,
            live,
            content,
            toasts,
            focus,
            opened: false,
            kept_unread: false,
            offline: false,
            _changes: None,
            _typing_task: None,
            _view_task: None,
        };
        if let Some(changes) = changes {
            this._changes = Some(cx.spawn_in(window, async move |this, cx| {
                while let Ok(change) = changes.recv().await {
                    let applied =
                        this.update_in(cx, |this, window, cx| this.apply(&change, window, cx));
                    if applied.is_err() {
                        break;
                    }
                }
            }));
        }
        this.open_with_sync(window, cx);
        this.set_title(window, cx);
        this.remember_place(window, cx);
        this
    }

    /// Opens the channel or thread with sync, once the channel's sidebar
    /// row is known for a channel.
    fn open_with_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if self.opened {
            return;
        }
        match (&self.target, &self.content) {
            (Target::Channel(id), Content::Channel(view)) => {
                let Some(row) = live.row(id) else {
                    return;
                };
                let model = live.open(row);
                view.update(cx, |view, cx| view.set_model(Some(model), window, cx));
            }
            (Target::Thread { root, .. }, Content::Thread(_)) => live.open_thread(root),
            _ => return,
        }
        self.opened = true;
        self.set_title(window, cx);
    }

    /// The window's title: the channel's name, or the thread's channel.
    fn title(&self, cx: &App) -> SharedString {
        let channel = self.target.channel();
        let name = self
            .live
            .as_ref()
            .and_then(|live| live.row(channel))
            .map(|row| (row.name.clone(), row.kind))
            .or_else(|| match &self.content {
                Content::Channel(view) => view
                    .read(cx)
                    .model()
                    .map(|model| (model.name.clone(), model.kind.clone())),
                Content::Thread(_) => None,
            });
        let shown = match name {
            Some((name, ChannelKind::Direct { .. } | ChannelKind::Group { .. })) => {
                name.to_string()
            }
            Some((name, _)) => format!("#{name}"),
            None => match &self.content {
                Content::Thread(pane) => match pane.read(cx).content() {
                    Some(RightPaneContent::Thread(thread)) => format!("#{}", thread.channel_name),
                    _ => String::new(),
                },
                Content::Channel(_) => String::new(),
            },
        };
        match (&self.target, shown.is_empty()) {
            (Target::Thread { .. }, true) => "Thread".into(),
            (Target::Thread { .. }, false) => format!("Thread in {shown}").into(),
            (Target::Channel(_), true) => "Matty".into(),
            (Target::Channel(_), false) => shown.into(),
        }
    }

    fn set_title(&self, window: &mut Window, cx: &App) {
        window.set_window_title(&self.title(cx));
    }

    /// Saves where this pop-out is, so it opens there at the next launch.
    fn remember_place(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.live.is_none() || window.is_fullscreen() {
            return;
        }
        let bounds = window.window_bounds().get_bounds();
        let place = PopOutPlace {
            server: self.server.clone(),
            channel_id: self.target.channel().to_string(),
            root_id: self.target.root().map(ToString::to_string),
            bounds: WindowGeometry {
                x: f32::from(bounds.origin.x),
                y: f32::from(bounds.origin.y),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
                maximized: window.is_maximized(),
            },
        };
        let (server, target) = (self.server.clone(), self.target.clone());
        SettingsStore::global(cx).update(cx, |store, cx| {
            store.update(cx, |settings| {
                match settings
                    .popouts
                    .iter_mut()
                    .find(|saved| target.is(&server, saved))
                {
                    Some(saved) => *saved = place,
                    None => settings.popouts.push(place),
                }
            })
        });
    }

    /// The user closed this pop-out: it does not open again at launch.
    fn forget_place(&mut self, cx: &mut Context<Self>) {
        let (server, target) = (self.server.clone(), self.target.clone());
        SettingsStore::global(cx).update(cx, |store, cx| {
            store.update(cx, |settings| {
                settings.popouts.retain(|saved| !target.is(&server, saved))
            })
        });
    }

    /// Closes the window from Cmd/Ctrl+W, the title bar or sign-out.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.forget_place(cx);
        window.remove_window();
    }

    /// Scrolls to `post` and tints it, as a notification's click asks.
    fn jump_to(&mut self, post: SharedString, cx: &mut Context<Self>) {
        match (&self.content, &self.target) {
            (Content::Channel(view), _) => view.update(cx, |view, cx| view.jump_to(post, cx)),
            (Content::Thread(pane), Target::Thread { root, .. }) => {
                let root = root.clone();
                pane.update(cx, |pane, cx| pane.jump_to(root, post, cx))
            }
            _ => {}
        }
    }

    fn apply(
        &mut self,
        change: &matty_sync::ChangeSet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        let update = live.apply(change);
        self.apply_update(update, window, cx);
    }

    /// Shows what sync's latest change means for this window.
    fn apply_update(
        &mut self,
        mut update: crate::live::Update,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if update.signed_out {
            return self.close(window, cx);
        }
        if update.categories.is_some() {
            self.open_with_sync(window, cx);
            let directory = self.live.as_ref().map(Live::directory).unwrap_or_default();
            match &self.content {
                Content::Channel(view) => {
                    view.update(cx, |view, cx| view.set_directory(directory, cx))
                }
                Content::Thread(pane) => pane.update(cx, |pane, _| pane.set_directory(directory)),
            }
            self.set_title(window, cx);
        }
        if let Some(state) = &update.connection {
            let away = *state != crate::main_window::ConnectionState::Online;
            self.offline = *state == crate::main_window::ConnectionState::Offline;
            let offline = self.offline;
            match &self.content {
                Content::Channel(view) => view.update(cx, |view, cx| {
                    view.set_offline(offline);
                    view.set_away(away, cx);
                }),
                Content::Thread(pane) => pane.update(cx, |pane, cx| {
                    pane.set_offline(offline, cx);
                    pane.set_away(away, cx);
                }),
            }
        }
        let mut retitle = false;
        let mut typing = update
            .typing
            .as_ref()
            .is_some_and(|names| !names.is_empty());
        match &self.content {
            Content::Channel(view) => Self::show_channel(view, &mut update, window, cx),
            Content::Thread(pane) => {
                if let Some(thread) = update.thread {
                    pane.update(cx, |pane, cx| pane.set_thread(thread, cx));
                    retitle = true;
                }
                if let Some(names) = update.thread_typing {
                    typing |= !names.is_empty();
                    pane.update(cx, |pane, cx| pane.set_typing(names, cx));
                }
                if let Some((term, people)) = update.suggested {
                    pane.update(cx, |pane, cx| pane.set_suggested(term, people, cx));
                }
                if let Some((url, card)) = update.link_preview {
                    pane.update(cx, |pane, cx| pane.set_link_preview(&url, card, cx));
                }
                if let Some((id, image)) = update.full_image {
                    pane.update(cx, |pane, cx| pane.set_full_image(&id, image, cx));
                }
                if let Some(bytes) = update.max_file_size {
                    pane.update(cx, |pane, _| pane.set_max_file_size(bytes));
                }
            }
        }
        if typing {
            self.expire_typing_later(cx);
        }
        if retitle {
            self.set_title(window, cx);
        }
        if let Some((file_id, state)) = update.download {
            self.show_download(file_id, state, cx);
        }
        if let Some((id, status)) = update.upload {
            self.set_upload_status(&id, status, window, cx);
        }
        for id in update.resume_uploads {
            if let Some(job) = self.live.as_ref().and_then(|live| live.upload_job(&id)) {
                let sending = crate::attachments::AttachmentStatus::Uploading(0.);
                self.set_upload_status(&id, sending, window, cx);
                self.start_upload(id, job, window, cx);
            }
        }
        if let Some(arrival) = &update.arrived
            && matches!(&self.target, Target::Channel(id) if *id == arrival.event.channel_id)
        {
            self.view_later(window, cx);
        }
        if let Some(action) = update.offline_refused {
            self.ask(Ask::Offline(action), cx);
        }
    }

    /// The channel view's part of an update.
    fn show_channel(
        view: &Entity<ChannelView>,
        update: &mut crate::live::Update,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        view.update(cx, |view, cx| {
            match update.channel.take() {
                Some(ChannelUpdate::Replace(model)) => view.set_model(Some(model), window, cx),
                Some(ChannelUpdate::Patch {
                    removed,
                    prepended,
                    updated,
                    appended,
                }) => {
                    // The composer's own "Sending…" rows give way to sync's
                    // copy of the same send, which this patch carries.
                    let local: Vec<SharedString> = view
                        .model()
                        .into_iter()
                        .flat_map(|model| &model.posts)
                        .filter(|post| post.id.starts_with("pending-"))
                        .map(|post| post.id.clone())
                        .collect();
                    for id in local.iter().chain(&removed) {
                        view.remove_post(id, window, cx);
                    }
                    for post in updated {
                        view.update_post(post, cx);
                    }
                    if !prepended.is_empty() {
                        view.prepend_posts(prepended, cx);
                    }
                    view.append_posts(appended, cx);
                }
                None => {}
            }
            if let Some((at_start, at_end, failed)) = update.history {
                view.set_history(at_start, at_end, failed, cx);
            }
            if let Some(details) = update.details.take() {
                view.set_details(details, cx);
            }
            if let Some((muted, notifications)) = update.notifications.take() {
                view.set_notifications(muted, notifications, cx);
            }
            if let Some(profiles) = update.profiles.take() {
                view.set_profiles(profiles, cx);
            }
            if let Some(names) = update.typing.clone() {
                view.set_typing(names, cx);
            }
            if let Some((term, people)) = update.suggested.take() {
                view.set_suggested(term, people, cx);
            }
            if let Some((url, card)) = update.link_preview.take() {
                view.set_link_preview(&url, card, cx);
            }
            if let Some((id, image)) = update.full_image.take() {
                view.set_full_image(id, image, cx);
            }
            if let Some(bytes) = update.max_file_size {
                view.set_max_file_size(bytes);
            }
        });
    }

    /// Keeps a download's card in step; the main window opens the file and
    /// says where it went.
    fn show_download(
        &mut self,
        file_id: SharedString,
        state: matty_sync::DownloadState,
        cx: &mut Context<Self>,
    ) {
        use crate::attachments::{Downloads, FileDownload};
        let state = match state {
            matty_sync::DownloadState::Progress { received, total } => {
                FileDownload::Downloading { received, total }
            }
            matty_sync::DownloadState::Done(path) => FileDownload::Saved(path),
            matty_sync::DownloadState::Failed(_) => FileDownload::Failed,
        };
        Downloads::set(cx, file_id, state);
        self.notify_content(cx);
    }

    fn notify_content(&self, cx: &mut Context<Self>) {
        match &self.content {
            Content::Channel(view) => view.update(cx, |_, cx| cx.notify()),
            Content::Thread(pane) => pane.update(cx, |_, cx| cx.notify()),
        }
    }

    /// Clears typing notices as they run out.
    fn expire_typing_later(&mut self, cx: &mut Context<Self>) {
        self._typing_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(crate::live::TYPING_FOR / 2)
                    .await;
                let left = this.update(cx, |this, cx| {
                    let live = this.live.as_mut()?;
                    let (names, thread) = live.expire_typing(web_time::Instant::now());
                    let left = live.is_typing();
                    match &this.content {
                        Content::Channel(view) => {
                            if let Some(names) = names {
                                view.update(cx, |view, cx| view.set_typing(names, cx));
                            }
                        }
                        Content::Thread(pane) => {
                            if let Some(names) = thread {
                                pane.update(cx, |pane, cx| pane.set_typing(names, cx));
                            }
                        }
                    }
                    Some(left)
                });
                if !matches!(left, Ok(Some(true))) {
                    break;
                }
            }
        }));
    }

    fn view_later(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._view_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(VIEW_DEBOUNCE).await;
            this.update_in(cx, |this, window, cx| this.view_if_seen(window, cx))
                .ok();
        }));
    }

    /// Views the channel on the server when this window has focus and
    /// shows its newest post, so a channel only counts as read while the
    /// user can see it (#145).
    fn view_if_seen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Target::Channel(id), Content::Channel(view)) = (&self.target, &self.content) else {
            return;
        };
        let Some(live) = &self.live else {
            return;
        };
        let unread = live.row(id).is_some_and(|row| row.unread);
        if unread && !self.kept_unread && window.is_window_active() && view.read(cx).at_newest(cx) {
            live.send(matty_sync::Command::ViewChannel {
                channel_id: id.to_string(),
                prev_channel_id: String::new(),
            });
        }
    }

    /// Hands `ask` to the main window.
    fn ask(&self, ask: Ask, cx: &mut Context<Self>) {
        let request = Request {
            server: self.server.clone(),
            ask,
        };
        requests(cx).update(cx, |_, cx| cx.emit(request));
    }

    fn toast(&self, id: &'static str, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let notice = Notice::new(text).timeout(std::time::Duration::from_secs(4));
        self.toasts
            .update(cx, |toasts, cx| toasts.push(id, notice, cx));
    }

    /// A post's permalink, from the channel or thread on screen.
    fn permalink(&self, post_id: &str) -> Option<String> {
        let live = self.live.as_ref()?;
        let channel = live
            .post_channel(post_id)
            .unwrap_or_else(|| self.target.channel().to_string());
        live.permalink(&channel, post_id)
    }

    fn copy_link(&mut self, post_id: &str, cx: &mut Context<Self>) {
        match self.permalink(post_id) {
            Some(link) => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(link));
                self.toast("copy-link", "Link copied", cx);
            }
            None => self.toast(
                "copy-link",
                "Couldn't copy the link. Try again once you're connected.",
                cx,
            ),
        }
    }

    fn remind(&mut self, post_id: &str, at: crate::channel::RemindAt, cx: &mut Context<Self>) {
        if self.offline {
            return self.ask(Ask::Offline("set reminder"), cx);
        }
        let Some(live) = &self.live else {
            return;
        };
        live.remind(post_id, at);
        let when = match at {
            crate::channel::RemindAt::Tomorrow => "tomorrow at 9:00".to_string(),
            other => other.label().to_lowercase(),
        };
        self.toast("reminder", format!("Reminder set for {when}"), cx);
    }

    fn mark_unread(&mut self, post_id: &str, cx: &mut Context<Self>) {
        if self.offline {
            return self.ask(Ask::Offline("mark unread"), cx);
        }
        if let Some(live) = &self.live {
            live.send(matty_sync::Command::MarkUnread {
                post_id: post_id.to_string(),
            });
        }
        self.kept_unread = true;
    }

    fn download_file(&mut self, file_id: &SharedString, cx: &mut Context<Self>) {
        use crate::attachments::{Downloads, FileDownload};
        if let Some(FileDownload::Downloading { .. }) = Downloads::get(cx, file_id) {
            return;
        }
        let Some(live) = &self.live else {
            return;
        };
        let Some(name) = live.file_name(file_id) else {
            tracing::warn!(%file_id, "no such file on screen");
            return;
        };
        let folder = crate::main_window::download_folder(
            SettingsStore::global(cx)
                .read(cx)
                .get()
                .download_folder
                .clone(),
        );
        live.send(matty_sync::Command::Download {
            file_id: file_id.to_string(),
            folder,
            name,
        });
        let starting = FileDownload::Downloading {
            received: 0,
            total: None,
        };
        Downloads::set(cx, file_id.clone(), starting);
        self.notify_content(cx);
    }

    /// Reads a tray file off the UI thread and hands its bytes to sync, as
    /// the main window does.
    fn start_upload(
        &mut self,
        id: SharedString,
        job: crate::live::UploadJob,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(live) = &mut self.live else {
            return;
        };
        live.track_upload(&id, job.clone());
        if live.is_offline() {
            // The file waits here and goes with its post through the
            // outbox, or uploads once the connection is back (#194).
            match crate::attachments::keep_for_outbox(&id, &job.name, &job.source) {
                Ok(path) => {
                    live.queue_upload(&id, path);
                    match &self.content {
                        Content::Channel(view) => {
                            view.update(cx, |view, cx| view.set_upload_queued(&id, window, cx))
                        }
                        Content::Thread(pane) => {
                            pane.update(cx, |pane, cx| pane.set_upload_queued(&id, window, cx))
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "could not keep a file to upload later");
                    let failed = crate::attachments::AttachmentStatus::Failed;
                    self.set_upload_status(&id, failed, window, cx);
                }
            }
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let bytes = cx
                .background_spawn(async move {
                    match job.source {
                        crate::attachments::UploadSource::Path(path) => std::fs::read(path),
                        crate::attachments::UploadSource::Image(image) => Ok(image.bytes.clone()),
                    }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                // Removed from the tray while it was being read.
                let Some(live) = this
                    .live
                    .as_ref()
                    .filter(|live| live.upload_job(&id).is_some())
                else {
                    return;
                };
                match bytes {
                    Ok(bytes) => live.send(matty_sync::Command::Upload {
                        upload_id: id.to_string(),
                        channel_id: job.channel_id,
                        file_name: job.name,
                        bytes,
                    }),
                    Err(error) => {
                        tracing::warn!(%error, "could not read a file to upload");
                        let failed = crate::attachments::AttachmentStatus::Failed;
                        this.set_upload_status(&id, failed, window, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn set_upload_status(
        &mut self,
        id: &str,
        status: crate::attachments::AttachmentStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &self.content {
            Content::Channel(view) => view.update(cx, |view, cx| {
                view.set_upload_status(id, status, window, cx)
            }),
            Content::Thread(pane) => pane.update(cx, |pane, cx| {
                pane.set_upload_status(id, status, window, cx)
            }),
        }
    }

    /// The thread under `root` in a window of its own, from this one.
    fn pop_out_thread(&mut self, root: SharedString, cx: &mut Context<Self>) {
        let channel = self.target.channel().clone();
        open(self.live.as_ref(), Target::Thread { root, channel }, cx);
    }

    /// What the channel view asks for. Sync answers what it can here; a
    /// trip elsewhere in Matty is the main window's.
    fn channel_event(
        &mut self,
        view: &Entity<ChannelView>,
        event: &ChannelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let channel = self.target.channel().clone();
        if let ChannelEvent::OpenThread(root) | ChannelEvent::OpenThreadWindow(root) = event {
            return self.pop_out_thread(root.clone(), cx);
        }
        let ask = match event {
            ChannelEvent::Offline(action) => Some(Ask::Offline(action)),
            ChannelEvent::OpenLink(url) => Some(Ask::OpenLink(url.clone())),
            ChannelEvent::OpenPinned => Some(Ask::Pinned(channel.clone())),
            ChannelEvent::OpenInfo => Some(Ask::Info(channel.clone())),
            ChannelEvent::OpenDirect { display_name, .. } => {
                Some(Ask::OpenDirect(display_name.clone()))
            }
            ChannelEvent::ViewProfile(profile) => Some(Ask::ViewProfile(profile.clone())),
            ChannelEvent::OpenPermalink { channel, post } => Some(Ask::Show {
                channel: channel.clone(),
                post: Some(post.clone()),
            }),
            ChannelEvent::Forward(post) => Some(Ask::Forward {
                channel: channel.clone(),
                post: post.clone(),
            }),
            ChannelEvent::Leave => Some(Ask::Leave(channel.clone())),
            ChannelEvent::Archive => Some(Ask::Archive(channel.clone())),
            _ => None,
        };
        if let Some(ask) = ask {
            return self.ask(ask, cx);
        }
        match event {
            ChannelEvent::CopyLink(post) => return self.copy_link(post, cx),
            ChannelEvent::OpenInBrowser(post) => {
                if let Some(link) = self.permalink(post) {
                    cx.open_url(&link);
                }
                return;
            }
            ChannelEvent::MarkUnread(post) => return self.mark_unread(post, cx),
            ChannelEvent::Remind { post, at } => return self.remind(post, *at, cx),
            ChannelEvent::OpenFile(id) => return self.download_file(id, cx),
            ChannelEvent::Upload { id, name, source } => {
                let job = crate::live::UploadJob::new(
                    channel.to_string(),
                    name.to_string(),
                    source.clone(),
                );
                return self.start_upload(id.clone(), job, window, cx);
            }
            ChannelEvent::RetryUpload(id) => {
                if let Some(job) = self.live.as_ref().and_then(|live| live.upload_job(id)) {
                    self.start_upload(id.clone(), job, window, cx);
                }
                return;
            }
            _ => {}
        }
        let Some(live) = self.live.as_mut() else {
            if let ChannelEvent::PreviewLink(url) = event {
                let card = crate::preview::link_card(url);
                view.update(cx, |view, cx| view.set_link_preview(url, card, cx));
            }
            return;
        };
        let channel_id = channel.to_string();
        let command = match event {
            ChannelEvent::Send {
                text,
                files,
                unfurl,
            } => {
                let (file_ids, local_files) = live.take_files(files);
                matty_sync::Command::SendPost {
                    pending_post_id: String::new(),
                    channel_id,
                    root_id: String::new(),
                    message: text.to_string(),
                    file_ids,
                    local_files,
                    props: crate::embeds::send_props(*unfurl),
                }
            }
            ChannelEvent::Typing { root } => matty_sync::Command::Typing {
                channel_id,
                parent_id: root.as_ref().map(ToString::to_string).unwrap_or_default(),
            },
            ChannelEvent::SuggestPeople(term) => matty_sync::Command::SuggestPeople {
                term: term.to_string(),
                team_id: live.team().map(str::to_string),
                channel_id: Some(channel_id),
            },
            ChannelEvent::PreviewLink(url) if live.link_previews() => {
                matty_sync::Command::LoadLinkPreview {
                    url: url.to_string(),
                }
            }
            ChannelEvent::SaveDraft(save) => matty_sync::Command::SaveDraft {
                channel_id: save.key.channel_id.to_string(),
                root_id: save.key.root_id.to_string(),
                message: save.text.to_string(),
                file_ids: live.file_ids(&save.files),
            },
            ChannelEvent::CancelUpload(id) => return live.cancel_upload(id),
            ChannelEvent::ViewImages(ids) => {
                for (id, image) in live.full_images(ids) {
                    view.update(cx, |view, cx| view.set_full_image(id, image, cx));
                }
                return;
            }
            ChannelEvent::ToggleReaction { post, emoji } => matty_sync::Command::ToggleReaction {
                post_id: post.to_string(),
                emoji_name: crate::transcript::emoji_name(emoji),
            },
            ChannelEvent::ToggleSave(post) => {
                let saved = !live.is_saved(post);
                return live.set_saved(post, saved);
            }
            ChannelEvent::TogglePin(post) => {
                // The view has already flipped the post.
                let pinned = view
                    .read(cx)
                    .model()
                    .and_then(|model| model.posts.iter().find(|shown| shown.id == *post))
                    .map(|shown| shown.pinned);
                if let Some(pinned) = pinned {
                    live.set_pinned(post, pinned);
                }
                return;
            }
            ChannelEvent::ToggleFollow { post, following } => {
                return live.follow_thread(post, *following);
            }
            ChannelEvent::SaveEdit { post, text } => matty_sync::Command::EditPost {
                post_id: post.to_string(),
                message: text.to_string(),
            },
            ChannelEvent::Delete(post) => matty_sync::Command::DeletePost {
                post_id: post.to_string(),
            },
            ChannelEvent::Retry(post) => matty_sync::Command::Retry {
                pending_post_id: post.to_string(),
            },
            ChannelEvent::Discard(post) => matty_sync::Command::Discard {
                pending_post_id: post.to_string(),
            },
            ChannelEvent::NearTop => matty_sync::Command::LoadOlder { channel_id },
            ChannelEvent::NearBottom => matty_sync::Command::LoadNewer { channel_id },
            ChannelEvent::JumpMissing(post) => matty_sync::Command::LoadAround {
                channel_id,
                post_id: post.to_string(),
            },
            ChannelEvent::JumpLatest => matty_sync::Command::LoadLatest { channel_id },
            ChannelEvent::ToggleFavorite(favorite) => matty_sync::Command::SetFavorite {
                channel_id,
                favorite: *favorite,
            },
            ChannelEvent::Rename(name) => matty_sync::Command::PatchChannel {
                channel_id,
                display_name: Some(name.to_string()),
                header: None,
                purpose: None,
            },
            ChannelEvent::SetHeader(text) => matty_sync::Command::PatchChannel {
                channel_id,
                display_name: None,
                header: Some(text.to_string()),
                purpose: None,
            },
            ChannelEvent::SetPurpose(text) => matty_sync::Command::PatchChannel {
                channel_id,
                display_name: None,
                header: None,
                purpose: Some(text.to_string()),
            },
            ChannelEvent::SetNotifications {
                muted,
                notifications,
            } => matty_sync::Command::SetChannelNotifications {
                channel_id,
                props: crate::live::notify_props(*muted, notifications),
            },
            ChannelEvent::AddMembers(people) => matty_sync::Command::AddMembers {
                channel_id,
                user_ids: people.iter().map(|p| p.user_id.to_string()).collect(),
            },
            ChannelEvent::ConvertToPrivate => matty_sync::Command::ConvertToPrivate { channel_id },
            _ => return,
        };
        live.send(command);
    }

    /// What the thread pane asks for.
    fn pane_event(
        &mut self,
        pane: &Entity<RightPaneView>,
        event: &RightPaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let channel = self.target.channel().clone();
        let ask = match event {
            RightPaneEvent::Close => return self.close(window, cx),
            RightPaneEvent::Offline(action) => Some(Ask::Offline(action)),
            RightPaneEvent::OpenLink(url) => Some(Ask::OpenLink(url.clone())),
            RightPaneEvent::OpenDirect { display_name, .. } => {
                Some(Ask::OpenDirect(display_name.clone()))
            }
            RightPaneEvent::ViewProfile(profile) => Some(Ask::ViewProfile(profile.clone())),
            RightPaneEvent::Forward(post) => Some(Ask::Forward {
                channel: channel.clone(),
                post: post.clone(),
            }),
            RightPaneEvent::Jump { channel, post } => Some(Ask::Show {
                channel: channel.clone(),
                post: Some(post.clone()),
            }),
            _ => None,
        };
        if let Some(ask) = ask {
            return self.ask(ask, cx);
        }
        match event {
            RightPaneEvent::CopyLink(post) => return self.copy_link(post, cx),
            RightPaneEvent::MarkUnread(post) => return self.mark_unread(post, cx),
            RightPaneEvent::Remind { post, at } => return self.remind(post, *at, cx),
            RightPaneEvent::OpenFile(id) => return self.download_file(id, cx),
            RightPaneEvent::Upload { id, name, source } => {
                let job = crate::live::UploadJob::new(
                    channel.to_string(),
                    name.to_string(),
                    source.clone(),
                );
                return self.start_upload(id.clone(), job, window, cx);
            }
            RightPaneEvent::RetryUpload(id) => {
                if let Some(job) = self.live.as_ref().and_then(|live| live.upload_job(id)) {
                    self.start_upload(id.clone(), job, window, cx);
                }
                return;
            }
            _ => {}
        }
        let Some(live) = self.live.as_mut() else {
            if let RightPaneEvent::PreviewLink(url) = event {
                let card = crate::preview::link_card(url);
                pane.update(cx, |pane, cx| pane.set_link_preview(url, card, cx));
            }
            return;
        };
        let command = match event {
            RightPaneEvent::Reply {
                root,
                text,
                files,
                unfurl,
            } => {
                let channel_id = live.thread_channel().unwrap_or_else(|| channel.to_string());
                let (file_ids, local_files) = live.take_files(files);
                matty_sync::Command::SendPost {
                    pending_post_id: String::new(),
                    channel_id,
                    root_id: root.to_string(),
                    message: text.to_string(),
                    file_ids,
                    local_files,
                    props: crate::embeds::send_props(*unfurl),
                }
            }
            RightPaneEvent::SuggestPeople(term) => matty_sync::Command::SuggestPeople {
                term: term.to_string(),
                team_id: live.team().map(str::to_string),
                channel_id: live.thread_channel(),
            },
            RightPaneEvent::PreviewLink(url) if live.link_previews() => {
                matty_sync::Command::LoadLinkPreview {
                    url: url.to_string(),
                }
            }
            RightPaneEvent::SaveDraft(save) => matty_sync::Command::SaveDraft {
                channel_id: save.key.channel_id.to_string(),
                root_id: save.key.root_id.to_string(),
                message: save.text.to_string(),
                file_ids: live.file_ids(&save.files),
            },
            RightPaneEvent::CancelUpload(id) => return live.cancel_upload(id),
            RightPaneEvent::ViewImages(ids) => {
                for (id, image) in live.full_images(ids) {
                    pane.update(cx, |pane, cx| pane.set_full_image(&id, image, cx));
                }
                return;
            }
            RightPaneEvent::SetSaved { post, saved } => return live.set_saved(post, *saved),
            RightPaneEvent::SetPinned { post, pinned } => return live.set_pinned(post, *pinned),
            RightPaneEvent::ToggleFollow { root, following } => {
                return live.follow_thread(root, *following);
            }
            RightPaneEvent::LoadOlderReplies(root) => return live.load_older_replies(root),
            RightPaneEvent::ToggleReaction { post, emoji } => matty_sync::Command::ToggleReaction {
                post_id: post.to_string(),
                emoji_name: crate::transcript::emoji_name(emoji),
            },
            RightPaneEvent::SaveEdit { post, text } => matty_sync::Command::EditPost {
                post_id: post.to_string(),
                message: text.to_string(),
            },
            RightPaneEvent::Delete(post) => matty_sync::Command::DeletePost {
                post_id: post.to_string(),
            },
            RightPaneEvent::Retry(post) => matty_sync::Command::Retry {
                pending_post_id: post.to_string(),
            },
            RightPaneEvent::Discard(post) => matty_sync::Command::Discard {
                pending_post_id: post.to_string(),
            },
            _ => return,
        };
        live.send(command);
    }

    /// The channel view, for tests.
    #[cfg(test)]
    fn channel_view(&self) -> Option<&Entity<ChannelView>> {
        match &self.content {
            Content::Channel(view) => Some(view),
            Content::Thread(_) => None,
        }
    }
}

impl Render for PopOutView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let title = self.title(cx);
        let view = cx.weak_entity();
        let content = match &self.content {
            Content::Channel(view) => view.clone().into_any_element(),
            Content::Thread(pane) => pane.clone().into_any_element(),
        };
        div()
            .key_context(actions::POP_OUT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| this.close(window, cx)))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme.colors.background)
            .text_color(theme.colors.foreground)
            .font_family(theme.fonts.body.clone())
            .type_style(style::UI, &theme)
            .child(
                TitleBar::new()
                    .on_close(move |_, window, cx| {
                        view.update(cx, |this, cx| this.close(window, cx)).ok();
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .justify_center()
                            .truncate()
                            .text_color(theme.colors.muted_foreground)
                            .child(title),
                    ),
            )
            .child(
                div().flex_1().min_h_0().w_full().child(
                    selection_root("pop-out.selection")
                        .size_full()
                        .child(content),
                ),
            )
            .child(self.toasts.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;

    #[test]
    fn a_saved_place_names_its_target() {
        let bounds = WindowGeometry {
            x: 10.,
            y: 20.,
            width: 700.,
            height: 600.,
            maximized: false,
        };
        let channel = PopOutPlace {
            server: "https://a".into(),
            channel_id: "c".into(),
            root_id: None,
            bounds,
        };
        let thread = PopOutPlace {
            root_id: Some("r".into()),
            ..channel.clone()
        };
        assert_eq!(Target::of(&channel), Target::Channel("c".into()));
        assert_eq!(
            Target::of(&thread),
            Target::Thread {
                root: "r".into(),
                channel: "c".into()
            }
        );
        assert!(Target::Channel("c".into()).is("https://a", &channel));
        assert!(!Target::Channel("c".into()).is("https://b", &channel));
        assert!(!Target::Channel("c".into()).is("https://a", &thread));
    }

    #[gpui_kit::test]
    fn a_popped_out_channel_sees_new_posts_like_the_main_window(cx: &mut TestAppContext) {
        use std::sync::Arc;

        cx.update(crate::test_support::init);
        let mut main = Live::with_channel_for_tests();
        let (fork, _) = main.fork();
        // The engine's own stream stays out of it: the test hands both
        // windows the same change sets.
        let (_keep, quiet) = async_channel::unbounded();
        let model = main.open(main.row("c").unwrap());
        let main_window =
            cx.add_window(|window, cx| ChannelView::new(Some(model), "me", window, cx));
        let server = main.server().to_owned();
        let popped = cx.add_window(|window, cx| {
            PopOutView::new(
                Target::Channel("c".into()),
                server,
                Some((fork, quiet)),
                window,
                cx,
            )
        });
        cx.run_until_parked();

        let bodies = |cx: &mut TestAppContext| {
            let main = main_window
                .update(cx, |view, _, _| {
                    view.model().map(|model| {
                        model
                            .posts
                            .iter()
                            .map(|p| p.body.to_string())
                            .collect::<Vec<_>>()
                    })
                })
                .unwrap()
                .unwrap_or_default();
            let popped = popped
                .update(cx, |view, _, cx| {
                    let channel = view.channel_view().unwrap().read(cx);
                    channel
                        .model()
                        .map(|model| {
                            model
                                .posts
                                .iter()
                                .map(|p| p.body.to_string())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .unwrap();
            (main, popped)
        };
        let post = |id: &str, channel_id: &str, message: &str| matty_api::models::Post {
            id: id.into(),
            channel_id: channel_id.into(),
            user_id: "someone".into(),
            message: message.into(),
            create_at: 1_700_000_000_000,
            ..Default::default()
        };
        let mut posts = vec![post("p1", "c", "hello")];
        for next in [None, Some(post("p2", "c", "a new message"))] {
            posts.extend(next);
            let change = matty_sync::ChangeSet::Posts {
                channel_id: "c".into(),
                posts: Arc::from(posts.clone()),
            };
            // The main window applies it as `MainWindow::apply_update` does.
            let update = main.apply(&change);
            main_window
                .update(cx, |view, window, cx| match update.channel {
                    Some(ChannelUpdate::Replace(model)) => view.set_model(Some(model), window, cx),
                    Some(ChannelUpdate::Patch { appended, .. }) => view.append_posts(appended, cx),
                    None => {}
                })
                .unwrap();
            popped
                .update(cx, |view, window, cx| view.apply(&change, window, cx))
                .unwrap();
            cx.run_until_parked();
        }
        let (main_bodies, popped_bodies) = bodies(cx);
        assert_eq!(main_bodies, ["hello", "a new message"]);
        assert_eq!(popped_bodies, main_bodies);

        // The window saved its place, and closing it forgets it.
        let saved = |cx: &mut TestAppContext| {
            cx.update(|cx| SettingsStore::global(cx).read(cx).get().popouts.len())
        };
        assert_eq!(saved(cx), 1);
        popped
            .update(cx, |view, window, cx| view.close(window, cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(saved(cx), 0);
    }
}
