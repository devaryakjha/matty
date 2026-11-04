//! The app's side of sync: owns the [`SyncHandle`] and turns change sets
//! into the view models the window hands its views. Nothing here draws,
//! blocks, or touches the disk; the database lives on the sync thread.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::SharedString;
use matty_api::models::{Channel, ChannelMember, ChannelStats, User};
use matty_api::{ApiError, Client, ServerCapabilities};
use matty_sync::{
    ChangeSet, Command, Connection, ImageSize, SidebarSection, SyncHandle, TeamWithUnreads,
    UploadState,
};

use crate::channel::{ChannelModel, Post};
use crate::login::{SignInMethods, SsoProvider};
use crate::main_window::ConnectionState;
use crate::profile::UserProfile;
use crate::settings::SavedAccount;
use crate::sidebar::{Category, ChannelKind, ChannelRow, Presence};

/// A running sync engine and what the window last heard from it.
pub struct Live {
    sync: SyncHandle,
    /// The team the sidebar shows: the first in the user's order until
    /// they pick another.
    team: Option<String>,
    teams: Vec<TeamWithUnreads>,
    sections: HashMap<String, Arc<[SidebarSection]>>,
    presence: HashMap<String, Presence>,
    /// The signed-in user's id and username.
    me: (String, String),
    /// What counts as mentioning the user, from their `notify_props`.
    keys: matty_notify::MentionKeys,
    /// The user's own `notify_props`, the whole map, for Preferences to
    /// edit and send back.
    notify_props: HashMap<String, String>,
    /// Everyone whose posts have been on screen.
    users: HashMap<String, User>,
    /// The channel on screen and its latest posts snapshot.
    open: Option<ChannelRow>,
    posts: Option<Arc<[matty_api::models::Post]>>,
    max_post_size: Option<usize>,
    zone: jiff::tz::TimeZone,
    /// The posts the channel view shows now, to send it only what changed.
    shown: Option<Vec<Post>>,
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
    /// A channel sync made ready (joined, created, a new direct message)
    /// that the sidebar has yet to show.
    ready: Option<String>,
    /// The search on screen and its results once they come.
    search: Option<(String, Option<Arc<[matty_api::models::Post]>>)>,
    /// The composer's files, by the tray's id: where each goes and its
    /// server id once up.
    uploads: HashMap<String, UploadJob>,
    /// Image files' thumbnails by file id, and those asked for.
    thumbnails: HashMap<String, Arc<gpui_kit::Image>>,
    asked_thumbnails: std::collections::HashSet<String>,
    /// Full-size images for the lightbox, by file id, once fetched.
    full_images: HashMap<String, Arc<gpui_kit::Image>>,
}

/// One tray file on its way to the server.
#[derive(Clone, Debug)]
pub struct UploadJob {
    pub channel_id: String,
    pub name: String,
    pub source: crate::attachments::UploadSource,
    file_id: Option<String>,
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
}

