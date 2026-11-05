//! The app's side of sync: owns the [`SyncHandle`] and turns change sets
//! into the view models the window hands its views. Nothing here draws,
//! blocks, or touches the disk; the database lives on the sync thread.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use web_time::Instant;

use gpui_kit::SharedString;
use matty_api::models::{Channel, ChannelMember, ChannelStats, User};
use matty_api::{ApiError, Client, ProbeError, ServerCapabilities};
use matty_sync::{
    ChangeSet, Command, Connection, ImageSize, SidebarChannel, SidebarSection, SyncHandle,
    TeamWithUnreads, UploadState,
};

use crate::attachments::{AttachmentStatus, Upload, UploadSource};
use crate::channel::{ChannelModel, Post};
use crate::login::{PasswordSignIn, SignInFailure, SignInMethods, SsoProvider};
use crate::main_window::ConnectionState;
use crate::profile::UserProfile;
use crate::settings::SavedAccount;
use crate::sidebar::{Category, ChannelKind, ChannelRow, Presence};

mod extras;
mod popout;
mod teams;

/// A running sync engine and what the window last heard from it.
pub struct Live {
    sync: SyncHandle,
    /// The team the sidebar shows: the first in the user's order until
    /// they pick another.
    team: Option<String>,
    teams: Vec<TeamWithUnreads>,
    /// The server's address, for message links.
    server: String,
    sections: HashMap<String, Arc<[SidebarSection]>>,
    presence: HashMap<String, Presence>,
    /// The signed-in user's id and username.
    me: (String, String),
    /// The user's notification preferences and what counts as mentioning
    /// them, from their `notify_props`.
    prefs: matty_notify::UserPrefs,
    /// The user's own `notify_props`, the whole map, for Preferences to
    /// edit and send back.
    notify_props: HashMap<String, String>,
    /// Everyone whose posts have been on screen.
    users: HashMap<String, User>,
    /// The channel on screen and its latest posts snapshot.
    open: Option<ChannelRow>,
    posts: Option<Arc<[matty_api::models::Post]>>,
    max_post_size: Option<usize>,
    /// The thread in the right pane: its root, and its posts once sync
    /// has published them (root first).
    thread: Option<(String, Option<Arc<[matty_api::models::Post]>>)>,
    /// Whether the open thread has older replies to page in, and whether
    /// the last page failed (#122).
    thread_pages: (bool, bool),
    /// The roots of the open channel's replies that its posts leave out,
    /// for the "Commented on" line (#123).
    reply_roots: Arc<[matty_api::models::Post]>,
    /// The shown team's followed threads and their root posts, for the
    /// Threads view and the sidebar's count (#124).
    thread_list: Option<ThreadList>,
    /// Whether the server lets users choose collapsed reply threads, and
    /// whether they are on for the user, once each is known (#157).
    threads_choice: Option<bool>,
    threads_on: Option<bool>,
    /// Whether link previews show (#187); until sync says, they do.
    link_previews: bool,
    /// How people's names read (#156), from settings.
    names: crate::settings::NameFormat,
    /// Not connected: an open channel with no posts may just not be saved
    /// here yet (#163).
    offline: bool,
    zone: jiff::tz::TimeZone,
    /// The posts the channel view shows now, to send it only what changed.
    shown: Option<Vec<Post>>,
    /// Ephemeral messages and interactive messages' waiting actions (#211).
    interactive: crate::interactive::Session,
    /// The open channel's sends that failed and wait for Retry or Delete;
    /// their rows read "Not sent".
    failed: std::collections::HashSet<String>,
    /// Per channel, sends still waiting to go.
    queued: HashMap<String, usize>,
    /// The open channel as stored, the user's membership, and its counts
    /// once fetched.
    channel: Option<Arc<Channel>>,
    member: Option<Arc<ChannelMember>>,
    stats: Option<Arc<ChannelStats>>,
    /// Who is typing in the open channel, and since when.
    typing: Vec<(String, Instant)>,
    /// Who is typing a reply in the open thread, and since when (#137).
    thread_typing: Vec<(String, Instant)>,
    /// A channel sync made ready (joined, created, a new direct message)
    /// that the sidebar has yet to show.
    ready: Option<String>,
    /// The search on screen and its results once they come.
    search: Option<(String, Option<Arc<[matty_api::models::Post]>>)>,
    /// The search's last page asked for, whether another may follow, and
    /// whether it is on its way.
    search_page: (u32, bool, bool),
    /// The saved, pinned or mentions listing on screen and its posts once
    /// they come (#130–#132).
    listing: Option<(matty_sync::Listing, Option<Arc<[matty_api::models::Post]>>)>,
    /// The ids of the posts the user saved.
    saved: std::collections::HashSet<String>,
    /// The composer's files, by the tray's id: where each goes and its
    /// server id once up.
    uploads: HashMap<String, UploadJob>,
    /// Image files' thumbnails by file id, and those asked for.
    thumbnails: HashMap<String, Arc<gpui_kit::Image>>,
    asked_thumbnails: std::collections::HashSet<String>,
    /// Images from outside the server, in cards and inline, shared with
    /// the views (#159).
    remote_images: crate::remote_images::RemoteImages,
    /// Full-size images for the lightbox, by file id, once fetched.
    full_images: HashMap<String, Arc<gpui_kit::Image>>,
    /// Profile pictures, custom emoji images and team icons, shared with
    /// the views (#153).
    avatars: crate::avatars::Avatars,
    emoji_images: crate::avatars::EmojiImages,
    team_icons: crate::avatars::TeamIcons,
    /// The server's custom emoji by name, shared with the views.
    custom_emoji: crate::emoji::CustomEmoji,
    /// Which windows hold each channel and thread open (#222).
    views: popout::OpenViews,
    /// The open channel's bookmarks and App bindings, and this view's App
    /// calls (#226).
    extras: extras::Extras,
}

/// One tray file on its way to the server.
#[derive(Clone, Debug)]
pub struct UploadJob {
    pub channel_id: String,
    pub name: String,
    pub source: crate::attachments::UploadSource,
    file_id: Option<String>,
    /// Waiting on this computer for the connection (#194): it goes with
    /// its post through the outbox, or uploads once back online.
    queued: Option<PathBuf>,
}

impl UploadJob {
    pub fn new(
        channel_id: impl Into<String>,
        name: impl Into<String>,
        source: crate::attachments::UploadSource,
    ) -> Self {
        Self {
            channel_id: channel_id.into(),
            name: name.into(),
            source,
            file_id: None,
            queued: None,
        }
    }
}

/// How long a typing notice lasts without another.
pub const TYPING_FOR: Duration = Duration::from_secs(5);

/// The channel header's text and counts.
#[derive(Debug, PartialEq)]
pub struct Details {
    pub header: Option<SharedString>,
    pub purpose: Option<SharedString>,
    pub members: usize,
    pub pinned: usize,
    /// See [`crate::channel::ChannelModel::can_manage`].
    pub can_manage: bool,
    /// See [`crate::channel::ChannelModel::delete_others`].
    pub delete_others: bool,
    /// The bookmarks bar, where the server has bookmarks (#226).
    pub bookmarks: Option<crate::bookmarks::BookmarksModel>,
    /// What installed Apps add to the channel (#226).
    pub apps: crate::apps::ChannelApps,
}

/// What a change set means for the window.
#[derive(Debug, Default, PartialEq)]
pub struct Update {
    /// The shown team's categories, to replace the sidebar's.
    pub categories: Option<Vec<Category>>,
    /// The shown team's display name, for the sidebar header.
    pub team_name: Option<SharedString>,
    /// The user's teams with their badges, for the rail, and the one on
    /// screen (#89).
    pub teams: Option<(Vec<crate::rail::Team>, Option<SharedString>)>,
    pub connection: Option<ConnectionState>,
    /// What changed in the open channel's view.
    pub channel: Option<ChannelUpdate>,
    /// The open channel's header and counts.
    pub details: Option<Details>,
    /// Something the server or an App refused, for a toast (#226).
    pub refused: Option<SharedString>,
    /// A link an App answered with, for Matty's link routing (#226).
    pub navigate: Option<SharedString>,
    /// Who is typing in the open channel now; the window checks again
    /// after [`TYPING_FOR`].
    pub typing: Option<Vec<SharedString>>,
    /// Who is typing a reply in the open thread now (#137).
    pub thread_typing: Option<Vec<SharedString>>,
    /// Messages waiting to send across every channel, failed ones left
    /// out, for the connection banner (#195).
    pub queued: Option<usize>,
    /// The user's own presence from the server, with when Do not disturb
    /// ends if the server said (#190).
    pub own_presence: Option<(Presence, Option<i64>)>,
    /// The user's own custom status and their recent ones, newest first,
    /// when the server sent the user (#221).
    pub own_status: Option<(
        Option<crate::custom_status::StatusLine>,
        Vec<crate::custom_status::StatusLine>,
    )>,
    /// The open thread, for the right pane.
    pub thread: Option<crate::right_pane::ThreadModel>,
    /// The shown team's followed threads, for the Threads view (#124).
    pub threads: Option<crate::threads::ThreadsModel>,
    /// A link's preview for the composer, or `None` when it has none
    /// (#188).
    pub link_preview: Option<(String, Option<crate::embeds::LinkCard>)>,
    /// The user's own `notify_props` from the server, for Preferences.
    pub notify_props: Option<HashMap<String, String>>,
    /// The user's own name and username when the server sends them, so the
    /// account menu follows a profile edit (#212).
    pub me: Option<(String, String)>,
    /// The user's display and advanced settings from the server, for
    /// Preferences to mirror (#156, #157).
    pub account_preferences: Option<Arc<[matty_api::models::Preference]>>,
    /// Whether the user may choose collapsed reply threads, and whether
    /// they are on, for Preferences (#157).
    pub collapsed_threads: Option<(bool, bool)>,
    /// Whether the server takes highlight-only keywords, for Preferences
    /// (#192).
    pub highlight_keys: Option<bool>,
    /// Someone else's post that just arrived, for the window to notify
    /// about if the policy says so (#141).
    pub arrived: Option<Arrival>,
    /// The people in the open channel's posts.
    pub profiles: Option<Vec<UserProfile>>,
    /// The shown team's public channels, for Browse channels.
    pub directory: Option<Vec<crate::directory::DirectoryChannel>>,
    /// The shown team's people, for a new direct message.
    pub people: Option<Vec<UserProfile>>,
    /// Who the server says matches a typed `@name`, channel members first.
    pub suggested: Option<(String, Vec<UserProfile>)>,
    /// A channel to switch to: the one the user joined, created or
    /// started, now in the sidebar.
    pub ready: Option<SharedString>,
    /// A join the server refused, for Browse to show as not joined again.
    pub join_failed: Option<SharedString>,
    /// An action that could not reach the server, as sync names it, for
    /// the window to say the user is offline (#163).
    pub offline_refused: Option<&'static str>,
    /// The server refused an interactive message's action, in its words
    /// (#211).
    pub action_refused: Option<SharedString>,
    /// The open channel's mute and notification preferences.
    pub notifications: Option<(bool, crate::channel::ChannelNotifications)>,
    /// The search's results, or its failure.
    pub search: Option<crate::right_pane::SearchModel>,
    /// The saved, pinned or mentions listing on screen, once loaded or
    /// changed.
    pub listing: Option<crate::right_pane::RightPaneContent>,
    /// A file's full-size image, for the lightbox.
    pub full_image: Option<(SharedString, Arc<gpui_kit::Image>)>,
    /// A download's progress, where it was saved, or the failure, by file id.
    pub download: Option<(SharedString, matty_sync::DownloadState)>,
    /// A tray file's progress, by the tray's id.
    pub upload: Option<(SharedString, crate::attachments::AttachmentStatus)>,
    /// Tray files queued while offline, to upload now that the connection
    /// is back (#194).
    pub resume_uploads: Vec<SharedString>,
    /// The server's `MaxFileSize`, for the trays to check files against.
    pub max_file_size: Option<u64>,
    /// What the server allows of message priority, acknowledgements and
    /// scheduling (#220).
    pub features: Option<crate::priority::Features>,
    /// The user's scheduled messages, when they changed (#220).
    pub scheduled: Option<Arc<[matty_api::models::ScheduledPost]>>,
    /// The open channel's history: whether its first post is loaded,
    /// whether it reaches the newest post, and whether the last page failed.
    pub history: Option<(bool, bool, bool)>,
    /// A profile picture, custom emoji, team icon or outside image
    /// arrived, or the custom emoji changed; the windows redraw to show it.
    pub avatars: bool,
    /// What a link in a message turned out to name (#85, #103).
    pub link: Option<(matty_sync::LinkQuery, Option<matty_sync::LinkAnswer>)>,
    /// The session ended; the window returns to sign-in.
    pub signed_out: bool,
    /// The server refused the saved session; the window asks the user to
    /// sign in again.
    pub expired: bool,
    /// A draft as sync stores it now, or `None` once deleted (#114).
    pub draft: Option<(
        crate::composer_draft::DraftKey,
        Option<Arc<matty_api::models::Draft>>,
    )>,
}

/// A post that just arrived, with the preferences that decide whether it
/// notifies. Only the window knows what is on screen, so it decides.
#[derive(Debug, PartialEq)]
pub struct Arrival {
    pub me: String,
    pub event: matty_notify::Event,
    pub user: matty_notify::UserPrefs,
    pub channel: matty_notify::ChannelPrefs,
    pub collapsed_threads: bool,
    /// An urgent post that asks for persistent notifications (#220).
    pub persistent: bool,
}

impl Arrival {
    pub(crate) fn new(
        arrived: &matty_sync::Arrived,
        me: &str,
        user: &matty_notify::UserPrefs,
    ) -> Self {
        use matty_notify::ChannelKind;
        let post = &arrived.post;
        let channel_kind = match arrived.channel_type.as_str() {
            "P" => ChannelKind::Private,
            "D" => ChannelKind::Direct,
            "G" => ChannelKind::Group,
            _ => ChannelKind::Public,
        };
        Self {
            me: me.to_owned(),
            event: matty_notify::Event {
                post_id: post.id.clone(),
                channel_id: post.channel_id.clone(),
                root_id: post.root_id.clone(),
                author_id: post.user_id.clone(),
                author_name: arrived.sender_name.trim_start_matches('@').to_owned(),
                channel_name: arrived.channel_display_name.clone(),
                channel_kind,
                message: post.message.clone(),
                system: post.kind.starts_with("system_"),
                server_mentioned: arrived.mentioned,
                followed_thread: arrived.followed,
            },
            user: user.clone(),
            channel: matty_notify::ChannelPrefs::from_props(&arrived.channel_props),
            collapsed_threads: arrived.collapsed_threads,
            persistent: post
                .metadata
                .priority
                .as_ref()
                .is_some_and(|priority| priority.is_urgent() && priority.persistent_notifications),
        }
    }

    /// The notification to show, if any, given what the window shows.
    pub fn decide(&self, app: matty_notify::AppState) -> Option<matty_notify::Notification> {
        let app = matty_notify::AppState {
            me: self.me.clone(),
            collapsed_threads: self.collapsed_threads,
            ..app
        };
        matty_notify::decide(&self.event, &self.user, &self.channel, &app)
    }
}

/// A change to the open channel's view.
#[derive(Debug, PartialEq)]
pub enum ChannelUpdate {
    /// Show this model, as on opening a channel; resets the scroll.
    Replace(ChannelModel),
    /// Patch the posts on screen and keep the scroll.
    Patch {
        removed: Vec<SharedString>,
        /// Older posts, to go above the first one shown.
        prepended: Vec<Post>,
        updated: Vec<Post>,
        appended: Vec<Post>,
    },
}

/// A membership's mute and notification preferences, from Mattermost's
/// `notify_props`. Muting is `mark_unread: mention`, as in the web app.
pub fn notifications(member: &ChannelMember) -> (bool, crate::channel::ChannelNotifications) {
    use crate::channel::{ChannelNotifications, NotifyLevel};
    let prop = |key: &str| member.notify_props.get(key).map_or("", String::as_str);
    let level = |key: &str| match prop(key) {
        "all" => NotifyLevel::All,
        "mention" => NotifyLevel::Mentions,
        "none" => NotifyLevel::None,
        _ => NotifyLevel::Default,
    };
    let muted = prop("mark_unread") == "mention";
    (
        muted,
        ChannelNotifications {
            desktop: level("desktop"),
            mobile: level("push"),
            ignore_channel_mentions: prop("ignore_channel_mentions") == "on",
            mark_unread_mentions_only: muted,
        },
    )
}

/// The `notify_props` to send for a mute switch and preferences.
pub fn notify_props(
    muted: bool,
    notifications: &crate::channel::ChannelNotifications,
) -> HashMap<String, String> {
    use crate::channel::NotifyLevel;
    let level = |level: NotifyLevel| match level {
        NotifyLevel::Default => "default",
        NotifyLevel::All => "all",
        NotifyLevel::Mentions => "mention",
        NotifyLevel::None => "none",
    };
    let mark_unread = if muted || notifications.mark_unread_mentions_only {
        "mention"
    } else {
        "all"
    };
    HashMap::from([
        (
            "desktop".to_string(),
            level(notifications.desktop).to_string(),
        ),
        ("push".to_string(), level(notifications.mobile).to_string()),
        ("mark_unread".to_string(), mark_unread.to_string()),
        (
            "ignore_channel_mentions".to_string(),
            if notifications.ignore_channel_mentions {
                "on"
            } else {
                "off"
            }
            .to_string(),
        ),
    ])
}

/// What an image's bytes are. The server's type is a fallback only: it
/// labels every thumbnail `image/jpeg`, even one it kept as PNG.
pub(crate) fn image_format(bytes: &[u8], content_type: &str) -> Option<gpui_kit::ImageFormat> {
    use gpui_kit::ImageFormat;
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some(ImageFormat::Png),
        [0xFF, 0xD8, 0xFF, ..] => Some(ImageFormat::Jpeg),
        [b'G', b'I', b'F', b'8', ..] => Some(ImageFormat::Gif),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some(ImageFormat::Webp),
        [b'B', b'M', ..] => Some(ImageFormat::Bmp),
        _ => ImageFormat::from_mime_type(content_type),
    }
}

