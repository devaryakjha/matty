//! The main window frame from PLAN §4.1 and DESIGN.md §4: a title bar that
//! holds navigation and the quick switcher, then the team rail, sidebar,
//! channel view and right pane. In M0 the rail and right pane stay hidden
//! (the rail appears with a second team, the right pane with threads and
//! search), the sidebar shows its loading rows and the channel view shows an
//! empty state until sync lands.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    Action, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, prelude::FluentBuilder as _, px,
};

use crate::account::{Account, AccountEvent, AccountView};
use crate::actions::{
    self, ChannelInfo, FocusSearch, HistoryBack, HistoryForward, MarkAllRead, NextChannel,
    NextUnreadChannel, OpenPreferences, Platform, PreviousChannel, PreviousUnreadChannel,
    QuickSwitcher, RecentMentions, ResetZoom, SavedMessages, Search, ShowShortcuts,
    ToggleRightPane, ToggleSidebar, ZoomIn, ZoomOut,
};
use crate::channel::{ChannelEvent, ChannelView};
use crate::drafts::{DraftsEvent, DraftsModel, DraftsView};
use crate::live::Live;
use crate::login::{LoginEvent, LoginModel, LoginStep, LoginView, SignInMethods, SsoProvider};
use crate::right_pane::{RightPaneContent, RightPaneEvent, RightPaneView, SearchModel};
use crate::settings::{
    ClockFormat, RIGHT_PANE_MAX, RIGHT_PANE_MIN, SIDEBAR_MAX, SIDEBAR_MIN, SettingsChanged,
    SettingsStore, ThemeChoice, WindowGeometry, ZOOM_STEP,
};
use crate::sidebar::{SidebarEvent, SidebarModel, SidebarView, Step};
use crate::style::{self, TypeStyleExt as _, metrics, scaled};
use crate::switcher::{Switcher, SwitcherEvent};
use crate::threads::{ThreadsEvent, ThreadsModel, ThreadsView};
use crate::ui::{
    button::{Button, ButtonSize, ButtonVariant},
    dialog::{DialogHandle, dialog, dialog_close, dialog_description, dialog_popup, dialog_title},
    kbd::{Kbd, format_shortcut},
    resizable::{PaneLimits, Resizable},
    switch::Switch,
    theme::UiTheme,
    title_bar::TitleBar,
    toggle_group::{ToggleGroup, ToggleGroupItem},
    tooltip::text_tooltip,
};

/// Whether Matty can reach the server (PLAN §4.5: network errors become a
/// banner, never a dialog). Sync sets it; the window only draws it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ConnectionState {
    #[default]
    Online,
    /// Trying again on its own; `attempt` counts from 1.
    Reconnecting { attempt: u32 },
    /// Gave up until asked; cached content stays usable.
    Offline,
}

/// `chat.example.com` from `https://chat.example.com`.
fn host(url: &str) -> String {
    url.split_once("://")
        .map_or(url, |(_, rest)| rest)
        .to_string()
}

fn event_url(event: &LoginEvent) -> String {
    match event {
        LoginEvent::CheckServer(url) => url.clone(),
        _ => String::new(),
    }
}

/// The channel view's minimum width beside the sidebar.
const CONTENT_MIN: f32 = 260.;
/// The width of gpuicn's Resizable handle, counted when turning the
/// persisted right-pane width into the channel width the split wants.
const SPLIT_HANDLE: f32 = 8.;
/// The text size presets the preferences dialog offers, within the
/// 0.8–1.5 range settings accept.
const TEXT_SIZES: &[(&str, f32)] = &[
    ("small", 0.9),
    ("default", 1.),
    ("large", 1.15),
    ("larger", 1.3),
];

pub struct MainWindow {
    settings: Entity<SettingsStore>,
    /// Shown until a session exists; sync replaces it with the workspace.
    login: Option<Entity<LoginView>>,
    sidebar: Entity<SidebarView>,
    channel: Entity<ChannelView>,
    switcher: Entity<Switcher>,
    right_pane: Entity<RightPaneView>,
    /// The title bar avatar and its menu: presence, status, sign out.
    account: Entity<AccountView>,
    focus: FocusHandle,
    /// The channel the sidebar selected; the channel view follows it in M1.
    selected_channel: Option<gpui_kit::SharedString>,
    /// Cmd+Shift+D hides the sidebar for this session.
    sidebar_visible: bool,
    shortcuts: DialogHandle,
    preferences: DialogHandle,
    /// Shown when the user belongs to more than one team (M1).
    rail_visible: bool,
    /// Holds threads, search results and the other right-pane views (M1).
    right_pane_open: bool,
    /// The Threads view stands in for the channel while the sidebar's
    /// Threads row is chosen.
    threads: Entity<ThreadsView>,
    threads_open: bool,
    drafts: Entity<DraftsView>,
    drafts_open: bool,
    connection: ConnectionState,
    /// Channels opened in order, for Back and Forward; `history_at` is the
    /// one shown now.
    history: Vec<SharedString>,
    history_at: usize,
    /// The sync engine, once there is an account to show.
    live: Option<Live>,
    _live_task: Option<gpui_kit::Task<()>>,
    /// The server the sign-in card checked, for its next step.
    login_server: Option<String>,
    sidebar_shown: bool,
}