/// What a change set means for the window.
#[derive(Debug, Default, PartialEq)]
pub struct Update {
    /// The shown team's categories, to replace the sidebar's.
    pub categories: Option<Vec<Category>>,
    /// The shown team's display name, for the sidebar header.
    pub team_name: Option<SharedString>,
    pub connection: Option<ConnectionState>,
    /// What changed in the open channel's view.
    pub channel: Option<ChannelUpdate>,
    /// The open channel's header and counts.
    pub details: Option<Details>,
    /// Who is typing in the open channel now; the window checks again
    /// after [`TYPING_FOR`].
    pub typing: Option<Vec<SharedString>>,
    /// Messages waiting to send across every channel, failed ones left
    /// out, for the connection banner (#195).
    pub queued: Option<usize>,
    /// The user's own presence from the server, with when Do not disturb
    /// ends if the server said (#190).
    pub own_presence: Option<(Presence, Option<i64>)>,
    /// The user's own `notify_props` from the server, for Preferences.
    pub notify_props: Option<HashMap<String, String>>,
    /// The people in the open channel's posts.
    pub profiles: Option<Vec<UserProfile>>,
    /// The shown team's public channels, for Browse channels.
    pub directory: Option<Vec<crate::directory::DirectoryChannel>>,
    /// The shown team's people, for a new direct message.
    pub people: Option<Vec<UserProfile>>,
    /// A channel to switch to: the one the user joined, created or
    /// started, now in the sidebar.
    pub ready: Option<SharedString>,
    /// A join the server refused, for Browse to show as not joined again.
    pub join_failed: Option<SharedString>,
    /// The open channel's mute and notification preferences.
    pub notifications: Option<(bool, crate::channel::ChannelNotifications)>,
    /// The search's results, or its failure.
    pub search: Option<crate::right_pane::SearchModel>,
    /// A file's full-size image, for the lightbox.
    pub full_image: Option<(SharedString, Arc<gpui_kit::Image>)>,
    /// A file saved to the download folder, or the failure.
    pub download: Option<matty_sync::DownloadState>,
    /// A tray file's progress, by the tray's id.
    pub upload: Option<(SharedString, crate::attachments::AttachmentStatus)>,
    /// The open channel's history: whether its first post is loaded,
    /// whether it reaches the newest post, and whether the last page failed.
    pub history: Option<(bool, bool, bool)>,
    /// The session ended; the window returns to sign-in.
    pub signed_out: bool,
    /// The server refused the saved session; the window asks the user to
    /// sign in again.
    pub expired: bool,
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
fn image_format(bytes: &[u8], content_type: &str) -> Option<gpui_kit::ImageFormat> {
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
/// and go on top.
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
    let (head, tail) = new.split_at(remaining.len().min(new.len()));
    if head
        .iter()
        .map(|post| post.id.as_ref())
        .ne(remaining.iter().copied())
    {
        return None;
    }
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
        let media = url::Url::parse(&account.server)
            .ok()
            .and_then(|server| matty_sync::media_dir(&server, &account.user_id));
        let sync = matty_sync::spawn_at(path, media)?;
        let changes = sync.subscribe();
        Ok((
            Self {
                sync,
                team: None,
                teams: Vec::new(),
                sections: HashMap::new(),
                presence: HashMap::new(),
                me: (account.user_id.clone(), account.username.clone()),
                keys: matty_notify::MentionKeys::username(account.username.clone()),
                notify_props: HashMap::new(),
                users: HashMap::new(),
                open: None,
                posts: None,
                max_post_size: None,
                zone: jiff::tz::TimeZone::system(),
                shown: None,
                failed: Default::default(),
                queued: HashMap::new(),
                channel: None,
                member: None,
                stats: None,
                typing: Vec::new(),
                ready: None,
                search: None,
                uploads: HashMap::new(),
                thumbnails: HashMap::new(),
                asked_thumbnails: std::collections::HashSet::new(),
                full_images: HashMap::new(),
            },
            changes,
        ))
    }

    pub fn send(&self, command: Command) {
        self.sync.send(command);
    }