/// How to go from the posts on screen to `new` without a reset, or `None`
/// when only a reset will do: a post appeared between two shown ones, or
/// the order changed. Posts before the first shown one are older history
/// and go on top. Unsent rows at the bottom may be passed by posts that
/// arrive under them, as after a reconnect's backfill (#164): they are
/// taken out and appended again in their new place.
fn patch(old: &[Post], new: &[Post]) -> Option<ChannelUpdate> {
    let kept: HashMap<&str, &Post> = old.iter().map(|post| (post.id.as_ref(), post)).collect();
    let present: std::collections::HashSet<&str> =
        new.iter().map(|post| post.id.as_ref()).collect();
    let removed: Vec<SharedString> = old
        .iter()
        .filter(|post| !present.contains(post.id.as_ref()))
        .map(|post| post.id.clone())
        .collect();
    let remaining: Vec<&str> = old
        .iter()
        .map(|post| post.id.as_ref())
        .filter(|id| present.contains(id))
        .collect();
    let start = remaining
        .first()
        .and_then(|first| new.iter().position(|post| post.id.as_ref() == *first))
        .unwrap_or(0);
    let (prepended, new) = new.split_at(start);
    let matched = new
        .iter()
        .zip(&remaining)
        .take_while(|(post, id)| post.id.as_ref() == **id)
        .count();
    // The shown posts after the matching run must all be unsent ones.
    let unsent = |id: &&str| {
        kept.get(id)
            .is_some_and(|post| post.status != crate::channel::PostStatus::Sent)
    };
    if !remaining[matched..].iter().all(unsent) {
        return None;
    }
    let mut removed = removed;
    removed.extend(
        remaining[matched..]
            .iter()
            .map(|id| SharedString::from(id.to_string())),
    );
    let (head, tail) = new.split_at(matched);
    let updated = head
        .iter()
        .filter(|post| kept.get(post.id.as_ref()) != Some(post))
        .cloned()
        .collect();
    Some(ChannelUpdate::Patch {
        removed,
        prepended: prepended.to_vec(),
        updated,
        appended: tail.to_vec(),
    })
}

impl Live {
    /// Starts sync over the database at `path` and returns it with the
    /// change sets to apply.
    pub fn start(
        path: PathBuf,
        account: &SavedAccount,
    ) -> std::io::Result<(Self, async_channel::Receiver<ChangeSet>)> {
        let media = media_dir(account);
        let sync = matty_sync::spawn_at(path, media)?;
        Ok(Self::start_with(sync, account))
    }

    /// Follows `sync`, already started for `account`.
    pub fn start_with(
        sync: matty_sync::SyncHandle,
        account: &SavedAccount,
    ) -> (Self, async_channel::Receiver<ChangeSet>) {
        let changes = sync.subscribe();
        let ask = |picture: matty_sync::Picture| -> crate::avatars::Ask {
            let asker = sync.clone();
            std::rc::Rc::new(move |id: &str, version| {
                asker.send(Command::LoadPicture {
                    picture,
                    id: id.to_owned(),
                    version,
                });
            })
        };
        let avatars = crate::avatars::Avatars::new(ask(matty_sync::Picture::Avatar));
        let emoji_images = crate::avatars::EmojiImages::new(ask(matty_sync::Picture::Emoji));
        let team_icons = crate::avatars::TeamIcons::new(ask(matty_sync::Picture::TeamIcon));
        let fetcher = sync.clone();
        let remote_images = crate::remote_images::RemoteImages::new(
            std::rc::Rc::new(move |url: &str| {
                fetcher.send(Command::LoadRemoteImage {
                    url: url.to_owned(),
                });
            }),
            false,
        );
        (
            Self {
                sync,
                team: None,
                teams: Vec::new(),
                server: account.server.clone(),
                sections: HashMap::new(),
                presence: HashMap::new(),
                me: (account.user_id.clone(), account.username.clone()),
                prefs: matty_notify::UserPrefs::from_props(&account.username, "", &HashMap::new()),
                notify_props: HashMap::new(),
                users: HashMap::new(),
                open: None,
                posts: None,
                max_post_size: None,
                thread: None,
                thread_pages: (false, false),
                reply_roots: Arc::from([]),
                thread_list: None,
                threads_choice: None,
                threads_on: None,
                link_previews: true,
                names: crate::settings::NameFormat::default(),
                offline: crate::offline(),
                zone: jiff::tz::TimeZone::system(),
                shown: None,
                interactive: Default::default(),
                failed: Default::default(),
                queued: HashMap::new(),
                channel: None,
                member: None,
                stats: None,
                typing: Vec::new(),
                thread_typing: Vec::new(),
                ready: None,
                search: None,
                search_page: (0, false, false),
                listing: None,
                saved: Default::default(),
                uploads: HashMap::new(),
                thumbnails: HashMap::new(),
                asked_thumbnails: std::collections::HashSet::new(),
                remote_images,
                full_images: HashMap::new(),
                avatars,
                emoji_images,
                team_icons,
                custom_emoji: crate::emoji::CustomEmoji::default(),
                views: popout::OpenViews::default(),
                extras: extras::Extras::default(),
            },
            changes,
        )
    }

    pub fn send(&self, command: Command) {
        if self.views.allows(&command) {
            self.sync.send(command);
        }
    }

    /// Remembers a tray file so its progress reaches the tray.
    pub fn track_upload(&mut self, id: &str, job: UploadJob) {
        self.uploads.insert(id.to_owned(), job);
    }

    /// Whether sync is offline, so a new tray file waits on this computer
    /// instead of failing to upload (#194).
    pub fn is_offline(&self) -> bool {
        self.offline || crate::offline()
    }

    /// Keeps a tray file on this computer until the post goes or the
    /// connection is back; `path` is where its bytes are.
    pub fn queue_upload(&mut self, id: &str, path: PathBuf) {
        if let Some(job) = self.uploads.get_mut(id) {
            job.queued = Some(path);
        }
    }

    /// The tray file `id`, to send again after a failure.
    pub fn upload_job(&self, id: &str) -> Option<UploadJob> {
        self.uploads.get(id).cloned()
    }

    /// Forgets a tray file and stops it if it is still sending.
    pub fn cancel_upload(&mut self, id: &str) {
        if self.uploads.remove(id).is_some() {
            self.sync.send(Command::CancelUpload {
                upload_id: id.to_owned(),
            });
        }
    }

    /// The server ids of tray files that are up, for a draft (#114).
    pub fn file_ids(&self, ids: &[SharedString]) -> Vec<String> {
        ids.iter()
            .filter_map(|id| self.uploads.get(id.as_ref())?.file_id.clone())
            .collect()
    }

    /// A file a stored draft carries, as a finished tray upload whose id is
    /// its file id, so a send or a later save finds it again.
    pub fn draft_upload(&mut self, channel_id: &str, file_id: &str) -> Upload {
        let name = self
            .uploads
            .values()
            .find(|job| job.file_id.as_deref() == Some(file_id))
            .map(|job| job.name.clone())
            .or_else(|| self.file_name(file_id))
            .unwrap_or_else(|| "Attachment".to_owned());
        let source = match self.thumbnails.get(file_id) {
            Some(image) => UploadSource::Image(image.clone()),
            None => UploadSource::Path(std::path::PathBuf::new()),
        };
        self.uploads.entry(file_id.to_owned()).or_insert(UploadJob {
            channel_id: channel_id.to_owned(),
            name: name.clone(),
            source: source.clone(),
            file_id: Some(file_id.to_owned()),
            queued: None,
        });
        Upload {
            id: file_id.to_owned().into(),
            name: name.into(),
            size: 0,
            source,
            status: AttachmentStatus::Ready,
            queued: false,
        }
    }

    /// The tray files a post carries, as server ids and, for files queued
    /// while offline, local paths for the outbox to upload (#194); they
    /// leave the tray with it.
    pub fn take_files(&mut self, ids: &[SharedString]) -> (Vec<String>, Vec<PathBuf>) {
        let mut file_ids = Vec::new();
        let mut local = Vec::new();
        for job in ids.iter().filter_map(|id| self.uploads.remove(id.as_ref())) {
            match (job.file_id, job.queued) {
                (Some(file_id), _) => file_ids.push(file_id),
                (None, Some(path)) => local.push(path),
                (None, None) => {}
            }
        }
        (file_ids, local)
    }

    /// Shows `row`'s channel: sync publishes its posts now and as they
    /// change. Returns the model to show until they come.
    pub fn open(&mut self, row: ChannelRow) -> ChannelModel {
        if let Some(old) = self.open.take()
            && old.id != row.id
        {
            self.send(Command::CloseChannel {
                channel_id: old.id.to_string(),
            });
        }
        self.send(Command::OpenChannel {
            channel_id: row.id.to_string(),
        });
        self.posts = None;
        self.reply_roots = Arc::from([]);
        self.shown = None;
        self.failed.clear();
        self.channel = None;
        self.member = None;
        self.stats = None;
        self.typing.clear();
        self.ask_bindings(&row.id);
        self.open = Some(row);
        self.channel_model().expect("a channel is open")
    }

    /// The open channel's view change for the latest posts: the first
    /// snapshot replaces the model, later ones patch it.
    /// Collapsed reply threads for Preferences, once the server's mode
    /// and the user's setting are both known.
    fn threads(&self) -> Option<(bool, bool)> {
        Some((self.threads_choice?, self.threads_on?))
    }

    fn channel_update(&mut self) -> Option<ChannelUpdate> {
        let model = self.channel_model()?;
        if model.loading {
            return None;
        }
        self.ask_thumbnails(&model.posts);
        self.ask_remote_images(&model.posts);
        let update = self
            .shown
            .as_deref()
            .and_then(|old| patch(old, &model.posts));
        self.shown = Some(model.posts.clone());
        Some(update.unwrap_or(ChannelUpdate::Replace(model)))
    }

    /// Takes the user's own `notify_props` from `user`. Other fetches of
    /// the user leave the map out, so an empty one changes nothing.
    fn take_me(&mut self, user: &User, update: &mut Update) {
        if user.id == self.me.0 {
            self.me.1 = user.username.clone();
            update.me = Some((crate::transcript::display_name(user), user.username.clone()));
            let now = jiff::Timestamp::now();
            let recent = user
                .recent_custom_statuses()
                .iter()
                // Recents offer the status again, so an old end does not
                // hide them.
                .filter_map(|status| {
                    crate::custom_status::StatusLine::from_server(
                        &matty_api::models::CustomStatus {
                            expires_at: String::new(),
                            ..status.clone()
                        },
                        now,
                    )
                })
                .collect();
            update.own_status = Some((crate::custom_status::StatusLine::of(user), recent));
        }
        self.take_notify_props(user, update);
    }

    fn take_notify_props(&mut self, user: &User, update: &mut Update) {
        if user.id == self.me.0 && !user.notify_props.is_empty() {
            self.prefs = matty_notify::UserPrefs::from_props(
                &user.username,
                &user.first_name,
                &user.notify_props,
            );
            self.notify_props = user.notify_props.clone();
            update.notify_props = Some(self.notify_props.clone());
        }
    }

    fn channel_model(&self) -> Option<ChannelModel> {
        let row = self.open.as_ref()?;
        let viewer = crate::transcript::Viewer {
            user_id: &self.me.0,
            keys: &self.prefs.keys,
            saved: &self.saved,
            zone: &self.zone,
            now: jiff::Timestamp::now(),
            link_previews: self.link_previews,
            collapsed_threads: self.threads_on.unwrap_or(false),
            names: self.names,
        };
        let with_ephemeral = crate::interactive::with_ephemeral(
            self.posts.as_deref(),
            self.interactive.ephemeral(),
            &row.id,
        );
        let mut model = crate::transcript::channel_model(
            row,
            with_ephemeral.as_deref().or(self.posts.as_deref()),
            &self.users,
            &viewer,
            self.max_post_size,
            &self.reply_roots,
        );
        let details = self.details(model.pinned);
        model.person = row
            .person
            .as_ref()
            .and_then(|id| self.users.get(id.as_ref()))
            .map(|user| Box::new(self.profile(user)));
        model.header = details.header;
        model.purpose = details.purpose;
        model.members = details.members;
        model.pinned = details.pinned;
        model.typing = self.typing_names();
        if let Some(member) = &self.member {
            (model.muted, model.notifications) = notifications(member);
        }
        model.profiles = self.profiles();
        model.offline = self.offline;
        for post in &mut model.posts {
            if self.failed.contains(post.id.as_ref()) {
                post.status = crate::channel::PostStatus::Failed;
            }
        }
        for file in model.posts.iter_mut().flat_map(|post| {
            let crate::channel::Post { files, embeds, .. } = post;
            files
                .iter_mut()
                .chain(embeds.iter_mut().flat_map(crate::embeds::Embed::files_mut))
        }) {
            if let Some(image) = self.thumbnails.get(file.id.as_ref()) {
                file.preview = Some(crate::attachments::UploadSource::Image(image.clone()));
            }
        }
        for image in model
            .posts
            .iter_mut()
            .flat_map(|post| &mut post.embeds)
            .flat_map(crate::embeds::Embed::images_mut)
        {
            image.loaded = self.remote_images.image(&image.url);
        }
        self.interactive.fill(
            &mut model.posts,
            &self.users,
            self.names,
            self.sections.values().flat_map(|sections| sections.iter()),
        );
        Some(model)
    }

    /// The name a file was posted with, when its post is in the open
    /// channel or the search results.
    pub fn file_name(&self, file_id: &str) -> Option<String> {
        let searched = self.search.as_ref().and_then(|(_, posts)| posts.as_deref());
        self.posts
            .as_deref()
            .into_iter()
            .chain(searched)
            .flatten()
            .flat_map(|post| &post.metadata.files)
            .find(|file| file.id == file_id)
            .map(|file| file.name.clone())
    }

    /// The full-size images already here for `file_ids`; sync is asked
    /// for the rest, which come as [`Update::full_image`].
    pub fn full_images(
        &self,
        file_ids: &[SharedString],
    ) -> Vec<(SharedString, Arc<gpui_kit::Image>)> {
        let mut here = Vec::new();
        for id in file_ids {
            match self.full_images.get(id.as_ref()) {
                Some(image) => here.push((id.clone(), image.clone())),
                None => {
                    self.sync.send(Command::LoadImage {
                        file_id: id.to_string(),
                        size: ImageSize::Preview,
                    });
                }
            }
        }
        here
    }

    /// Asks sync for the cards' outside images not yet asked for.
    fn ask_remote_images(&mut self, posts: &[Post]) {
        for image in posts
            .iter()
            .flat_map(|post| &post.embeds)
            .flat_map(crate::embeds::Embed::images)
        {
            self.remote_images.want(&image.url);
        }
    }

    /// Asks sync for the thumbnails of image files not yet asked for.
    fn ask_thumbnails(&mut self, posts: &[Post]) {
        for file in posts.iter().flat_map(|post| {
            post.files
                .iter()
                .chain(post.embeds.iter().flat_map(crate::embeds::Embed::files))
        }) {
            if crate::attachments::is_image(&file.name)
                && self.asked_thumbnails.insert(file.id.to_string())
            {
                self.sync.send(Command::LoadImage {
                    file_id: file.id.to_string(),
                    size: ImageSize::Thumbnail,
                });
            }
        }
    }

    /// The header, purpose and counts as far as they are known; until the
    /// counts come, pins are those among the loaded posts.
    fn details(&self, pinned_loaded: usize) -> Details {
        let text =
            |text: &str| (!text.trim().is_empty()).then(|| SharedString::from(text.to_string()));
        let count = |count: i64| usize::try_from(count).unwrap_or(0);
        Details {
            header: self.channel.as_ref().and_then(|c| text(&c.header)),
            purpose: self.channel.as_ref().and_then(|c| text(&c.purpose)),
            members: self.stats.as_ref().map_or(0, |s| count(s.member_count)),
            pinned: self
                .stats
                .as_ref()
                .map_or(pinned_loaded, |s| count(s.pinnedpost_count)),
            can_manage: self.can_manage(),
            delete_others: self.is_system_admin(),
            bookmarks: self.bookmarks_model(),
            apps: self.channel_apps(),
        }
    }

    /// Whether the user administers the open channel: its admin, or the
    /// system's. Team admins pass on the server, but their role is not kept.
    fn can_manage(&self) -> bool {
        let has = |roles: &str, role: &str| roles.split_whitespace().any(|r| r == role);
        let channel_admin = self
            .member
            .as_ref()
            .is_some_and(|member| member.scheme_admin || has(&member.roles, "channel_admin"));
        channel_admin || self.is_system_admin()
    }

    /// Whether the user is a system admin.
    fn is_system_admin(&self) -> bool {
        self.users.get(&self.me.0).is_some_and(|me| {
            me.roles
                .split_whitespace()
                .any(|role| role == "system_admin")
        })
    }

    fn typing_names(&self) -> Vec<SharedString> {
        self.names_of(&self.typing)
    }

    fn thread_typing_names(&self) -> Vec<SharedString> {
        self.names_of(&self.thread_typing)
    }

    fn names_of(&self, typing: &[(String, Instant)]) -> Vec<SharedString> {
        typing
            .iter()
            .map(|(user_id, _)| {
                self.users
                    .get(user_id)
                    .map(|user| crate::transcript::display_name_as(user, self.names))
                    .unwrap_or_else(|| "Someone".into())
                    .into()
            })
            .collect()
    }

    /// Everyone with a post in the open channel's snapshot, first poster
    /// first.
    fn profiles(&self) -> Vec<UserProfile> {
        let mut seen = std::collections::HashSet::new();
        self.posts
            .iter()
            .flat_map(|posts| posts.iter())
            .filter(|post| seen.insert(post.user_id.as_str()))
            .filter_map(|post| self.users.get(&post.user_id))
            .map(|user| self.profile(user))
            .collect()
    }

    /// Drops typing notices older than [`TYPING_FOR`]; the names left in
    /// the channel and in the open thread, for each where any went.
    pub fn expire_typing(
        &mut self,
        now: Instant,
    ) -> (Option<Vec<SharedString>>, Option<Vec<SharedString>>) {
        let fresh =
            |(_, since): &(String, Instant)| now.saturating_duration_since(*since) < TYPING_FOR;
        let (channel, thread) = (self.typing.len(), self.thread_typing.len());
        self.typing.retain(fresh);
        self.thread_typing.retain(fresh);
        (
            (self.typing.len() != channel).then(|| self.typing_names()),
            (self.thread_typing.len() != thread).then(|| self.thread_typing_names()),
        )
    }

    /// Searches the shown team; the results come as [`Update::search`].
    pub fn search(&mut self, terms: &str) {
        let Some(team_id) = self.team.clone() else {
            return;
        };
        let offset = self.zone.to_offset(jiff::Timestamp::now()).seconds();
        self.search = Some((terms.to_owned(), None));
        self.search_page = (0, false, false);
        self.send(Command::Search {
            team_id,
            terms: terms.to_owned(),
            time_zone_offset: offset,
            page: 0,
        });
    }

