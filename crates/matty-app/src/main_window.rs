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
    self, BrowseChannels, ChannelInfo, CreateChannel, DirectMessagesMenu, FocusSearch, HistoryBack,
    HistoryForward, MarkAllRead, MarkChannelRead, NextChannel, NextUnreadChannel, OpenPreferences,
    Platform, PreviousChannel, PreviousUnreadChannel, QuickSwitcher, RecentMentions, ResetZoom,
    SavedMessages, Search, ShowShortcuts, ToggleRightPane, ToggleSidebar, UnreadsFilter, ZoomIn,
    ZoomOut,
};
use crate::channel::{ChannelEvent, ChannelView};
use crate::directory::{BrowseView, CreateChannelView, DirectoryEvent, NewMessageView};
use crate::drafts::{DraftsEvent, DraftsModel, DraftsView};
use crate::live::{ChannelUpdate, Live};
use crate::login::{LoginEvent, LoginModel, LoginStep, LoginView, SignInMethods, SsoProvider};
use crate::rail::{RailEvent, RailView, Team};
use crate::right_pane::{RightPaneContent, RightPaneEvent, RightPaneView, SearchModel};
use crate::settings::{
    ClockFormat, LogLevel, MessageDisplay, RIGHT_PANE_MAX, RIGHT_PANE_MIN, SIDEBAR_MAX,
    SIDEBAR_MIN, SendKey, SettingsChanged, SettingsStore, ThemeChoice, WindowGeometry, ZOOM_STEP,
};
use crate::sidebar::{SidebarEvent, SidebarModel, SidebarView, Step};
use crate::style::{self, TypeStyleExt as _, metrics, scaled};
use crate::switcher::{Switcher, SwitcherEvent};
use crate::threads::{ThreadsEvent, ThreadsModel, ThreadsView};
use crate::ui::{
    button::{Button, ButtonSize, ButtonVariant},
    dialog::{DialogHandle, dialog, dialog_close, dialog_description, dialog_popup, dialog_title},
    drawer::{Drawer, DrawerContent, DrawerHandle, DrawerSide},
    field::Field,
    input::{InputEvent, InputState},
    kbd::Kbd,
    markdown::selection_root,
    resizable::{PaneLimits, Resizable},
    scroll_area::ScrollArea,
    select::{Select, SelectEvent, SelectItem, SelectState},
    switch::Switch,
    theme::{UiTheme, transition_value},
    title_bar::TitleBar,
    toast::{Notice, ToastState},
    toggle_group::{ToggleGroup, ToggleGroupItem},
};

/// The log detail choices: the setting, the picker's value and its label.
const LOG_LEVELS: [(LogLevel, &str, &str); 5] = [
    (LogLevel::Error, "error", "Errors only"),
    (LogLevel::Warn, "warn", "Warnings"),
    (LogLevel::Info, "info", "Normal"),
    (LogLevel::Debug, "debug", "Detailed"),
    (LogLevel::Trace, "trace", "Everything"),
];

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
/// How long the open channel waits after a post before it checks whether
/// the user saw it (#145).
const VIEW_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(1);

/// What decides whether the open channel counts as viewed (#145).
#[derive(Clone, Copy, Debug)]
struct Seen {
    /// A Matty window has focus.
    focused: bool,
    /// The channel is on screen, not Threads or Drafts.
    channel_shown: bool,
    /// A drawer covers the channel.
    covered: bool,
    /// The list is at its newest post.
    at_newest: bool,
    /// The sidebar marks the channel unread.
    unread: bool,
    /// The user marked it unread and has not left it since.
    kept_unread: bool,
}

impl Seen {
    fn views(self) -> bool {
        self.unread
            && !self.kept_unread
            && self.focused
            && self.channel_shown
            && !self.covered
            && self.at_newest
    }
}

/// Below this window width the right pane overlays the channel as a drawer
/// instead of splitting it (PLAN §4.1 narrow windows).
const NARROW_RIGHT_PANE: f32 = 900.;
/// Below this window width the sidebar collapses to a drawer opened from the
/// title bar (PLAN §4.1 narrow windows).
const NARROW_SIDEBAR: f32 = 640.;
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