    /// Remembers a tray file so its progress reaches the tray.
    pub fn track_upload(&mut self, id: &str, job: UploadJob) {
        self.uploads.insert(id.to_owned(), job);
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

    /// The server ids for the tray files a post carries; they leave the
    /// tray with it.
    pub fn take_file_ids(&mut self, ids: &[SharedString]) -> Vec<String> {
        ids.iter()
            .filter_map(|id| self.uploads.remove(id.as_ref())?.file_id)
            .collect()
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
        self.shown = None;
        self.failed.clear();
        self.channel = None;
        self.member = None;
        self.stats = None;
        self.typing.clear();
        self.open = Some(row);
        self.channel_model().expect("a channel is open")
    }

    /// The open channel's view change for the latest posts: the first
    /// snapshot replaces the model, later ones patch it.
    fn channel_update(&mut self) -> Option<ChannelUpdate> {
        let model = self.channel_model()?;
        if model.loading {
            return None;
        }
        self.ask_thumbnails(&model.posts);
        let update = self
            .shown
            .as_deref()
            .and_then(|old| patch(old, &model.posts));
        self.shown = Some(model.posts.clone());
        Some(update.unwrap_or(ChannelUpdate::Replace(model)))
    }

    /// Takes the user's own `notify_props` from `user`. Other fetches of
    /// the user leave the map out, so an empty one changes nothing.
    fn take_notify_props(&mut self, user: &User, update: &mut Update) {
        if user.id == self.me.0 && !user.notify_props.is_empty() {
            self.keys = matty_notify::MentionKeys::from_props(
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
            keys: &self.keys,
            zone: &self.zone,
            now: jiff::Timestamp::now(),
        };
        let mut model = crate::transcript::channel_model(
            row,
            self.posts.as_deref(),
            &self.users,
            &viewer,
            self.max_post_size,
        );
        let details = self.details(model.pinned);
        model.header = details.header;
        model.purpose = details.purpose;
        model.members = details.members;
        model.pinned = details.pinned;
        model.typing = self.typing_names();
        if let Some(member) = &self.member {
            (model.muted, model.notifications) = notifications(member);
        }
        model.profiles = self.profiles();
        for post in &mut model.posts {
            if self.failed.contains(post.id.as_ref()) {
                post.status = crate::channel::PostStatus::Failed;
            }
        }
        for file in model.posts.iter_mut().flat_map(|post| &mut post.files) {
            if let Some(image) = self.thumbnails.get(file.id.as_ref()) {
                file.preview = Some(crate::attachments::UploadSource::Image(image.clone()));
            }
        }
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

    /// Asks sync for the thumbnails of image files not yet asked for.
    fn ask_thumbnails(&mut self, posts: &[Post]) {
        for file in posts.iter().flat_map(|post| &post.files) {
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
        }
    }

    fn typing_names(&self) -> Vec<SharedString> {
        self.typing
            .iter()
            .map(|(user_id, _)| {
                self.users
                    .get(user_id)
                    .map(crate::transcript::display_name)
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

    /// Drops typing notices older than [`TYPING_FOR`]; the names left when
    /// any went.
    pub fn expire_typing(&mut self, now: Instant) -> Option<Vec<SharedString>> {
        let before = self.typing.len();
        self.typing
            .retain(|(_, since)| now.saturating_duration_since(*since) < TYPING_FOR);
        (self.typing.len() != before).then(|| self.typing_names())
    }

    /// Searches the shown team; the results come as [`Update::search`].
    pub fn search(&mut self, terms: &str) {
        let Some(team_id) = self.team.clone() else {
            return;
        };
        let offset = self.zone.to_offset(jiff::Timestamp::now()).seconds();
        self.search = Some((terms.to_owned(), None));
        self.send(Command::Search {
            team_id,
            terms: terms.to_owned(),
            time_zone_offset: offset,
        });
    }

    /// The search model for what has come back so far.
    fn search_model(&self) -> Option<crate::right_pane::SearchModel> {
        let (terms, posts) = self.search.as_ref()?;
        let viewer = crate::transcript::Viewer {
            user_id: &self.me.0,
            keys: &self.keys,
            zone: &self.zone,
            now: jiff::Timestamp::now(),
        };
        Some(crate::right_pane::SearchModel {
            query: terms.clone().into(),
            searching: posts.is_none(),
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
            .map(|channel| channel.display_name.clone().into())
    }

    /// The team on screen.
    pub fn team(&self) -> Option<&str> {
        self.team.as_deref()
    }

    fn profile(&self, user: &User) -> UserProfile {
        UserProfile {
            user_id: user.id.clone().into(),
            username: user.username.clone().into(),
            display_name: crate::transcript::display_name(user).into(),
            position: None,
            presence: self
                .presence
                .get(&user.id)
                .copied()
                .unwrap_or(Presence::Offline),
            status_text: None,
            local_time: None,
        }
    }

    /// The ready channel's id once its row is in the shown sidebar.
    fn take_ready(&mut self) -> Option<SharedString> {
        let id = self.ready.as_deref()?;
        self.row(id)?;
        self.ready.take().map(Into::into)
    }

    pub fn is_typing(&self) -> bool {
        !self.typing.is_empty()
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
        match change {
            ChangeSet::Sidebar { team_id, sections } => {
                // Unreads, which comes in the same batch, picks the team.
                self.sections.insert(team_id.clone(), sections.clone());
                if self.team.as_ref() == Some(team_id) {
                    update.categories = Some(self.categories(sections));
                    update.ready = self.take_ready();
                }
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
                    self.team = team;
                    update.categories = self.shown_categories();
                }
                update.team_name = self.team_name();
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
            ChangeSet::Download { state, .. } => update.download = Some(state.clone()),
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
            ChangeSet::Upload { upload_id, state } => {
                if let Some(job) = self.uploads.get_mut(upload_id) {
                    use crate::attachments::AttachmentStatus;
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
            ChangeSet::Search { terms, posts } => {
                if let Some((asked, results)) = self.search.as_mut()
                    && asked == terms
                {
                    *results = Some(posts.clone());
                    update.search = self.search_model();
                }
            }
            ChangeSet::Failed(matty_sync::CommandError::Server {
                action: "search", ..
            }) => {
                // Shown as no results rather than searching forever.
                if let Some((_, results)) = self.search.as_mut() {
                    *results = Some(Arc::from(Vec::new()));
                    update.search = self.search_model();
                }
            }
            ChangeSet::Channel { channel, member } if self.is_open(&channel.id) => {
                self.channel = Some(channel.clone());
                update.details = self.shown_details();
                if let Some(member) = member {
                    self.member = Some(member.clone());
                    if self.shown.is_some() {
                        update.notifications = Some(notifications(member));
                    }
                }
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
            ChangeSet::Me(user) => {
                self.take_notify_props(user, &mut update);
                if self.posts.is_some() {
                    update.channel = self.channel_update();
                }
            }
            ChangeSet::Users(users) => {
                for user in users.iter() {
                    self.users.insert(user.id.clone(), user.clone());
                    self.take_notify_props(user, &mut update);
                }
                if self
                    .search
                    .as_ref()
                    .is_some_and(|(_, posts)| posts.is_some())
                {
                    update.search = self.search_model();
                }
                if self.posts.is_some() {
                    update.channel = self.channel_update();
                    update.profiles = Some(self.profiles());
                    if !self.typing.is_empty() {
                        update.typing = Some(self.typing_names());
                    }
                }
            }
            ChangeSet::Config(capabilities) => {
                self.max_post_size = Some(capabilities.max_post_size);
            }
            ChangeSet::Connection(connection) => match connection {
                Connection::Socket(matty_api::ws::ConnectionState::Unauthorized)
                | Connection::Expired => {
                    update.expired = true;
                    update.connection = Some(ConnectionState::Offline);
                }
                Connection::Socket(state) => update.connection = Some(connection_state(state)),
                Connection::Failed { .. } => update.connection = Some(ConnectionState::Offline),
                Connection::SignedOut => update.signed_out = true,
                _ => {}
            },
            _ => {}
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
                            ChannelRow::new(channel.id.clone(), channel.display_name.clone(), kind);
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

fn connection_state(state: &matty_api::ws::ConnectionState) -> ConnectionState {
    use matty_api::ws::ConnectionState as Ws;
    match state {
        Ws::Connecting | Ws::Connected { .. } => ConnectionState::Online,
        Ws::Reconnecting { attempt, .. } => ConnectionState::Reconnecting { attempt: *attempt },
        Ws::Unauthorized | Ws::Stopped => ConnectionState::Offline,
    }
}

/// Checks `url` and reads how its server lets people sign in.
pub async fn check_server(url: String) -> Result<ServerCapabilities, String> {
    let answer = matty_sync::io::run(async move {
        let client = Client::new(&url)?;
        client.capabilities().await
    });
    match answer.recv().await {
        Ok(Ok(capabilities)) => Ok(capabilities),
        Ok(Err(error)) => Err(server_error(&error)),
        Err(_) => Err(STOPPED.into()),
    }
}

/// A signed-in session, ready to hand to sync.
pub struct Session {
    pub client: Client,
    pub user: User,
}

/// Signs in to `url` with a login id and password.
pub async fn sign_in(url: String, login_id: String, password: String) -> Result<Session, String> {
    let answer = matty_sync::io::run(async move {
        let credentials = matty_api::Credentials {
            login_id,
            password,
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
        Err(_) => Err(STOPPED.into()),
    }
}

/// Signs in with a personal access token.
pub async fn sign_in_with_token(url: String, token: String) -> Result<Session, String> {
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
        Err(_) => Err(STOPPED.into()),
    }
}

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
pub fn saved_account(server: &str, user: &User) -> SavedAccount {
    let display_name = crate::transcript::display_name(user);
    SavedAccount {
        server: server.to_string(),
        user_id: user.id.clone(),
        username: user.username.clone(),
        display_name,
    }
}

/// Where `account`'s database lives: one file per server and user.
pub fn database_path(account: &SavedAccount) -> PathBuf {
    url::Url::parse(&account.server)
        .ok()
        .and_then(|server| matty_sync::database_path(&server, &account.user_id))
        .unwrap_or_else(|| std::env::temp_dir().join(format!("matty-{}.sqlite", account.user_id)))
}

fn server_error(error: &ApiError) -> String {
    match error {
        ApiError::Server { status: 404, .. } => {
            "That address answered, but it is not a Mattermost server.".into()
        }
        ApiError::Server { .. } | ApiError::Decode(_) => {
            "That address answered, but not like a Mattermost server.".into()
        }
        _ => "Matty could not reach that server. Check the address and your connection.".into(),
    }
}

fn sign_in_error(error: &ApiError) -> String {
    match error {
        ApiError::MfaRequired => {
            "This account uses multi-factor authentication, which Matty does not support yet."
                .into()
        }
        ApiError::Server { status: 401, .. } => {
            "That login or password is not right. Check them and try again.".into()
        }
        ApiError::Server { message, .. } if !message.is_empty() => message.clone(),
        _ => "Matty could not reach the server. Check your connection and try again.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use matty_api::models::Status;
    use matty_sync::{SidebarChannel, Unreads};

    fn channel(id: &str, kind: &str, messages: i64, mentions: i64) -> SidebarChannel {
        SidebarChannel {
            id: id.into(),
            kind: kind.into(),
            display_name: id.into(),
            dm_user_id: (kind == "D").then(|| format!("{id}-user")),
            unreads: Unreads {
                messages,
                mentions,
                ..Unreads::default()
            },
            muted: false,
            last_post_at: 0,
        }
    }

    fn section(id: &str, channels: Vec<SidebarChannel>) -> SidebarSection {
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

    fn team(id: &str) -> TeamWithUnreads {
        TeamWithUnreads {
            id: id.into(),
            name: id.into(),
            display_name: format!("Team {id}"),
            has_unreads: false,
            mentions: 0,
        }
    }

    fn live() -> Live {
        let dir = tempfile::tempdir().unwrap();
        let account = SavedAccount {
            server: "http://localhost".into(),
            user_id: "me".into(),
            username: "me".into(),
            display_name: "Me".into(),
        };
        let live = Live::start(dir.path().join("account.sqlite"), &account)
            .unwrap()
            .0;
        // The engine may still be opening the file; let the folder outlive it.
        std::mem::forget(dir);
        live
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
        assert!(live.keys.mentioned("the deploy is out"));
        // A later fetch without the map keeps the keywords.
        let bare = User {
            notify_props: Default::default(),
            ..me
        };
        let update = live.apply(&ChangeSet::Users(Arc::from(vec![bare])));
        assert_eq!(update.notify_props, None);
        assert!(live.keys.mentioned("the deploy is out"));
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
        assert!(
            live.apply(&ChangeSet::Connection(Connection::SignedOut))
                .signed_out
        );
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
            id: id.into(),
            author: "a".into(),
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
            files: vec![],
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
    fn a_post_between_shown_ones_resets() {
        let old = [shown("a", "one"), shown("c", "three")];
        let new = [shown("a", "one"), shown("b", "two"), shown("c", "three")];
        assert_eq!(patch(&old, &new), None);
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
        assert_eq!(live.expire_typing(Instant::now()), None);
        assert_eq!(
            live.expire_typing(Instant::now() + TYPING_FOR),
            Some(Vec::new())
        );
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
            terms: "lunc".into(),
            posts: Arc::from(vec![hit("p0", "town")]),
        });
        assert_eq!(stale.search, None);
        let model = live
            .apply(&ChangeSet::Search {
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
        assert_eq!(live.take_file_ids(&["upload-1".into()]), vec!["f1"]);
        // It left with the post; a late report changes nothing.
        let update = live.apply(&ChangeSet::Upload {
            upload_id: "upload-1".into(),
            state: UploadState::Failed("gone".into()),
        });
        assert_eq!(update.upload, None);
    }

    #[test]
    fn a_removed_upload_is_forgotten() {
        use crate::attachments::UploadSource;
        let mut live = live();
        let job = UploadJob::new("c1", "notes.txt", UploadSource::Path("notes.txt".into()));
        live.track_upload("upload-1", job);
        live.cancel_upload("upload-1");
        assert!(live.upload_job("upload-1").is_none());
        assert!(live.take_file_ids(&["upload-1".into()]).is_empty());
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