    /// Asks for the search's next page; it comes as [`Update::search`]
    /// with the results so far.
    pub fn search_more(&mut self) -> Option<crate::right_pane::SearchModel> {
        let (page, more, loading) = self.search_page;
        let (terms, Some(_)) = self.search.as_ref()? else {
            return None;
        };
        if !more || loading {
            return None;
        }
        let team_id = self.team.clone()?;
        let offset = self.zone.to_offset(jiff::Timestamp::now()).seconds();
        self.send(Command::Search {
            team_id,
            terms: terms.clone(),
            time_zone_offset: offset,
            page: page + 1,
        });
        self.search_page = (page + 1, more, true);
        self.search_model()
    }

    /// What search's `in:` and `from:` can suggest: the shown team's
    /// public and private channels, and the people Matty has seen.
    pub fn search_hints(&self) -> crate::right_pane::SearchHints {
        let channels = self
            .team
            .as_ref()
            .and_then(|team| self.sections.get(team))
            .into_iter()
            .flat_map(|sections| sections.iter())
            .flat_map(|section| section.channels.iter())
            .filter(|channel| matches!(channel.kind.as_str(), "O" | "P"))
            .map(|channel| {
                (
                    channel.name.clone().into(),
                    channel.display_name.clone().into(),
                )
            })
            .collect();
        let mut people: Vec<_> = self
            .users
            .values()
            .filter(|user| user.delete_at == 0 && !user.username.is_empty())
            .map(|user| {
                (
                    user.username.clone().into(),
                    crate::transcript::display_name(user).into(),
                )
            })
            .collect();
        people.sort();
        crate::right_pane::SearchHints { channels, people }
    }

    /// Loads the user's saved posts; they come as [`Update::listing`].
    pub fn show_saved(&mut self) {
        self.show_listing(matty_sync::Listing::Saved);
    }

    /// Loads a channel's pinned posts; they come as [`Update::listing`].
    pub fn show_pinned(&mut self, channel_id: &str) {
        self.show_listing(matty_sync::Listing::Pinned {
            channel_id: channel_id.to_owned(),
        });
    }

    /// Loads the shown team's recent mentions of the user: an "or" search
    /// for their mention keys, as the webapp does. They come as
    /// [`Update::listing`].
    pub fn show_mentions(&mut self) {
        let Some(team_id) = self.team.clone() else {
            return;
        };
        self.show_listing(matty_sync::Listing::Mentions {
            team_id,
            terms: mention_terms(&self.prefs.keys),
        });
    }

    /// Loads the unread digest (#223): the first unread posts of each of
    /// `channel_ids`, in that order. They come as [`Update::listing`].
    pub fn show_unreads(&mut self, channel_ids: Vec<String>) {
        self.show_listing(matty_sync::Listing::Unreads {
            channel_ids,
            collapsed_threads: self.threads_on.unwrap_or(false),
        });
    }

    fn show_listing(&mut self, listing: matty_sync::Listing) {
        self.listing = Some((listing.clone(), None));
        self.send(Command::LoadListing(listing));
    }

    /// The right pane moved on from the listing.
    pub fn close_listing(&mut self) {
        self.listing = None;
    }

    /// Whether the user saved the post.
    pub fn is_saved(&self, post_id: &str) -> bool {
        self.saved.contains(post_id)
    }

    /// Saves or unsaves a post: shown at once, sent to the server.
    pub fn set_saved(&mut self, post_id: &str, saved: bool) {
        self.send(Command::SetSaved {
            post_id: post_id.to_owned(),
            saved,
        });
    }

    /// Asks the server to remind the user of a post; the system bot posts
    /// the reminder in a DM when it is due.
    pub fn remind(&self, post_id: &str, at: crate::channel::RemindAt) {
        self.send(Command::SetReminder {
            post_id: post_id.to_owned(),
            target_time: at.target(jiff::Timestamp::now(), &self.zone),
        });
    }

    /// The people and channels an interactive dialog's menus offer (#211).
    pub fn menu_sources(&self) -> crate::interactive_dialog::MenuSources {
        crate::interactive_dialog::MenuSources {
            people: crate::interactive::people(&self.users, self.names),
            channels: crate::interactive::channels(
                self.sections.values().flat_map(|sections| sections.iter()),
            ),
        }
    }

    /// Presses an interactive message's button or picks from its menu
    /// (#211): the control waits until the server answers.
    pub fn run_post_action(&mut self, choice: &crate::interactive::ActionChoice) -> Update {
        self.interactive.sent(choice);
        self.send(Command::RunPostAction {
            post_id: choice.post_id.to_string(),
            action_id: choice.action_id.to_string(),
            selected_option: choice.selected.as_ref().map(ToString::to_string),
            cookie: choice.cookie.to_string(),
        });
        Update {
            channel: self.channel_update(),
            thread: self.thread_model(),
            ..Update::default()
        }
    }

    /// Forgets an ephemeral message the user dismissed. The view it was
    /// dismissed in has already removed it; the other one, channel or
    /// thread, drops it with the update.
    pub fn dismiss_ephemeral(&mut self, post_id: &str) -> Update {
        if !self.interactive.dismiss(post_id) {
            return Update::default();
        }
        Update {
            channel: self.channel_update(),
            thread: self.thread_model(),
            ..Update::default()
        }
    }

    /// Pins or unpins a post; the edited post comes back over the socket.
    pub fn set_pinned(&mut self, post_id: &str, pinned: bool) {
        self.send(Command::SetPinned {
            post_id: post_id.to_owned(),
            pinned,
        });
    }

    /// Whether a channel's new posts pin or unpin one against the open
    /// Pinned listing, which then loads again.
    fn pins_changed(&self, channel_id: &str, posts: &[matty_api::models::Post]) -> bool {
        let Some((matty_sync::Listing::Pinned { channel_id: shown }, Some(listed))) = &self.listing
        else {
            return false;
        };
        shown == channel_id
            && posts.iter().any(|post| {
                let listed = listed.iter().any(|pinned| pinned.id == post.id);
                post.is_pinned != listed && post.delete_at == 0 || listed && post.delete_at != 0
            })
    }

    /// The listing as the pane shows it, once its posts have come.
    fn listing_content(&self) -> Option<crate::right_pane::RightPaneContent> {
        use crate::right_pane::RightPaneContent;
        let (listing, Some(posts)) = self.listing.as_ref()? else {
            return None;
        };
        let viewer = crate::transcript::Viewer {
            user_id: &self.me.0,
            keys: &self.prefs.keys,
            saved: &self.saved,
            zone: &self.zone,
            now: jiff::Timestamp::now(),
            link_previews: self.link_previews,
            collapsed_threads: self.threads_on.unwrap_or(false),
            names: self.names,
        };
        let listed: Vec<_> = posts
            .iter()
            .filter(|post| post.delete_at == 0)
            // An unsaved post leaves the Saved listing at once.
            .filter(|post| {
                !matches!(listing, matty_sync::Listing::Saved) || self.saved.contains(&post.id)
            })
            .map(|post| crate::right_pane::ListedPost {
                channel_id: post.channel_id.clone().into(),
                channel_name: self
                    .channel_name(&post.channel_id)
                    .unwrap_or_else(|| "Another channel".into()),
                post: crate::transcript::listed_post(post, &self.users, &viewer),
            })
            .collect();
        Some(match listing {
            matty_sync::Listing::Saved => RightPaneContent::Saved(listed),
            matty_sync::Listing::Mentions { .. } => RightPaneContent::Mentions(listed),
            matty_sync::Listing::Unreads { .. } => RightPaneContent::Unreads(listed),
            matty_sync::Listing::Pinned { channel_id } => RightPaneContent::Pinned {
                channel_name: self.channel_name(channel_id).unwrap_or_default(),
                posts: listed,
            },
        })
    }

    /// Shows the thread under `root_id` in the right pane; its posts come
    /// as [`Update::thread`] now and whenever they change.
    pub fn open_thread(&mut self, root_id: &str) {
        if self
            .thread
            .as_ref()
            .is_some_and(|(open, _)| open == root_id)
        {
            return;
        }
        self.close_thread();
        self.thread = Some((root_id.to_owned(), None));
        self.thread_pages = (false, false);
        self.send(Command::OpenThread {
            root_id: root_id.to_owned(),
        });
    }

    /// The right pane moved on from the thread.
    /// The channel of a post on screen: in the open thread, the channel or
    /// the listing.
    pub fn post_channel(&self, post_id: &str) -> Option<String> {
        let thread = self.thread.as_ref().and_then(|(_, posts)| posts.as_deref());
        let listing = self
            .listing
            .as_ref()
            .and_then(|(_, posts)| posts.as_deref());
        thread
            .into_iter()
            .chain(self.posts.as_deref())
            .chain(listing)
            .flat_map(|posts| posts.iter())
            .find(|post| post.id == post_id)
            .map(|post| post.channel_id.clone())
    }

    pub fn close_thread(&mut self) {
        self.thread_typing.clear();
        if let Some((root_id, _)) = self.thread.take() {
            self.send(Command::CloseThread { root_id });
        }
    }

    /// Stops following what the window showed, as it moves to another
    /// server: the engine runs on for this one's badges (#206).
    pub fn release(mut self) {
        if let Some(row) = self.open.take() {
            self.send(Command::CloseChannel {
                channel_id: row.id.to_string(),
            });
        }
        self.close_thread();
        if let Some(team_id) = self.team.take() {
            self.send(Command::CloseThreadList { team_id });
        }
    }

    /// Names people in `names` from now on (#156): the views on screen
    /// come back renamed.
    pub fn set_names(&mut self, names: crate::settings::NameFormat) -> Update {
        let mut update = Update::default();
        if names == self.names {
            return update;
        }
        self.names = names;
        if self.posts.is_some() {
            update.channel = self.channel_update();
            update.profiles = Some(self.profiles());
        }
        update.thread = self.thread_model();
        if self.thread_list.is_some() {
            update.threads = Some(self.threads_model());
        }
        update.search = self.search_model();
        update.categories = self.shown_categories();
        update
    }

    /// Whether a draft's link is worth asking the server about (#187).
    pub fn link_previews(&self) -> bool {
        self.link_previews
    }

    /// The open thread's channel, for a reply: the root's own.
    pub fn thread_channel(&self) -> Option<String> {
        let (_, posts) = self.thread.as_ref()?;
        Some(posts.as_ref()?.first()?.channel_id.clone())
    }

    /// The user read the thread under `root_id` in the Threads view.
    pub fn read_thread(&self, root_id: &str) {
        if let Some(team_id) = self.team.clone() {
            self.send(Command::ReadThread {
                team_id,
                root_id: root_id.to_owned(),
            });
        }
    }

    /// The thread pane reached its oldest loaded reply (#122).
    pub fn load_older_replies(&self, root_id: &str) {
        if self
            .thread
            .as_ref()
            .is_some_and(|(open, _)| open == root_id)
        {
            self.send(Command::LoadOlderReplies {
                root_id: root_id.to_owned(),
            });
        }
    }

    /// The Threads list reached its last row (#122).
    pub fn load_more_threads(&self) {
        if let Some(team_id) = self.team.clone() {
            self.send(Command::LoadMoreThreads { team_id });
        }
    }

    /// "Mark all as read" in the Threads view.
    pub fn read_all_threads(&self) {
        if let Some(team_id) = self.team.clone() {
            self.send(Command::ReadAllThreads { team_id });
        }
    }

    /// Follow or unfollow the thread under `root_id`.
    pub fn follow_thread(&self, root_id: &str, follow: bool) {
        if let Some(team_id) = self.team.clone() {
            self.send(Command::FollowThread {
                team_id,
                root_id: root_id.to_owned(),
                follow,
            });
        }
    }

    /// The shown team's followed threads as the Threads view lists them;
    /// a thread whose root is not cached yet waits for it.
    fn threads_model(&self) -> crate::threads::ThreadsModel {
        let Some((threads, roots, more, failed)) = &self.thread_list else {
            return crate::threads::ThreadsModel::default();
        };
        let viewer = crate::transcript::Viewer {
            user_id: &self.me.0,
            keys: &self.prefs.keys,
            saved: &self.saved,
            zone: &self.zone,
            now: jiff::Timestamp::now(),
            link_previews: self.link_previews,
            collapsed_threads: self.threads_on.unwrap_or(false),
            names: self.names,
        };
        let rows = threads
            .iter()
            .filter_map(|thread| {
                let root = roots.iter().find(|root| root.id == thread.id)?;
                let channel = self.channel_name(&root.channel_id).unwrap_or_default();
                Some(crate::transcript::thread_row(
                    thread,
                    root,
                    channel,
                    &self.users,
                    &viewer,
                ))
            })
            .collect();
        crate::threads::ThreadsModel {
            threads: rows,
            loading: false,
            more: *more,
            failed: *failed,
        }
    }

    /// The open thread as the pane shows it.
    fn thread_model(&mut self) -> Option<crate::right_pane::ThreadModel> {
        let (_, Some(posts)) = self.thread.as_ref()? else {
            return None;
        };
        let posts = crate::interactive::with_thread_ephemeral(posts, self.interactive.ephemeral());
        let (root, replies) = posts.split_first()?;
        let viewer = crate::transcript::Viewer {
            user_id: &self.me.0,
            keys: &self.prefs.keys,
            saved: &self.saved,
            zone: &self.zone,
            now: jiff::Timestamp::now(),
            link_previews: self.link_previews,
            collapsed_threads: self.threads_on.unwrap_or(false),
            names: self.names,
        };
        let shown = |post| crate::transcript::listed_post(post, &self.users, &viewer);
        let mut model = crate::right_pane::ThreadModel {
            channel_id: root.channel_id.clone().into(),
            channel_name: self.channel_name(&root.channel_id).unwrap_or_default(),
            root: shown(root),
            replies: replies
                .iter()
                .filter(|post| post.delete_at == 0)
                .map(shown)
                .collect(),
            following: false,
            reply_count: usize::try_from(root.reply_count).unwrap_or(0),
            older: self.thread_pages.0,
            failed_older: self.thread_pages.1,
        };
        for post in std::iter::once(&mut model.root).chain(model.replies.iter_mut()) {
            if self.failed.contains(post.id.as_ref()) {
                post.status = crate::channel::PostStatus::Failed;
            }
        }
        // The cards' images and controls, as in the channel (#211).
        for image in std::iter::once(&mut model.root)
            .chain(model.replies.iter_mut())
            .flat_map(|post| &mut post.embeds)
            .flat_map(crate::embeds::Embed::images_mut)
        {
            image.loaded = self.remote_images.image(&image.url);
        }
        for posts in [std::slice::from_mut(&mut model.root), &mut model.replies] {
            self.interactive.fill(
                posts,
                &self.users,
                self.names,
                self.sections.values().flat_map(|sections| sections.iter()),
            );
        }
        let all: Vec<_> = std::iter::once(model.root.clone())
            .chain(model.replies.iter().cloned())
            .collect();
        self.ask_thumbnails(&all);
        self.ask_remote_images(&all);
        Some(model)
    }

    /// The search model for what has come back so far.
    fn search_model(&self) -> Option<crate::right_pane::SearchModel> {
        let (terms, posts) = self.search.as_ref()?;
        let viewer = crate::transcript::Viewer {
            user_id: &self.me.0,
            keys: &self.prefs.keys,
            saved: &self.saved,
            zone: &self.zone,
            now: jiff::Timestamp::now(),
            link_previews: self.link_previews,
            collapsed_threads: self.threads_on.unwrap_or(false),
            names: self.names,
        };
        Some(crate::right_pane::SearchModel {
            query: terms.clone().into(),
            searching: posts.is_none(),
            more: self.search_page.1,
            loading_more: self.search_page.2,
            results: posts
                .iter()
                .flat_map(|posts| posts.iter())
                .map(|post| crate::right_pane::ListedPost {
                    channel_id: post.channel_id.clone().into(),
                    channel_name: self
                        .channel_name(&post.channel_id)
                        .unwrap_or_else(|| "Another channel".into()),
                    post: crate::transcript::listed_post(post, &self.users, &viewer),
                })
                .collect(),
        })
    }

    /// A channel's name in any team's sidebar.
    fn channel_name(&self, channel_id: &str) -> Option<SharedString> {
        self.sections
            .values()
            .flat_map(|sections| sections.iter())
            .flat_map(|section| section.channels.iter())
            .find(|channel| channel.id == channel_id)
            .map(|channel| self.sidebar_name(channel).into())
    }

    /// A sidebar channel's name: a direct message's in the user's name
    /// format (#156), any other channel's as the server names it.
    fn sidebar_name(&self, channel: &SidebarChannel) -> String {
        match &channel.dm_user {
            Some(person) => crate::transcript::display_name_as(person, self.names),
            None => channel.display_name.clone(),
        }
    }

    /// The team on screen.
    /// The signed-in user, once sync has loaded them.
    pub fn me_user(&self) -> Option<&User> {
        self.users.get(&self.me.0)
    }

    pub fn team(&self) -> Option<&str> {
        self.team.as_deref()
    }

    /// The server's address, for telling its links from others.
    pub fn server(&self) -> &str {
        &self.server
    }

    /// The user's teams as (id, URL name).
    pub fn team_names(&self) -> Vec<(String, String)> {
        self.teams
            .iter()
            .map(|team| (team.id.clone(), team.name.clone()))
            .collect()
    }

    /// The profile of someone whose posts have been on screen, by username.
    pub fn known_user(&self, username: &str) -> Option<UserProfile> {
        self.users
            .values()
            .find(|user| user.username.eq_ignore_ascii_case(username))
            .map(|user| self.profile(user))
    }

    /// The profile for a user sync found.
    pub fn user_profile(&self, user: &User) -> UserProfile {
        self.profile(user)
    }

    fn profile(&self, user: &User) -> UserProfile {
        UserProfile {
            user_id: user.id.clone().into(),
            username: user.username.clone().into(),
            display_name: crate::transcript::display_name_as(user, self.names).into(),
            position: (!user.position.is_empty()).then(|| user.position.clone().into()),
            presence: self
                .presence
                .get(&user.id)
                .copied()
                .unwrap_or(Presence::Offline),
            status: crate::custom_status::StatusLine::of(user),
            time_zone: crate::profile::time_zone(&user.timezone),
        }
    }

    /// The ready channel's id once its row is in a team's sidebar.
    fn take_ready(&mut self) -> Option<SharedString> {
        let id = self.ready.as_deref()?;
        self.team_of(id)?;
        self.ready.take().map(Into::into)
    }

    /// The profile pictures, for the window to share with its views.
    pub fn avatars(&self) -> crate::avatars::Avatars {
        self.avatars.clone()
    }

    /// The outside images, for the views to read through the app global.
    pub fn remote_images(&self) -> crate::remote_images::RemoteImages {
        self.remote_images.clone()
    }