impl MainWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = SettingsStore::global(cx);
        cx.observe_window_appearance(window, |_, window, cx| crate::apply_theme(window, cx))
            .detach();
        cx.subscribe_in(
            &settings,
            window,
            |_, _, _: &SettingsChanged, window, cx| {
                crate::apply_theme(window, cx);
                cx.notify();
            },
        )
        .detach();
        cx.observe_window_bounds(window, |this, window, cx| this.remember_bounds(window, cx))
            .detach();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let model = if crate::preview::enabled() {
            crate::preview::sidebar()
        } else {
            SidebarModel::loading()
        };
        let selected_channel = model.selected.clone();
        let switcher = cx.new(|cx| Switcher::new(&model, window, cx));
        cx.subscribe_in(
            &switcher,
            window,
            |this, _, event: &SwitcherEvent, window, cx| match event {
                SwitcherEvent::Open(id) => this.open_channel(id.clone(), window, cx),
                SwitcherEvent::Search(query) => this.search(query.clone(), window, cx),
            },
        )
        .detach();
        let directory: Vec<_> = model
            .categories
            .iter()
            .flat_map(|category| category.rows.iter().cloned())
            .collect();
        let sidebar = cx.new(|_| SidebarView::new("Matty", model));
        let initial = selected_channel
            .as_deref()
            .filter(|_| crate::preview::enabled())
            .and_then(crate::preview::channel);
        // The display name comes from the session once sync owns it.
        let me = if crate::preview::enabled() {
            "Arya"
        } else {
            "You"
        };
        let channel = cx.new(|cx| ChannelView::new(initial, me, window, cx));
        channel.update(cx, |channel, cx| channel.set_directory(directory, cx));
        let right_pane = cx.new(|cx| RightPaneView::new(me, window, cx));
        let account = cx.new(|cx| AccountView::new(crate::preview::account(), window, cx));
        cx.subscribe_in(
            &account,
            window,
            |this, _, event: &AccountEvent, window, cx| match event {
                // The menu already shows the change.
                AccountEvent::SetPresence(presence) => {
                    if let Some(live) = &this.live {
                        live.send(matty_sync::Command::SetStatus {
                            status: crate::live::status(*presence).into(),
                            dnd_end_time: None,
                        });
                    }
                }
                // Custom status text has no sync command yet (#136).
                AccountEvent::SetStatus(_) => {}
                AccountEvent::SignOut => this.sign_out(window, cx),
            },
        )
        .detach();
        let threads_model = if crate::preview::enabled() {
            crate::preview::threads()
        } else {
            ThreadsModel::default()
        };
        let threads = cx.new(|cx| ThreadsView::new(threads_model, me, window, cx));
        let drafts_model = if crate::preview::enabled() {
            crate::preview::drafts()
        } else {
            DraftsModel::default()
        };
        let drafts = cx.new(|_| DraftsView::new(drafts_model));
        cx.subscribe_in(
            &drafts,
            window,
            |this, drafts, event: &DraftsEvent, window, cx| {
                // Sync sends or discards the draft; the sidebar count follows.
                let count = drafts.read(cx).model().drafts.len();
                this.sidebar.update(cx, |sidebar, cx| {
                    let mut model = sidebar.model().clone();
                    model.drafts = count;
                    sidebar.set_model(model, cx);
                });
                if let DraftsEvent::Open(draft) = event {
                    this.open_channel(draft.channel_id.clone(), window, cx);
                    if let Some(root) = draft.thread_root.clone() {
                        this.open_thread(root, cx);
                    }
                }
            },
        )
        .detach();
        cx.subscribe(&threads, |this, _, event: &ThreadsEvent, cx| {
            // Sync owns the server side; the sidebar count follows at once.
            if let ThreadsEvent::MarkAllRead = event {
                this.sidebar.update(cx, |sidebar, cx| {
                    let mut model = sidebar.model().clone();
                    model.unread_threads = 0;
                    sidebar.set_model(model, cx);
                });
            }
        })
        .detach();
        // No session store yet: the sign-in card shows unless the preview
        // fixtures stand in for a signed-in workspace.
        let login_model = match crate::preview::login() {
            Some(model) => Some(model),
            None if crate::preview::enabled() => None,
            None => Some(LoginModel::default()),
        };
        let login = login_model.map(|model| {
            let view = cx.new(|cx| LoginView::new(model, window, cx));
            cx.subscribe_in(&view, window, Self::on_login_event)
                .detach();
            view
        });
        cx.subscribe_in(
            &channel,
            window,
            |this, _, event: &ChannelEvent, window, cx| match event {
                ChannelEvent::OpenThread(post_id) => this.open_thread(post_id.clone(), cx),
                ChannelEvent::OpenPinned => this.show_pinned(cx),
                ChannelEvent::OpenDirect { display_name, .. } => {
                    this.open_direct(display_name.clone(), window, cx)
                }
                ChannelEvent::Send { text, files } => {
                    if let (Some(live), Some(channel)) = (&this.live, &this.selected_channel) {
                        live.send(matty_sync::Command::SendPost {
                            pending_post_id: String::new(),
                            channel_id: channel.to_string(),
                            root_id: String::new(),
                            message: text.to_string(),
                            file_ids: files.iter().map(ToString::to_string).collect(),
                        });
                    }
                }
                ChannelEvent::ToggleReaction { post, emoji } => {
                    if let Some(live) = &this.live {
                        live.send(matty_sync::Command::ToggleReaction {
                            post_id: post.to_string(),
                            emoji_name: crate::transcript::emoji_name(emoji),
                        });
                    }
                }
                ChannelEvent::Delete(post) => {
                    if let Some(live) = &this.live {
                        live.send(matty_sync::Command::DeletePost {
                            post_id: post.to_string(),
                        });
                    }
                }
                // Sync picks these up in later changes; the view already
                // shows the change.
                ChannelEvent::Upload { .. }
                | ChannelEvent::CancelUpload(_)
                | ChannelEvent::RetryUpload(_)
                | ChannelEvent::OpenFile(_)
                | ChannelEvent::ToggleSave(_)
                | ChannelEvent::TogglePin(_)
                | ChannelEvent::Edit(_)
                | ChannelEvent::Retry(_)
                | ChannelEvent::ToggleFollow { .. } => {}
            },
        )
        .detach();
        cx.subscribe_in(
            &right_pane,
            window,
            |this, _, event: &RightPaneEvent, window, cx| match event {
                RightPaneEvent::Close => {
                    this.right_pane_open = false;
                    cx.notify();
                }
                RightPaneEvent::Search(query) => this.search(query.clone(), window, cx),
                // The post itself scrolls into view once sync can page to it.
                RightPaneEvent::Jump { channel, .. } => {
                    this.open_channel(channel.clone(), window, cx)
                }
                // Sync owns the rest (PLAN §4.3); the pane already shows them.
                _ => {}
            },
        )
        .detach();
        cx.subscribe_in(
            &sidebar,
            window,
            |this, _, event: &SidebarEvent, window, cx| {
                let id = match event {
                    SidebarEvent::Select(id) => id.clone(),
                    SidebarEvent::OpenThreads => return this.show_threads(cx),
                    SidebarEvent::OpenDrafts => return this.show_drafts(cx),
                };
                this.open_channel(id, window, cx);
            },
        )
        .detach();
        let history: Vec<SharedString> = selected_channel.iter().cloned().collect();
        let mut this = Self {
            settings,
            login,
            sidebar,
            channel,
            switcher,
            right_pane,
            account,
            focus,
            selected_channel,
            sidebar_visible: true,
            shortcuts: DialogHandle::new(false),
            preferences: DialogHandle::new(false),
            rail_visible: crate::preview::enabled(),
            right_pane_open: false,
            threads,
            threads_open: false,
            drafts,
            drafts_open: false,
            connection: crate::preview::connection(),
            history,
            history_at: 0,
            live: None,
            _live_task: None,
            login_server: None,
            sidebar_shown: false,
        };
        // `matty --offline` opens the last account's cache without a session.
        let saved = this.settings.read(cx).get().account.clone();
        if crate::offline()
            && !crate::preview::enabled()
            && let Some(account) = saved
        {
            this.login = None;
            this.connection = ConnectionState::Offline;
            this.start_live(&account, window, cx);
            if let Some(live) = &this.live {
                live.send(matty_sync::Command::ShowCache);
            }
        }
        this
    }

    /// Starts sync over `account`'s database and applies what it publishes.
    fn start_live(
        &mut self,
        account: &crate::settings::SavedAccount,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = crate::live::database_path(account);
        let (live, changes) = match Live::start(path, account) {
            Ok(started) => started,
            Err(error) => {
                tracing::error!(%error, "could not start sync");
                return;
            }
        };
        self.live = Some(live);
        self.account.update(cx, |view, cx| {
            view.set_account(
                Account {
                    display_name: account.display_name.clone().into(),
                    username: account.username.clone().into(),
                    presence: if crate::offline() {
                        crate::sidebar::Presence::Offline
                    } else {
                        crate::sidebar::Presence::Online
                    },
                    status_text: None,
                },
                cx,
            )
        });
        self._live_task = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(change) = changes.recv().await {
                let applied =
                    this.update_in(cx, |this, window, cx| this.apply(&change, window, cx));
                if applied.is_err() {
                    break;
                }
            }
        }));
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
        if let Some(name) = update.team_name {
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.set_team_name(name, cx));
        }
        if let Some(categories) = update.categories {
            if !self.sidebar_shown {
                // Launch-to-sidebar time, read against "matty starting" (#54).
                tracing::info!("first sidebar from sync shown");
                self.sidebar_shown = true;
            }
            let selected = self.selected_channel.clone();
            let model = self.sidebar.update(cx, |sidebar, cx| {
                let mut model = sidebar.model().clone();
                model.loading = false;
                model.categories = categories;
                model.selected = selected;
                sidebar.set_model(model, cx);
                sidebar.model().clone()
            });
            self.switcher
                .update(cx, |switcher, cx| switcher.set_model(&model, cx));
        }
        if let Some(model) = update.channel {
            self.channel
                .update(cx, |view, cx| view.set_model(Some(model), window, cx));
        }
        if let Some(state) = update.connection {
            self.set_connection(state, cx);
        }
        if update.signed_out {
            self.live = None;
            self._live_task = None;
        }
    }

    /// The sign-in card's requests. The preview answers locally; otherwise
    /// the server is checked and signed in to on sync's I/O thread, and a
    /// session starts sync over that account's cache.
    fn on_login_event(
        this: &mut Self,
        view: &Entity<LoginView>,
        event: &LoginEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if crate::preview::enabled() {
            return Self::on_preview_login_event(view, event, window, cx);
        }
        let mut model = view.read(cx).model().clone();
        model.error = None;
        match event {
            LoginEvent::CheckServer(url) => {
                model.busy = true;
                let url = url.clone();
                let view = view.downgrade();
                cx.spawn_in(window, async move |_, cx| {
                    let checked = crate::live::check_server(url.clone()).await;
                    let _ = view.update_in(cx, |view, window, cx| {
                        let mut model = view.model().clone();
                        model.busy = false;
                        match checked {
                            Ok(capabilities) => {
                                model.step = LoginStep::Methods {
                                    server_name: capabilities
                                        .site_name
                                        .clone()
                                        .unwrap_or_else(|| host(&url))
                                        .into(),
                                    methods: crate::live::methods(&capabilities),
                                };
                                model.error = None;
                            }
                            Err(error) => model.error = Some(error.into()),
                        }
                        view.set_model(model, window, cx);
                    });
                })
                .detach();
                this.login_server = Some(event_url(event));
            }
            LoginEvent::ChangeServer => {
                model.step = LoginStep::Server;
                this.login_server = None;
            }
            LoginEvent::SignIn { login_id, password } => {
                let Some(server) = this.login_server.clone() else {
                    model.step = LoginStep::Server;
                    return view.update(cx, |view, cx| view.set_model(model, window, cx));
                };
                model.busy = true;
                let signing_in =
                    crate::live::sign_in(server.clone(), login_id.clone(), password.clone());
                this.finish_sign_in(server, signing_in, view, window, cx);
            }
            LoginEvent::Token(token) => {
                let Some(server) = this.login_server.clone() else {
                    model.step = LoginStep::Server;
                    return view.update(cx, |view, cx| view.set_model(model, window, cx));
                };
                model.busy = true;
                let signing_in = crate::live::sign_in_with_token(server.clone(), token.clone());
                this.finish_sign_in(server, signing_in, view, window, cx);
            }
            LoginEvent::Sso(provider) => {
                // The browser flow is #75.
                model.error = Some(
                    format!(
                        "{} sign-in is not in Matty yet. Use a password or a personal access token.",
                        provider.label()
                    )
                    .into(),
                );
            }
        }
        view.update(cx, |view, cx| view.set_model(model, window, cx));
    }

    /// Waits for `signing_in`, then either starts sync and leaves the card
    /// or shows the card's error.
    fn finish_sign_in(
        &mut self,
        server: String,
        signing_in: impl std::future::Future<Output = Result<crate::live::Session, String>> + 'static,
        view: &Entity<LoginView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = view.downgrade();
        cx.spawn_in(window, async move |this, cx| {
            let signed_in = signing_in.await;
            let _ = this.update_in(cx, |this, window, cx| match signed_in {
                Ok(session) => {
                    let account = crate::live::saved_account(&server, &session.user);
                    this.settings.update(cx, |settings, cx| {
                        settings.update(cx, |s| s.account = Some(account.clone()))
                    });
                    this.start_live(&account, window, cx);
                    if let Some(live) = &this.live {
                        live.send(matty_sync::Command::Connect {
                            client: session.client,
                        });
                    }
                    this.login = None;
                    this.login_server = None;
                    cx.notify();
                }
                Err(error) => {
                    let _ = view.update(cx, |view, cx| {
                        let mut model = view.model().clone();
                        model.busy = false;
                        model.error = Some(error.into());
                        view.set_model(model, window, cx);
                    });
                }
            });
        })
        .detach();
    }

    /// The preview's card validates locally: a good URL moves to the
    /// methods step with fixed methods, and signing in says it is a preview.
    fn on_preview_login_event(
        view: &Entity<LoginView>,
        event: &LoginEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut model = view.read(cx).model().clone();
        match event {
            LoginEvent::CheckServer(url) => {
                model.step = LoginStep::Methods {
                    server_name: host(url).into(),
                    methods: SignInMethods {
                        password: true,
                        ldap: false,
                        sso: vec![SsoProvider::GitLab],
                    },
                };
                model.error = None;
            }
            LoginEvent::ChangeServer => {
                model.step = LoginStep::Server;
                model.error = None;
            }
            LoginEvent::SignIn { .. } | LoginEvent::Token(_) | LoginEvent::Sso(_) => {
                model.error = Some("The preview does not sign in to a server.".into());
            }
        }
        view.update(cx, |view, cx| view.set_model(model, window, cx));
    }

    /// Shows `id` in the channel view and marks it in the sidebar. Until
    /// sync lands only the preview fixtures have content.
    fn open_channel(
        &mut self,
        id: gpui_kit::SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_channel.as_ref() != Some(&id) {
            self.history.truncate(self.history_at + 1);
            self.history.push(id.clone());
            self.history_at = self.history.len() - 1;
        }
        self.show_channel(id, window, cx);
    }

    /// Shows `id` without touching the history: Back and Forward use it.
    /// Shows the unsent messages in place of the channel.
    fn show_drafts(&mut self, cx: &mut Context<Self>) {
        self.drafts_open = true;
        self.threads_open = false;
        self.right_pane_open = false;
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.select(Some(crate::sidebar::DRAFTS_ID.into()), cx)
        });
        cx.notify();
    }

    /// Shows the followed threads in place of the channel.
    fn show_threads(&mut self, cx: &mut Context<Self>) {
        self.threads_open = true;
        self.drafts_open = false;
        self.right_pane_open = false;
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.select(Some(crate::sidebar::THREADS_ID.into()), cx)
        });
        cx.notify();
    }

    fn show_channel(
        &mut self,
        id: gpui_kit::SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.threads_open = false;
        self.drafts_open = false;
        let previous = self.selected_channel.replace(id.clone());
        if let Some(live) = &self.live
            && previous.as_ref() != Some(&id)
        {
            live.send(matty_sync::Command::ViewChannel {
                channel_id: id.to_string(),
                prev_channel_id: previous.map(|p| p.to_string()).unwrap_or_default(),
            });
        }
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.select(Some(id.clone()), cx));
        if crate::preview::enabled() {
            let model = crate::preview::channel(&id);
            self.channel
                .update(cx, |view, cx| view.set_model(model, window, cx));
        } else if let Some(live) = self.live.as_mut()
            && let Some(row) = live.row(&id)
        {
            let model = live.open(row);
            self.channel
                .update(cx, |view, cx| view.set_model(Some(model), window, cx));
        }
        cx.notify();
    }

    fn go_history(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.history_at.checked_add_signed(delta) else {
            return;
        };
        let Some(id) = self.history.get(at).cloned() else {
            return;
        };
        self.history_at = at;
        self.show_channel(id, window, cx);
    }

    /// Alt+Up/Down and Alt+Shift+Up/Down from PLAN §4.4.
    fn step_channel(
        &mut self,
        step: Step,
        unread_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next = self.sidebar.read(cx).model().neighbor(step, unread_only);
        if let Some(id) = next {
            self.open_channel(id, window, cx);
        }
    }

    /// Opens the thread under `post_id` in the right pane. Until sync lands
    /// only the preview fixtures have threads.
    fn open_thread(&mut self, post_id: gpui_kit::SharedString, cx: &mut Context<Self>) {
        let Some(channel_id) = self.selected_channel.clone() else {
            return;
        };
        let thread = crate::preview::enabled()
            .then(|| crate::preview::thread(&channel_id, &post_id))
            .flatten();
        self.right_pane.update(cx, |pane, cx| {
            pane.set_content(thread.map(RightPaneContent::Thread), cx)
        });
        self.right_pane_open = true;
        cx.notify();
    }

    /// Opens the direct message with `display_name`; until sync can create
    /// one, only an existing sidebar row can be opened.
    fn open_direct(
        &mut self,
        display_name: gpui_kit::SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self
            .sidebar
            .read(cx)
            .model()
            .categories
            .iter()
            .flat_map(|category| category.rows.iter())
            .find(|row| {
                matches!(row.kind, crate::sidebar::ChannelKind::Direct { .. })
                    && row.name == display_name
            })
            .map(|row| row.id.clone());
        if let Some(id) = id {
            self.open_channel(id, window, cx);
        }
    }

    fn show_listing(&mut self, content: RightPaneContent, cx: &mut Context<Self>) {
        self.threads_open = false;
        self.drafts_open = false;
        self.right_pane
            .update(cx, |pane, cx| pane.set_content(Some(content), cx));
        self.right_pane_open = true;
        cx.notify();
    }

    /// Runs a message search in the right pane. Until sync owns it, the
    /// preview answers from its fixtures and a live session shows the
    /// searching state.
    fn search(&mut self, query: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let preview = crate::preview::enabled();
        let model = SearchModel {
            results: if preview {
                crate::preview::search(&query)
            } else {
                vec![]
            },
            searching: !preview && !query.is_empty(),
            query,
        };
        self.threads_open = false;
        self.drafts_open = false;
        self.right_pane
            .update(cx, |pane, cx| pane.set_search(model, window, cx));
        self.right_pane_open = true;
        cx.notify();
    }

    /// Cmd+F: back to the search field, keeping a search that is open.
    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = match self.right_pane.read(cx).content() {
            Some(RightPaneContent::Search(model)) if self.right_pane_open => Some(model.clone()),
            _ => None,
        };
        match current {
            Some(model) => self
                .right_pane
                .update(cx, |pane, cx| pane.set_search(model, window, cx)),
            None => self.search("".into(), window, cx),
        }
    }

    fn show_pinned(&mut self, cx: &mut Context<Self>) {
        let Some(channel) = self.channel.read(cx).model().cloned() else {
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
    }

    fn show_saved(&mut self, cx: &mut Context<Self>) {
        let posts = if crate::preview::enabled() {
            crate::preview::saved()
        } else {
            vec![]
        };
        self.show_listing(RightPaneContent::Saved(posts), cx);
    }

    fn show_mentions(&mut self, cx: &mut Context<Self>) {
        let posts = if crate::preview::enabled() {
            crate::preview::mentions()
        } else {
            vec![]
        };
        self.show_listing(RightPaneContent::Mentions(posts), cx);
    }

    /// Shows the current channel's info in the right pane.
    fn show_channel_info(&mut self, cx: &mut Context<Self>) {
        let info = self.channel.read(cx).model().cloned();
        self.right_pane.update(cx, |pane, cx| {
            pane.set_content(info.map(RightPaneContent::ChannelInfo), cx)
        });
        self.right_pane_open = true;
        cx.notify();
    }

    /// The channel view, with the right pane beside it when open. gpuicn's
    /// Resizable sizes its first pane, so the persisted right-pane width is
    /// turned into a channel width from the space the split has.
    fn content(&self, window: &Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        if self.threads_open {
            return self.threads.clone().into_any_element();
        }
        if self.drafts_open {
            return self.drafts.clone().into_any_element();
        }
        if !self.right_pane_open {
            return self.channel.clone().into_any_element();
        }
        let settings = self.settings.read(cx).get().clone();
        let theme = UiTheme::read(cx);
        let rail = if self.rail_visible {
            f32::from(scaled(theme, metrics::RAIL_WIDTH))
        } else {
            0.
        };
        let sidebar = if self.sidebar_visible {
            settings.sidebar_width + SPLIT_HANDLE
        } else {
            0.
        };
        let extent = f32::from(window.viewport_size().width) - rail - sidebar - SPLIT_HANDLE;
        let channel_width = (extent - settings.right_pane_width).max(CONTENT_MIN);
        let store = self.settings.clone();
        Resizable::new("main.right-split", "Resize right pane", px(channel_width))
            .first(self.channel.clone())
            .second(self.right_pane.clone())
            .first_limits(PaneLimits::new(px(CONTENT_MIN), px(100_000.)))
            .second_limits(PaneLimits::new(px(RIGHT_PANE_MIN), px(RIGHT_PANE_MAX)))
            .on_resize(move |width: &Pixels, _, cx| {
                let right = (extent - f32::from(*width)).clamp(RIGHT_PANE_MIN, RIGHT_PANE_MAX);
                store.update(cx, |store, cx| {
                    store.update(cx, |s| s.right_pane_width = right)
                });
            })
            .into_any_element()
    }

    fn remember_bounds(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let maximized = window.is_maximized();
        let bounds = window.window_bounds().get_bounds();
        if window.is_fullscreen() {
            return;
        }
        let geometry = WindowGeometry {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
            maximized,
        };
        self.settings.update(cx, |store, cx| {
            store.update(cx, |s| s.window = Some(geometry))
        });
    }

    fn zoom(&mut self, step: Option<f32>, cx: &mut Context<Self>) {
        self.settings.update(cx, |store, cx| {
            store.update(cx, |s| {
                s.zoom = match step {
                    Some(step) => ((s.zoom + step) * 10.).round() / 10.,
                    None => 1.,
                }
            })
        });
    }

    /// The keystroke bound to the action with this label in the shortcut
    /// sheet, formatted for the current platform.
    fn shortcut_for(label: &str) -> Option<String> {
        let platform = Platform::current();
        actions::bindings()
            .into_iter()
            .find(|row| row.label == label)
            .map(|row| {
                let os = match platform {
                    Platform::MacOs => gpui_kit::OperatingSystem::Mac,
                    Platform::Other => gpui_kit::OperatingSystem::Linux,
                };
                format_shortcut(row.keys(platform), os).to_string()
            })
    }

    /// A ghost icon button that dispatches `action`, with a tooltip naming
    /// it and its shortcut when the keymap has one.
    fn icon_button(
        id: &'static str,
        label: &'static str,
        glyph: LucideIcon,
        action: Box<dyn Action>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = UiTheme::read(cx);
        let tip: SharedString = match Self::shortcut_for(label) {
            Some(keys) => format!("{label}  {keys}").into(),
            None => label.into(),
        };
        let button = Button::new(id)
            .variant(ButtonVariant::Ghost)
            .size(ButtonSize::IconSm)
            .aria_label(label)
            .disabled(!enabled)
            .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
            .child(
                lucide(glyph)
                    .size(scaled(theme, metrics::ICON))
                    .text_color(theme.colors.muted_foreground),
            );
        div()
            .id(SharedString::from(format!("{id}.tooltip")))
            .tooltip(move |_, cx| text_tooltip(tip.clone(), cx))
            .child(button)
    }

    /// DESIGN.md §4: navigation on the left, the quick switcher as a search
    /// pill in the middle, mentions, saved, help and the user on the right.
    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Signed out there is nothing to navigate: window controls only.
        if self.login.is_some() {
            return TitleBar::new().into_any_element();
        }
        let theme = UiTheme::read(cx).clone();
        let platform = Platform::current();
        let switcher_keys = Self::shortcut_for("Quick switcher").unwrap_or_default();
        let switcher = Button::new("titlebar.switcher")
            .variant(ButtonVariant::Outline)
            .size(ButtonSize::Sm)
            .aria_label(format!("Search Matty, {switcher_keys}"))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(QuickSwitcher), cx))
            .flex_1()
            .max_w(scaled(&theme, metrics::SWITCHER_MAX_WIDTH))
            .justify_start()
            .bg(theme.colors.background)
            .text_color(theme.colors.muted_foreground)
            .font_weight(gpui_kit::FontWeight::NORMAL)
            .child(
                lucide(LucideIcon::Search)
                    .size(scaled(&theme, 14.))
                    .text_color(theme.colors.muted_foreground),
            )
            .child(div().flex_1().text_left().child("Search Matty"))
            .child(Kbd::shortcut(match platform {
                Platform::MacOs => "cmd-k",
                Platform::Other => "ctrl-k",
            }));
        let me = self.account.clone();
        TitleBar::new()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space(0.5))
                    .flex_shrink_0()
                    .child(Self::icon_button(
                        "titlebar.back",
                        "Back",
                        LucideIcon::ChevronLeft,
                        Box::new(HistoryBack),
                        self.history_at > 0,
                        cx,
                    ))
                    .child(Self::icon_button(
                        "titlebar.forward",
                        "Forward",
                        LucideIcon::ChevronRight,
                        Box::new(HistoryForward),
                        self.history_at + 1 < self.history.len(),
                        cx,
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .justify_center()
                    .px(theme.space(2.))
                    .child(switcher),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space(0.5))
                    .pr(theme.space(2.))
                    .flex_shrink_0()
                    .child(Self::icon_button(
                        "titlebar.mentions",
                        "Recent mentions",
                        LucideIcon::AtSign,
                        Box::new(RecentMentions),
                        true,
                        cx,
                    ))
                    .child(Self::icon_button(
                        "titlebar.saved",
                        "Saved messages",
                        LucideIcon::Bookmark,
                        Box::new(SavedMessages),
                        true,
                        cx,
                    ))
                    .child(Self::icon_button(
                        "titlebar.help",
                        "Keyboard shortcuts",
                        LucideIcon::CircleQuestionMark,
                        Box::new(ShowShortcuts),
                        true,
                        cx,
                    ))
                    .child(div().pl(theme.space(1.)).child(me)),
            )
            .into_any_element()
    }

    /// Sign out returns to the sign-in card at once; sync ends the session
    /// on the server and then stops. The cache stays on disk until #160.
    fn sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.login.is_some() {
            return;
        }
        match &self.live {
            Some(live) => live.send(matty_sync::Command::SignOut),
            None => self._live_task = None,
        }
        self.settings
            .update(cx, |settings, cx| settings.update(cx, |s| s.account = None));
        self.selected_channel = None;
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_model(SidebarModel::loading(), cx)
        });
        let view = cx.new(|cx| LoginView::new(LoginModel::default(), window, cx));
        cx.subscribe_in(&view, window, Self::on_login_event)
            .detach();
        self.login = Some(view);
        self.right_pane_open = false;
        cx.notify();
    }

    /// Replaces the connection state; sync calls this as it goes.
    pub fn set_connection(&mut self, state: ConnectionState, cx: &mut Context<Self>) {
        if self.connection != state {
            self.connection = state;
            cx.notify();
        }
    }

    /// A 32px strip under the title bar: `muted` while reconnecting with a
    /// spinner, `destructive`-tinted once offline with a Retry button.
    fn connection_banner(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let theme = UiTheme::read(cx).clone();
        let (icon, text, offline): (gpui_kit::AnyElement, SharedString, bool) =
            match &self.connection {
                ConnectionState::Online => return None,
                ConnectionState::Reconnecting { attempt } => (
                    crate::ui::spinner::Spinner::new("banner.spinner")
                        .color(theme.colors.muted_foreground)
                        .into_any_element(),
                    if *attempt <= 1 {
                        "Connection lost. Reconnecting…".into()
                    } else {
                        format!("Connection lost. Reconnecting (attempt {attempt})…").into()
                    },
                    false,
                ),
                ConnectionState::Offline => (
                    lucide(LucideIcon::WifiOff)
                        .size(scaled(&theme, 14.))
                        .text_color(theme.colors.destructive)
                        .into_any_element(),
                    "You're offline. Matty shows what it has and sends when you're back.".into(),
                    true,
                ),
            };
        let foreground = if offline {
            theme.colors.destructive
        } else {
            theme.colors.foreground
        };
        Some(
            div()
                .flex_shrink_0()
                .h(theme.space(8.))
                .px(scaled(&theme, metrics::GUTTER))
                .flex()
                .items_center()
                .justify_center()
                .gap(theme.space(2.))
                .bg(if offline {
                    theme.colors.destructive.opacity(0.1)
                } else {
                    theme.colors.muted
                })
                .border_b(style::hairline())
                .border_color(theme.colors.border)
                .type_style(style::UI, &theme)
                .text_color(foreground)
                .child(icon)
                .child(text)
                .when(offline, |row| {
                    row.child(
                        Button::new("banner.retry")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Xs)
                            .aria_label("Retry now")
                            .text_color(theme.colors.destructive)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_connection(
                                    ConnectionState::Reconnecting { attempt: 1 },
                                    cx,
                                )
                            }))
                            .child("Retry now"),
                    )
                }),
        )
    }

    fn shortcut_sheet(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let platform = Platform::current();
        let rows = actions::bindings().into_iter().map(|row| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(theme.space(6.))
                .h(scaled(&theme, metrics::ROW_HEIGHT))
                .type_style(style::UI, &theme)
                .child(row.label)
                .child(Kbd::shortcut(row.keys(platform)))
        });
        let popup = dialog_popup("shortcuts.popup", "Keyboard shortcuts", cx)
            .child(dialog_title("shortcuts.title", cx).child("Keyboard shortcuts"))
            .child(
                dialog_description("shortcuts.description", cx)
                    .child("Every shortcut works anywhere in the window unless it names a view."),
            )
            .child(div().flex().flex_col().children(rows))
            .child(dialog_close("shortcuts.close", cx).label("Close"));
        dialog("shortcuts", &self.shortcuts, popup, window, cx)
    }

    /// Preferences (DESIGN.md §5): appearance first, with every control a
    /// gpuicn one and every change written to settings straight away so
    /// the window re-themes behind the dialog.
    fn preferences(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let settings = self.settings.read(cx).get().clone();
        let store = self.settings.clone();
        let label = |text: &'static str| {
            div()
                .type_style(style::LABEL, &theme)
                .text_color(theme.colors.muted_foreground)
                .child(text.to_uppercase())
        };
        let section = |title: &'static str, control: gpui_kit::AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(theme.space(2.))
                .child(label(title))
                .child(control)
        };
        let theme_value: SharedString = match settings.theme {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
        }
        .into();
        let theme_item = |id: &'static str, value: &'static str, glyph: LucideIcon, text: &str| {
            ToggleGroupItem::new(id, value)
                .aria_label(text.to_string())
                .flex_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space(1.5))
                        .child(lucide(glyph).size(scaled(&theme, 14.)))
                        .child(text.to_string()),
                )
        };
        let theme_store = store.clone();
        let theme_group = ToggleGroup::new("preferences.theme")
            .aria_label("Theme")
            .joined(true)
            .w_full()
            .value([theme_value])
            .item(theme_item(
                "preferences.theme.system",
                "system",
                LucideIcon::Monitor,
                "System",
            ))
            .item(theme_item(
                "preferences.theme.light",
                "light",
                LucideIcon::Sun,
                "Light",
            ))
            .item(theme_item(
                "preferences.theme.dark",
                "dark",
                LucideIcon::Moon,
                "Dark",
            ))
            .on_change(move |next, _, cx| {
                let choice = match next.first().map(|v| v.as_ref()) {
                    Some("light") => ThemeChoice::Light,
                    Some("dark") => ThemeChoice::Dark,
                    _ => ThemeChoice::System,
                };
                theme_store.update(cx, |store, cx| store.update(cx, |s| s.theme = choice));
            });
        let size_value: SharedString = TEXT_SIZES
            .iter()
            .min_by(|a, b| {
                (a.1 - settings.text_scale)
                    .abs()
                    .total_cmp(&(b.1 - settings.text_scale).abs())
            })
            .map(|(name, _)| *name)
            .unwrap_or("default")
            .into();
        let size_store = store.clone();
        let mut size_group = ToggleGroup::new("preferences.text-size")
            .aria_label("Text size")
            .joined(true)
            .w_full()
            .value([size_value]);
        for (name, scale) in TEXT_SIZES {
            let text = format!("{}%", (scale * 100.).round());
            size_group = size_group.item(
                ToggleGroupItem::new(
                    gpui_kit::ElementId::Name(format!("preferences.text-size.{name}").into()),
                    *name,
                )
                .aria_label(format!("Text size {text}"))
                .flex_1()
                .child(text),
            );
        }
        let size_group = size_group.on_change(move |next, _, cx| {
            let scale = TEXT_SIZES
                .iter()
                .find(|(name, _)| next.first().map(|v| v.as_ref()) == Some(*name))
                .map(|(_, scale)| *scale)
                .unwrap_or(1.);
            size_store.update(cx, |store, cx| store.update(cx, |s| s.text_scale = scale));
        });
        let clock_value: SharedString = match settings.clock {
            ClockFormat::TwelveHour => "12",
            ClockFormat::TwentyFourHour => "24",
        }
        .into();
        let clock_store = store.clone();
        let clock_item = |id: &'static str, value: &'static str, text: &'static str| {
            ToggleGroupItem::new(id, value)
                .aria_label(text)
                .flex_1()
                .child(text)
        };
        let clock_group = ToggleGroup::new("preferences.clock")
            .aria_label("Clock")
            .joined(true)
            .w_full()
            .value([clock_value])
            .item(clock_item("preferences.clock.12", "12", "12-hour"))
            .item(clock_item("preferences.clock.24", "24", "24-hour"))
            .on_change(move |next, _, cx| {
                let clock = match next.first().map(|v| v.as_ref()) {
                    Some("12") => ClockFormat::TwelveHour,
                    _ => ClockFormat::TwentyFourHour,
                };
                clock_store.update(cx, |store, cx| store.update(cx, |s| s.clock = clock));
            });
        let switch_row =
            |id: &'static str,
             title: &'static str,
             detail: &'static str,
             checked: bool,
             write: fn(&mut crate::settings::Settings, bool)| {
                let store = store.clone();
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme.space(4.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.space(0.5))
                            .child(
                                div()
                                    .type_style(style::UI, &theme)
                                    .text_color(theme.colors.foreground)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .type_style(style::META, &theme)
                                    .text_color(theme.colors.muted_foreground)
                                    .child(detail),
                            ),
                    )
                    .child(
                        Switch::new(id)
                            .checked(checked)
                            .aria_label(title)
                            .on_change(move |next, _, _, cx| {
                                store.update(cx, |store, cx| store.update(cx, |s| write(s, next)))
                            }),
                    )
            };
        let popup = dialog_popup("preferences.popup", "Preferences", cx)
            .child(dialog_title("preferences.title", cx).child("Preferences"))
            .child(
                dialog_description("preferences.description", cx)
                    .child("Changes apply right away and are saved for next time."),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space(5.))
                    .pt(theme.space(1.))
                    .child(section("Theme", theme_group.into_any_element()))
                    .child(section("Text size", size_group.into_any_element()))
                    .child(section("Clock", clock_group.into_any_element()))
                    .child(section(
                        "Window",
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.space(3.))
                            .child(switch_row(
                                "preferences.close-to-tray",
                                "Keep running when closed",
                                "Closing the window leaves Matty in the tray.",
                                settings.close_to_tray,
                                |s, v| s.close_to_tray = v,
                            ))
                            .child(switch_row(
                                "preferences.launch-at-login",
                                "Open at login",
                                "Start Matty when you sign in.",
                                settings.launch_at_login,
                                |s, v| s.launch_at_login = v,
                            ))
                            .into_any_element(),
                    )),
            )
            .child(dialog_close("preferences.close", cx).label("Done"));
        dialog("preferences", &self.preferences, popup, window, cx)
    }

    /// DESIGN.md §4: 56px rail of 32px team squares, the current one
    /// outlined in `primary`. Hidden until the user has a second team.
    fn team_rail(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let teams = if crate::preview::enabled() {
            crate::preview::teams()
        } else {
            vec![("Matty", true)]
        };
        let squares = teams
            .into_iter()
            .enumerate()
            .map(|(index, (name, current))| {
                let initials: String = name
                    .split_whitespace()
                    .filter_map(|word| word.chars().next())
                    .take(2)
                    .collect();
                let square = Button::new(gpui_kit::ElementId::Name(
                    format!("rail.team.{index}").into(),
                ))
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::Icon)
                .aria_label(name)
                .rounded(theme.radius.lg)
                .bg(if current {
                    theme.colors.primary
                } else {
                    theme.colors.sidebar_accent
                })
                .text_color(if current {
                    theme.colors.primary_foreground
                } else {
                    theme.colors.sidebar_foreground
                })
                .type_style(style::UI_STRONG, &theme)
                .child(initials);
                // The current team carries a 2px marker in `primary` on the rail's
                // edge, so the square itself stays a plain tile.
                div()
                    .relative()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .id(gpui_kit::ElementId::Name(
                                format!("rail.team.{index}.tip").into(),
                            ))
                            .tooltip(move |_, cx| text_tooltip(name.into(), cx))
                            .child(square),
                    )
                    .when(current, |cell| {
                        cell.child(
                            div()
                                .absolute()
                                .left_0()
                                .top(theme.space(1.))
                                .bottom(theme.space(1.))
                                .w(px(2.))
                                .rounded_r(px(2.))
                                .bg(theme.colors.primary),
                        )
                    })
            });
        div()
            .w(scaled(&theme, metrics::RAIL_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(theme.space(2.))
            .py(theme.space(2.))
            .bg(theme.colors.sidebar)
            .border_r(style::hairline())
            .border_color(theme.colors.sidebar_border)
            .children(squares)
            .child(
                div()
                    .id("rail.add.tip")
                    .tooltip(move |_, cx| text_tooltip("Join another team".into(), cx))
                    .child(
                        Button::new("rail.add")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Icon)
                            .aria_label("Join another team")
                            .rounded(theme.radius.lg)
                            .border_1()
                            .border_dashed()
                            .border_color(theme.colors.border)
                            .child(
                                lucide(LucideIcon::Plus)
                                    .size(scaled(&theme, metrics::ICON))
                                    .text_color(theme.colors.muted_foreground),
                            ),
                    ),
            )
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let sidebar_width = px(self.settings.read(cx).get().sidebar_width);
        let settings = self.settings.clone();
        let content = self.content(window, cx);
        let frame = if self.sidebar_visible {
            Resizable::new("main.sidebar-split", "Resize sidebar", sidebar_width)
                .first(self.sidebar.clone())
                .second(content)
                .first_limits(PaneLimits::new(px(SIDEBAR_MIN), px(SIDEBAR_MAX)))
                .second_limits(PaneLimits::new(px(CONTENT_MIN), px(100_000.)))
                .on_resize(move |width: &Pixels, _, cx| {
                    let width = f32::from(*width);
                    settings.update(cx, |store, cx| {
                        store.update(cx, |s| s.sidebar_width = width)
                    });
                })
                .into_any_element()
        } else {
            content
        };
        let shortcuts = self.shortcuts.clone();
        let preferences = self.preferences.clone();
        let switcher = self.switcher.clone();
        let switcher_element = self.switcher.update(cx, |switcher, cx| {
            switcher.element(window, cx).into_any_element()
        });
        div()
            .key_context(actions::MAIN_WINDOW)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.sidebar_visible = !this.sidebar_visible;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(Some(ZOOM_STEP), cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(Some(-ZOOM_STEP), cx)))
            .on_action(cx.listener(|this, _: &ResetZoom, _, cx| this.zoom(None, cx)))
            .on_action(move |_: &ShowShortcuts, window, cx| shortcuts.open(window, cx))
            .on_action(
                cx.listener(|this, _: &HistoryBack, window, cx| this.go_history(-1, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &HistoryForward, window, cx| this.go_history(1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &PreviousChannel, window, cx| {
                this.step_channel(Step::Previous, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NextChannel, window, cx| {
                this.step_channel(Step::Next, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousUnreadChannel, window, cx| {
                this.step_channel(Step::Previous, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NextUnreadChannel, window, cx| {
                this.step_channel(Step::Next, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MarkAllRead, _, cx| {
                this.sidebar
                    .update(cx, |sidebar, cx| sidebar.mark_all_read(cx))
            }))
            .on_action(move |_: &OpenPreferences, window, cx| preferences.open(window, cx))
            .on_action(move |_: &QuickSwitcher, window, cx| {
                switcher.update(cx, |switcher, cx| switcher.open(window, cx))
            })
            .on_action(cx.listener(|this, _: &ToggleRightPane, _, cx| {
                this.right_pane_open = !this.right_pane_open;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ChannelInfo, _, cx| this.show_channel_info(cx)))
            .on_action(cx.listener(|this, _: &RecentMentions, _, cx| this.show_mentions(cx)))
            .on_action(cx.listener(|this, _: &SavedMessages, _, cx| this.show_saved(cx)))
            .on_action(
                cx.listener(|this, _: &Search, window, cx| this.search("".into(), window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FocusSearch, window, cx| this.focus_search(window, cx)),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.colors.background)
            .text_color(theme.colors.foreground)
            .font_family(theme.fonts.body.clone())
            .type_style(style::UI, &theme)
            .child(self.title_bar(cx))
            .children(if self.login.is_none() {
                self.connection_banner(cx)
            } else {
                None
            })
            .child(match &self.login {
                Some(login) => div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(login.clone())
                    .into_any_element(),
                None => div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .when(self.rail_visible, |row| row.child(self.team_rail(cx)))
                    .child(div().flex_1().min_w_0().h_full().child(frame))
                    .into_any_element(),
            })
            .child(self.shortcut_sheet(window, cx))
            .child(self.preferences(window, cx))
            .child(switcher_element)
    }
}