/// What a toast's Retry button runs.
type Retry = Box<dyn Fn(&mut Window, &mut gpui_kit::App)>;

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
    /// Preferences' mention keywords, saved to the server on Enter or when
    /// the field loses focus (#192).
    keywords: Entity<InputState>,
    /// Preferences' log detail picker (#160).
    log_level: Entity<SelectState>,
    /// The user's `notify_props` as the server last sent them.
    notify_props: Option<std::collections::HashMap<String, String>>,
    /// Whether the user may choose collapsed reply threads, and whether
    /// they are on (#157).
    collapsed_threads: Option<(bool, bool)>,
    /// Where notifications go on this platform (#141).
    notifier: Box<dyn matty_notify::Notifier>,
    /// The Browse channels dialog, opened from the sidebar's plus menu.
    browse: Entity<BrowseView>,
    new_message: Entity<NewMessageView>,
    create_channel: Entity<CreateChannelView>,
    /// Channels joined from Browse while in preview, so they open with an
    /// empty transcript until sync owns joining.
    joined: Vec<crate::channel::ChannelModel>,
    /// The team rail, shown when the user belongs to more than one team.
    rail: Entity<RailView>,
    /// Holds threads, search results and the other right-pane views (M1).
    right_pane_open: bool,
    /// In a narrow window the right pane is this drawer over the channel;
    /// `right_drawer_shown` says whether the handle was opened for the
    /// current `right_pane_open`, so a dismissal from the drawer (Escape,
    /// the backdrop) closes the pane too.
    right_drawer: DrawerHandle,
    right_drawer_shown: bool,
    /// In a narrow window the sidebar is this drawer from the left edge,
    /// opened from the title bar or Cmd/Ctrl+Shift+D and closed by picking a
    /// channel.
    sidebar_drawer: DrawerHandle,
    sidebar_drawer_open: bool,
    sidebar_drawer_shown: bool,
    /// The Threads view stands in for the channel while the sidebar's
    /// Threads row is chosen.
    threads: Entity<ThreadsView>,
    /// The notification queue, mounted once above the page (PLAN §4.5:
    /// results and failures of background work are toasts, never dialogs).
    toasts: Entity<ToastState>,
    /// Files asked for and not yet saved, oldest first, so a failed
    /// download can offer Retry when only one was in flight.
    downloads: std::collections::VecDeque<SharedString>,
    threads_open: bool,
    drafts: Entity<DraftsView>,
    drafts_open: bool,
    connection: ConnectionState,
    /// What the banner last said, kept while it slides away after reconnecting.
    banner_last: ConnectionState,
    /// Messages waiting to send, for the banner (#195).
    queued: usize,
    /// Channels opened in order, for Back and Forward; `history_at` is the
    /// one shown now.
    history: Vec<SharedString>,
    history_at: usize,
    /// The sync engine, once there is an account to show.
    live: Option<Live>,
    _live_task: Option<gpui_kit::Task<()>>,
    /// The signed-out account's database and media folder, deleted once
    /// sync has ended the session (#160).
    forget: Option<(std::path::PathBuf, Option<std::path::PathBuf>)>,
    /// Clears typing notices as they run out.
    _typing_task: Option<gpui_kit::Task<()>>,
    /// Views the open channel a moment after a post arrives in it, if the
    /// user can see it then (#145).
    _view_task: Option<gpui_kit::Task<()>>,
    /// The open channel the user marked unread: it is not viewed again
    /// until they switch away and back (#146).
    kept_unread: Option<gpui_kit::SharedString>,
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
        // Coming back to the window views the open channel if its newest
        // posts are on screen (#145).
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.view_if_seen(window, cx);
            }
        })
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
                SwitcherEvent::Forward { post, channel } => this.forward(post, channel, cx),
            },
        )
        .detach();
        let directory: Vec<_> = model
            .categories
            .iter()
            .flat_map(|category| category.rows.iter().cloned())
            .collect();
        let sidebar = cx.new(|cx| SidebarView::new("Matty", model, cx));
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
        let browse = cx.new(|cx| BrowseView::new("Matty", window, cx));
        if crate::preview::enabled() {
            browse.update(cx, |browse, cx| {
                browse.set_channels(crate::preview::directory(), cx)
            });
        }
        let new_message = cx.new(|cx| NewMessageView::new(window, cx));
        if crate::preview::enabled() {
            new_message.update(cx, |view, cx| {
                view.set_profiles(crate::preview::profiles(), cx)
            });
        }
        let create_channel = cx.new(|cx| CreateChannelView::new(window, cx));
        let rail = cx.new(|_| {
            let teams = if crate::preview::enabled() {
                crate::preview::teams()
            } else {
                vec![Team::new("team", "Matty")]
            };
            let current = teams.first().map(|team| team.id.clone());
            RailView::new(teams, current)
        });
        cx.subscribe(&rail, |_, _, event: &RailEvent, cx| match event {
            // Switching teams and joining one are sync's (PLAN §3 M2); the
            // rail already shows the pick.
            RailEvent::Select(_) | RailEvent::Join => cx.notify(),
        })
        .detach();
        cx.subscribe_in(&browse, window, Self::on_directory_event)
            .detach();
        cx.subscribe_in(&new_message, window, Self::on_directory_event)
            .detach();
        cx.subscribe_in(&create_channel, window, Self::on_directory_event)
            .detach();
        cx.subscribe_in(
            &account,
            window,
            |this, _, event: &AccountEvent, window, cx| match event {
                // The menu already shows the change.
                AccountEvent::SetPresence(presence, dnd_end) => {
                    if let Some(live) = &this.live {
                        live.send(matty_sync::Command::SetStatus {
                            status: crate::live::status(*presence).into(),
                            dnd_end_time: *dnd_end,
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
        let toasts = cx.new(ToastState::new);
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
        cx.subscribe(&threads, |this, threads, event: &ThreadsEvent, cx| {
            // Sync owns the server side; the sidebar count follows at once.
            let unread = threads.read(cx).model().unread();
            this.sidebar.update(cx, |sidebar, cx| {
                let mut model = sidebar.model().clone();
                model.unread_threads = unread;
                sidebar.set_model(model, cx);
            });
            let Some(live) = &mut this.live else {
                return;
            };
            match event {
                ThreadsEvent::MarkAllRead => live.read_all_threads(),
                ThreadsEvent::Unfollow(root) => live.follow_thread(root, false),
                ThreadsEvent::Open { root, unread } => {
                    // Only while on screen: the view keeps its selection
                    // when the user goes back to a channel.
                    if this.threads_open {
                        live.open_thread(root);
                        if *unread {
                            live.read_thread(root);
                        }
                    }
                }
            }
        })
        .detach();
        let detail = threads.read(cx).detail().clone();
        cx.subscribe_in(&detail, window, Self::pane_event).detach();
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
                ChannelEvent::OpenInfo => this.show_channel_info(cx),
                ChannelEvent::OpenDirect { display_name, .. } => {
                    this.open_direct(display_name.clone(), window, cx)
                }
                ChannelEvent::Send {
                    text,
                    files,
                    unfurl,
                } => {
                    if let (Some(live), Some(channel)) = (&mut this.live, &this.selected_channel) {
                        let file_ids = live.take_file_ids(files);
                        live.send(matty_sync::Command::SendPost {
                            pending_post_id: String::new(),
                            channel_id: channel.to_string(),
                            root_id: String::new(),
                            message: text.to_string(),
                            file_ids,
                            props: crate::embeds::send_props(*unfurl),
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
                ChannelEvent::PreviewLink(url) => {
                    if let Some(live) = &this.live {
                        if !live.link_previews() {
                            return;
                        }
                        live.send(matty_sync::Command::LoadLinkPreview {
                            url: url.to_string(),
                        });
                    } else if crate::preview::enabled() {
                        let card = crate::preview::link_card(url);
                        this.channel
                            .update(cx, |view, cx| view.set_link_preview(url, card, cx));
                    }
                }
                ChannelEvent::Typing { root } => {
                    if let (Some(live), Some(channel)) = (&this.live, &this.selected_channel) {
                        live.send(matty_sync::Command::Typing {
                            channel_id: channel.to_string(),
                            parent_id: root.as_ref().map(ToString::to_string).unwrap_or_default(),
                        });
                    }
                }
                ChannelEvent::SaveEdit { post, text } => {
                    if let Some(live) = &this.live {
                        live.send(matty_sync::Command::EditPost {
                            post_id: post.to_string(),
                            message: text.to_string(),
                        });
                    }
                }
                ChannelEvent::ToggleFavorite(favorite) => {
                    if let Some(id) = this.selected_channel.clone() {
                        this.set_favorite(id, *favorite, cx);
                    }
                    this.refresh_channel_info(cx);
                }
                ChannelEvent::Rename(name) => {
                    let name = name.clone();
                    if let Some(id) = this.selected_channel.clone() {
                        this.patch_channel(&id, Some(name.to_string()), None, None);
                        this.rename_row(&id, name, cx);
                    }
                    this.refresh_channel_info(cx);
                }
                ChannelEvent::SetHeader(text) => {
                    if let Some(id) = this.selected_channel.clone() {
                        this.patch_channel(&id, None, Some(text.to_string()), None);
                    }
                    this.refresh_channel_info(cx);
                }
                ChannelEvent::SetPurpose(text) => {
                    if let Some(id) = this.selected_channel.clone() {
                        this.patch_channel(&id, None, None, Some(text.to_string()));
                    }
                    this.refresh_channel_info(cx);
                }
                ChannelEvent::SetNotifications {
                    muted,
                    notifications,
                } => {
                    if let (Some(live), Some(id)) = (&this.live, &this.selected_channel) {
                        live.send(matty_sync::Command::SetChannelNotifications {
                            channel_id: id.to_string(),
                            props: crate::live::notify_props(*muted, notifications),
                        });
                    }
                    this.refresh_channel_info(cx);
                }
                ChannelEvent::AddMembers(people) => {
                    if let (Some(live), Some(id)) = (&this.live, &this.selected_channel) {
                        live.send(matty_sync::Command::AddMembers {
                            channel_id: id.to_string(),
                            user_ids: people.iter().map(|p| p.user_id.to_string()).collect(),
                        });
                    }
                    this.refresh_channel_info(cx);
                }
                // The page comes back through Live as a prepend and a
                // `ChangeSet::History` (#352).
                ChannelEvent::NearTop => {
                    if let (Some(live), Some(id)) = (&this.live, this.selected_channel.clone()) {
                        live.send(matty_sync::Command::LoadOlder {
                            channel_id: id.to_string(),
                        });
                    }
                }
                ChannelEvent::NearBottom => {
                    if let (Some(live), Some(id)) = (&this.live, this.selected_channel.clone()) {
                        live.send(matty_sync::Command::LoadNewer {
                            channel_id: id.to_string(),
                        });
                    }
                }
                ChannelEvent::JumpMissing(post) => {
                    if let (Some(live), Some(id)) = (&this.live, this.selected_channel.clone()) {
                        live.send(matty_sync::Command::LoadAround {
                            channel_id: id.to_string(),
                            post_id: post.to_string(),
                        });
                    }
                }
                ChannelEvent::JumpLatest => {
                    if let (Some(live), Some(id)) = (&this.live, this.selected_channel.clone()) {
                        live.send(matty_sync::Command::LoadLatest {
                            channel_id: id.to_string(),
                        });
                    }
                }
                ChannelEvent::CopyLink(post) => this.copy_link(post, cx),
                ChannelEvent::MarkUnread(post) => {
                    if let Some(live) = &this.live {
                        live.send(matty_sync::Command::MarkUnread {
                            post_id: post.to_string(),
                        });
                    }
                    if let Some(id) = this.selected_channel.clone() {
                        this.sidebar
                            .update(cx, |sidebar, cx| sidebar.mark_unread(&id, cx));
                        // Stays unread until the user leaves and comes back.
                        this.kept_unread = Some(id);
                    }
                }
                ChannelEvent::Leave => {
                    if let Some(id) = this.selected_channel.clone() {
                        this.leave_channel(id, window, cx);
                    }
                }
                // Sync picks these up in later changes; the view already
                // shows the change.
                ChannelEvent::Upload { id, name, source } => {
                    if let Some(channel) = this.selected_channel.clone() {
                        let job = crate::live::UploadJob::new(
                            channel.to_string(),
                            name.to_string(),
                            source.clone(),
                        );
                        this.start_upload(id.clone(), job, cx);
                    }
                }
                ChannelEvent::CancelUpload(id) => {
                    if let Some(live) = &mut this.live {
                        live.cancel_upload(id);
                    }
                }
                ChannelEvent::RetryUpload(id) => {
                    if let Some(job) = this.live.as_ref().and_then(|live| live.upload_job(id)) {
                        this.start_upload(id.clone(), job, cx);
                    }
                }
                ChannelEvent::OpenFile(id) => this.download_file(id, cx),
                ChannelEvent::ViewImages(ids) => {
                    let here = this
                        .live
                        .as_ref()
                        .map(|live| live.full_images(ids))
                        .unwrap_or_default();
                    for (id, image) in here {
                        this.channel
                            .update(cx, |view, cx| view.set_full_image(id, image, cx));
                    }
                }
                ChannelEvent::Retry(post) => {
                    if let Some(live) = &this.live {
                        live.send(matty_sync::Command::Retry {
                            pending_post_id: post.to_string(),
                        });
                    }
                }
                ChannelEvent::Discard(post) => {
                    if let Some(live) = &this.live {
                        live.send(matty_sync::Command::Discard {
                            pending_post_id: post.to_string(),
                        });
                    }
                }
                ChannelEvent::ToggleSave(post) => {
                    if let Some(live) = this.live.as_mut() {
                        let saved = !live.is_saved(post);
                        live.set_saved(post, saved);
                    }
                }
                ChannelEvent::TogglePin(post) => {
                    // The view has already flipped the post.
                    let pinned = this
                        .channel
                        .read(cx)
                        .model()
                        .and_then(|model| model.posts.iter().find(|shown| shown.id == *post))
                        .map(|shown| shown.pinned);
                    if let Some(live) = &mut this.live
                        && let Some(pinned) = pinned
                    {
                        live.set_pinned(post, pinned);
                    }
                }
                ChannelEvent::ToggleFollow { post, following } => {
                    if let Some(live) = &this.live {
                        live.follow_thread(post, *following);
                    }
                }
                ChannelEvent::Remind { post, at } => this.remind(post, *at, cx),
                ChannelEvent::OpenPermalink { channel, post } => {
                    this.open_channel(channel.clone(), window, cx);
                    let post = post.clone();
                    this.channel
                        .update(cx, |channel, cx| channel.jump_to(post, cx));
                }
                ChannelEvent::Forward(post) => {
                    // After the menu has closed and handed focus back, so
                    // the switcher's field keeps it.
                    let post = post.clone();
                    let switcher = this.switcher.clone();
                    window.on_next_frame(move |window, cx| {
                        switcher.update(cx, |switcher, cx| switcher.open_forward(post, window, cx));
                    });
                }
            },
        )
        .detach();
        cx.subscribe_in(&right_pane, window, Self::pane_event)
            .detach();
        cx.subscribe_in(
            &sidebar,
            window,
            |this, _, event: &SidebarEvent, window, cx| {
                // Picking a place to go closes the narrow-window drawer.
                if matches!(
                    event,
                    SidebarEvent::Select(_) | SidebarEvent::OpenThreads | SidebarEvent::OpenDrafts
                ) {
                    this.sidebar_drawer_open = false;
                }
                let id = match event {
                    SidebarEvent::Select(id) => id.clone(),
                    SidebarEvent::OpenThreads => return this.show_threads(cx),
                    SidebarEvent::OpenDrafts => return this.show_drafts(cx),
                    SidebarEvent::BrowseChannels => return this.browse_channels(window, cx),
                    SidebarEvent::NewMessage => return this.new_message(window, cx),
                    SidebarEvent::CreateChannel => return this.open_create_channel(window, cx),
                    SidebarEvent::MarkRead(id) => {
                        if let Some(live) = &this.live {
                            live.send(matty_sync::Command::ViewChannel {
                                channel_id: id.to_string(),
                                prev_channel_id: String::new(),
                            });
                        }
                        let id = id.clone();
                        return this
                            .sidebar
                            .update(cx, |sidebar, cx| sidebar.mark_read(&id, cx));
                    }
                    SidebarEvent::SetFavorite { id, favorite } => {
                        return this.set_favorite(id.clone(), *favorite, cx);
                    }
                    SidebarEvent::SetMuted { id, muted } => {
                        return this.set_muted(id.clone(), *muted, cx);
                    }
                    SidebarEvent::Leave(id) => return this.leave_channel(id.clone(), window, cx),
                    SidebarEvent::Close(id) => return this.close_direct(id.clone(), window, cx),
                };
                this.open_channel(id, window, cx);
            },
        )
        .detach();
        let preview_props = crate::preview::enabled().then(crate::preview::notify_props);
        let keywords = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("deploy, on-call");
            if let Some(props) = &preview_props {
                let text = matty_notify::mention::keywords_text("arya", props);
                input.set_value(text, window, cx);
            }
            input
        });
        cx.subscribe(&keywords, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.save_keywords(cx);
            }
        })
        .detach();
        let level = settings.read(cx).get().log_level;
        let log_level = cx.new(|cx| {
            let mut state = SelectState::new(
                LOG_LEVELS
                    .iter()
                    .map(|(_, value, label)| SelectItem::new(*value, *label)),
                window,
                cx,
            );
            let value = LOG_LEVELS
                .iter()
                .find(|(l, _, _)| *l == level)
                .map(|(_, value, _)| SharedString::from(*value));
            state.set_value(value, window, cx);
            state
        });
        cx.subscribe(&log_level, |this, _, event: &SelectEvent, cx| {
            let SelectEvent::Change(Some(value)) = event else {
                return;
            };
            if let Some((level, _, _)) = LOG_LEVELS.iter().find(|(_, v, _)| *v == value.as_ref()) {
                let level = *level;
                this.settings
                    .update(cx, |store, cx| store.update(cx, |s| s.log_level = level));
            }
        })
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
            keywords,
            log_level,
            notifier: matty_notify::backend::platform(),
            notify_props: preview_props,
            collapsed_threads: crate::preview::enabled().then_some((true, true)),
            browse,
            new_message,
            create_channel,
            joined: Vec::new(),
            rail,
            right_pane_open: false,
            right_drawer: DrawerHandle::new(false),
            right_drawer_shown: false,
            sidebar_drawer: DrawerHandle::new(false),
            sidebar_drawer_open: false,
            sidebar_drawer_shown: false,
            threads,
            toasts,
            downloads: Default::default(),
            threads_open: false,
            drafts,
            drafts_open: false,
            connection: crate::preview::connection(),
            banner_last: crate::preview::connection(),
            queued: crate::preview::queued(),
            history,
            history_at: 0,
            live: None,
            _live_task: None,
            forget: None,
            _typing_task: None,
            _view_task: None,
            kept_unread: None,
            login_server: None,
            sidebar_shown: false,
        };
        // `matty --offline` opens the last account's cache without a session;
        // otherwise a saved session opens it and connects (PLAN §6.6).
        let saved = this.settings.read(cx).get().account.clone();
        if !crate::preview::enabled()
            && let Some(account) = saved
        {
            if crate::offline() {
                this.login = None;
                this.connection = ConnectionState::Offline;
                this.start_live(&account, window, cx);
                if let Some(live) = &this.live {
                    live.send(matty_sync::Command::ShowCache);
                }
            } else if let Some(token) = crate::secrets::load(&account)
                && let Ok(client) = matty_api::Client::new(&account.server)
            {
                this.login = None;
                this.start_live(&account, window, cx);
                if let Some(live) = &this.live {
                    live.send(matty_sync::Command::Connect {
                        client: client.with_token(token),
                    });
                }
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
        // Signed in again before the last sign out finished: keep the cache
        // if it is this account's, else delete it now.
        if self.forget.as_ref().is_some_and(|(old, _)| *old == path) {
            self.forget = None;
        }
        self.forget_signed_out(cx);
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
                    dnd_end: None,
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

    /// Clears typing notices once they run out, checking again while
    /// anyone is left.
    /// Reads a tray file off the UI thread and hands its bytes to sync,
    /// which reports progress back through [`Live`].
    fn start_upload(
        &mut self,
        id: SharedString,
        job: crate::live::UploadJob,
        cx: &mut Context<Self>,
    ) {
        let Some(live) = &mut self.live else {
            return;
        };
        live.track_upload(&id, job.clone());
        cx.spawn(async move |this, cx| {
            let bytes = cx
                .background_spawn(async move {
                    match job.source {
                        crate::attachments::UploadSource::Path(path) => std::fs::read(path),
                        crate::attachments::UploadSource::Image(image) => Ok(image.bytes.clone()),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
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
                        this.channel.update(cx, |view, cx| {
                            view.set_upload_status(
                                &id,
                                crate::attachments::AttachmentStatus::Failed,
                                cx,
                            );
                        });
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Saves a post's file to the download folder; it opens once saved
    /// (see [`open_download`]).
    fn download_file(&mut self, file_id: &SharedString, cx: &mut Context<Self>) {
        let Some(live) = &self.live else {
            if let Some(name) = crate::preview::file_name(file_id) {
                // Preview has no server: show the toast a finished download
                // would.
                let folder = dirs::download_dir().unwrap_or_else(std::env::temp_dir);
                self.push_download_toast(&folder.join(name), cx);
            }
            return;
        };
        let Some(name) = live.file_name(file_id) else {
            tracing::warn!(%file_id, "no such file on screen");
            return;
        };
        let folder = self
            .settings
            .read(cx)
            .get()
            .download_folder
            .clone()
            .or_else(dirs::download_dir)
            .or_else(dirs::home_dir)
            .unwrap_or_else(std::env::temp_dir);
        self.downloads.push_back(file_id.clone());
        live.send(matty_sync::Command::Download {
            file_id: file_id.to_string(),
            folder,
            name,
        });
    }

    /// Tells the user a file was saved: the file name as the title, the
    /// folder below it, and "Show in folder" to reveal it.
    pub fn push_download_toast(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "File".to_string());
        let folder = path
            .parent()
            .map(|folder| {
                let shown = dirs::home_dir()
                    .and_then(|home| folder.strip_prefix(home).ok())
                    .map(|rest| format!("~/{}", rest.display()))
                    .unwrap_or_else(|| folder.display().to_string());
                format!("Saved to {shown}")
            })
            .unwrap_or_else(|| "Saved".to_string());
        self.toasts.update(cx, |toasts, cx| {
            toasts.push(
                format!("download.{}", path.display()),
                Notice::new(name)
                    .description(folder)
                    .action("Show in folder", {
                        let path = path.to_path_buf();
                        move |_, cx| cx.reveal_path(&path)
                    })
                    .timeout(std::time::Duration::from_secs(6)),
                cx,
            );
        });
    }

    /// Puts a post's permalink on the clipboard and says so (#133).
    /// Forwards a post from the open channel as a new post holding its
    /// permalink, which the server and the webapp show as the quoted post.
    fn forward(&mut self, post_id: &str, to: &SharedString, cx: &mut Context<Self>) {
        let link = self.live.as_ref().and_then(|live| {
            let from = self.selected_channel.as_ref()?;
            live.permalink(from, post_id)
        });
        let (Some(live), Some(link)) = (&self.live, link) else {
            self.push_error(
                "Couldn't forward the message. Try again once you're connected.",
                cx,
            );
            return;
        };
        live.send(matty_sync::Command::SendPost {
            pending_post_id: String::new(),
            channel_id: to.to_string(),
            root_id: String::new(),
            message: link,
            file_ids: Vec::new(),
            props: crate::embeds::send_props(true),
        });
        let name: SharedString = self
            .sidebar
            .read(cx)
            .model()
            .categories
            .iter()
            .flat_map(|category| category.rows.iter())
            .find(|row| row.id == *to)
            .map_or_else(|| "the channel".into(), |row| row.name.clone());
        self.toasts.update(cx, |toasts, cx| {
            toasts.push(
                "forward",
                Notice::new(format!("Forwarded to {name}"))
                    .timeout(std::time::Duration::from_secs(4)),
                cx,
            );
        });
    }

    /// Asks the server to remind the user of a post, and says when.
    fn remind(&mut self, post_id: &str, at: crate::channel::RemindAt, cx: &mut Context<Self>) {
        let Some(live) = &self.live else {
            if !crate::preview::enabled() {
                self.push_error(
                    "Couldn't set the reminder. Try again once you're connected.",
                    cx,
                );
                return;
            }
            return self.reminder_set(at, cx);
        };
        live.remind(post_id, at);
        self.reminder_set(at, cx);
    }

    fn reminder_set(&mut self, at: crate::channel::RemindAt, cx: &mut Context<Self>) {
        let when = match at {
            crate::channel::RemindAt::Tomorrow => "tomorrow at 9:00".to_string(),
            other => other.label().to_lowercase(),
        };
        self.toasts.update(cx, |toasts, cx| {
            toasts.push(
                "reminder",
                Notice::new(format!("Reminder set for {when}"))
                    .timeout(std::time::Duration::from_secs(4)),
                cx,
            );
        });
    }

    fn copy_link(&mut self, post_id: &str, cx: &mut Context<Self>) {
        let link = self.live.as_ref().and_then(|live| {
            let channel = self.selected_channel.as_ref()?;
            live.permalink(channel, post_id)
        });
        let link = link.or_else(|| {
            crate::preview::enabled()
                .then(|| format!("https://chat.example.com/matty/pl/{post_id}"))
        });
        let Some(link) = link else {
            self.push_error_with(
                "Couldn't copy the link. Try again once you're connected.",
                None,
                cx,
            );
            return;
        };
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(link));
        self.toasts.update(cx, |toasts, cx| {
            toasts.push(
                "copy-link",
                Notice::new("Link copied").timeout(std::time::Duration::from_secs(4)),
                cx,
            );
        });
    }

    /// Tells the user something failed, in one sentence, for eight seconds,
    /// with a Dismiss button.
    pub fn push_error(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.push_error_with(text, None, cx);
    }

    /// `push_error` with a Retry button when there is something to retry.
    fn push_error_with(
        &mut self,
        text: impl Into<SharedString>,
        retry: Option<Retry>,
        cx: &mut Context<Self>,
    ) {
        let text = text.into();
        let notice = Notice::new(text.clone()).timeout(std::time::Duration::from_secs(8));
        let notice = match retry {
            Some(retry) => notice.action("Retry", retry),
            None => notice.cancel("Dismiss"),
        };
        self.toasts.update(cx, |toasts, cx| {
            toasts.push(format!("error.{text}"), notice, cx);
        });
    }

    fn expire_typing_later(&mut self, cx: &mut Context<Self>) {
        self._typing_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(crate::live::TYPING_FOR / 2)
                    .await;
                let left = this.update(cx, |this, cx| {
                    let live = this.live.as_mut()?;
                    let names = live.expire_typing(std::time::Instant::now());
                    let left = live.is_typing();
                    if let Some(names) = names {
                        this.channel
                            .update(cx, |view, cx| view.set_typing(names, cx));
                    }
                    Some(left)
                });
                if !matches!(left, Ok(Some(true))) {
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
            self.browse
                .update(cx, |browse, cx| browse.set_team_name(name.clone(), cx));
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.set_team_name(name, cx));
        }
        if let Some(channels) = update.directory {
            self.browse
                .update(cx, |browse, cx| browse.set_channels(channels, cx));
        }
        if let Some(people) = update.people {
            self.new_message
                .update(cx, |view, cx| view.set_profiles(people, cx));
        }
        // Only while the pane still shows that kind of listing.
        if let Some(listing) = update.listing
            && self.right_pane_open
            && self.right_pane.read(cx).content().is_some_and(|shown| {
                std::mem::discriminant(shown) == std::mem::discriminant(&listing)
            })
        {
            self.right_pane
                .update(cx, |pane, cx| pane.set_content(Some(listing), cx));
        }
        // Only while the pane still shows that search.
        if let Some(model) = update.search
            && let Some(RightPaneContent::Search(shown)) = self.right_pane.read(cx).content()
            && shown.query == model.query
        {
            self.right_pane
                .update(cx, |pane, cx| pane.set_search(model, window, cx));
        }
        if let Some(id) = update.join_failed {
            self.browse
                .update(cx, |browse, cx| browse.set_joined(&id, false, cx));
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
        match update.channel {
            Some(ChannelUpdate::Replace(model)) => self
                .channel
                .update(cx, |view, cx| view.set_model(Some(model), window, cx)),
            Some(ChannelUpdate::Patch {
                removed,
                prepended,
                updated,
                appended,
            }) => self.channel.update(cx, |view, cx| {
                // The composer's own "Sending…" rows give way to sync's copy
                // of the same send, which this patch carries.
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
            }),
            None => {}
        }
        if let Some((at_start, at_end, failed)) = update.history {
            self.channel.update(cx, |view, cx| {
                view.set_history(at_start, at_end, failed, cx)
            });
        }
        if let Some(details) = update.details {
            self.channel.update(cx, |view, cx| {
                view.set_details(
                    details.header,
                    details.purpose,
                    details.members,
                    details.pinned,
                    cx,
                );
            });
        }
        if let Some((muted, notifications)) = update.notifications {
            self.channel.update(cx, |view, cx| {
                view.set_notifications(muted, notifications, cx);
            });
        }
        if let Some(profiles) = update.profiles {
            self.channel
                .update(cx, |view, cx| view.set_profiles(profiles, cx));
        }
        if let Some((id, image)) = update.full_image {
            self.right_pane
                .update(cx, |pane, cx| pane.set_full_image(&id, image.clone(), cx));
            self.threads
                .read(cx)
                .detail()
                .clone()
                .update(cx, |pane, cx| pane.set_full_image(&id, image.clone(), cx));
            self.channel
                .update(cx, |view, cx| view.set_full_image(id, image, cx));
        }
        if let Some(download) = update.download {
            match download {
                matty_sync::DownloadState::Done(path) => {
                    self.downloads.pop_front();
                    open_download(&path, cx);
                    self.push_download_toast(&path, cx);
                }
                matty_sync::DownloadState::Failed(error) => {
                    tracing::warn!(%error, "download failed");
                    // Sync does not say which file failed, so Retry only
                    // shows when there was one.
                    let only = (self.downloads.len() == 1)
                        .then(|| self.downloads.pop_front())
                        .flatten();
                    if only.is_none() {
                        self.downloads.pop_front();
                    }
                    let text = format!("Couldn't download the file: {error}");
                    match only {
                        Some(file_id) => {
                            let view = cx.entity().downgrade();
                            self.push_error_with(
                                text,
                                Some(Box::new(move |_, cx| {
                                    view.update(cx, |this, cx| this.download_file(&file_id, cx))
                                        .ok();
                                })),
                                cx,
                            );
                        }
                        None => self.push_error(text, cx),
                    }
                }
            }
        }
        if let Some((id, status)) = update.upload {
            self.channel
                .update(cx, |view, cx| view.set_upload_status(&id, status, cx));
        }
        if let Some(names) = update.typing {
            let someone = !names.is_empty();
            self.channel
                .update(cx, |view, cx| view.set_typing(names, cx));
            if someone {
                self.expire_typing_later(cx);
            }
        }
        if let Some(queued) = update.queued {
            self.queued = queued;
            cx.notify();
        }
        if let Some(props) = update.notify_props {
            let text = matty_notify::mention::keywords_text(&self.username(cx), &props);
            self.keywords.update(cx, |input, cx| {
                if input.value() != text.as_str() {
                    input.set_value(text, window, cx);
                }
            });
            self.notify_props = Some(props);
        }
        if let Some(threads) = update.collapsed_threads {
            self.collapsed_threads = Some(threads);
            cx.notify();
        }
        if let Some(thread) = update.thread {
            if self.threads_open {
                self.threads
                    .update(cx, |view, cx| view.set_thread(thread, cx));
            } else {
                self.right_pane
                    .update(cx, |pane, cx| pane.set_thread(thread, cx));
            }
        }
        if let Some(model) = update.threads {
            let unread = model.unread();
            self.threads
                .update(cx, |view, cx| view.set_model(model, cx));
            self.sidebar.update(cx, |sidebar, cx| {
                let mut model = sidebar.model().clone();
                model.unread_threads = unread;
                sidebar.set_model(model, cx);
            });
        }
        if let Some((url, card)) = update.link_preview {
            self.threads
                .read(cx)
                .detail()
                .clone()
                .update(cx, |view, cx| view.set_link_preview(&url, card.clone(), cx));
            self.right_pane
                .update(cx, |view, cx| view.set_link_preview(&url, card.clone(), cx));
            self.channel
                .update(cx, |view, cx| view.set_link_preview(&url, card, cx));
        }
        if let Some(arrival) = update.arrived {
            self.notify(&arrival, window, cx);
            if self.selected_channel.as_deref() == Some(arrival.event.channel_id.as_str()) {
                self.view_later(window, cx);
            }
        }
        if let Some((presence, dnd_end)) = update.own_presence {
            self.account.update(cx, |view, cx| {
                view.set_server_presence(presence, dnd_end, cx)
            });
        }
        if let Some(state) = update.connection {
            self.set_connection(state, cx);
        }
        if let Some(id) = update.ready {
            self.open_channel(id, window, cx);
        }
        if update.signed_out {
            self.live = None;
            self._live_task = None;
            self.forget_signed_out(cx);
        }
        if update.expired && self.login.is_none() {
            self.session_expired(window, cx);
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
                    if let Some(token) = session.client.token_value() {
                        crate::secrets::save(&account, token);
                    }
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
        if std::mem::take(&mut self.threads_open) {
            self.leave_thread();
        }
        self.right_pane_open = false;
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.select(Some(crate::sidebar::DRAFTS_ID.into()), cx)
        });
        cx.notify();
    }

    /// Shows the followed threads in place of the channel.
    fn show_threads(&mut self, cx: &mut Context<Self>) {
        self.threads_open = true;
        // The thread beside the list is the one sync follows now.
        let selected = self.threads.read(cx).selected().cloned();
        if let Some(live) = &mut self.live {
            live.close_thread();
            if let Some(root) = selected {
                live.open_thread(&root);
            }
        }
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
        if std::mem::take(&mut self.threads_open) {
            self.leave_thread();
        }
        self.drafts_open = false;
        let previous = self.selected_channel.replace(id.clone());
        if previous.as_ref() != Some(&id) {
            self.kept_unread = None;
        }
        self.notifier.withdraw(&id);
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
            let model = crate::preview::channel(&id)
                .or_else(|| self.joined.iter().find(|c| c.id == id).cloned());
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

    fn on_directory_event<V>(
        &mut self,
        _: &Entity<V>,
        event: &DirectoryEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            DirectoryEvent::Join(id) => self.join_channel(id.clone(), window, cx),
            DirectoryEvent::Open(id) => self.open_channel(id.clone(), window, cx),
            DirectoryEvent::Message(profile) => self.message(profile, window, cx),
            DirectoryEvent::Create {
                name,
                purpose,
                private,
            } => self.create_channel(name.clone(), purpose.clone(), *private, window, cx),
        }
    }

    /// Opens the direct message with `profile`, starting one when there is
    /// none yet. Sync owns creating the channel (PLAN §4.3); in preview the
    /// row appears under Direct messages with an empty transcript.
    fn message(
        &mut self,
        profile: &crate::profile::UserProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = self
            .sidebar
            .read(cx)
            .model()
            .categories
            .iter()
            .flat_map(|category| category.rows.iter())
            .find(|row| {
                matches!(row.kind, crate::sidebar::ChannelKind::Direct { .. })
                    && row.name == profile.display_name
            })
            .map(|row| row.id.clone());
        if let Some(id) = existing {
            return self.open_channel(id, window, cx);
        }
        // Sync finds or creates it, and it opens once in the sidebar.
        if let Some(live) = &self.live {
            return live.send(matty_sync::Command::OpenDirect {
                user_ids: vec![profile.user_id.to_string()],
            });
        }
        if crate::preview::enabled() {
            let kind = crate::sidebar::ChannelKind::Direct {
                presence: profile.presence,
            };
            let row = crate::sidebar::ChannelRow::new(
                profile.user_id.clone(),
                profile.display_name.clone(),
                kind.clone(),
            );
            self.add_row("direct", row, cx);
            self.joined.push(crate::preview::empty_channel(
                profile.user_id.clone(),
                profile.display_name.clone(),
                kind,
                None,
            ));
            self.open_channel(profile.user_id.clone(), window, cx);
        }
    }

    /// Creates a channel from the dialog. Sync owns the server side; in
    /// preview the row appears under Channels and opens empty.
    fn create_channel(
        &mut self,
        name: SharedString,
        purpose: Option<SharedString>,
        private: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(live) = &self.live {
            if let Some(team_id) = live.team() {
                live.send(matty_sync::Command::CreateChannel {
                    team_id: team_id.to_string(),
                    name: crate::directory::channel_handle(&name),
                    display_name: name.to_string(),
                    purpose: purpose.map(|p| p.to_string()).unwrap_or_default(),
                    private,
                });
            }
            return;
        }
        if !crate::preview::enabled() {
            return;
        }
        let id: SharedString = crate::directory::channel_handle(&name).into();
        let kind = if private {
            crate::sidebar::ChannelKind::Private
        } else {
            crate::sidebar::ChannelKind::Public
        };
        let row = crate::sidebar::ChannelRow::new(id.clone(), id.clone(), kind.clone());
        self.add_row("channels", row, cx);
        self.joined.push(crate::preview::empty_channel(
            id.clone(),
            id.clone(),
            kind,
            purpose,
        ));
        self.open_channel(id, window, cx);
    }

    /// Renames the sidebar row for `id` after a rename from the header.
    /// Sends a channel's new name, header or purpose to the server.
    fn patch_channel(
        &self,
        id: &str,
        display_name: Option<String>,
        header: Option<String>,
        purpose: Option<String>,
    ) {
        if let Some(live) = &self.live {
            live.send(matty_sync::Command::PatchChannel {
                channel_id: id.to_owned(),
                display_name,
                header,
                purpose,
            });
        }
    }

    fn rename_row(&mut self, id: &str, name: SharedString, cx: &mut Context<Self>) {
        let mut model = self.sidebar.read(cx).model().clone();
        for row in model.categories.iter_mut().flat_map(|c| c.rows.iter_mut()) {
            if row.id.as_ref() == id {
                row.name = name.clone();
            }
        }
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_model(model, cx));
    }

    /// Leaves `id`: sync owns the server side; the row goes and the first
    /// remaining public channel opens.
    /// Moves `id` into or out of Favorites in the sidebar and on the open
    /// channel's header star. Sync owes the server-side category move.
    fn set_favorite(&mut self, id: SharedString, favorite: bool, cx: &mut Context<Self>) {
        if let Some(live) = &self.live {
            live.send(matty_sync::Command::SetFavorite {
                channel_id: id.to_string(),
                favorite,
            });
        }
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_favorite(&id, favorite, cx));
        if self.selected_channel.as_ref() == Some(&id) {
            self.channel
                .update(cx, |channel, cx| channel.set_favorite(favorite, cx));
            self.refresh_channel_info(cx);
        }
    }

    /// Mutes or unmutes `id`: the sidebar row, the open channel and the
    /// server's `mark_unread` notify prop, as the info pane's switch does.
    fn set_muted(&mut self, id: SharedString, muted: bool, cx: &mut Context<Self>) {
        if let Some(live) = &self.live {
            live.send(matty_sync::Command::SetChannelNotifications {
                channel_id: id.to_string(),
                props: std::collections::HashMap::from([(
                    "mark_unread".to_string(),
                    if muted { "mention" } else { "all" }.to_string(),
                )]),
            });
        }
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_muted(&id, muted, cx));
        if self.selected_channel.as_ref() == Some(&id) {
            self.channel
                .update(cx, |channel, cx| channel.set_muted(muted, cx));
            self.refresh_channel_info(cx);
        }
    }

    /// Closes a direct or group conversation: the row goes and the next
    /// channel opens. Mattermost keeps the channel and hides it by
    /// preference until it is opened again; the posts stay on the server.
    fn close_direct(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(live) = &self.live {
            live.send(matty_sync::Command::CloseDirect {
                channel_id: id.to_string(),
            });
        }
        let mut model = self.sidebar.read(cx).model().clone();
        model.remove(&id);
        let next = model
            .categories
            .iter()
            .flat_map(|c| c.rows.iter())
            .find(|row| matches!(row.kind, crate::sidebar::ChannelKind::Public))
            .map(|row| row.id.clone());
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_model(model, cx));
        if self.selected_channel.as_ref() == Some(&id)
            && let Some(next) = next
        {
            self.right_pane_open = false;
            self.open_channel(next, window, cx);
        }
        cx.notify();
    }

    fn leave_channel(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(live) = &self.live {
            live.send(matty_sync::Command::LeaveChannel {
                channel_id: id.to_string(),
            });
        }
        let mut model = self.sidebar.read(cx).model().clone();
        for category in model.categories.iter_mut() {
            category.rows.retain(|row| row.id != id);
        }
        let next = model
            .categories
            .iter()
            .flat_map(|c| c.rows.iter())
            .find(|row| matches!(row.kind, crate::sidebar::ChannelKind::Public))
            .map(|row| row.id.clone());
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_model(model, cx));
        self.joined.retain(|c| c.id != id);
        self.browse
            .update(cx, |browse, cx| browse.set_joined(&id, false, cx));
        self.right_pane_open = false;
        match next {
            Some(next) => self.open_channel(next, window, cx),
            None => {
                self.selected_channel = None;
                self.channel
                    .update(cx, |view, cx| view.set_model(None, window, cx));
                cx.notify();
            }
        }
    }

    /// Appends `row` to the sidebar category `category_id`.
    fn add_row(
        &mut self,
        category_id: &str,
        row: crate::sidebar::ChannelRow,
        cx: &mut Context<Self>,
    ) {
        let mut model = self.sidebar.read(cx).model().clone();
        if let Some(category) = model.categories.iter_mut().find(|c| c.id == category_id) {
            category.rows.push(row);
        }
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_model(model, cx));
    }

    /// Opens the Browse channels dialog (PLAN §3 M1).
    /// Opens the New message people picker, asking sync for the team's
    /// people first when connected.
    fn new_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(live) = &self.live
            && let Some(team_id) = live.team()
        {
            live.send(matty_sync::Command::FindPeople {
                team_id: team_id.to_string(),
            });
        }
        self.new_message
            .update(cx, |view, cx| view.open(window, cx));
    }

    /// Opens the Create channel dialog.
    fn open_create_channel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.create_channel
            .update(cx, |view, cx| view.open(window, cx));
    }

    fn browse_channels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(live) = &self.live
            && let Some(team_id) = live.team()
        {
            live.send(matty_sync::Command::Browse {
                team_id: team_id.to_string(),
            });
        }
        self.browse.update(cx, |browse, cx| browse.open(window, cx));
    }

    /// Joins `id` from Browse: the sidebar gains the row and the channel
    /// opens. Sync owns the server side (PLAN §4.3); in preview the channel
    /// starts empty.
    fn join_channel(
        &mut self,
        id: gpui_kit::SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Sync joins, and the channel opens once it is in the sidebar.
        if let Some(live) = &self.live {
            return live.send(matty_sync::Command::JoinChannel {
                channel_id: id.to_string(),
            });
        }
        if crate::preview::enabled() {
            let directory = crate::preview::directory();
            if let Some(channel) = directory.iter().find(|c| c.id == id) {
                let row = crate::sidebar::ChannelRow::new(
                    channel.id.clone(),
                    channel.name.clone(),
                    crate::sidebar::ChannelKind::Public,
                );
                self.add_row("channels", row, cx);
                self.joined.push(crate::preview::joined_channel(channel));
            }
        }
        self.open_channel(id, window, cx);
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
        let next = self.sidebar.read(cx).shown().neighbor(step, unread_only);
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
        if let Some(live) = &mut self.live {
            // Sync publishes the thread from the cache at once and again as
            // replies arrive; until then the pane is empty.
            live.open_thread(&post_id);
            self.right_pane
                .update(cx, |pane, cx| pane.set_content(None, cx));
            self.right_pane_open = true;
            cx.notify();
            return;
        }
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

    /// The right pane moves on from a thread: sync stops publishing it.
    /// What the right pane, or the Threads view's thread beside its list,
    /// asks for.
    fn pane_event(
        &mut self,
        pane: &Entity<RightPaneView>,
        event: &RightPaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            RightPaneEvent::Close => {
                self.leave_thread();
                self.right_pane_open = false;
                cx.notify();
            }
            RightPaneEvent::Search(query) => self.search(query.clone(), window, cx),
            RightPaneEvent::OpenPinned => self.show_pinned(cx),
            RightPaneEvent::OpenDirect { display_name, .. } => {
                self.open_direct(display_name.clone(), window, cx)
            }
            RightPaneEvent::Mention(username) => {
                let text = format!("@{username} ");
                self.channel
                    .update(cx, |channel, cx| channel.insert_text(text, window, cx));
            }
            RightPaneEvent::ToggleFavorite { channel, favorite } => {
                self.set_favorite(channel.clone(), *favorite, cx);
            }
            RightPaneEvent::ToggleMute { channel, muted } => {
                let muted = *muted;
                if let Some(live) = &self.live {
                    live.send(matty_sync::Command::SetChannelNotifications {
                        channel_id: channel.to_string(),
                        props: std::collections::HashMap::from([(
                            "mark_unread".to_string(),
                            if muted { "mention" } else { "all" }.to_string(),
                        )]),
                    });
                }
                self.channel
                    .update(cx, |channel, cx| channel.set_muted(muted, cx));
            }
            RightPaneEvent::Jump { channel, post } => {
                self.open_channel(channel.clone(), window, cx);
                let post = post.clone();
                self.channel
                    .update(cx, |channel, cx| channel.jump_to(post, cx));
            }
            RightPaneEvent::Leave(channel) => self.leave_channel(channel.clone(), window, cx),
            RightPaneEvent::OpenFile(id) => self.download_file(id, cx),
            RightPaneEvent::ViewImages(ids) => {
                let here = self
                    .live
                    .as_ref()
                    .map(|live| live.full_images(ids))
                    .unwrap_or_default();
                for (id, image) in here {
                    pane.update(cx, |pane, cx| pane.set_full_image(&id, image, cx));
                }
            }
            RightPaneEvent::PreviewLink(url) => {
                if let Some(live) = &self.live {
                    if !live.link_previews() {
                        return;
                    }
                    live.send(matty_sync::Command::LoadLinkPreview {
                        url: url.to_string(),
                    });
                } else if crate::preview::enabled() {
                    let card = crate::preview::link_card(url);
                    pane.update(cx, |view, cx| view.set_link_preview(url, card, cx));
                }
            }
            RightPaneEvent::Reply { root, text, unfurl } => {
                if let Some(live) = &self.live
                    && let Some(channel_id) = live.thread_channel()
                {
                    live.send(matty_sync::Command::SendPost {
                        pending_post_id: String::new(),
                        channel_id,
                        root_id: root.to_string(),
                        message: text.to_string(),
                        file_ids: Vec::new(),
                        props: crate::embeds::send_props(*unfurl),
                    });
                }
            }
            RightPaneEvent::Retry(post) => {
                if let Some(live) = &self.live {
                    live.send(matty_sync::Command::Retry {
                        pending_post_id: post.to_string(),
                    });
                }
            }
            RightPaneEvent::ToggleFollow { root, following } => {
                if let Some(live) = &self.live {
                    live.follow_thread(root, *following);
                }
            }
            // Sync owns the rest (PLAN §4.3); the pane already shows them.
            _ => {}
        }
    }

    /// The right pane moves on: sync stops keeping its thread or listing.
    fn leave_thread(&mut self) {
        if let Some(live) = &mut self.live {
            live.close_thread();
            live.close_listing();
        }
    }

    fn show_listing(&mut self, content: RightPaneContent, cx: &mut Context<Self>) {
        self.leave_thread();
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
        self.leave_thread();
        if !query.trim().is_empty() {
            SettingsStore::global(cx).update(cx, |store, cx| {
                store.update(cx, |settings| settings.remember_search(&query))
            });
        }
        if let Some(live) = self.live.as_mut()
            && !query.is_empty()
        {
            live.search(&query);
        }
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
        if let Some(live) = self.live.as_mut() {
            live.show_pinned(&channel.id);
            self.right_pane
                .update(cx, |pane, cx| pane.set_loading(true, cx));
        }
    }

    fn show_saved(&mut self, cx: &mut Context<Self>) {
        let posts = if crate::preview::enabled() {
            crate::preview::saved()
        } else {
            vec![]
        };
        self.show_listing(RightPaneContent::Saved(posts), cx);
        if let Some(live) = self.live.as_mut() {
            live.show_saved();
            self.right_pane
                .update(cx, |pane, cx| pane.set_loading(true, cx));
        }
    }

    fn show_mentions(&mut self, cx: &mut Context<Self>) {
        let posts = if crate::preview::enabled() {
            crate::preview::mentions()
        } else {
            vec![]
        };
        self.show_listing(RightPaneContent::Mentions(posts), cx);
        if let Some(live) = self.live.as_mut() {
            live.show_mentions();
            self.right_pane
                .update(cx, |pane, cx| pane.set_loading(true, cx));
        }
    }

    /// Keeps an open info pane in step with the channel header's star.
    fn refresh_channel_info(&mut self, cx: &mut Context<Self>) {
        let showing = matches!(
            self.right_pane.read(cx).content(),
            Some(RightPaneContent::ChannelInfo(_))
        );
        if showing && self.right_pane_open {
            self.show_channel_info(cx);
        }
    }

    /// Shows the current channel's info in the right pane.
    fn show_channel_info(&mut self, cx: &mut Context<Self>) {
        self.leave_thread();
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
        if !self.right_pane_open || Self::narrow_right_pane(window) {
            return self.channel.clone().into_any_element();
        }
        let settings = self.settings.read(cx).get().clone();
        let theme = UiTheme::read(cx);
        let rail = if self.rail.read(cx).is_multi() {
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

    /// Whether the window is too narrow to split the channel and the right
    /// pane side by side.
    fn narrow_right_pane(window: &Window) -> bool {
        f32::from(window.viewport_size().width) < NARROW_RIGHT_PANE
    }

    /// Whether the window is too narrow for the sidebar beside the channel.
    fn narrow_sidebar(window: &Window) -> bool {
        f32::from(window.viewport_size().width) < NARROW_SIDEBAR
    }

    /// Keeps the narrow-window drawers in step with their open flags, and
    /// lets a dismissal from a drawer (Escape, the backdrop) clear the flag.
    fn sync_drawers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let signed_in = self.login.is_none();
        let right = self.right_pane_open
            && Self::narrow_right_pane(window)
            && signed_in
            && !self.threads_open
            && !self.drafts_open;
        if sync_drawer(
            &self.right_drawer,
            &mut self.right_drawer_shown,
            right,
            window,
            cx,
        ) {
            self.right_pane_open = false;
        }
        let sidebar = self.sidebar_drawer_open && Self::narrow_sidebar(window) && signed_in;
        if sync_drawer(
            &self.sidebar_drawer,
            &mut self.sidebar_drawer_shown,
            sidebar,
            window,
            cx,
        ) {
            self.sidebar_drawer_open = false;
        }
    }

    /// The sidebar as a drawer from the left edge, mounted always and shown
    /// only in a narrow window (PLAN §4.1).
    fn sidebar_drawer(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = f32::from(window.viewport_size().width);
        let width = self
            .settings
            .read(cx)
            .get()
            .sidebar_width
            .min(viewport * 0.85);
        Drawer::new("main.sidebar-drawer", &self.sidebar_drawer)
            .direction(DrawerSide::Left)
            .content(
                DrawerContent::new("main.sidebar-drawer.panel", "Sidebar")
                    .w(px(width))
                    .child(self.sidebar.clone()),
            )
    }

    /// The right pane as a drawer from the right edge, mounted always and
    /// shown only in a narrow window (PLAN §4.1).
    fn right_drawer(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = f32::from(window.viewport_size().width);
        let width = self
            .settings
            .read(cx)
            .get()
            .right_pane_width
            .min(viewport - CONTENT_MIN / 2.)
            .max(RIGHT_PANE_MIN.min(viewport * 0.85));
        Drawer::new("main.right-drawer", &self.right_drawer)
            .direction(DrawerSide::Right)
            .content(
                DrawerContent::new("main.right-drawer.panel", "Right pane")
                    .w(px(width))
                    .child(self.right_pane.clone()),
            )
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
        style::tipped(
            SharedString::from(format!("{id}.tooltip")),
            actions::tooltip_text(label, label),
            button,
        )
    }

    /// DESIGN.md §4: navigation on the left, the quick switcher as a search
    /// pill in the middle, mentions, saved, help and the user on the right.
    fn title_bar(&self, narrow: bool, cx: &mut Context<Self>) -> impl IntoElement {
        // Signed out there is nothing to navigate: window controls only.
        if self.login.is_some() {
            return TitleBar::new().into_any_element();
        }
        let theme = UiTheme::read(cx).clone();
        let platform = Platform::current();
        let switcher_keys = actions::shortcut_for("Quick switcher").unwrap_or_default();
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
                    .size(scaled(&theme, metrics::ICON_SMALL))
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
                    // The sidebar lives in a drawer at this width; this is
                    // the pointer's way to it.
                    .when(narrow, |group| {
                        group.child(Self::icon_button(
                            "titlebar.sidebar",
                            "Toggle sidebar",
                            LucideIcon::PanelLeft,
                            Box::new(ToggleSidebar),
                            true,
                            cx,
                        ))
                    })
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
    /// on the server and then stops, and the account's cache is deleted
    /// (#160). Offline, the server is told nothing but the cache still goes.
    fn sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.login.is_some() {
            return;
        }
        let account = self.settings.read(cx).get().account.clone();
        if let Some(account) = &account
            && !crate::preview::enabled()
            && !crate::offline()
        {
            self.forget = Some((
                crate::live::database_path(account),
                crate::live::media_dir(account),
            ));
        }
        match &self.live {
            Some(live) => live.send(matty_sync::Command::SignOut),
            None => {
                self._live_task = None;
                self.forget_signed_out(cx);
            }
        }
        if let Some(account) = &account {
            crate::secrets::delete(account);
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

    /// Deletes the signed-out account's cache. Sync may still be closing
    /// the database, so a failed delete is tried again for a few seconds.
    fn forget_signed_out(&mut self, cx: &mut Context<Self>) {
        let Some((database, media)) = self.forget.take() else {
            return;
        };
        let executor = cx.background_executor().clone();
        cx.background_executor()
            .spawn(async move {
                for _ in 0..20 {
                    match matty_sync::forget_account(&database, media.as_deref()) {
                        Ok(()) => return,
                        Err(error) => {
                            tracing::debug!(%error, "the cache is still in use");
                            executor.timer(std::time::Duration::from_millis(250)).await;
                        }
                    }
                }
                tracing::warn!(path = %database.display(), "could not delete the signed-out cache");
            })
            .detach();
    }

    /// The server refused the saved session: forget it and ask the user to
    /// sign in again, saying why.
    fn session_expired(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(account) = self.settings.read(cx).get().account.clone() {
            crate::secrets::delete(&account);
        }
        self.live = None;
        self._live_task = None;
        self.selected_channel = None;
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_model(SidebarModel::loading(), cx)
        });
        let model = LoginModel {
            error: Some("Your session expired. Sign in again.".into()),
            ..LoginModel::default()
        };
        let view = cx.new(|cx| LoginView::new(model, window, cx));
        cx.subscribe_in(&view, window, Self::on_login_event)
            .detach();
        self.login = Some(view);
        self.right_pane_open = false;
        cx.notify();
    }

    /// Replaces the connection state; sync calls this as it goes.
    pub fn set_connection(&mut self, state: ConnectionState, cx: &mut Context<Self>) {
        if self.connection != state {
            if state != ConnectionState::Online {
                self.banner_last = state.clone();
            }
            self.connection = state;
            let away = self.connection != ConnectionState::Online;
            self.channel.update(cx, |view, cx| view.set_away(away, cx));
            cx.notify();
        }
    }

    /// The offline banner's Retry now: the socket redials at once and the
    /// banner shows the attempt (#162).
    fn retry_connection(&mut self, cx: &mut Context<Self>) {
        if let Some(live) = &self.live {
            live.send(matty_sync::Command::Reconnect);
        }
        self.set_connection(ConnectionState::Reconnecting { attempt: 1 }, cx);
    }

    /// A 32px strip under the title bar: `muted` while reconnecting with a
    /// spinner, `destructive`-tinted once offline with a Retry button.
    fn connection_banner(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let theme = UiTheme::read(cx).clone();
        // The strip slides open and closed (DESIGN.md §1 motion); its row keeps
        // its full height inside and is clipped, so the text never squashes.
        let full = f32::from(theme.space(8.));
        let shown = self.connection != ConnectionState::Online;
        let height = transition_value(
            "banner.height",
            if shown { full } else { 0. },
            theme.motion.normal,
            window,
            cx,
        );
        if height <= 0. {
            return None;
        }
        let state = if shown {
            &self.connection
        } else {
            &self.banner_last
        };
        let (icon, text, offline): (gpui_kit::AnyElement, SharedString, bool) = match state {
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
                    .size(scaled(&theme, metrics::ICON_SMALL))
                    .text_color(theme.colors.destructive)
                    .into_any_element(),
                "You're offline. Matty shows what it has and sends when you're back.".into(),
                true,
            ),
        };
        // What is waiting goes when the connection is back (#195).
        let text: SharedString = match self.queued {
            0 => text,
            1 => format!("{text} 1 message waiting to send.").into(),
            n => format!("{text} {n} messages waiting to send.").into(),
        };
        let foreground = if offline {
            theme.colors.destructive
        } else {
            theme.colors.foreground
        };
        let row = div()
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
                        .on_click(cx.listener(|this, _, _, cx| this.retry_connection(cx)))
                        .child("Retry now"),
                )
            });
        Some(
            div()
                .flex_shrink_0()
                .w_full()
                .h(px(height))
                .overflow_hidden()
                .flex()
                .flex_col()
                .justify_end()
                .child(row),
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
    /// Escape in the channel (#147): closes the right pane when one is
    /// open, else views the open channel on the server and clears its
    /// unread mark.
    fn mark_channel_read(&mut self, cx: &mut Context<Self>) {
        if self.right_pane_open {
            self.right_pane_open = false;
            cx.notify();
            return;
        }
        if self.threads_open || self.drafts_open {
            return;
        }
        let Some(id) = self.selected_channel.clone() else {
            return;
        };
        self.kept_unread = None;
        self.view_channels(std::slice::from_ref(&id));
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.mark_read(&id, cx));
    }

    /// Shift+Escape (#147): views every unread channel in the team on the
    /// server and clears the sidebar's unread marks.
    fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        let unread = self.sidebar.read(cx).unread_ids();
        self.view_channels(&unread);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.mark_all_read(cx));
    }

    /// Checks again a second after a post arrives in the open channel, so a
    /// burst of posts views the channel once.
    fn view_later(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._view_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(VIEW_DEBOUNCE).await;
            this.update_in(cx, |this, window, cx| this.view_if_seen(window, cx))
                .ok();
        }));
    }

    /// Views the open channel when it shows unread posts the user can see:
    /// the window has focus, nothing covers the channel, and the list is at
    /// its newest post. Otherwise the unread mark and the pill stay.
    fn view_if_seen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_channel.clone() else {
            return;
        };
        let seen = Seen {
            focused: window.is_window_active(),
            channel_shown: !self.threads_open && !self.drafts_open,
            covered: self.right_drawer_shown || self.sidebar_drawer_shown,
            at_newest: self.channel.read(cx).at_newest(cx),
            unread: self.sidebar.read(cx).is_unread(&id),
            kept_unread: self.kept_unread.as_ref() == Some(&id),
        };
        if seen.views() {
            self.view_channels(std::slice::from_ref(&id));
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.mark_read(&id, cx));
        }
    }

    /// Tells the server the user viewed `ids`, one request each.
    fn view_channels(&self, ids: &[gpui_kit::SharedString]) {
        let Some(live) = &self.live else {
            return;
        };
        for id in ids {
            live.send(matty_sync::Command::ViewChannel {
                channel_id: id.to_string(),
                prev_channel_id: String::new(),
            });
        }
    }

    /// Shows a notification for a post that just arrived, unless the
    /// policy says the user already sees it or does not want it.
    fn notify(&self, arrival: &crate::live::Arrival, window: &Window, cx: &Context<Self>) {
        let visible_thread = self
            .right_pane_open
            .then(|| match self.right_pane.read(cx).content() {
                Some(RightPaneContent::Thread(thread)) => Some(thread.root.id.to_string()),
                _ => None,
            })
            .flatten();
        let app = matty_notify::AppState {
            dnd: self.account.read(cx).account().presence == crate::sidebar::Presence::Dnd,
            focused: window.is_window_active(),
            visible_channel: (!self.threads_open && !self.drafts_open)
                .then(|| self.selected_channel.as_ref().map(|id| id.to_string()))
                .flatten(),
            visible_thread,
            show_preview: true,
            ..matty_notify::AppState::default()
        };
        if let Some(notification) = arrival.decide(app) {
            self.notifier.show(&notification);
        }
    }

    fn username(&self, cx: &gpui_kit::App) -> String {
        self.account.read(cx).account().username.to_string()
    }

    /// Sends the typed keywords when they differ from the server's.
    fn save_keywords(&mut self, cx: &mut Context<Self>) {
        let (Some(live), Some(props)) = (&self.live, &self.notify_props) else {
            return;
        };
        let typed = self.keywords.read(cx).value().to_string();
        let next = matty_notify::mention::with_keywords(&self.username(cx), props, &typed);
        if next != *props {
            live.send(matty_sync::Command::SetNotifyProps {
                notify_props: next.clone(),
            });
            self.notify_props = Some(next);
        }
    }

    /// Writes one of the user's `notify_props` to the server (#158). The
    /// server sends the user back, which updates the policy's preferences.
    fn set_notify_prop(&mut self, key: &str, value: &str, cx: &mut Context<Self>) {
        let Some(props) = &self.notify_props else {
            return;
        };
        if props.get(key).map(String::as_str) == Some(value) {
            return;
        }
        let mut next = props.clone();
        next.insert(key.to_owned(), value.to_owned());
        if let Some(live) = &self.live {
            live.send(matty_sync::Command::SetNotifyProps {
                notify_props: next.clone(),
            });
        }
        self.notify_props = Some(next);
        cx.notify();
    }

    /// Turns collapsed reply threads on or off for the user (#157). The
    /// server's preferences come back and every count follows.
    fn set_collapsed_threads(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some((choice, current)) = self.collapsed_threads else {
            return;
        };
        if current == on {
            return;
        }
        if let Some(live) = &self.live {
            live.send(matty_sync::Command::SetCollapsedThreads { on });
        }
        self.collapsed_threads = Some((choice, on));
        cx.notify();
    }

    /// Asks for a folder and saves downloads there from now on (#160).
    fn choose_download_folder(&mut self, cx: &mut Context<Self>) {
        let answer = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        let settings = self.settings.clone();
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(paths))) = answer.await else {
                return;
            };
            let Some(folder) = paths.into_iter().next() else {
                return;
            };
            settings.update(cx, |store, cx| {
                store.update(cx, |s| s.download_folder = Some(folder))
            });
        })
        .detach();
    }

    /// Preferences' Notifications section, from the user's `notify_props`
    /// with Mattermost's defaults (#158).
    fn notification_prefs(
        &self,
        props: &std::collections::HashMap<String, String>,
        labelled_row: &dyn Fn(SharedString, SharedString, gpui_kit::AnyElement) -> gpui_kit::Div,
        section: &dyn Fn(&'static str, gpui_kit::AnyElement) -> gpui_kit::Div,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let theme = UiTheme::read(cx).clone();
        let user = matty_notify::UserPrefs::from_props(&self.username(cx), "", props);
        let level: SharedString = match user.desktop {
            matty_notify::Level::All => "all",
            matty_notify::Level::Mention => "mention",
            matty_notify::Level::None => "none",
        }
        .into();
        let view = cx.entity().downgrade();
        let item = |id: &'static str, value: &'static str, text: &'static str| {
            ToggleGroupItem::new(id, value)
                .aria_label(text)
                .flex_1()
                .child(text)
        };
        let desktop = ToggleGroup::new("preferences.desktop")
            .aria_label("Notify me about")
            .joined(true)
            .w_full()
            .value([level])
            .item(item("preferences.desktop.all", "all", "All messages"))
            .item(item("preferences.desktop.mention", "mention", "Mentions"))
            .item(item("preferences.desktop.none", "none", "Nothing"))
            .on_change({
                let view = view.clone();
                move |next, _, cx| {
                    let value = next.first().map(|v| v.to_string());
                    view.update(cx, |this, cx| {
                        this.set_notify_prop("desktop", value.as_deref().unwrap_or("mention"), cx)
                    })
                    .ok();
                }
            });
        let on = |key: &str, default: bool| props.get(key).map_or(default, |v| v == "true");
        let switch = |id: &'static str,
                      title: &'static str,
                      detail: SharedString,
                      checked: bool,
                      key: &'static str,
                      values: (&'static str, &'static str)| {
            let view = view.clone();
            labelled_row(
                title.into(),
                detail,
                Switch::new(id)
                    .checked(checked)
                    .aria_label(title)
                    .on_change(move |next, _, _, cx| {
                        let value = if next { values.0 } else { values.1 };
                        view.update(cx, |this, cx| this.set_notify_prop(key, value, cx))
                            .ok();
                    })
                    .into_any_element(),
            )
        };
        let first_name = self.account.read(cx).account().display_name.clone();
        let first_name = first_name
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_owned();
        let first_name_detail: SharedString = if first_name.is_empty() {
            "Your first name counts as a mention.".into()
        } else {
            format!("\u{201c}{first_name}\u{201d} counts as a mention.").into()
        };
        div()
            .flex()
            .flex_col()
            .gap(theme.space(5.))
            .child(section(
                "Notifications",
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space(3.))
                    .child(desktop)
                    .child(switch(
                        "preferences.desktop-threads",
                        "Replies in threads I follow",
                        "Notify for every reply, not only mentions.".into(),
                        user.desktop_threads == matty_notify::Level::All,
                        "desktop_threads",
                        ("all", "mention"),
                    ))
                    .child(switch(
                        "preferences.desktop-sound",
                        "Sound",
                        "Play a sound with each notification.".into(),
                        user.desktop_sound,
                        "desktop_sound",
                        ("true", "false"),
                    ))
                    .into_any_element(),
            ))
            .child(section(
                "Mentions",
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space(3.))
                    .child(switch(
                        "preferences.first-name",
                        "First name",
                        first_name_detail,
                        on("first_name", false),
                        "first_name",
                        ("true", "false"),
                    ))
                    .child(switch(
                        "preferences.channel-wide",
                        "@channel, @all and @here",
                        "Messages to the whole channel count as mentions.".into(),
                        on("channel", true),
                        "channel",
                        ("true", "false"),
                    ))
                    .child(
                        Field::new("preferences.keywords", &self.keywords)
                            .label("Keywords")
                            .description(
                                "Words that count as mentioning you, separated by commas. \
                                 @username always does.",
                            ),
                    )
                    .into_any_element(),
            ))
    }

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
                        .child(lucide(glyph).size(scaled(&theme, metrics::ICON_SMALL)))
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
        let send_value: SharedString = match settings.send_on {
            SendKey::Enter => "enter",
            SendKey::ModEnter => "mod-enter",
        }
        .into();
        let mod_enter_label = match Platform::current() {
            Platform::MacOs => "Cmd+Enter",
            Platform::Other => "Ctrl+Enter",
        };
        let send_store = store.clone();
        let send_group = ToggleGroup::new("preferences.send")
            .aria_label("Send message with")
            .joined(true)
            .w_full()
            .value([send_value])
            .item(clock_item("preferences.send.enter", "enter", "Enter"))
            .item(clock_item(
                "preferences.send.mod-enter",
                "mod-enter",
                mod_enter_label,
            ))
            .on_change(move |next, _, cx| {
                let send_on = match next.first().map(|v| v.as_ref()) {
                    Some("mod-enter") => SendKey::ModEnter,
                    _ => SendKey::Enter,
                };
                send_store.update(cx, |store, cx| store.update(cx, |s| s.send_on = send_on));
            });
        let display_value: SharedString = match settings.display {
            MessageDisplay::Standard => "standard",
            MessageDisplay::Compact => "compact",
        }
        .into();
        let display_store = store.clone();
        let display_group = ToggleGroup::new("preferences.display")
            .aria_label("Message display")
            .joined(true)
            .w_full()
            .value([display_value])
            .item(clock_item(
                "preferences.display.standard",
                "standard",
                "Standard",
            ))
            .item(clock_item(
                "preferences.display.compact",
                "compact",
                "Compact",
            ))
            .on_change(move |next, _, cx| {
                let display = match next.first().map(|v| v.as_ref()) {
                    Some("compact") => MessageDisplay::Compact,
                    _ => MessageDisplay::Standard,
                };
                display_store.update(cx, |store, cx| store.update(cx, |s| s.display = display));
            });
        let display_hint = match settings.display {
            MessageDisplay::Standard => "Avatars and a name line above each message.",
            MessageDisplay::Compact => "One line per message, with the time in the margin.",
        };
        let display_section = div()
            .flex()
            .flex_col()
            .gap(theme.space(2.))
            .child(display_group)
            .child(
                div()
                    .type_style(style::META, &theme)
                    .text_color(theme.colors.muted_foreground)
                    .child(display_hint),
            );
        let send_hint = match settings.send_on {
            SendKey::Enter => "Shift+Enter starts a new line.",
            SendKey::ModEnter => "Enter starts a new line.",
        };
        let send_section = div()
            .flex()
            .flex_col()
            .gap(theme.space(2.))
            .child(send_group)
            .child(
                div()
                    .type_style(style::META, &theme)
                    .text_color(theme.colors.muted_foreground)
                    .child(send_hint),
            );
        let labelled_row =
            |title: SharedString, detail: SharedString, control: gpui_kit::AnyElement| {
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme.space(4.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
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
                    .child(control)
            };
        let switch_row =
            |id: &'static str,
             title: &'static str,
             detail: &'static str,
             checked: bool,
             write: fn(&mut crate::settings::Settings, bool)| {
                let store = store.clone();
                labelled_row(
                    title.into(),
                    detail.into(),
                    Switch::new(id)
                        .checked(checked)
                        .aria_label(title)
                        .on_change(move |next, _, _, cx| {
                            store.update(cx, |store, cx| store.update(cx, |s| write(s, next)))
                        })
                        .into_any_element(),
                )
            };
        let threads = self
            .collapsed_threads
            .filter(|(choice, _)| *choice)
            .map(|(_, on)| {
                let view = cx.entity().downgrade();
                section(
                    "Threads",
                    labelled_row(
                        "Collapsed reply threads".into(),
                        "Replies stay in their thread instead of the channel.".into(),
                        Switch::new("preferences.collapsed-threads")
                            .checked(on)
                            .aria_label("Collapsed reply threads")
                            .on_change(move |next, _, _, cx| {
                                view.update(cx, |this, cx| this.set_collapsed_threads(next, cx))
                                    .ok();
                            })
                            .into_any_element(),
                    )
                    .into_any_element(),
                )
            });
        let download_folder = settings
            .download_folder
            .clone()
            .or_else(dirs::download_dir)
            .map(|path| SharedString::from(path.display().to_string()))
            .unwrap_or_else(|| "Your Downloads folder".into());
        let files = div()
            .flex()
            .flex_col()
            .gap(theme.space(3.))
            .child(
                // A path can be long: one truncated line, the whole path
                // in a tooltip.
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme.space(4.))
                    .child(
                        div()
                            .id("preferences.download-folder.path")
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(theme.space(0.5))
                            .tooltip({
                                let path = download_folder.clone();
                                move |_, cx| crate::ui::tooltip::text_tooltip(path.clone(), cx)
                            })
                            .child(
                                div()
                                    .type_style(style::UI, &theme)
                                    .text_color(theme.colors.foreground)
                                    .child("Download folder"),
                            )
                            .child(
                                div()
                                    .type_style(style::META, &theme)
                                    .text_color(theme.colors.muted_foreground)
                                    .truncate()
                                    .child(download_folder),
                            ),
                    )
                    .child(
                        Button::new("preferences.download-folder")
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Sm)
                            .aria_label("Change download folder")
                            .label("Change…")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.choose_download_folder(cx)),
                            ),
                    ),
            )
            .child(labelled_row(
                "Log detail".into(),
                "How much goes into the logs.".into(),
                div()
                    .flex_shrink_0()
                    .w(scaled(&theme, metrics::FIELD_NARROW_WIDTH))
                    .child(Select::new(&self.log_level).aria_label("Log detail"))
                    .into_any_element(),
            ))
            .child(labelled_row(
                "Logs".into(),
                "Attach them when you report a problem.".into(),
                Button::new("preferences.open-logs")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Sm)
                    .aria_label("Open logs folder")
                    .label("Open folder")
                    .on_click(|_, _, cx| {
                        if let Some(dir) = crate::logging::default_dir() {
                            let _ = std::fs::create_dir_all(&dir);
                            cx.open_with_system(&dir);
                        }
                    })
                    .into_any_element(),
            ));
        let notifications = self
            .notify_props
            .as_ref()
            .map(|props| self.notification_prefs(props, &labelled_row, &section, cx));
        // The sections scroll inside the popup, which stays within the
        // window: the title and Done above them take about space(24.).
        let body_height = theme
            .space(158.)
            .min(window.viewport_size().height - theme.space(40.));
        let popup = dialog_popup("preferences.popup", "Preferences", cx)
            .child(dialog_title("preferences.title", cx).child("Preferences"))
            .child(
                dialog_description("preferences.description", cx)
                    .child("Changes apply right away and are saved for next time."),
            )
            .child(
                ScrollArea::new("preferences.body")
                    .aria_label("Preferences")
                    .h(body_height)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.space(5.))
                            .pt(theme.space(1.))
                            // Room for the scrollbar beside the switches.
                            .pr(theme.space(3.))
                            .child(section("Theme", theme_group.into_any_element()))
                            .child(section("Text size", size_group.into_any_element()))
                            .child(section("Clock", clock_group.into_any_element()))
                            .child(section(
                                "Send message with",
                                send_section.into_any_element(),
                            ))
                            .child(section(
                                "Message display",
                                display_section.into_any_element(),
                            ))
                            .children(threads)
                            .children(notifications)
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
                                    .child(switch_row(
                                        "preferences.reduce-motion",
                                        "Reduce motion",
                                        "Panes and banners change without sliding.",
                                        settings.reduce_motion,
                                        |s, v| s.reduce_motion = v,
                                    ))
                                    .into_any_element(),
                            ))
                            .child(section("Files and logs", files.into_any_element())),
                    ),
            )
            .child(dialog_close("preferences.close", cx).label("Done"));
        dialog("preferences", &self.preferences, popup, window, cx)
    }

    fn browse(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let handle = self.browse.read(cx).handle().clone();
        let popup = dialog_popup("browse.popup", "Browse channels", cx).child(self.browse.clone());
        dialog("browse", &handle, popup, window, cx)
    }

    fn directory_dialogs(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let message = self.new_message.read(cx).handle().clone();
        let create = self.create_channel.read(cx).handle().clone();
        div()
            .child(dialog(
                "new-message",
                &message,
                dialog_popup("new-message.popup", "New message", cx)
                    .child(self.new_message.clone()),
                window,
                cx,
            ))
            .child(dialog(
                "create-channel",
                &create,
                // Taller than gpuicn's default so the form never scrolls.
                dialog_popup("create-channel.popup", "Create channel", cx)
                    .max_h(UiTheme::read(cx).space(140.))
                    .child(self.create_channel.clone()),
                window,
                cx,
            ))
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let reduce_motion = self.settings.read(cx).get().reduce_motion;
        style::set_reduced_motion(reduce_motion, cx);
        let theme = UiTheme::read(cx).clone();
        let sidebar_width = px(self.settings.read(cx).get().sidebar_width);
        let settings = self.settings.clone();
        self.sync_drawers(window, cx);
        let banner = if self.login.is_none() {
            self.connection_banner(window, cx)
        } else {
            None
        };
        let narrow_sidebar = Self::narrow_sidebar(window);
        let content = self.content(window, cx);
        let frame = if self.sidebar_visible && !narrow_sidebar {
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
            .on_action(cx.listener(|this, _: &ToggleSidebar, window, cx| {
                if Self::narrow_sidebar(window) {
                    this.sidebar_drawer_open = !this.sidebar_drawer_open;
                } else {
                    this.sidebar_visible = !this.sidebar_visible;
                }
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
            .on_action(cx.listener(|this, _: &UnreadsFilter, _, cx| {
                this.sidebar
                    .update(cx, |sidebar, cx| sidebar.toggle_unreads(cx))
            }))
            .on_action(cx.listener(|this, _: &MarkChannelRead, _, cx| this.mark_channel_read(cx)))
            .on_action(cx.listener(|this, _: &MarkAllRead, _, cx| this.mark_all_read(cx)))
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
                cx.listener(|this, _: &DirectMessagesMenu, window, cx| {
                    this.new_message(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &BrowseChannels, window, cx| {
                    this.browse_channels(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &CreateChannel, window, cx| {
                this.open_create_channel(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &Search, window, cx| this.search("".into(), window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FocusSearch, window, cx| this.focus_search(window, cx)),
            )
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme.colors.background)
            .text_color(theme.colors.foreground)
            .font_family(theme.fonts.body.clone())
            .type_style(style::UI, &theme)
            .child(self.title_bar(narrow_sidebar, cx))
            .children(banner)
            .child(match &self.login {
                Some(login) => div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(login.clone())
                    .into_any_element(),
                // One selection root over the rail, sidebar, channel and
                // right pane, so a drag can select text across messages and
                // Cmd/Ctrl+C copies it in reading order (PLAN §4.2).
                None => div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        selection_root("window.selection")
                            .flex()
                            .when(self.rail.read(cx).is_multi(), |row| {
                                row.child(self.rail.clone())
                            })
                            .child(div().flex_1().min_w_0().h_full().child(frame)),
                    )
                    .into_any_element(),
            })
            .child(self.right_drawer(window, cx))
            .child(self.sidebar_drawer(window, cx))
            .child(self.shortcut_sheet(window, cx))
            .child(self.preferences(window, cx))
            .child(self.browse(window, cx))
            .child(self.directory_dialogs(window, cx))
            .child(switcher_element)
            .child(self.toasts.clone())
    }
}

/// Opens or closes `handle` to match `wanted`, remembering in `shown`
/// whether this code opened it. Returns true when the drawer was dismissed
/// on its own (Escape, the backdrop) while still wanted, so the caller can
/// clear the state that wanted it.
fn sync_drawer(
    handle: &DrawerHandle,
    shown: &mut bool,
    wanted: bool,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) -> bool {
    if wanted && !*shown {
        handle.open(window, cx);
        *shown = true;
    } else if wanted && !handle.is_open() {
        *shown = false;
        return true;
    } else if !wanted && *shown {
        handle.close(window, cx);
        *shown = false;
    }
    false
}

/// Opens a saved file in its app when it is a kind that only shows
/// content; anything else, such as a program or a script, is shown in its
/// folder for the user to decide (PLAN §4.6 Files).
fn open_download(path: &std::path::Path, cx: &mut gpui_kit::App) {
    if opens_safely(path) {
        cx.open_with_system(path);
    } else {
        cx.reveal_path(path);
    }
}

/// Whether a file of this kind is safe to open without asking.
fn opens_safely(path: &std::path::Path) -> bool {
    const SAFE: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "heic", "pdf", "txt", "md", "csv",
        "json", "log", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf",
        "mp3", "m4a", "wav", "ogg", "mp4", "mov", "webm", "zip",
    ];
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| SAFE.contains(&extension.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Seen, opens_safely};

    #[test]
    fn the_open_channel_counts_as_viewed_only_when_seen() {
        let seen = Seen {
            focused: true,
            channel_shown: true,
            covered: false,
            at_newest: true,
            unread: true,
            kept_unread: false,
        };
        assert!(seen.views(), "focused and at the newest post");
        assert!(
            !Seen {
                focused: false,
                ..seen
            }
            .views(),
            "unfocused"
        );
        assert!(
            !Seen {
                at_newest: false,
                ..seen
            }
            .views(),
            "scrolled up"
        );
        assert!(
            !Seen {
                covered: true,
                ..seen
            }
            .views(),
            "a drawer covers it"
        );
        assert!(
            !Seen {
                channel_shown: false,
                ..seen
            }
            .views(),
            "Threads is open"
        );
        assert!(
            !Seen {
                unread: false,
                ..seen
            }
            .views(),
            "nothing to view"
        );
        assert!(
            !Seen {
                kept_unread: true,
                ..seen
            }
            .views(),
            "marked unread and not left since"
        );
    }

    #[test]
    fn only_content_opens_without_asking() {
        assert!(opens_safely(Path::new("/d/menu.PNG")));
        assert!(opens_safely(Path::new("/d/notes (1).pdf")));
        assert!(!opens_safely(Path::new("/d/setup.exe")));
        assert!(!opens_safely(Path::new("/d/run.sh")));
        assert!(!opens_safely(Path::new("/d/Makefile")));
    }
}