    /// Follows "Load remote images" (#159). Switching to Always fetches
    /// the cards on screen; inline images fetch as they redraw.
    pub fn set_ask_remote_images(&mut self, ask_first: bool) -> Update {
        let mut update = Update::default();
        if self.remote_images.set_ask_first(ask_first) && self.posts.is_some() {
            update.channel = self.channel_update();
        }
        update
    }

    /// Makes the pictures and the custom emoji list the app's globals, for
    /// the views to read while rendering.
    pub fn share_pictures(&self, cx: &mut gpui_kit::App) {
        cx.set_global(self.avatars.clone());
        cx.set_global(self.emoji_images.clone());
        cx.set_global(self.team_icons.clone());
        cx.set_global(self.custom_emoji.clone());
    }

    pub fn is_typing(&self) -> bool {
        !self.typing.is_empty() || !self.thread_typing.is_empty()
    }

    fn is_open(&self, channel_id: &str) -> bool {
        self.open
            .as_ref()
            .is_some_and(|row| row.id.as_ref() == channel_id)
    }

    /// The sidebar row for `channel_id` in the shown team.
    pub fn row(&self, channel_id: &str) -> Option<ChannelRow> {
        self.shown_categories()?
            .into_iter()
            .flat_map(|category| category.rows)
            .find(|row| row.id.as_ref() == channel_id)
    }

    /// Folds in one change set.
    pub fn apply(&mut self, change: &ChangeSet) -> Update {
        let mut update = Update::default();
        if self.apply_extra(change, &mut update) {
            return update;
        }
        match change {
            ChangeSet::Sidebar { team_id, sections } => {
                // Unreads, which comes in the same batch, picks the team.
                self.sections.insert(team_id.clone(), sections.clone());
                if self.team.as_ref() == Some(team_id) {
                    update.categories = Some(self.categories(sections));
                }
                update.ready = self.take_ready();
            }
            ChangeSet::Unreads { teams } => {
                self.teams = teams.to_vec();
                let shown = self
                    .team
                    .as_ref()
                    .filter(|id| self.teams.iter().any(|team| &team.id == *id))
                    .cloned();
                let team = shown.or_else(|| self.teams.first().map(|team| team.id.clone()));
                if team != self.team {
                    self.show_team(team, &mut update);
                }
                update.team_name = self.team_name();
                update.teams = Some(self.rail_teams());
            }
            ChangeSet::Presence(statuses) => {
                for status in statuses.iter() {
                    self.presence
                        .insert(status.user_id.clone(), presence(&status.status));
                    if status.user_id == self.me.0 {
                        let end = (status.dnd_end_time > 0).then_some(status.dnd_end_time);
                        update.own_presence = Some((presence(&status.status), end));
                    }
                }
                update.categories = self.shown_categories();
                if self.shown.is_some() {
                    update.profiles = Some(self.profiles());
                }
            }
            ChangeSet::ChannelReady { channel_id } => {
                self.ready = Some(channel_id.clone());
                update.ready = self.take_ready();
            }
            ChangeSet::Directory {
                team_id,
                channels,
                members,
            } if self.team.as_ref() == Some(team_id) => {
                let mut rows: Vec<_> = channels
                    .iter()
                    .filter(|channel| channel.delete_at == 0)
                    .map(|channel| crate::directory::DirectoryChannel {
                        id: channel.id.clone().into(),
                        name: channel.display_name.clone().into(),
                        purpose: (!channel.purpose.trim().is_empty())
                            .then(|| channel.purpose.clone().into()),
                        members: members
                            .get(&channel.id)
                            .and_then(|count| usize::try_from(*count).ok())
                            .unwrap_or(0),
                        joined: self.row(&channel.id).is_some(),
                    })
                    .collect();
                rows.sort_by_key(|row| row.name.to_lowercase());
                update.directory = Some(rows);
            }
            ChangeSet::People { team_id, users } if self.team.as_ref() == Some(team_id) => {
                for user in users.iter() {
                    self.avatars.learn(&user.id, user.last_picture_update);
                }
                let mut people: Vec<_> = users
                    .iter()
                    .filter(|user| user.id != self.me.0 && user.delete_at == 0 && !user.is_bot)
                    .map(|user| self.profile(user))
                    .collect();
                people.sort_by_key(|person| person.display_name.to_lowercase());
                update.people = Some(people);
            }
            ChangeSet::Failed(matty_sync::CommandError::Server {
                action: "join channel",
                target,
                ..
            }) => update.join_failed = Some(target.clone().into()),
            ChangeSet::Download { file_id, state } => {
                update.download = Some((file_id.as_str().into(), state.clone()));
            }
            ChangeSet::Image {
                file_id,
                size,
                bytes,
                content_type,
            } => {
                let Some(format) = image_format(bytes, content_type) else {
                    return update;
                };
                let image = Arc::new(gpui_kit::Image::from_bytes(format, bytes.to_vec()));
                if *size == ImageSize::Preview {
                    self.full_images.insert(file_id.clone(), image.clone());
                    update.full_image = Some((file_id.as_str().into(), image));
                    return update;
                }
                self.thumbnails.insert(file_id.clone(), image);
                let shown = self.shown.as_ref().is_some_and(|posts| {
                    posts
                        .iter()
                        .flat_map(|post| &post.files)
                        .any(|file| file.id.as_ref() == file_id)
                });
                if shown {
                    update.channel = self.channel_update();
                }
            }
            ChangeSet::Picture {
                picture,
                id,
                version,
                image,
            } => {
                let image = image
                    .as_ref()
                    .and_then(|(bytes, content_type)| crate::avatars::picture(bytes, content_type));
                let store: &crate::avatars::Pictures = match picture {
                    matty_sync::Picture::Avatar => &self.avatars,
                    matty_sync::Picture::Emoji => &self.emoji_images,
                    matty_sync::Picture::TeamIcon => &self.team_icons,
                };
                update.avatars = store.landed(id, *version, image);
            }
            ChangeSet::CustomEmoji(emoji) => {
                self.custom_emoji.set(emoji);
                for one in emoji.iter() {
                    self.emoji_images.learn(&one.id, one.update_at);
                }
                update.avatars = true;
            }
            ChangeSet::RemoteImage {
                url,
                bytes,
                content_type,
            } => {
                let Some(format) = image_format(bytes, content_type) else {
                    return update;
                };
                let image = Arc::new(gpui_kit::Image::from_bytes(format, bytes.to_vec()));
                self.remote_images.landed(url, image);
                let shown = self.shown.as_ref().is_some_and(|posts| {
                    posts
                        .iter()
                        .flat_map(|post| &post.embeds)
                        .flat_map(crate::embeds::Embed::images)
                        .any(|image| image.url.as_ref() == url)
                });
                if shown {
                    update.channel = self.channel_update();
                }
                update.thread = self.thread_model().filter(|thread| {
                    std::iter::once(&thread.root)
                        .chain(&thread.replies)
                        .flat_map(|post| &post.embeds)
                        .flat_map(crate::embeds::Embed::images)
                        .any(|image| image.url.as_ref() == url)
                });
                // Inline Markdown images read the store as they render.
                update.avatars = true;
            }
            ChangeSet::Upload { upload_id, state } => {
                if let Some(job) = self.uploads.get_mut(upload_id) {
                    let status = match state {
                        UploadState::Sending(fraction) => AttachmentStatus::Uploading(*fraction),
                        UploadState::Done { file_id } => {
                            job.file_id = Some(file_id.clone());
                            AttachmentStatus::Ready
                        }
                        UploadState::Failed(error) => {
                            tracing::warn!(%error, "upload failed");
                            AttachmentStatus::Failed
                        }
                    };
                    update.upload = Some((upload_id.as_str().into(), status));
                }
            }
            ChangeSet::Suggested { term, users } => {
                for user in users.iter() {
                    self.avatars.learn(&user.id, user.last_picture_update);
                }
                let people = users
                    .iter()
                    .filter(|user| user.delete_at == 0)
                    .map(|user| self.profile(user))
                    .collect();
                update.suggested = Some((term.clone(), people));
            }
            ChangeSet::Draft {
                channel_id,
                root_id,
                draft,
            } => {
                let key =
                    crate::composer_draft::DraftKey::thread(channel_id.clone(), root_id.clone());
                update.draft = Some((key, draft.clone()));
            }
            ChangeSet::LinkResolved { query, answer } => {
                update.link = Some((query.clone(), answer.clone()));
            }
            ChangeSet::Search {
                terms,
                page,
                posts,
                more,
            } => {
                if let Some((asked, results)) = self.search.as_mut()
                    && asked == terms
                    && *page == self.search_page.0
                {
                    *results = Some(match (results.take(), page) {
                        // A later page adds what earlier ones lacked.
                        (Some(shown), 1..) => shown
                            .iter()
                            .chain(
                                posts
                                    .iter()
                                    .filter(|post| shown.iter().all(|seen| seen.id != post.id)),
                            )
                            .cloned()
                            .collect(),
                        _ => posts.clone(),
                    });
                    self.search_page = (*page, *more, false);
                    update.search = self.search_model();
                }
            }
            ChangeSet::Listing { listing, posts } => {
                if let Some((asked, shown)) = self.listing.as_mut()
                    && asked == listing
                {
                    *shown = Some(posts.clone());
                    update.listing = self.listing_content();
                }
                // The open channel's pins, fresh from the server, also
                // correct the header's count.
                if let matty_sync::Listing::Pinned { channel_id } = listing
                    && self.is_open(channel_id)
                    && let Some(stats) = self.stats.as_mut()
                {
                    let pinned = i64::try_from(posts.len()).unwrap_or(i64::MAX);
                    if stats.pinnedpost_count != pinned {
                        Arc::make_mut(stats).pinnedpost_count = pinned;
                        update.details = self.shown_details();
                    }
                }
            }
            ChangeSet::Saved(ids) => {
                let saved: std::collections::HashSet<String> = ids.iter().cloned().collect();
                if saved != self.saved {
                    self.saved = saved;
                    if self.posts.is_some() {
                        update.channel = self.channel_update();
                    }
                    if self
                        .thread
                        .as_ref()
                        .is_some_and(|(_, posts)| posts.is_some())
                    {
                        update.thread = self.thread_model();
                    }
                    update.listing = self.listing_content();
                }
            }
            ChangeSet::Failed(
                matty_sync::CommandError::Server {
                    action: "load saved messages" | "load pinned messages" | "load recent mentions",
                    ..
                }
                | matty_sync::CommandError::Offline {
                    action: "load saved messages" | "load pinned messages" | "load recent mentions",
                },
            ) => {
                // Shown as an empty listing rather than loading forever.
                if let Some((_, shown)) = self.listing.as_mut() {
                    *shown = Some(Arc::from(Vec::new()));
                    update.listing = self.listing_content();
                }
            }
            ChangeSet::Failed(
                matty_sync::CommandError::Server {
                    action: "search", ..
                }
                | matty_sync::CommandError::Offline { action: "search" },
            ) => {
                // Shown as no results rather than searching forever; a
                // later page that failed keeps what came before.
                if let Some((_, results)) = self.search.as_mut() {
                    if results.is_none() {
                        *results = Some(Arc::from(Vec::new()));
                    }
                    self.search_page = (self.search_page.0, false, false);
                    update.search = self.search_model();
                }
            }
            ChangeSet::Ephemeral(_)
            | ChangeSet::PostActionDone { .. }
            | ChangeSet::Failed(
                matty_sync::CommandError::Server {
                    action: "run action",
                    ..
                }
                | matty_sync::CommandError::Offline {
                    action: "run action",
                },
            ) => {
                let open = self.open.as_ref().map(|row| row.id.to_string());
                let applied = self.interactive.apply(change, open.as_deref());
                if applied.redraw {
                    update.channel = self.channel_update();
                }
                // The open thread's replies and buttons may have changed.
                update.thread = self.thread_model();
                update.action_refused = applied.refused;
                if applied.offline {
                    update.offline_refused = Some("run action");
                }
            }
            ChangeSet::Failed(matty_sync::CommandError::Offline { action }) => {
                update.offline_refused = Some(action);
            }
            ChangeSet::Channel { channel, member } if self.is_open(&channel.id) => {
                self.channel = Some(channel.clone());
                if let Some(member) = member {
                    self.member = Some(member.clone());
                    if self.shown.is_some() {
                        update.notifications = Some(notifications(member));
                    }
                }
                // After the member: whether the user may manage it follows.
                update.details = self.shown_details();
            }
            ChangeSet::History {
                channel_id,
                at_start,
                at_end,
                failed,
            } if self.is_open(channel_id) => {
                update.history = Some((*at_start, *at_end, *failed));
            }
            ChangeSet::Stats(stats) if self.is_open(&stats.channel_id) => {
                self.stats = Some(stats.clone());
                update.details = self.shown_details();
            }
            ChangeSet::Typing {
                user_id,
                channel_id,
                parent_id,
            } if self.is_open(channel_id) && parent_id.is_empty() && *user_id != self.me.0 => {
                self.typing.retain(|(id, _)| id != user_id);
                self.typing.push((user_id.clone(), Instant::now()));
                update.typing = Some(self.typing_names());
            }
            ChangeSet::Typing {
                user_id, parent_id, ..
            } if !parent_id.is_empty()
                && *user_id != self.me.0
                && self
                    .thread
                    .as_ref()
                    .is_some_and(|(root_id, _)| root_id == parent_id) =>
            {
                self.thread_typing.retain(|(id, _)| id != user_id);
                self.thread_typing.push((user_id.clone(), Instant::now()));
                update.thread_typing = Some(self.thread_typing_names());
            }
            ChangeSet::Outbox { channel_id, sends } => {
                let waiting = sends
                    .iter()
                    .filter(|send| send.last_error.is_none())
                    .count();
                let before: usize = self.queued.values().sum();
                if waiting == 0 {
                    self.queued.remove(channel_id);
                } else {
                    self.queued.insert(channel_id.clone(), waiting);
                }
                let now: usize = self.queued.values().sum();
                if now != before {
                    update.queued = Some(now);
                }
                if !self.is_open(channel_id) {
                    return update;
                }
                let failed = sends
                    .iter()
                    .filter(|send| send.last_error.is_some())
                    .map(|send| send.pending_post_id.clone())
                    .collect();
                if failed != self.failed {
                    self.failed = failed;
                    update.channel = self.channel_update();
                }
            }
            ChangeSet::Posts { channel_id, posts } => {
                if self.pins_changed(channel_id, posts) {
                    self.send(Command::LoadListing(matty_sync::Listing::Pinned {
                        channel_id: channel_id.clone(),
                    }));
                }
                if self
                    .open
                    .as_ref()
                    .is_some_and(|row| row.id.as_ref() == channel_id)
                {
                    // Someone who just posted has stopped typing.
                    let newest = posts.last().map(|post| post.user_id.as_str());
                    let typing = self.typing.len();
                    self.typing.retain(|(id, _)| Some(id.as_str()) != newest);
                    if self.typing.len() != typing {
                        update.typing = Some(self.typing_names());
                    }
                    self.posts = Some(posts.clone());
                    update.channel = self.channel_update();
                    update.profiles = self.shown.is_some().then(|| self.profiles());
                }
            }
            ChangeSet::Arrived(arrived) => {
                // A new mention refreshes an open Recent mentions pane.
                if let Some((listing @ matty_sync::Listing::Mentions { .. }, Some(_))) =
                    &self.listing
                    && arrived.post.user_id != self.me.0
                    && (arrived.mentioned || self.prefs.keys.mentioned(&arrived.post.message))
                {
                    self.send(Command::LoadListing(listing.clone()));
                }
                update.arrived = Some(Arrival::new(arrived, &self.me.0, &self.prefs));
            }
            ChangeSet::Me(user) => {
                self.avatars.learn(&user.id, user.last_picture_update);
                self.take_me(user, &mut update);
                if self.posts.is_some() {
                    update.channel = self.channel_update();
                }
            }
            ChangeSet::Thread {
                root_id,
                posts,
                older,
                failed,
            } => {
                if let Some((open, shown)) = &mut self.thread
                    && open == root_id
                {
                    *shown = Some(posts.clone());
                    self.thread_pages = (*older, *failed);
                    update.thread = self.thread_model();
                }
            }
            ChangeSet::ReplyRoots { channel_id, roots } => {
                if self
                    .open
                    .as_ref()
                    .is_some_and(|row| row.id.as_ref() == channel_id)
                {
                    self.reply_roots = roots.clone();
                }
            }
            ChangeSet::ThreadList {
                team_id,
                threads,
                roots,
                more,
                failed,
            } if self.team.as_ref() == Some(team_id) => {
                self.thread_list = Some((threads.clone(), roots.clone(), *more, *failed));
                update.threads = Some(self.threads_model());
            }
            ChangeSet::Users(users) => {
                for user in users.iter() {
                    self.avatars.learn(&user.id, user.last_picture_update);
                    self.users.insert(user.id.clone(), user.clone());
                    self.take_me(user, &mut update);
                }
                if self.thread_list.is_some() {
                    update.threads = Some(self.threads_model());
                }
                if self
                    .thread
                    .as_ref()
                    .is_some_and(|(_, posts)| posts.is_some())
                {
                    update.thread = self.thread_model();
                }
                if self
                    .search
                    .as_ref()
                    .is_some_and(|(_, posts)| posts.is_some())
                {
                    update.search = self.search_model();
                }
                if self
                    .listing
                    .as_ref()
                    .is_some_and(|(_, posts)| posts.is_some())
                {
                    update.listing = self.listing_content();
                }
                if self.posts.is_some() {
                    update.channel = self.channel_update();
                    update.profiles = Some(self.profiles());
                    if !self.typing.is_empty() {
                        update.typing = Some(self.typing_names());
                    }
                }
            }
            ChangeSet::LinkPreview { url, graph } => {
                update.link_preview = Some((
                    url.clone(),
                    graph
                        .as_deref()
                        .map(|graph| crate::embeds::link_card(url, graph)),
                ));
            }
            ChangeSet::Scheduled(posts) => update.scheduled = Some(posts.clone()),
            ChangeSet::Config(capabilities) => {
                self.max_post_size = Some(capabilities.max_post_size);
                update.max_file_size = Some(capabilities.max_file_size);
                update.features = Some(crate::priority::Features::of(capabilities));
                self.threads_choice = Some(matches!(
                    capabilities.collapsed_threads,
                    matty_api::capabilities::CollapsedThreads::DefaultOn
                        | matty_api::capabilities::CollapsedThreads::DefaultOff
                ));
                update.collapsed_threads = self.threads();
                // `highlight_keys` came in 9.3 (below Matty's floor) for
                // licensed servers; an unlicensed one drops the prop.
                update.highlight_keys = Some(capabilities.licensed);
            }
            ChangeSet::CollapsedThreads(on) => {
                let changed = self.threads_on != Some(*on);
                self.threads_on = Some(*on);
                update.collapsed_threads = self.threads();
                // The footer gives way to inline replies, or back (#123).
                if changed && self.posts.is_some() {
                    update.channel = self.channel_update();
                }
            }
            ChangeSet::AccountPreferences(preferences) => {
                update.account_preferences = Some(preferences.clone());
            }
            ChangeSet::LinkPreviews(on) if *on != self.link_previews => {
                self.link_previews = *on;
                if self.posts.is_some() {
                    update.channel = self.channel_update();
                }
                update.thread = self.thread_model();
            }
            ChangeSet::Connection(connection) => match connection {
                Connection::Socket(matty_api::ws::ConnectionState::Unauthorized)
                | Connection::Expired => {
                    update.expired = true;
                    update.connection = Some(ConnectionState::Offline);
                }
                Connection::Socket(state) => {
                    // Pictures that could not load while offline get
                    // another try.
                    if matches!(state, matty_api::ws::ConnectionState::Connected { .. }) {
                        self.avatars.retry_failed();
                        self.emoji_images.retry_failed();
                        self.team_icons.retry_failed();
                    }
                    update.connection = Some(connection_state(state));
                }
                Connection::Failed { .. } => update.connection = Some(ConnectionState::Offline),
                Connection::SignedOut => update.signed_out = true,
                _ => {}
            },
            _ => {}
        }
        if let Some(state) = &update.connection {
            let offline = *state != ConnectionState::Online;
            if offline != self.offline {
                self.offline = offline;
                if !offline {
                    // Back online: files queued in a tray upload now.
                    update.resume_uploads = self
                        .uploads
                        .iter_mut()
                        .filter_map(|(id, job)| job.queued.take().map(|_| id.as_str().into()))
                        .collect();
                }
                // Only an empty channel reads it, and a patch would not
                // carry it.
                if self.posts.as_ref().is_some_and(|posts| posts.is_empty()) {
                    self.shown = None;
                    update.channel = self.channel_update();
                }
            }
        }
        update
    }

    /// The details for a channel view that has its model; before that the
    /// first snapshot carries them.
    fn shown_details(&self) -> Option<Details> {
        let shown = self.shown.as_ref()?;
        Some(self.details(shown.iter().filter(|post| post.pinned).count()))
    }

    fn shown_categories(&self) -> Option<Vec<Category>> {
        let sections = self.sections.get(self.team.as_ref()?)?;
        Some(self.categories(sections))
    }

    /// The link to a post, `{server}/{team}/pl/{post_id}` (#133). The team
    /// is the one whose sidebar holds the channel; direct and group
    /// messages, which belong to no team, use the team on screen, as the
    /// webapp does.
    pub fn permalink(&self, channel_id: &str, post_id: &str) -> Option<String> {
        let team_id = self
            .sections
            .iter()
            .find(|(_, sections)| {
                sections.iter().any(|section| {
                    section.channels.iter().any(|channel| {
                        channel.id == channel_id && !matches!(channel.kind.as_str(), "D" | "G")
                    })
                })
            })
            .map(|(team_id, _)| team_id)
            .or(self.team.as_ref())?;
        let team = self.teams.iter().find(|team| &team.id == team_id)?;
        Some(format!(
            "{}/{}/pl/{post_id}",
            self.server.trim_end_matches('/'),
            team.name
        ))
    }

    fn team_name(&self) -> Option<SharedString> {
        let team = self.team.as_ref()?;
        let team = self.teams.iter().find(|candidate| &candidate.id == team)?;
        Some(team.display_name.clone().into())
    }

    fn categories(&self, sections: &[SidebarSection]) -> Vec<Category> {
        sections
            .iter()
            .map(|section| {
                let rows = section
                    .channels
                    .iter()
                    .map(|channel| {
                        let kind = match channel.kind.as_str() {
                            "P" => ChannelKind::Private,
                            "D" => ChannelKind::Direct {
                                presence: channel
                                    .dm_user_id
                                    .as_ref()
                                    .and_then(|id| self.presence.get(id).copied())
                                    .unwrap_or(Presence::Offline),
                            },
                            // Mattermost names a group by its people, comma-separated.
                            "G" => ChannelKind::Group {
                                members: channel.display_name.split(", ").count(),
                            },
                            _ => ChannelKind::Public,
                        };
                        let mut row =
                            ChannelRow::new(channel.id.clone(), self.sidebar_name(channel), kind);
                        if channel.kind == "D"
                            && let Some(user_id) = &channel.dm_user_id
                        {
                            row = row.person(user_id.clone());
                            // The freshest copy of the person: a status
                            // change arrives as a user update.
                            let person = self.users.get(user_id).or(channel.dm_user.as_ref());
                            if let Some(status) =
                                person.and_then(crate::custom_status::StatusLine::of)
                            {
                                row = row.status_emoji(status.emoji);
                            }
                        }
                        if let Some(person) = &channel.dm_user {
                            row = row.handle(person.username.clone());
                        } else if matches!(row.kind, ChannelKind::Public | ChannelKind::Private) {
                            // `~` mentions and links use the URL name.
                            row = row.handle(channel.name.clone());
                        }
                        if channel.unreads.messages > 0 {
                            row = row.unread();
                        }
                        let mentions = usize::try_from(channel.unreads.mentions).unwrap_or(0);
                        row = row.mentions(mentions);
                        if channel.muted {
                            row = row.muted();
                        }
                        row
                    })
                    .collect();
                let mut category =
                    Category::new(section.id.clone(), section.display_name.clone(), rows);
                category.collapsed = section.collapsed;
                category.kind = crate::sidebar::CategoryKind::from_server(&section.kind);
                category
            })
            .collect()
    }
}

pub fn presence(status: &str) -> Presence {
    match status {
        "online" => Presence::Online,
        "away" => Presence::Away,
        "dnd" => Presence::Dnd,
        _ => Presence::Offline,
    }
}

/// The status string sync sends for `presence`.
pub fn status(presence: Presence) -> &'static str {
    match presence {
        Presence::Online => "online",
        Presence::Away => "away",
        Presence::Dnd => "dnd",
        Presence::Offline => "offline",
    }
}

/// Reconnect attempts that read as a blip; after them the banner says the
/// app is offline and offers Retry now (#162). The backoff waits about
/// 1, 2 and 4 seconds before these, so a dropped Wi-Fi shows as offline
/// within ten seconds.
const OFFLINE_AFTER: u32 = 3;

fn connection_state(state: &matty_api::ws::ConnectionState) -> ConnectionState {
    use matty_api::ws::ConnectionState as Ws;
    match state {
        Ws::Connecting | Ws::Connected { .. } => ConnectionState::Online,
        Ws::Reconnecting { attempt, .. } if *attempt > OFFLINE_AFTER => ConnectionState::Offline,
        Ws::Reconnecting { attempt, .. } => ConnectionState::Reconnecting { attempt: *attempt },
        Ws::Unauthorized | Ws::Stopped => ConnectionState::Offline,
    }
}

/// Checks `url` and reads how its server lets people sign in: `system/ping`
/// first, then the client config. A server older than Matty supports is
/// refused here, before anyone signs in.
pub async fn check_server(url: String) -> Result<ServerCapabilities, String> {
    let address = url.clone();
    let answer = matty_sync::io::run(async move {
        let client = Client::new(&url).map_err(ProbeError::Ping)?;
        client.probe().await
    });
    match answer.recv().await {
        Ok(Ok(capabilities)) => supported(capabilities),
        Ok(Err(error)) => Err(probe_error(&address, &error)),
        Err(_) => Err(STOPPED.into()),
    }
}

/// `capabilities` if the server is at or above Matty's version floor
/// (PLAN §6.3), otherwise the sentence naming both versions.
fn supported(capabilities: ServerCapabilities) -> Result<ServerCapabilities, String> {
    match capabilities.minimum_supported() {
        Ok(()) => Ok(capabilities),
        Err(old) => Err(old.to_string()),
    }
}

/// A signed-in session, ready to hand to sync.
pub struct Session {
    pub client: Client,
    pub user: User,
}

/// Signs in to `url` with a login id and password, an MFA code once the
/// server asked for one, and `ldap_only` for AD/LDAP. `device_id` stays
/// empty: the server only takes a push-notification device there
/// (PLAN §6.3).
pub async fn sign_in(url: String, sign_in: PasswordSignIn) -> Result<Session, SignInFailure> {
    let answer = matty_sync::io::run(async move {
        let credentials = matty_api::Credentials {
            login_id: sign_in.login_id,
            password: sign_in.password,
            mfa_token: sign_in.mfa_token,
            ldap_only: sign_in.ldap_only,
            ..Default::default()
        };
        Client::new(&url)?.login(&credentials).await
    });
    match answer.recv().await {
        Ok(Ok(signed_in)) => Ok(Session {
            client: signed_in.client,
            user: signed_in.user,
        }),
        Ok(Err(error)) => Err(sign_in_error(&error)),
        Err(_) => Err(SignInFailure::Message(STOPPED.into())),
    }
}

/// Signs in with a personal access token.
pub async fn sign_in_with_token(url: String, token: String) -> Result<Session, SignInFailure> {
    let answer =
        matty_sync::io::run(
            async move { Client::new(&url)?.login_personal_access_token(&token).await },
        );
    match answer.recv().await {
        Ok(Ok(signed_in)) => Ok(Session {
            client: signed_in.client,
            user: signed_in.user,
        }),
        Ok(Err(error)) => Err(sign_in_error(&error)),
        Err(_) => Err(SignInFailure::Message(STOPPED.into())),
    }
}

/// Finishes browser SSO with the `server_token` from the `mattermost://`
/// hand-off (PLAN §6.6, #75).
pub async fn sign_in_with_desktop_token(
    url: String,
    server_token: String,
) -> Result<Session, SignInFailure> {
    let answer = matty_sync::io::run(async move {
        Client::new(&url)?
            .login_desktop_token(&server_token, "")
            .await
    });
    match answer.recv().await {
        Ok(Ok(signed_in)) => Ok(Session {
            client: signed_in.client,
            user: signed_in.user,
        }),
        Ok(Err(error)) => Err(sign_in_error(&error)),
        Err(_) => Err(SignInFailure::Message(STOPPED.into())),
    }
}

/// A team's followed threads and their cached root posts, as sync sent
/// them, then whether more are on the server and whether the last page of
/// them failed (#122).
type ThreadList = (
    Arc<[matty_sync::ThreadSummary]>,
    Arc<[matty_api::models::Post]>,
    bool,
    bool,
);

const STOPPED: &str = "Matty could not start its network thread. Quit and open it again.";

/// What the sign-in card offers for a server.
pub fn methods(capabilities: &ServerCapabilities) -> SignInMethods {
    let sso = &capabilities.sso;
    let providers = [
        (sso.gitlab, SsoProvider::GitLab),
        (sso.google, SsoProvider::Google),
        (sso.office365, SsoProvider::Office365),
        (sso.openid, SsoProvider::OpenId),
        (sso.saml, SsoProvider::Saml),
    ];
    SignInMethods {
        password: capabilities.email_login || capabilities.username_login,
        ldap: capabilities.ldap,
        sso: providers
            .into_iter()
            .filter_map(|(on, provider)| on.then_some(provider))
            .collect(),
    }
}

/// The account to remember after `user` signs in to `server`.
/// The search terms for the user's recent mentions: `@username`, the first
/// name and keywords, leaving out `@channel`, `@all` and `@here` as the
/// webapp does.
fn mention_terms(keys: &matty_notify::MentionKeys) -> String {
    std::iter::once(format!("@{}", keys.username))
        .chain(keys.first_name.clone())
        .chain(keys.keywords.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn saved_account(server: &str, user: &User) -> SavedAccount {
    let display_name = crate::transcript::display_name(user);
    SavedAccount {
        server: server.to_string(),
        user_id: user.id.clone(),
        username: user.username.clone(),
        display_name,
        ..SavedAccount::default()
    }
}

/// Where `account`'s database lives: one file per server and user.
/// Where `account`'s media cache lives, if the system has a cache folder.
pub fn media_dir(account: &SavedAccount) -> Option<PathBuf> {
    matty_sync::media_dir_for_key(&account.data_key())
}

pub fn database_path(account: &SavedAccount) -> PathBuf {
    matty_sync::database_path_for_key(&account.data_key()).unwrap_or_else(|| {
        // The browser has no temporary folder; this path only names
        // the database, which is kept in memory there.
        #[cfg(target_family = "wasm")]
        let dir = PathBuf::new();
        #[cfg(not(target_family = "wasm"))]
        let dir = std::env::temp_dir();
        dir.join(format!("matty-{}.sqlite", account.user_id))
    })
}

/// The sentence for a server check that stopped at `error`, naming
/// `url`'s host where that helps.
fn probe_error(url: &str, error: &ProbeError) -> String {
    match error {
        ProbeError::Ping(ApiError::Server { .. } | ApiError::Decode(_)) => format!(
            "{} answered, but it is not a Mattermost server. Check the address.",
            url_host(url)
        ),
        ProbeError::Ping(error) => server_error(url, error),
        ProbeError::Unhealthy(_) => format!(
            "The Mattermost server at {} says it is not working right now. Try again later or ask its administrator.",
            url_host(url)
        ),
        ProbeError::Config(ApiError::Server { .. } | ApiError::Decode(_)) => format!(
            "The Mattermost server at {} did not share its sign-in settings. Try again or ask its administrator.",
            url_host(url)
        ),
        ProbeError::Config(error) => server_error(url, error),
    }
}

/// `chat.example.com` from `https://chat.example.com/sub`.
pub(crate) fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

/// The server address a redirect points at: its `Location` without the
/// API path Matty asked for.
fn redirect_target(location: &str) -> &str {
    let base = location
        .find("/api/v4")
        .map_or(location, |at| &location[..at]);
    base.trim_end_matches('/')
}

/// The sentence for a request that never got a Mattermost answer.
fn server_error(url: &str, error: &ApiError) -> String {
    match error {
        // Already one sentence for a person, naming what is wrong with the
        // certificate (`matty_api::error::describe_tls`).
        ApiError::Tls(text) => text.clone(),
        ApiError::Timeout => format!(
            "{} did not answer in time. Check the address and try again.",
            url_host(url)
        ),
        ApiError::Redirected { location } if location.contains("://") => format!(
            "That address sends Matty to {}. Enter that address instead.",
            redirect_target(location)
        ),
        ApiError::InvalidUrl(_) => "That does not look like a server address.".into(),
        _ => format!(
            "Can't reach {}. Check the address and your connection.",
            url_host(url)
        ),
    }
}

fn sign_in_error(error: &ApiError) -> SignInFailure {
    let message: String = match (error, error.id()) {
        (ApiError::MfaRequired, _) => return SignInFailure::MfaRequired,
        (
            _,
            Some(
                "api.user.check_user_login_attempts.too_many.app_error"
                | "api.user.check_user_login_attempts.too_many_ldap.app_error",
            ),
        ) => "This account is locked after too many failed attempts. Reset your password or ask your server's administrator to unlock it.".into(),
        (_, Some("api.user.login.inactive.app_error")) => {
            "This account is deactivated. Ask your server's administrator to reactivate it.".into()
        }
        (_, Some("api.user.login.not_verified.app_error")) => {
            "Verify your email address first, with the link the server sent you.".into()
        }
        (ApiError::Server { status: 401, .. }, _) => {
            "That login or password is not right. Check them and try again.".into()
        }
        (ApiError::Server { message, .. }, _) if !message.is_empty() => message.clone(),
        (ApiError::RateLimited, _) => {
            "Too many tries in a row. Wait a moment and try again.".into()
        }
        (ApiError::Tls(text), _) => text.clone(),
        _ => "Matty could not reach the server. Check your connection and try again.".into(),
    };
    SignInFailure::Message(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use matty_api::models::Status;
    use matty_sync::{SidebarChannel, Unreads};

    pub(super) fn channel(id: &str, kind: &str, messages: i64, mentions: i64) -> SidebarChannel {
        SidebarChannel {
            id: id.into(),
            kind: kind.into(),
            name: id.into(),
            display_name: id.into(),
            dm_user_id: (kind == "D").then(|| format!("{id}-user")),
            dm_user: None,
            unreads: Unreads {
                messages,
                mentions,
                ..Unreads::default()
            },
            muted: false,
            last_post_at: 0,
        }
    }

    pub(super) fn section(id: &str, channels: Vec<SidebarChannel>) -> SidebarSection {
        SidebarSection {
            id: id.into(),
            kind: "custom".into(),
            display_name: id.to_uppercase(),
            sorting: String::new(),
            collapsed: false,
            muted: false,
            channels,
        }
    }

    pub(super) fn team(id: &str) -> TeamWithUnreads {
        TeamWithUnreads {
            id: id.into(),
            name: id.into(),
            display_name: format!("Team {id}"),
            has_unreads: false,
            mentions: 0,
            icon: 0,
        }
    }

    pub(super) fn live() -> Live {
        let dir = tempfile::tempdir().unwrap();
        let account = SavedAccount {
            server: "http://localhost".into(),
            user_id: "me".into(),
            username: "me".into(),
            display_name: "Me".into(),
            ..SavedAccount::default()
        };
        let live = Live::start(dir.path().join("account.sqlite"), &account)
            .unwrap()
            .0;
        // The engine may still be opening the file; let the folder outlive it.
        std::mem::forget(dir);
        live
    }

    fn capabilities(version: &str) -> ServerCapabilities {
        let config = HashMap::from([("Version".to_string(), serde_json::json!(version))]);
        ServerCapabilities::from_old_format(&config, &HashMap::new()).unwrap()
    }

    #[test]
    fn a_server_below_the_floor_is_refused_naming_both_versions() {
        let error = supported(capabilities("9.3.2")).unwrap_err();
        let minimum = matty_api::capabilities::MINIMUM_VERSION.to_string();
        assert!(error.contains("9.3.2"), "{error}");
        assert!(error.contains(&minimum), "{error}");
        assert!(supported(capabilities(&minimum)).is_ok());
        assert!(supported(capabilities("11.0.1")).is_ok());
    }

    #[test]
    fn certificate_errors_keep_their_own_words() {
        let text =
            "The server's certificate has expired. Ask the server's administrator to renew it.";
        assert_eq!(
            probe_error(
                "https://chat.example.com",
                &ProbeError::Ping(ApiError::Tls(text.into()))
            ),
            text
        );
        assert_eq!(
            sign_in_error(&ApiError::Tls(text.into())),
            SignInFailure::Message(text.into())
        );
    }

    #[test]
    fn server_check_errors_say_what_went_wrong() {
        let url = "https://chat.example.com";
        let say = |error: ProbeError| probe_error(url, &error);
        assert_eq!(
            say(ProbeError::Ping(ApiError::Network("dns error".into()))),
            "Can't reach chat.example.com. Check the address and your connection."
        );
        assert!(
            say(ProbeError::Ping(ApiError::Timeout)).starts_with("chat.example.com did not answer")
        );
        // A redirect names the server it points at, not the API path.
        assert_eq!(
            say(ProbeError::Ping(ApiError::Redirected {
                location: "https://mm.example.org/api/v4/system/ping".into()
            })),
            "That address sends Matty to https://mm.example.org. Enter that address instead."
        );
        for not_mattermost in [
            ApiError::Decode("expected value".into()),
            refused(404, ""),
            refused(500, ""),
        ] {
            assert_eq!(
                say(ProbeError::Ping(not_mattermost)),
                "chat.example.com answered, but it is not a Mattermost server. Check the address."
            );
        }
        assert!(say(ProbeError::Unhealthy("UNHEALTHY".into())).contains("not working right now"));
        assert!(
            say(ProbeError::Config(ApiError::Decode("x".into())))
                .contains("did not share its sign-in settings")
        );
        // A config request that could not connect reads like any other.
        assert!(say(ProbeError::Config(ApiError::Timeout)).contains("did not answer in time"));
        assert_eq!(url_host("http://localhost:8065/sub"), "localhost:8065");
    }

    fn refused(status: u16, id: &str) -> ApiError {
        ApiError::Server {
            status,
            id: id.into(),
            message: "Server words.".into(),
            request_id: None,
        }
    }

    #[test]
    fn sign_in_errors_are_specific() {
        assert_eq!(
            sign_in_error(&ApiError::MfaRequired),
            SignInFailure::MfaRequired
        );
        let says = |error: ApiError| match sign_in_error(&error) {
            SignInFailure::Message(text) => text.to_string(),
            SignInFailure::MfaRequired => String::new(),
        };
        assert!(
            says(refused(
                401,
                "api.user.login.invalid_credentials_email_username"
            ))
            .contains("login or password is not right")
        );
        assert!(
            says(refused(
                401,
                "api.user.check_user_login_attempts.too_many.app_error"
            ))
            .contains("locked")
        );
        assert!(
            says(refused(
                401,
                "api.user.check_user_login_attempts.too_many_ldap.app_error"
            ))
            .contains("locked")
        );
        assert!(says(refused(401, "api.user.login.inactive.app_error")).contains("deactivated"));
        assert_eq!(says(refused(400, "some.other.error")), "Server words.");
    }

    #[test]
    fn a_profile_edit_renames_the_account() {
        let mut live = live();
        let me = User {
            id: "me".into(),
            username: "ada".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            ..Default::default()
        };
        let other = User {
            id: "bo".into(),
            username: "bo".into(),
            ..Default::default()
        };
        let update = live.apply(&ChangeSet::Users(Arc::from(vec![other.clone()])));
        assert_eq!(update.me, None);
        let update = live.apply(&ChangeSet::Users(Arc::from(vec![me, other])));
        let (name, username) = update.me.unwrap();
        assert_eq!(username, "ada");
        assert!(name.contains("Ada"), "{name}");
        assert_eq!(live.me_user().map(|u| u.username.as_str()), Some("ada"));
    }

    #[test]
    fn the_first_team_in_the_users_order_is_shown() {
        let mut live = live();
        let t2 = ChangeSet::Sidebar {
            team_id: "t2".into(),
            sections: Arc::from(vec![section("c", vec![channel("town", "O", 0, 0)])]),
        };
        let t1 = ChangeSet::Sidebar {
            team_id: "t1".into(),
            sections: Arc::from(vec![section("c", vec![channel("general", "O", 3, 1)])]),
        };
        live.apply(&t2);
        live.apply(&t1);
        let update = live.apply(&ChangeSet::Unreads {
            teams: Arc::from(vec![team("t1"), team("t2")]),
        });
        assert_eq!(update.team_name.as_deref(), Some("Team t1"));
        let categories = update.categories.unwrap();
        let row = &categories[0].rows[0];
        assert_eq!(
            (row.id.as_ref(), row.unread, row.mentions),
            ("general", true, 1)
        );
        // Another team's sidebar changing leaves the shown one alone.
        assert_eq!(live.apply(&t2).categories, None);
    }

    #[test]
    fn a_channel_row_keeps_its_url_name_for_tilde_mentions() {
        let mut live = live();
        live.apply(&ChangeSet::Unreads {
            teams: Arc::from(vec![team("t1")]),
        });
        let mut town = channel("c1", "O", 0, 0);
        town.name = "town-square".into();
        town.display_name = "Town Square".into();
        let update = live.apply(&ChangeSet::Sidebar {
            team_id: "t1".into(),
            sections: Arc::from(vec![section("c", vec![town])]),
        });
        let row = &update.categories.unwrap()[0].rows[0];
        assert_eq!(row.name.as_ref(), "Town Square");
        assert_eq!(row.handle.as_deref(), Some("town-square"));
    }

    #[test]
    fn a_dm_row_follows_the_name_format_and_keeps_the_username_to_find_it() {
        use crate::settings::NameFormat;
        let mut live = live();
        live.apply(&ChangeSet::Unreads {
            teams: Arc::from(vec![team("t1")]),
        });
        let mut dm = channel("dm", "D", 0, 0);
        dm.display_name = "ada".into();
        dm.dm_user = Some(User {
            id: "dm-user".into(),
            username: "ada".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            nickname: "Countess".into(),
            ..User::default()
        });
        let update = live.apply(&ChangeSet::Sidebar {
            team_id: "t1".into(),
            sections: Arc::from(vec![section("c", vec![dm])]),
        });
        let row = &update.categories.unwrap()[0].rows[0];
        assert_eq!(row.name.as_ref(), "Countess");
        assert_eq!(row.handle.as_deref(), Some("ada"));
        // A new format renames the sidebar (and so the switcher) at once.
        let update = live.set_names(NameFormat::FullName);
        assert_eq!(
            update.categories.unwrap()[0].rows[0].name.as_ref(),
            "Ada Lovelace"
        );
        let update = live.set_names(NameFormat::Username);
        assert_eq!(update.categories.unwrap()[0].rows[0].name.as_ref(), "ada");
        assert_eq!(live.channel_name("dm").as_deref(), Some("ada"));
    }

    #[test]
    fn rows_take_their_kind_and_states_from_the_snapshot() {
        let mut live = live();
        live.apply(&ChangeSet::Unreads {
            teams: Arc::from(vec![team("t1")]),
        });
        let mut muted = channel("quiet", "P", 2, 0);
        muted.muted = true;
        let mut group = channel("gm", "G", 0, 0);
        group.display_name = "ada, alan, grace".into();
        let update = live.apply(&ChangeSet::Sidebar {
            team_id: "t1".into(),
            sections: Arc::from(vec![section(
                "c",
                vec![muted, group, channel("dm", "D", 0, 0)],
            )]),
        });
        let rows = &update.categories.unwrap()[0].rows;
        assert_eq!(rows[0].kind, ChannelKind::Private);
        assert!(rows[0].muted && rows[0].unread);
        assert_eq!(rows[1].kind, ChannelKind::Group { members: 3 });
        assert_eq!(
            rows[2].kind,
            ChannelKind::Direct {
                presence: Presence::Offline
            }
        );

        // A status change redraws the direct message's dot.
        let update = live.apply(&ChangeSet::Presence(Arc::from(vec![Status {
            user_id: "dm-user".into(),
            status: "dnd".into(),
            ..Status::default()
        }])));
        assert_eq!(
            update.categories.unwrap()[0].rows[2].kind,
            ChannelKind::Direct {
                presence: Presence::Dnd
            }
        );
    }

    #[test]
    fn a_permalink_uses_the_channels_team() {
        let mut live = live();
        let team = |id: &str, name: &str| TeamWithUnreads {
            id: id.into(),
            name: name.into(),
            display_name: name.into(),
            has_unreads: false,
            mentions: 0,
            icon: 0,
        };
        live.teams = vec![team("t1", "core"), team("t2", "design")];
        live.sections.insert(
            "t2".into(),
            Arc::from(vec![section(
                "s",
                vec![channel("c2", "O", 0, 0), channel("dm", "D", 0, 0)],
            )]),
        );
        live.team = Some("t1".into());
        assert_eq!(
            live.permalink("c2", "p1").as_deref(),
            Some("http://localhost/design/pl/p1")
        );
        // A direct message belongs to no team: the team on screen.
        assert_eq!(
            live.permalink("dm", "p2").as_deref(),
            Some("http://localhost/core/pl/p2")
        );
    }

    #[test]
    fn custom_statuses_reach_the_account_and_profiles() {
        let mut live = live();
        let status = |emoji: &str, text: &str, expires_at: &str| {
            serde_json::to_string(&matty_api::models::CustomStatus {
                emoji: emoji.into(),
                text: text.into(),
                duration: "one_hour".into(),
                expires_at: expires_at.into(),
            })
            .unwrap()
        };
        let me = User {
            id: live.me.0.clone(),
            username: "me".into(),
            props: [
                (
                    "customStatus".to_string(),
                    status("calendar", "In a meeting", ""),
                ),
                (
                    "recentCustomStatuses".to_string(),
                    format!(
                        "[{}]",
                        status("palm_tree", "On a vacation", "2020-01-01T00:00:00Z")
                    ),
                ),
            ]
            .into_iter()
            .collect(),
            ..User::default()
        };
        let update = live.apply(&ChangeSet::Me(Arc::new(me)));
        let (own, recent) = update.own_status.unwrap();
        assert_eq!(own.unwrap().text, "In a meeting");
        // A recent status is offered again even though its end has passed.
        assert_eq!(recent[0].text, "On a vacation");

        // Someone else's expired status shows nowhere.
        let other = |expires_at: &str| User {
            id: "u2".into(),
            username: "ana".into(),
            props: [(
                "customStatus".to_string(),
                status("coffee", "Break", expires_at),
            )]
            .into_iter()
            .collect(),
            ..User::default()
        };
        live.apply(&ChangeSet::Users(Arc::from(vec![other(
            "2020-01-01T00:00:00Z",
        )])));
        assert_eq!(live.known_user("ana").unwrap().status, None);
        live.apply(&ChangeSet::Users(Arc::from(vec![other(
            "2999-01-01T00:00:00Z",
        )])));
        let profile = live.known_user("ana").unwrap();
        assert_eq!(profile.status.unwrap().emoji, "coffee");
    }

    #[test]
    fn the_users_notify_props_set_the_mention_keys() {
        let mut live = live();
        let me = User {
            id: live.me.0.clone(),
            username: "me".into(),
            notify_props: [("mention_keys".to_string(), "me,@me,deploy".to_string())]
                .into_iter()
                .collect(),
            ..User::default()
        };
        let update = live.apply(&ChangeSet::Me(Arc::new(me.clone())));
        assert_eq!(update.notify_props.as_ref().map(|p| p.len()), Some(1));
        assert!(live.prefs.keys.mentioned("the deploy is out"));
        // A later fetch without the map keeps the keywords.
        let bare = User {
            notify_props: Default::default(),
            ..me
        };
        let update = live.apply(&ChangeSet::Users(Arc::from(vec![bare])));
        assert_eq!(update.notify_props, None);
        assert!(live.prefs.keys.mentioned("the deploy is out"));
    }

    #[test]
    fn an_arriving_post_notifies_with_the_users_keywords() {
        let mut live = live();
        let me = User {
            id: live.me.0.clone(),
            username: "me".into(),
            notify_props: [("mention_keys".to_string(), "deploy".to_string())]
                .into_iter()
                .collect(),
            ..User::default()
        };
        live.apply(&ChangeSet::Me(Arc::new(me)));
        let arrived = |message: &str, channel_props: &[(&str, &str)]| matty_sync::Arrived {
            post: matty_api::models::Post {
                id: "p9".into(),
                channel_id: "c2".into(),
                user_id: "u2".into(),
                message: message.into(),
                ..Default::default()
            },
            channel_type: "O".into(),
            channel_display_name: "Town Square".into(),
            sender_name: "@sam".into(),
            mentioned: false,
            followed: false,
            channel_props: channel_props
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            collapsed_threads: false,
        };
        let away = matty_notify::AppState::default();
        let update = live.apply(&ChangeSet::Arrived(Arc::new(arrived(
            "the deploy is out",
            &[],
        ))));
        let arrival = update.arrived.unwrap();
        assert_eq!(arrival.event.author_name, "sam");
        let shown = arrival.decide(away.clone()).unwrap();
        assert_eq!(
            (shown.channel_id.as_str(), shown.post_id.as_str()),
            ("c2", "p9")
        );
        // No keyword, and the user's default is mentions only.
        let update = live.apply(&ChangeSet::Arrived(Arc::new(arrived("lunch?", &[]))));
        assert_eq!(update.arrived.unwrap().decide(away.clone()), None);
        // A muted channel stays quiet even on a keyword.
        let muted = arrived("the deploy is out", &[("mark_unread", "mention")]);
        let update = live.apply(&ChangeSet::Arrived(Arc::new(muted)));
        assert_eq!(update.arrived.unwrap().decide(away.clone()), None);
        // The channel on screen in a focused window does not notify.
        let update = live.apply(&ChangeSet::Arrived(Arc::new(arrived(
            "the deploy is out",
            &[],
        ))));
        let watching = matty_notify::AppState {
            focused: true,
            visible_channel: Some("c2".into()),
            ..away
        };
        assert_eq!(update.arrived.unwrap().decide(watching), None);
    }

    #[test]
    fn the_users_own_status_reaches_the_account_menu() {
        let mut live = live();
        let me = live.me.0.clone();
        let update = live.apply(&ChangeSet::Presence(Arc::from(vec![Status {
            user_id: me.clone(),
            status: "dnd".into(),
            dnd_end_time: 1_800_000_000,
            ..Status::default()
        }])));
        assert_eq!(
            update.own_presence,
            Some((Presence::Dnd, Some(1_800_000_000)))
        );
        // A status_change event carries no end time.
        let update = live.apply(&ChangeSet::Presence(Arc::from(vec![Status {
            user_id: me,
            status: "online".into(),
            ..Status::default()
        }])));
        assert_eq!(update.own_presence, Some((Presence::Online, None)));
        // Someone else's status leaves the menu alone.
        let update = live.apply(&ChangeSet::Presence(Arc::from(vec![Status {
            user_id: "someone".into(),
            status: "away".into(),
            ..Status::default()
        }])));
        assert_eq!(update.own_presence, None);
    }

    #[test]
    fn network_failures_show_the_banner_and_never_a_dialog() {
        let mut live = live();
        // The account could not load because the network is down.
        let update = live.apply(&ChangeSet::Connection(Connection::Failed {
            error: matty_api::ApiError::Network("dns error".into()).to_string(),
        }));
        assert_eq!(update.connection, Some(ConnectionState::Offline));
        assert!(!update.expired, "a network error is not a sign-in prompt");
        assert!(!update.signed_out);
        // Commands that need a connection while there is none, or that the
        // network dropped, leave the banner to speak.
        for failure in [
            matty_sync::CommandError::Offline { action: "view" },
            matty_sync::CommandError::Server {
                action: "edit",
                target: "p1".into(),
                error: matty_api::ApiError::Network("connection reset".into()).to_string(),
            },
        ] {
            let update = live.apply(&ChangeSet::Failed(failure));
            assert!(update.join_failed.is_none());
            assert!(!update.expired);
            assert!(!update.signed_out);
        }
    }

    /// #163: an action that could not reach the server reaches the window
    /// by name, and a search or a mentions listing that could not run stops
    /// loading rather than spin.
    #[test]
    fn offline_failures_name_the_action_and_stop_loading() {
        let mut live = live();
        let refused = |action| ChangeSet::Failed(matty_sync::CommandError::Offline { action });
        let update = live.apply(&refused("add reaction"));
        assert_eq!(update.offline_refused, Some("add reaction"));

        live.apply(&ChangeSet::Unreads {
            teams: Arc::from(vec![team("t1")]),
        });
        live.search("lunch");
        let update = live.apply(&refused("search"));
        let search = update.search.expect("the search stops");
        assert!(!search.searching && search.results.is_empty());
        assert_eq!(update.offline_refused, None, "the pane says it");

        live.show_mentions();
        let update = live.apply(&refused("load recent mentions"));
        assert!(matches!(
            update.listing,
            Some(crate::right_pane::RightPaneContent::Mentions(ref posts)) if posts.is_empty()
        ));
    }

    #[test]
    fn the_connection_and_sign_out_reach_the_window() {
        let mut live = live();
        let update = live.apply(&ChangeSet::Connection(Connection::Socket(
            matty_api::ws::ConnectionState::Reconnecting {
                attempt: 2,
                reason: "closed".into(),
            },
        )));
        assert_eq!(
            update.connection,
            Some(ConnectionState::Reconnecting { attempt: 2 })
        );
        let update = live.apply(&ChangeSet::Connection(Connection::Socket(
            matty_api::ws::ConnectionState::Reconnecting {
                attempt: OFFLINE_AFTER + 1,
                reason: "dns error".into(),
            },
        )));
        assert_eq!(update.connection, Some(ConnectionState::Offline));
        let update = live.apply(&ChangeSet::Connection(Connection::Socket(
            matty_api::ws::ConnectionState::Connected {
                connection_id: "c".into(),
            },
        )));
        assert_eq!(update.connection, Some(ConnectionState::Online));
        assert!(
            live.apply(&ChangeSet::Connection(Connection::SignedOut))
                .signed_out
        );
    }

    /// #194: a file added offline waits by its path, goes with its post
    /// as a local file, and uploads once the connection is back.
    #[test]
    fn files_added_offline_wait_and_resume_when_back() {
        let mut live = live();
        live.apply(&ChangeSet::Connection(Connection::Socket(
            matty_api::ws::ConnectionState::Stopped,
        )));
        assert!(live.is_offline());
        for id in ["upload-1", "upload-2"] {
            let path = PathBuf::from(format!("/home/me/{id}.pdf"));
            live.track_upload(
                id,
                UploadJob::new("c", "x.pdf", UploadSource::Path(path.clone())),
            );
            live.queue_upload(id, path);
        }
        let (file_ids, local) = live.take_files(&["upload-1".into()]);
        assert!(file_ids.is_empty());
        assert_eq!(local, [PathBuf::from("/home/me/upload-1.pdf")]);

        let update = live.apply(&ChangeSet::Connection(Connection::Socket(
            matty_api::ws::ConnectionState::Connected {
                connection_id: "c".into(),
            },
        )));
        assert!(!live.is_offline());
        assert_eq!(update.resume_uploads, [SharedString::from("upload-2")]);
        // Resumed, it no longer goes as a local file.
        assert_eq!(live.take_files(&["upload-2".into()]), (vec![], vec![]));
    }

    #[test]
    fn the_remembered_name_prefers_the_nickname_then_the_full_name() {
        let mut user = User {
            id: "u1".into(),
            username: "ada".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            ..User::default()
        };
        assert_eq!(
            saved_account("https://x", &user).display_name,
            "Ada Lovelace"
        );
        user.nickname = "Countess".into();
        assert_eq!(saved_account("https://x", &user).display_name, "Countess");
        user.nickname.clear();
        user.first_name.clear();
        user.last_name.clear();
        assert_eq!(saved_account("https://x", &user).display_name, "ada");
    }

    fn shown(id: &str, body: &str) -> Post {
        Post {
            webhook: None,
            id: id.into(),
            author: "a".into(),
            author_id: SharedString::default(),
            kind: crate::channel::PostKind::Regular,
            time: 600,
            day: "Today".into(),
            date: SharedString::default(),
            body: body.into(),
            reactions: vec![],
            mentions_me: false,
            edited: false,
            continues: false,
            mine: false,
            saved: false,
            pinned: false,
            status: crate::channel::PostStatus::Sent,
            replies: None,
            root_id: "".into(),
            reply_to: None,
            files: vec![],
            embeds: vec![],
            priority: None,
        }
    }

    #[test]
    fn new_posts_append_and_changes_patch_in_place() {
        let old = [shown("a", "one"), shown("b", "two"), shown("c", "three")];
        let new = [shown("a", "one"), shown("c", "3"), shown("d", "four")];
        assert_eq!(
            patch(&old, &new),
            Some(ChannelUpdate::Patch {
                removed: vec!["b".into()],
                prepended: vec![],
                updated: vec![shown("c", "3")],
                appended: vec![shown("d", "four")],
            })
        );
    }

    #[test]
    fn older_history_goes_on_top() {
        let old = [shown("c", "three"), shown("d", "four")];
        let new = [
            shown("a", "one"),
            shown("b", "two"),
            shown("c", "three"),
            shown("d", "four"),
        ];
        assert_eq!(
            patch(&old, &new),
            Some(ChannelUpdate::Patch {
                removed: vec![],
                prepended: vec![shown("a", "one"), shown("b", "two")],
                updated: vec![],
                appended: vec![],
            })
        );
    }

    #[test]
    fn posts_arriving_under_an_unsent_row_keep_the_scroll() {
        let mut failed = shown("me:1", "not sent");
        failed.status = crate::channel::PostStatus::Failed;
        let old = [shown("a", "one"), failed.clone()];
        // A backfill brings posts written before the failed send.
        let new = [
            shown("a", "one"),
            shown("b", "two"),
            shown("c", "three"),
            failed.clone(),
        ];
        assert_eq!(
            patch(&old, &new),
            Some(ChannelUpdate::Patch {
                removed: vec!["me:1".into()],
                prepended: vec![],
                updated: vec![],
                appended: vec![shown("b", "two"), shown("c", "three"), failed],
            })
        );
    }

    #[test]
    fn a_post_between_shown_ones_resets() {
        let old = [shown("a", "one"), shown("c", "three")];
        let new = [shown("a", "one"), shown("b", "two"), shown("c", "three")];
        assert_eq!(patch(&old, &new), None);
    }

    #[test]
    fn the_open_thread_reaches_the_pane_and_others_do_not() {
        let mut live = live();
        let post = |id: &str, message: &str| matty_api::models::Post {
            id: id.into(),
            channel_id: "c1".into(),
            user_id: "ada".into(),
            message: message.into(),
            root_id: if id == "root" {
                String::new()
            } else {
                "root".into()
            },
            ..Default::default()
        };
        let thread = |root: &str| ChangeSet::Thread {
            root_id: root.into(),
            posts: Arc::from(vec![post("root", "Plan?"), post("r1", "Yes")]),
            older: true,
            failed: false,
        };
        assert_eq!(live.apply(&thread("root")).thread, None, "not open");
        live.open_thread("root");
        assert_eq!(live.thread_channel(), None, "no posts yet");
        let model = live.apply(&thread("root")).thread.unwrap();
        assert_eq!(model.root.body.as_ref(), "Plan?");
        let replies: Vec<_> = model.replies.iter().map(|r| r.body.as_ref()).collect();
        assert_eq!(replies, ["Yes"]);
        assert!(model.older, "older replies wait on the server");
        assert_eq!(live.thread_channel().as_deref(), Some("c1"));
        assert_eq!(live.apply(&thread("other")).thread, None);
        live.close_thread();
        assert_eq!(live.apply(&thread("root")).thread, None, "closed");
    }

    #[test]
    fn turning_link_previews_off_drops_the_cards_in_place() {
        let mut live = live();
        live.open(ChannelRow::new("c1", "town", ChannelKind::Public));
        let post = matty_api::models::Post {
            id: "p1".into(),
            user_id: "ada".into(),
            message: "https://github.com/a".into(),
            metadata: matty_api::models::PostMetadata {
                embeds: vec![matty_api::models::PostEmbed {
                    kind: matty_api::models::EmbedKind::OpenGraph,
                    url: "https://github.com/a".into(),
                    data: Some(serde_json::json!({"title": "Repo", "site_name": "GitHub"})),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let update = live.apply(&ChangeSet::Posts {
            channel_id: "c1".into(),
            posts: Arc::from(vec![post]),
        });
        let Some(ChannelUpdate::Replace(model)) = update.channel else {
            panic!("the channel shows");
        };
        assert_eq!(model.posts[0].embeds.len(), 1);
        let update = live.apply(&ChangeSet::LinkPreviews(false));
        let Some(ChannelUpdate::Patch { updated, .. }) = update.channel else {
            panic!("patched in place");
        };
        assert!(updated[0].embeds.is_empty());
        assert!(!live.link_previews());
        assert!(
            live.apply(&ChangeSet::LinkPreviews(false))
                .channel
                .is_none(),
            "no change"
        );
    }

    #[test]
    fn the_shown_teams_thread_list_reaches_the_threads_view() {
        let mut live = live();
        live.apply(&ChangeSet::Unreads {
            teams: Arc::from(vec![team("t1")]),
        });
        let root = matty_api::models::Post {
            id: "root".into(),
            channel_id: "c1".into(),
            user_id: "ada".into(),
            message: "Plan?".into(),
            ..Default::default()
        };
        let summary = |id: &str, unread| matty_sync::ThreadSummary {
            id: id.into(),
            channel_id: "c1".into(),
            reply_count: 2,
            last_reply_at: 0,
            last_viewed_at: 0,
            unread_replies: unread,
            unread_mentions: unread / 2,
            participants: vec!["ada".into()],
        };
        let list = |team: &str| ChangeSet::ThreadList {
            team_id: team.into(),
            // "gone" has no cached root yet, so it waits.
            threads: Arc::from(vec![summary("root", 2), summary("gone", 1)]),
            roots: Arc::from(vec![root.clone()]),
            more: true,
            failed: false,
        };
        assert_eq!(live.apply(&list("t2")).threads, None, "another team");
        let model = live.apply(&list("t1")).threads.unwrap();
        assert_eq!(model.threads.len(), 1);
        assert!(model.more && !model.failed);
        let row = &model.threads[0];
        assert_eq!(row.thread.root.body.as_ref(), "Plan?");
        // The row carries the server's unread counts (#124).
        assert_eq!(
            (
                row.summary.count,
                row.summary.unread_replies,
                row.summary.unread_mentions
            ),
            (2, 2, 1)
        );
        assert_eq!(model.unread(), 1);
        assert_eq!(model.mentions(), 1);
    }

    #[test]
    fn an_empty_channel_says_so_only_while_online() {
        let mut live = live();
        live.open(ChannelRow::new("c1", "town", ChannelKind::Public));
        let empty = ChangeSet::Posts {
            channel_id: "c1".into(),
            posts: Arc::from(Vec::new()),
        };
        let Some(ChannelUpdate::Replace(model)) = live.apply(&empty).channel else {
            panic!("the channel shows");
        };
        assert!(!model.offline);
        let socket = |state| ChangeSet::Connection(Connection::Socket(state));
        let update = live.apply(&socket(matty_api::ws::ConnectionState::Stopped));
        let Some(ChannelUpdate::Replace(model)) = update.channel else {
            panic!("redrawn offline");
        };
        assert!(model.offline && model.posts.is_empty());
        let update = live.apply(&socket(matty_api::ws::ConnectionState::Connected {
            connection_id: "x".into(),
        }));
        let Some(ChannelUpdate::Replace(model)) = update.channel else {
            panic!("redrawn online");
        };
        assert!(!model.offline);
    }

    fn opened(live: &mut Live) {
        live.open(ChannelRow::new("c1", "town", ChannelKind::Public));
        live.apply(&ChangeSet::Posts {
            channel_id: "c1".into(),
            posts: Arc::from(vec![matty_api::models::Post {
                id: "p1".into(),
                user_id: "ada".into(),
                is_pinned: true,
                ..Default::default()
            }]),
        });
    }

    #[test]
    fn only_channel_and_system_admins_may_manage_the_channel() {
        let mut live = live();
        opened(&mut live);
        let member = |roles: &str, scheme_admin: bool| ChangeSet::Channel {
            channel: Arc::new(Channel {
                id: "c1".into(),
                ..Channel::default()
            }),
            member: Some(Arc::new(ChannelMember {
                channel_id: "c1".into(),
                user_id: "me".into(),
                roles: roles.into(),
                scheme_admin,
                ..ChannelMember::default()
            })),
        };
        let manage = |update: Update| update.details.map(|d| d.can_manage);
        assert_eq!(
            manage(live.apply(&member("channel_user", false))),
            Some(false)
        );
        assert_eq!(
            manage(live.apply(&member("channel_user channel_admin", false))),
            Some(true)
        );
        assert_eq!(
            manage(live.apply(&member("channel_user", true))),
            Some(true)
        );
        live.apply(&member("channel_user", false));
        live.apply(&ChangeSet::Users(Arc::from(vec![User {
            id: "me".into(),
            roles: "system_user system_admin".into(),
            ..User::default()
        }])));
        assert!(live.details(0).can_manage);
        assert!(live.details(0).delete_others);
    }

    #[test]
    fn the_banner_counts_waiting_sends_across_channels() {
        let mut live = live();
        let outbox = |channel: &str, errors: &[Option<&str>]| ChangeSet::Outbox {
            channel_id: channel.into(),
            sends: errors
                .iter()
                .enumerate()
                .map(|(at, error)| matty_sync::Outgoing {
                    pending_post_id: format!("{channel}:{at}"),
                    channel_id: channel.into(),
                    last_error: error.map(str::to_owned),
                    ..Default::default()
                })
                .collect(),
        };
        assert_eq!(live.apply(&outbox("c1", &[None, None])).queued, Some(2));
        // A failed send waits for the user, not the connection.
        assert_eq!(
            live.apply(&outbox("c2", &[None, Some("no")])).queued,
            Some(3)
        );
        assert_eq!(live.apply(&outbox("c1", &[None, None])).queued, None);
        assert_eq!(live.apply(&outbox("c1", &[])).queued, Some(1));
    }

    #[test]
    fn a_failed_send_reads_not_sent_until_it_goes_again() {
        let mut live = live();
        live.open(ChannelRow::new("c1", "town", ChannelKind::Public));
        live.apply(&ChangeSet::Posts {
            channel_id: "c1".into(),
            posts: Arc::from(vec![matty_api::models::Post {
                id: "me:1".into(),
                pending_post_id: "me:1".into(),
                user_id: "ada".into(),
                ..Default::default()
            }]),
        });
        let outbox = |error: Option<&str>| ChangeSet::Outbox {
            channel_id: "c1".into(),
            sends: Arc::from(vec![matty_sync::Outgoing {
                pending_post_id: "me:1".into(),
                channel_id: "c1".into(),
                last_error: error.map(str::to_owned),
                ..Default::default()
            }]),
        };
        let status = |update: Update| match update.channel {
            Some(ChannelUpdate::Patch { updated, .. }) => updated.first().map(|post| post.status),
            _ => None,
        };
        assert_eq!(
            status(live.apply(&outbox(Some("refused")))),
            Some(crate::channel::PostStatus::Failed)
        );
        // Retry: queued again, so it reads "Sending…".
        assert_eq!(
            status(live.apply(&outbox(None))),
            Some(crate::channel::PostStatus::Pending)
        );
    }

    #[test]
    fn the_header_and_counts_follow_the_first_snapshot() {
        let mut live = live();
        opened(&mut live);
        let update = live.apply(&ChangeSet::Channel {
            channel: Arc::new(Channel {
                id: "c1".into(),
                header: "Ship it".into(),
                ..Channel::default()
            }),
            member: None,
        });
        assert_eq!(
            update.details,
            Some(Details {
                header: Some("Ship it".into()),
                purpose: None,
                members: 0,
                pinned: 1,
                can_manage: false,
                delete_others: false,
                bookmarks: None,
                apps: crate::apps::ChannelApps {
                    channel_id: "c1".into(),
                    ..Default::default()
                },
            })
        );
        let update = live.apply(&ChangeSet::Stats(Arc::new(ChannelStats {
            channel_id: "c1".into(),
            member_count: 12,
            pinnedpost_count: 4,
            ..ChannelStats::default()
        })));
        let details = update.details.unwrap();
        assert_eq!((details.members, details.pinned), (12, 4));
        // Another channel's counts are not this one's.
        let update = live.apply(&ChangeSet::Stats(Arc::new(ChannelStats {
            channel_id: "c2".into(),
            ..ChannelStats::default()
        })));
        assert_eq!(update.details, None);
    }

    #[test]
    fn typing_shows_names_and_runs_out() {
        let mut live = live();
        opened(&mut live);
        live.apply(&ChangeSet::Users(Arc::from(vec![User {
            id: "ada".into(),
            username: "ada".into(),
            ..User::default()
        }])));
        let typing = |user: &str, channel: &str| ChangeSet::Typing {
            user_id: user.into(),
            channel_id: channel.into(),
            parent_id: String::new(),
        };
        assert_eq!(
            live.apply(&typing("ada", "c1")).typing,
            Some(vec!["ada".into()])
        );
        assert_eq!(live.apply(&typing("me", "c1")).typing, None);
        assert_eq!(live.apply(&typing("bob", "c2")).typing, None);
        assert_eq!(live.expire_typing(Instant::now()), (None, None));
        assert_eq!(
            live.expire_typing(Instant::now() + TYPING_FOR),
            (Some(Vec::new()), None)
        );
        assert!(!live.is_typing());
    }

    #[test]
    fn typing_a_reply_shows_in_the_open_thread_only() {
        let mut live = live();
        opened(&mut live);
        let reply = |user: &str, root: &str| ChangeSet::Typing {
            user_id: user.into(),
            channel_id: "c1".into(),
            parent_id: root.into(),
        };
        // No thread open: nothing shows, not even in the channel.
        let update = live.apply(&reply("ada", "r1"));
        assert_eq!((update.typing, update.thread_typing), (None, None));
        live.open_thread("r1");
        let update = live.apply(&reply("ada", "r1"));
        assert_eq!(update.typing, None);
        assert_eq!(update.thread_typing, Some(vec!["Someone".into()]));
        assert_eq!(live.apply(&reply("bob", "r2")).thread_typing, None);
        assert_eq!(live.apply(&reply("me", "r1")).thread_typing, None);
        assert!(live.is_typing());
        assert_eq!(
            live.expire_typing(Instant::now() + TYPING_FOR),
            (None, Some(Vec::new()))
        );
        live.apply(&reply("ada", "r1"));
        live.close_thread();
        assert!(!live.is_typing());
    }

    #[test]
    fn browse_marks_joined_channels_and_a_join_opens_once_in_the_sidebar() {
        let mut live = live();
        let sidebar = |channels| ChangeSet::Sidebar {
            team_id: "t1".into(),
            sections: Arc::from(vec![section("c", channels)]),
        };
        live.apply(&sidebar(vec![channel("town", "O", 0, 0)]));
        live.apply(&ChangeSet::Unreads {
            teams: Arc::from(vec![team("t1")]),
        });
        let public = |id: &str, name: &str| Channel {
            id: id.into(),
            display_name: name.into(),
            ..Channel::default()
        };
        let update = live.apply(&ChangeSet::Directory {
            team_id: "t1".into(),
            channels: Arc::from(vec![
                public("town", "Town Square"),
                public("design", "Design"),
            ]),
            members: Arc::new(HashMap::from([("design".to_string(), 9)])),
        });
        let rows = update.directory.unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| (row.name.as_ref(), row.members, row.joined))
                .collect::<Vec<_>>(),
            [("Design", 9, false), ("Town Square", 0, true)]
        );

        // Ready comes ahead of the sidebar that shows the channel.
        let update = live.apply(&ChangeSet::ChannelReady {
            channel_id: "design".into(),
        });
        assert_eq!(update.ready, None);
        let update = live.apply(&sidebar(vec![
            channel("town", "O", 0, 0),
            channel("design", "O", 0, 0),
        ]));
        assert_eq!(update.ready.as_deref(), Some("design"));
        assert_eq!(
            live.apply(&sidebar(vec![channel("design", "O", 0, 0)]))
                .ready,
            None
        );
    }

    #[test]
    fn people_leave_out_the_user_and_bots() {
        let mut live = live();
        live.apply(&ChangeSet::Unreads {
            teams: Arc::from(vec![team("t1")]),
        });
        let person = |id: &str, bot: bool| User {
            id: id.into(),
            username: id.into(),
            is_bot: bot,
            ..User::default()
        };
        let update = live.apply(&ChangeSet::People {
            team_id: "t1".into(),
            users: Arc::from(vec![
                person("me", false),
                person("zed", false),
                person("bot", true),
                person("ada", false),
            ]),
        });
        let names: Vec<_> = update
            .people
            .unwrap()
            .iter()
            .map(|p| p.username.to_string())
            .collect();
        assert_eq!(names, ["ada", "zed"]);
    }

    #[test]
    fn the_unread_digest_keeps_the_channel_order_it_asked_for() {
        let mut live = live();
        let post = |id: &str, channel: &str, at: i64| matty_api::models::Post {
            id: id.into(),
            channel_id: channel.into(),
            user_id: "ada".into(),
            message: "news".into(),
            create_at: at,
            ..Default::default()
        };
        live.show_unreads(vec!["b".into(), "a".into()]);
        let asked = matty_sync::Listing::Unreads {
            channel_ids: vec!["b".into(), "a".into()],
            collapsed_threads: false,
        };
        // Channel by channel, oldest first within each, as sync sends them;
        // never re-sorted newest first like the other listings.
        let shown = live
            .apply(&ChangeSet::Listing {
                listing: asked,
                posts: Arc::from(vec![
                    post("b1", "b", 1),
                    post("b2", "b", 5),
                    post("a1", "a", 3),
                ]),
            })
            .listing;
        let Some(crate::right_pane::RightPaneContent::Unreads(posts)) = shown else {
            panic!("expected the unread digest, got {shown:?}");
        };
        let ids: Vec<&str> = posts.iter().map(|item| item.post.id.as_ref()).collect();
        assert_eq!(ids, ["b1", "b2", "a1"]);
    }

    #[test]
    fn listings_show_once_loaded_and_unsaving_drops_a_saved_post() {
        let mut live = live();
        live.apply(&ChangeSet::Unreads {
            teams: Arc::from(vec![team("t1")]),
        });
        let post = |id: &str| matty_api::models::Post {
            id: id.into(),
            channel_id: "elsewhere".into(),
            user_id: "ada".into(),
            message: "keep this".into(),
            ..Default::default()
        };
        live.apply(&ChangeSet::Saved(Arc::from(vec![
            "p1".to_string(),
            "p2".to_string(),
        ])));
        assert!(live.is_saved("p1"));
        live.show_saved();
        // A listing the pane no longer asks for is ignored.
        let other = live.apply(&ChangeSet::Listing {
            listing: matty_sync::Listing::Pinned {
                channel_id: "town".into(),
            },
            posts: Arc::from(vec![post("p9")]),
        });
        assert_eq!(other.listing, None);
        let shown = live
            .apply(&ChangeSet::Listing {
                listing: matty_sync::Listing::Saved,
                posts: Arc::from(vec![post("p1"), post("p2")]),
            })
            .listing;
        let Some(crate::right_pane::RightPaneContent::Saved(posts)) = shown else {
            panic!("expected the saved listing, got {shown:?}");
        };
        assert_eq!(posts.len(), 2);
        assert!(posts[0].post.saved);
        let shown = live
            .apply(&ChangeSet::Saved(Arc::from(vec!["p2".to_string()])))
            .listing;
        let Some(crate::right_pane::RightPaneContent::Saved(posts)) = shown else {
            panic!("expected the saved listing, got {shown:?}");
        };
        assert_eq!(
            posts.iter().map(|p| p.post.id.as_ref()).collect::<Vec<_>>(),
            ["p2"]
        );
    }

    #[test]
    fn unpinning_a_listed_post_reloads_the_pinned_listing() {
        let mut live = live();
        let post = |id: &str, pinned: bool| matty_api::models::Post {
            id: id.into(),
            channel_id: "c1".into(),
            is_pinned: pinned,
            ..Default::default()
        };
        live.show_pinned("c1");
        let pinned = matty_sync::Listing::Pinned {
            channel_id: "c1".into(),
        };
        live.apply(&ChangeSet::Listing {
            listing: pinned,
            posts: Arc::from(vec![post("p1", true)]),
        });
        assert!(!live.pins_changed("c1", &[post("p1", true), post("p2", false)]));
        assert!(live.pins_changed("c1", &[post("p1", false)]));
        assert!(live.pins_changed("c1", &[post("p2", true)]));
        assert!(!live.pins_changed("c2", &[post("p2", true)]));
    }

    #[test]
    fn more_search_results_add_to_the_first_page() {
        let mut live = live();
        live.apply(&ChangeSet::Unreads {
            teams: Arc::from(vec![team("t1")]),
        });
        let hit = |id: &str| matty_api::models::Post {
            id: id.into(),
            channel_id: "c1".into(),
            ..Default::default()
        };
        live.search("lunch");
        assert!(
            live.search_more().is_none(),
            "nothing to page before results"
        );
        let first = live
            .apply(&ChangeSet::Search {
                terms: "lunch".into(),
                page: 0,
                posts: Arc::from(vec![hit("p1"), hit("p2")]),
                more: true,
            })
            .search
            .unwrap();
        assert!(first.more && !first.loading_more);
        let asked = live.search_more().unwrap();
        assert!(asked.loading_more);
        assert!(live.search_more().is_none(), "one page at a time");
        let model = live
            .apply(&ChangeSet::Search {
                terms: "lunch".into(),
                page: 1,
                posts: Arc::from(vec![hit("p2"), hit("p3")]),
                more: false,
            })
            .search
            .unwrap();
        assert_eq!(
            model
                .results
                .iter()
                .map(|r| r.post.id.as_ref())
                .collect::<Vec<_>>(),
            ["p1", "p2", "p3"]
        );
        assert!(!model.more && !model.loading_more);
    }

    #[test]
    fn mention_terms_leave_out_channel_wide_mentions() {
        let keys = matty_notify::MentionKeys {
            username: "ada".into(),
            first_name: Some("Ada".into()),
            channel_wide: true,
            keywords: vec!["deploy".into()],
            ..Default::default()
        };
        assert_eq!(mention_terms(&keys), "@ada Ada deploy");
    }

    #[test]
    fn search_results_name_their_channel_and_ignore_older_searches() {
        let mut live = live();
        live.apply(&ChangeSet::Sidebar {
            team_id: "t1".into(),
            sections: Arc::from(vec![section("c", vec![channel("town", "O", 0, 0)])]),
        });
        live.apply(&ChangeSet::Unreads {
            teams: Arc::from(vec![team("t1")]),
        });
        live.search("lunch");
        let hit = |id: &str, channel: &str| matty_api::models::Post {
            id: id.into(),
            channel_id: channel.into(),
            user_id: "ada".into(),
            message: "lunch?".into(),
            ..Default::default()
        };
        let stale = live.apply(&ChangeSet::Search {
            page: 0,
            more: false,
            terms: "lunc".into(),
            posts: Arc::from(vec![hit("p0", "town")]),
        });
        assert_eq!(stale.search, None);
        let model = live
            .apply(&ChangeSet::Search {
                page: 0,
                more: false,
                terms: "lunch".into(),
                posts: Arc::from(vec![hit("p1", "town"), hit("p2", "elsewhere")]),
            })
            .search
            .unwrap();
        assert!(!model.searching);
        assert_eq!(
            model
                .results
                .iter()
                .map(|r| (r.post.id.as_ref(), r.channel_name.as_ref()))
                .collect::<Vec<_>>(),
            [("p1", "town"), ("p2", "Another channel")]
        );
        // Names arriving later redraw the results.
        let update = live.apply(&ChangeSet::Users(Arc::from(vec![User {
            id: "ada".into(),
            username: "ada".into(),
            ..User::default()
        }])));
        assert_eq!(
            update.search.unwrap().results[0].post.author.as_ref(),
            "ada"
        );
    }

    #[test]
    fn notification_preferences_round_trip_through_notify_props() {
        use crate::channel::{ChannelNotifications, NotifyLevel};
        let member = ChannelMember {
            notify_props: HashMap::from([
                ("desktop".to_string(), "mention".to_string()),
                ("push".to_string(), "none".to_string()),
                ("mark_unread".to_string(), "mention".to_string()),
                ("ignore_channel_mentions".to_string(), "on".to_string()),
            ]),
            ..ChannelMember::default()
        };
        let (muted, prefs) = notifications(&member);
        assert!(muted);
        assert_eq!(
            prefs,
            ChannelNotifications {
                desktop: NotifyLevel::Mentions,
                mobile: NotifyLevel::None,
                ignore_channel_mentions: true,
                mark_unread_mentions_only: true,
            }
        );
        let props = notify_props(muted, &prefs);
        assert_eq!(
            notifications(&ChannelMember {
                notify_props: props,
                ..ChannelMember::default()
            }),
            (muted, prefs)
        );
        let (muted, prefs) = notifications(&ChannelMember::default());
        assert!(!muted);
        assert_eq!(prefs, ChannelNotifications::default());
    }

    #[test]
    fn upload_progress_reaches_the_tray_and_the_file_id_the_post() {
        use crate::attachments::{AttachmentStatus, UploadSource};
        let mut live = live();
        let job = UploadJob::new("c1", "notes.txt", UploadSource::Path("notes.txt".into()));
        live.track_upload("upload-1", job);
        let update = live.apply(&ChangeSet::Upload {
            upload_id: "upload-1".into(),
            state: UploadState::Sending(0.5),
        });
        assert_eq!(
            update.upload,
            Some(("upload-1".into(), AttachmentStatus::Uploading(0.5)))
        );
        let update = live.apply(&ChangeSet::Upload {
            upload_id: "upload-1".into(),
            state: UploadState::Done {
                file_id: "f1".into(),
            },
        });
        assert_eq!(
            update.upload,
            Some(("upload-1".into(), AttachmentStatus::Ready))
        );
        assert_eq!(live.take_files(&["upload-1".into()]).0, vec!["f1"]);
        // It left with the post; a late report changes nothing.
        let update = live.apply(&ChangeSet::Upload {
            upload_id: "upload-1".into(),
            state: UploadState::Failed("gone".into()),
        });
        assert_eq!(update.upload, None);
    }

    #[test]
    fn a_stored_draft_reaches_the_window_with_its_files() {
        let mut live = live();
        let draft = matty_api::models::Draft {
            channel_id: "c1".into(),
            root_id: "r1".into(),
            message: "later".into(),
            file_ids: vec!["f1".into()],
            ..Default::default()
        };
        let update = live.apply(&ChangeSet::Draft {
            channel_id: "c1".into(),
            root_id: "r1".into(),
            draft: Some(Arc::new(draft)),
        });
        let (key, stored) = update.draft.unwrap();
        assert_eq!(key, crate::composer_draft::DraftKey::thread("c1", "r1"));
        assert_eq!(stored.unwrap().message, "later");
        // Its file comes back as a finished upload a send can carry.
        let upload = live.draft_upload("c1", "f1");
        assert!(upload.is_ready());
        assert_eq!(live.file_ids(std::slice::from_ref(&upload.id)), ["f1"]);
        assert_eq!(live.take_files(&[upload.id]).0, ["f1"]);
    }

    #[test]
    fn a_removed_upload_is_forgotten() {
        use crate::attachments::UploadSource;
        let mut live = live();
        let job = UploadJob::new("c1", "notes.txt", UploadSource::Path("notes.txt".into()));
        live.track_upload("upload-1", job);
        live.cancel_upload("upload-1");
        assert!(live.upload_job("upload-1").is_none());
        assert!(live.take_files(&["upload-1".into()]).0.is_empty());
    }

    #[test]
    fn a_thumbnail_fills_its_file_on_screen() {
        let mut live = live();
        live.open(ChannelRow::new("c1", "town", ChannelKind::Public));
        let mut post = matty_api::models::Post {
            id: "p1".into(),
            user_id: "ada".into(),
            ..Default::default()
        };
        post.metadata.files.push(matty_api::models::FileInfo {
            id: "f1".into(),
            name: "menu.png".into(),
            ..Default::default()
        });
        live.apply(&ChangeSet::Posts {
            channel_id: "c1".into(),
            posts: Arc::from(vec![post]),
        });
        assert!(live.asked_thumbnails.contains("f1"));
        let update = live.apply(&ChangeSet::Image {
            file_id: "f1".into(),
            size: ImageSize::Thumbnail,
            bytes: Arc::from(&b"png"[..]),
            content_type: "image/png".into(),
        });
        let Some(ChannelUpdate::Patch { updated, .. }) = update.channel else {
            panic!("expected a patch, got {:?}", update.channel);
        };
        assert!(updated[0].files[0].preview.is_some());
    }

    #[test]
    fn a_link_previews_image_is_fetched_and_shown() {
        let mut live = live();
        live.open(ChannelRow::new("c1", "town", ChannelKind::Public));
        let mut post = matty_api::models::Post {
            id: "p1".into(),
            user_id: "ada".into(),
            message: "https://example.com/post".into(),
            ..Default::default()
        };
        post.metadata.embeds.push(matty_api::models::PostEmbed {
            kind: matty_api::models::EmbedKind::OpenGraph,
            url: "https://example.com/post".into(),
            data: Some(serde_json::json!({
                "title": "A post",
                "images": [{ "secure_url": "https://example.com/og.png" }],
            })),
        });
        post.metadata.images.insert(
            "https://example.com/og.png".into(),
            matty_api::models::PostImage {
                width: 1200,
                height: 630,
                ..Default::default()
            },
        );
        live.apply(&ChangeSet::Posts {
            channel_id: "c1".into(),
            posts: Arc::from(vec![post]),
        });
        assert!(live.remote_images.asked("https://example.com/og.png"));
        let update = live.apply(&ChangeSet::RemoteImage {
            url: "https://example.com/og.png".into(),
            bytes: Arc::from(&b"\x89PNG\r\n\x1a\n"[..]),
            content_type: "image/png".into(),
        });
        let Some(ChannelUpdate::Patch { updated, .. }) = update.channel else {
            panic!("expected a patch, got {:?}", update.channel);
        };
        let image = updated[0].embeds[0].image().unwrap();
        assert_eq!((image.width, image.height), (1200, 630));
        assert!(image.loaded.is_some());
    }

    #[test]
    fn with_ask_on_a_link_previews_image_waits_for_a_click() {
        let mut live = live();
        let _ = live.set_ask_remote_images(true);
        live.open(ChannelRow::new("c1", "town", ChannelKind::Public));
        let mut post = matty_api::models::Post {
            id: "p1".into(),
            user_id: "ada".into(),
            message: "https://example.com/post".into(),
            ..Default::default()
        };
        post.metadata.embeds.push(matty_api::models::PostEmbed {
            kind: matty_api::models::EmbedKind::Image,
            url: "https://example.com/og.png".into(),
            data: None,
        });
        post.metadata.images.insert(
            "https://example.com/og.png".into(),
            matty_api::models::PostImage {
                width: 1200,
                height: 630,
                ..Default::default()
            },
        );
        live.apply(&ChangeSet::Posts {
            channel_id: "c1".into(),
            posts: Arc::from(vec![post]),
        });
        let url = "https://example.com/og.png";
        assert!(!live.remote_images.asked(url), "nothing fetched yet");
        assert!(live.remote_images.waiting(url));
        live.remote_images.load(url);
        assert!(live.remote_images.asked(url));
        // Back to Always: the next image on screen loads on its own.
        let _ = live.set_ask_remote_images(false);
        live.remote_images.want("https://example.com/other.png");
        assert!(live.remote_images.asked("https://example.com/other.png"));
    }

    #[test]
    fn a_profile_picture_lands_for_the_views_and_a_failure_keeps_initials() {
        let mut live = live();
        let avatars = live.avatars();
        live.apply(&ChangeSet::Users(Arc::from(vec![User {
            id: "ada".into(),
            last_picture_update: 5,
            ..User::default()
        }])));
        assert!(avatars.image("ada").is_none(), "initials until it lands");
        let answer = |user_id: &str, image: Option<&[u8]>| ChangeSet::Picture {
            picture: matty_sync::Picture::Avatar,
            id: user_id.into(),
            version: 5,
            image: image.map(|bytes| (Arc::from(bytes), "image/png".to_owned())),
        };
        let update = live.apply(&answer("ada", Some(b"\x89PNG\r\n\x1a\nface")));
        assert!(update.avatars, "the windows redraw");
        assert!(avatars.image("ada").is_some());
        // No picture, or bytes that are not an image: the initials stay.
        assert!(!live.apply(&answer("bob", None)).avatars);
        let page = ChangeSet::Picture {
            picture: matty_sync::Picture::Avatar,
            id: "eve".into(),
            version: 5,
            image: Some((Arc::from(&b"<html>"[..]), "text/html".to_owned())),
        };
        assert!(!live.apply(&page).avatars);
        assert!(avatars.image("bob").is_none());
        assert!(avatars.image("eve").is_none());
    }

    #[test]
    fn custom_emoji_and_team_icons_land_in_their_own_stores() {
        let mut live = live();
        let update = live.apply(&ChangeSet::CustomEmoji(Arc::from(vec![
            matty_api::models::Emoji {
                id: "e1".into(),
                name: "shipit".into(),
                update_at: 3,
                ..Default::default()
            },
        ])));
        assert!(update.avatars, "views redraw with the new names");
        assert_eq!(live.custom_emoji.id("shipit").as_deref(), Some("e1"));
        assert!(!live.custom_emoji.contains("tada"));
        let png = Some((
            Arc::from(&b"\x89PNG\r\n\x1a\nimg"[..]),
            "image/png".to_owned(),
        ));
        let landed = |picture, id: &str| ChangeSet::Picture {
            picture,
            id: id.into(),
            version: 3,
            image: png.clone(),
        };
        assert!(
            live.apply(&landed(matty_sync::Picture::Emoji, "e1"))
                .avatars
        );
        assert!(live.emoji_images.image("e1").is_some());
        assert!(
            live.apply(&landed(matty_sync::Picture::TeamIcon, "t1"))
                .avatars
        );
        assert!(live.team_icons.image("t1").is_some());
        // Each kind keeps to its own store.
        assert!(live.avatars.image("e1").is_none());
        assert!(live.emoji_images.image("t1").is_none());
    }

    #[test]
    fn an_image_format_comes_from_its_bytes() {
        use gpui_kit::ImageFormat;
        let png = b"\x89PNG\r\n\x1a\n";
        assert_eq!(image_format(png, "image/jpeg"), Some(ImageFormat::Png));
        assert_eq!(
            image_format(&[0xFF, 0xD8, 0xFF, 0xE0], "image/png"),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(image_format(b"??", "image/gif"), Some(ImageFormat::Gif));
        assert_eq!(image_format(b"??", "text/plain"), None);
    }

    #[test]
    fn a_file_on_screen_has_its_posted_name() {
        let mut live = live();
        live.open(ChannelRow::new("c1", "town", ChannelKind::Public));
        let mut post = matty_api::models::Post {
            id: "p1".into(),
            ..Default::default()
        };
        post.metadata.files.push(matty_api::models::FileInfo {
            id: "f1".into(),
            name: "notes.pdf".into(),
            ..Default::default()
        });
        live.apply(&ChangeSet::Posts {
            channel_id: "c1".into(),
            posts: Arc::from(vec![post]),
        });
        assert_eq!(live.file_name("f1").as_deref(), Some("notes.pdf"));
        assert_eq!(live.file_name("f2"), None);
    }

    #[test]
    fn a_full_image_goes_to_the_lightbox_and_is_kept() {
        let mut live = live();
        let update = live.apply(&ChangeSet::Image {
            file_id: "f1".into(),
            size: ImageSize::Preview,
            bytes: Arc::from(&b"\x89PNG...."[..]),
            content_type: "image/jpeg".into(),
        });
        let (id, image) = update.full_image.unwrap();
        assert_eq!(id.as_ref(), "f1");
        assert_eq!(image.format, gpui_kit::ImageFormat::Png);
        assert_eq!(live.full_images(&["f1".into()]).len(), 1);
    }
}
