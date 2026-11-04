//! The loop on the sync thread: apply what arrives to the store in batches,
//! then publish what changed.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use matty_api::capabilities::CollapsedThreads;
use matty_api::models::{Draft, PostChange, Preference, Reaction};
use matty_api::ws::{ConnectionState, WsClient, WsEvent, WsSignal};
use matty_api::{Client, ServerCapabilities};
use matty_store::posts::{self, Window};
use matty_store::sidebar::{self, UnreadMode};
use matty_store::{Store, StoreError};
use rusqlite::Connection as Db;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::change::{
    ChangeSet, CommandError, Connection, DownloadState, ImageSize, POSTS_WINDOW, UploadState,
};
use crate::commands::{self, Write};
use crate::events::{self, Fetch};
use crate::load::{self, Loaded};
use crate::outbox;
use crate::typing::TypingThrottle;
use matty_store::outbox::{self as queue, Outgoing};

/// Followed threads in a team's Threads list snapshot.
const THREAD_LIST: usize = 200;
/// A `since` response bigger than this reloads the channel instead (#37).
const MAX_SINCE_BYTES: usize = 2 * 1024 * 1024;

/// Most messages applied in one transaction, so a flood still publishes.
const BATCH: usize = 512;

/// Where the signed-in user's id is kept, to publish from cache at launch.
const ME_KEY: &str = "me";
/// Whether Collapsed Reply Threads was on at the last load.
const CRT_KEY: &str = "collapsed_threads";

/// What the app asks of the engine.
#[derive(Clone)]
#[non_exhaustive]
pub enum Command {
    /// Signed in: publish what the cache has, then load the account from
    /// the server with this client (it carries the session token).
    Connect {
        /// The signed-in client.
        client: Client,
    },
    /// Publishes what the cache has without touching the network, as
    /// `matty --offline` and the launch before a session is restored do.
    ShowCache,
    /// Signs out: the session ends here at once, then on the server.
    /// [`Connection::SignedOut`](crate::Connection::SignedOut) follows.
    SignOut,
    /// A decoded server event to apply, from the WebSocket.
    Event(Box<WsEvent>),
    /// A channel is on screen: publish its posts now and whenever they
    /// change, and fetch them once per connection.
    OpenChannel {
        /// The channel.
        channel_id: String,
    },
    /// Loads the page of posts before the oldest one the open channel's
    /// snapshot holds; [`ChangeSet::History`](crate::ChangeSet::History)
    /// answers and the next posts snapshot reaches back that far.
    LoadOlder {
        /// The channel.
        channel_id: String,
    },
    /// Loads the page after the newest post in the open channel's
    /// snapshot, while it stops short of the newest post (see
    /// [`Command::LoadAround`]).
    LoadNewer {
        /// The channel.
        channel_id: String,
    },
    /// Shows the posts around `post_id`, such as a search result's,
    /// fetching them when they are not cached: the snapshot becomes the
    /// post with a page on each side, and the older and newer pages extend
    /// it. With collapsed threads a reply is shown by its root.
    LoadAround {
        /// The channel.
        channel_id: String,
        /// The post.
        post_id: String,
    },
    /// Returns the open channel's snapshot to its newest posts.
    LoadLatest {
        /// The channel.
        channel_id: String,
    },
    /// The channel left the screen.
    CloseChannel {
        /// The channel.
        channel_id: String,
    },
    /// A thread is on screen: publish it now and whenever it changes.
    OpenThread {
        /// The root post's id.
        root_id: String,
    },
    /// The thread left the screen.
    CloseThread {
        /// The root post's id.
        root_id: String,
    },
    /// The Threads view of a team is on screen: publish its list now and
    /// whenever it changes.
    OpenThreadList {
        /// The team.
        team_id: String,
    },
    /// The Threads view left the screen.
    CloseThreadList {
        /// The team.
        team_id: String,
    },
    /// Catch up with the server now, as after a reconnect: memberships,
    /// each cached channel since its cursor, and thread lists.
    Resync,
    /// Sends a post: an optimistic row shows at once, the outbox keeps the
    /// send until the server has it, and a failure keeps the text for
    /// [`Command::Retry`] (#38).
    SendPost {
        /// The client's id for the post; empty to have one made
        /// (`{user_id}:{millis}`, as Mattermost clients do).
        pending_post_id: String,
        /// Channel.
        channel_id: String,
        /// Thread root for a reply, or empty.
        root_id: String,
        /// Message text.
        message: String,
        /// Uploaded file ids.
        file_ids: Vec<String>,
    },
    /// Sends a failed post again.
    Retry {
        /// Its pending id.
        pending_post_id: String,
    },
    /// Drops an unsent post.
    Discard {
        /// Its pending id.
        pending_post_id: String,
    },
    /// Changes a sent post's text. A post still in the outbox is refused
    /// with [`CommandError::NotSentYet`](crate::CommandError::NotSentYet).
    EditPost {
        /// The post.
        post_id: String,
        /// The new text.
        message: String,
    },
    /// Deletes a sent post; refused like [`Command::EditPost`] while unsent.
    DeletePost {
        /// The post.
        post_id: String,
    },
    /// The user opened a channel, leaving `prev_channel_id` (or empty): both
    /// read at once here, then on the server.
    ViewChannel {
        /// The channel now on screen.
        channel_id: String,
        /// The channel left, or empty.
        prev_channel_id: String,
    },
    /// Marks the channel (or, with collapsed threads, the thread) unread
    /// from this post.
    MarkUnread {
        /// The first post to show as unread.
        post_id: String,
    },
    /// Adds the user's reaction, or removes it when they already reacted
    /// with this emoji. Shown at once; undone if the server refuses.
    ToggleReaction {
        /// The post.
        post_id: String,
        /// Emoji name without colons, such as `+1`.
        emoji_name: String,
    },
    /// Keeps what the user typed but has not sent. Always stored here; also
    /// synced to the server when it allows synced drafts. An empty message
    /// with no files deletes the draft.
    SaveDraft {
        /// Channel.
        channel_id: String,
        /// Thread root for a reply draft, or empty.
        root_id: String,
        /// Draft text.
        message: String,
        /// Uploaded file ids.
        file_ids: Vec<String>,
    },
    /// The user is typing; sent at most once per five seconds per place.
    Typing {
        /// Channel.
        channel_id: String,
        /// Thread root when typing a reply, or empty.
        parent_id: String,
    },
    /// Sets the user's status: `online`, `away`, `dnd` or `offline`.
    SetStatus {
        /// The status.
        status: String,
        /// For `dnd` with a duration: when it ends, in seconds since the epoch.
        dnd_end_time: Option<i64>,
    },
    /// Opens the direct message with one user, or the group message with
    /// several (3 to 8 people counting the user), creating it if needed.
    /// [`ChangeSet::ChannelReady`](crate::ChangeSet::ChannelReady) follows.
    OpenDirect {
        /// The other people.
        user_ids: Vec<String>,
    },
    /// Joins a public channel;
    /// [`ChangeSet::ChannelReady`](crate::ChangeSet::ChannelReady) follows.
    JoinChannel {
        /// The channel.
        channel_id: String,
    },
    /// Leaves a channel; it drops out of the sidebar once the server agrees.
    LeaveChannel {
        /// The channel.
        channel_id: String,
    },
    /// Lists the team's public channels with their member counts, for
    /// Browse channels; [`ChangeSet::Directory`](crate::ChangeSet::Directory)
    /// answers.
    Browse {
        /// The team.
        team_id: String,
    },
    /// Lists the team's active people, for starting a direct message;
    /// [`ChangeSet::People`](crate::ChangeSet::People) answers.
    FindPeople {
        /// The team.
        team_id: String,
    },
    /// Moves a channel into or out of the team's Favorites, by the
    /// `favorite_channel` preference the server turns into the move.
    SetFavorite {
        /// The channel.
        channel_id: String,
        /// In Favorites, or out.
        favorite: bool,
    },
    /// Hides a direct or group conversation from the sidebar until it is
    /// opened again; its posts stay on the server.
    CloseDirect {
        /// The conversation.
        channel_id: String,
    },
    /// Uploads a file into a channel for the next post; progress, the
    /// server's file id or the failure come back in
    /// [`ChangeSet::Upload`](crate::ChangeSet::Upload).
    Upload {
        /// The caller's id for the upload, echoed in the change sets.
        upload_id: String,
        /// The channel.
        channel_id: String,
        /// The file name the server stores.
        file_name: String,
        /// The contents.
        bytes: Vec<u8>,
    },
    /// Fetches one of a file's images, from the media cache or the server;
    /// [`ChangeSet::Image`](crate::ChangeSet::Image) answers. A file
    /// without one gets no answer.
    LoadImage {
        /// The file.
        file_id: String,
        /// Which rendition.
        size: ImageSize,
    },
    /// Saves a file into `folder` under `name`, numbered when the name is
    /// taken; [`ChangeSet::Download`](crate::ChangeSet::Download) answers.
    Download {
        /// The file.
        file_id: String,
        /// The folder to save into.
        folder: std::path::PathBuf,
        /// The name it was posted with.
        name: String,
    },
    /// Stops an upload that is still sending.
    CancelUpload {
        /// The caller's id for it.
        upload_id: String,
    },
    /// Renames a channel or changes its header or purpose; `None` leaves
    /// a field alone. The channel comes back in
    /// [`ChangeSet::Channel`](crate::ChangeSet::Channel).
    PatchChannel {
        /// The channel.
        channel_id: String,
        /// The new name people see.
        display_name: Option<String>,
        /// The new header.
        header: Option<String>,
        /// The new purpose.
        purpose: Option<String>,
    },
    /// Changes these of the user's notification properties for a channel
    /// (`desktop`, `push`, `mark_unread`, `ignore_channel_mentions`).
    SetChannelNotifications {
        /// The channel.
        channel_id: String,
        /// The properties to change.
        props: std::collections::HashMap<String, String>,
    },
    /// Adds people to a channel; the counts come back in
    /// [`ChangeSet::Stats`](crate::ChangeSet::Stats).
    AddMembers {
        /// The channel.
        channel_id: String,
        /// The people.
        user_ids: Vec<String>,
    },
    /// Searches the team's messages;
    /// [`ChangeSet::Search`](crate::ChangeSet::Search) answers.
    Search {
        /// The team.
        team_id: String,
        /// What the user typed, modifiers included.
        terms: String,
        /// The user's offset from UTC in seconds, for `on:` and the like.
        time_zone_offset: i32,
    },
    /// Creates a channel the user is then in;
    /// [`ChangeSet::ChannelReady`](crate::ChangeSet::ChannelReady) follows.
    CreateChannel {
        /// The team.
        team_id: String,
        /// The URL handle.
        name: String,
        /// The name people see.
        display_name: String,
        /// What it is for; may be empty.
        purpose: String,
        /// Private instead of public.
        private: bool,
    },
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect { .. } => f.write_str("Connect"),
            Self::ShowCache => f.write_str("ShowCache"),
            Self::SignOut => f.write_str("SignOut"),
            Self::Event(event) => f.debug_tuple("Event").field(event).finish(),
            Self::SetFavorite {
                channel_id,
                favorite,
            } => f
                .debug_struct("SetFavorite")
                .field("channel_id", channel_id)
                .field("favorite", favorite)
                .finish(),
            Self::CloseDirect { channel_id } => f
                .debug_struct("CloseDirect")
                .field("channel_id", channel_id)
                .finish(),
            Self::OpenChannel { channel_id } => f
                .debug_struct("OpenChannel")
                .field("channel_id", channel_id)
                .finish(),
            Self::LoadOlder { channel_id } => f
                .debug_struct("LoadOlder")
                .field("channel_id", channel_id)
                .finish(),
            Self::LoadNewer { channel_id } => f
                .debug_struct("LoadNewer")
                .field("channel_id", channel_id)
                .finish(),
            Self::LoadAround {
                channel_id,
                post_id,
            } => f
                .debug_struct("LoadAround")
                .field("channel_id", channel_id)
                .field("post_id", post_id)
                .finish(),
            Self::LoadLatest { channel_id } => f
                .debug_struct("LoadLatest")
                .field("channel_id", channel_id)
                .finish(),
            Self::CloseChannel { channel_id } => f
                .debug_struct("CloseChannel")
                .field("channel_id", channel_id)
                .finish(),
            Self::OpenThread { root_id } => f
                .debug_struct("OpenThread")
                .field("root_id", root_id)
                .finish(),
            Self::CloseThread { root_id } => f
                .debug_struct("CloseThread")
                .field("root_id", root_id)
                .finish(),
            Self::OpenThreadList { team_id } => f
                .debug_struct("OpenThreadList")
                .field("team_id", team_id)
                .finish(),
            Self::CloseThreadList { team_id } => f
                .debug_struct("CloseThreadList")
                .field("team_id", team_id)
                .finish(),
            Self::Resync => f.write_str("Resync"),
            Self::SendPost {
                pending_post_id,
                channel_id,
                root_id,
                ..
            } => f
                .debug_struct("SendPost")
                .field("pending_post_id", pending_post_id)
                .field("channel_id", channel_id)
                .field("root_id", root_id)
                .finish_non_exhaustive(),
            Self::Retry { pending_post_id } => f
                .debug_struct("Retry")
                .field("pending_post_id", pending_post_id)
                .finish(),
            Self::Discard { pending_post_id } => f
                .debug_struct("Discard")
                .field("pending_post_id", pending_post_id)
                .finish(),
            Self::EditPost { post_id, .. } => f
                .debug_struct("EditPost")
                .field("post_id", post_id)
                .finish_non_exhaustive(),
            Self::DeletePost { post_id } => f
                .debug_struct("DeletePost")
                .field("post_id", post_id)
                .finish(),
            Self::ViewChannel {
                channel_id,
                prev_channel_id,
            } => f
                .debug_struct("ViewChannel")
                .field("channel_id", channel_id)
                .field("prev_channel_id", prev_channel_id)
                .finish(),
            Self::MarkUnread { post_id } => f
                .debug_struct("MarkUnread")
                .field("post_id", post_id)
                .finish(),
            Self::ToggleReaction {
                post_id,
                emoji_name,
            } => f
                .debug_struct("ToggleReaction")
                .field("post_id", post_id)
                .field("emoji_name", emoji_name)
                .finish(),
            Self::SaveDraft {
                channel_id,
                root_id,
                ..
            } => f
                .debug_struct("SaveDraft")
                .field("channel_id", channel_id)
                .field("root_id", root_id)
                .finish_non_exhaustive(),
            Self::Typing {
                channel_id,
                parent_id,
            } => f
                .debug_struct("Typing")
                .field("channel_id", channel_id)
                .field("parent_id", parent_id)
                .finish(),
            Self::SetStatus { status, .. } => f
                .debug_struct("SetStatus")
                .field("status", status)
                .finish_non_exhaustive(),
            Self::OpenDirect { user_ids } => f
                .debug_struct("OpenDirect")
                .field("user_ids", user_ids)
                .finish(),
            Self::JoinChannel { channel_id } => f
                .debug_struct("JoinChannel")
                .field("channel_id", channel_id)
                .finish(),
            Self::LeaveChannel { channel_id } => f
                .debug_struct("LeaveChannel")
                .field("channel_id", channel_id)
                .finish(),
            Self::Browse { team_id } => f.debug_struct("Browse").field("team_id", team_id).finish(),
            Self::Upload {
                upload_id,
                channel_id,
                file_name,
                bytes,
            } => f
                .debug_struct("Upload")
                .field("upload_id", upload_id)
                .field("channel_id", channel_id)
                .field("file_name", file_name)
                .field("bytes", &bytes.len())
                .finish(),
            Self::LoadImage { file_id, size } => f
                .debug_struct("LoadImage")
                .field("file_id", file_id)
                .field("size", size)
                .finish(),
            Self::Download { file_id, .. } => f
                .debug_struct("Download")
                .field("file_id", file_id)
                .finish_non_exhaustive(),
            Self::CancelUpload { upload_id } => f
                .debug_struct("CancelUpload")
                .field("upload_id", upload_id)
                .finish(),
            Self::PatchChannel { channel_id, .. } => f
                .debug_struct("PatchChannel")
                .field("channel_id", channel_id)
                .finish_non_exhaustive(),
            Self::SetChannelNotifications { channel_id, props } => f
                .debug_struct("SetChannelNotifications")
                .field("channel_id", channel_id)
                .field("props", props)
                .finish(),
            Self::AddMembers {
                channel_id,
                user_ids,
            } => f
                .debug_struct("AddMembers")
                .field("channel_id", channel_id)
                .field("user_ids", user_ids)
                .finish(),
            Self::Search { team_id, .. } => f
                .debug_struct("Search")
                .field("team_id", team_id)
                .finish_non_exhaustive(),
            Self::FindPeople { team_id } => f
                .debug_struct("FindPeople")
                .field("team_id", team_id)
                .finish(),
            Self::CreateChannel { team_id, name, .. } => f
                .debug_struct("CreateChannel")
                .field("team_id", team_id)
                .field("name", name)
                .finish_non_exhaustive(),
        }
    }
}

pub(crate) enum Message {
    Command(Command),
    Subscribe(async_channel::Sender<ChangeSet>),
}

/// What a subscriber has of an open channel: its `update_at` and the
/// user's notification properties for it.
type SentChannel = (i64, Option<std::collections::HashMap<String, String>>);

/// Search results asked for at once.
const SEARCH_PAGE: u32 = 60;

/// What a batch touched, so each view gets one fresh snapshot.
#[derive(Default)]
pub(crate) struct Dirty {
    /// Channels whose posts changed.
    pub(crate) channels: BTreeSet<String>,
    /// Thread roots whose thread changed.
    pub(crate) threads: BTreeSet<String>,
    /// Teams whose Threads list changed.
    pub(crate) thread_lists: BTreeSet<String>,
    /// Channels whose outbox changed.
    pub(crate) outbox: BTreeSet<String>,
    /// Teams whose sidebar changed.
    pub(crate) teams: BTreeSet<String>,
    /// Every team's sidebar changed (a direct message, a user's name).
    pub(crate) all_teams: bool,
    /// The team rail's badges changed.
    pub(crate) unreads: bool,
    /// Preferences changed.
    pub(crate) preferences: bool,
    /// The cache lost track of the server and must catch up (#37).
    pub(crate) resync: bool,
    /// Change sets that carry their own data.
    pub(crate) direct: Vec<ChangeSet>,
}

/// The signed-in session the engine loads with.
struct Session {
    client: Client,
    /// Empty until the account has loaded.
    user_id: String,
    collapsed_threads: bool,
    /// Open channels fetched over this session.
    fetched: BTreeSet<String>,
    load: JoinHandle<()>,
    /// The WebSocket; dropping it closes the connection.
    socket: WsClient,
    /// The socket has been connected before, so the next connect is a
    /// reconnect that needs a catch-up.
    was_connected: bool,
    /// False once a socket that was connected drops; queued sends wait
    /// for it to come back. Before the first connect sends go over REST
    /// as soon as they are written.
    online: bool,
    /// A resync is running; another request waits for it.
    resyncing: bool,
    resync_again: bool,
}

pub(crate) struct Engine {
    store: Store,
    /// The signed-in user, from the cache until the account loads.
    me: Option<String>,
    capabilities: Option<ServerCapabilities>,
    open_channels: BTreeSet<String>,
    open_threads: BTreeSet<String>,
    open_thread_lists: BTreeSet<String>,
    subscribers: Vec<async_channel::Sender<ChangeSet>>,
    /// Authors already sent in [`ChangeSet::Users`], and those asked of
    /// the server, so neither repeats.
    sent_users: BTreeSet<String>,
    asked_users: BTreeSet<String>,
    /// Open channels sent in [`ChangeSet::Channel`], by their `update_at`,
    /// so a posts change does not repeat them.
    sent_channels: std::collections::BTreeMap<String, SentChannel>,
    session: Option<Session>,
    loaded: mpsc::UnboundedSender<Loaded>,
    loaded_inbox: Option<mpsc::UnboundedReceiver<Loaded>>,
    typing: TypingThrottle,
    /// Uploads still sending, by the caller's id; aborting one cancels it.
    uploads: std::collections::HashMap<String, JoinHandle<()>>,
    /// Where images are kept between runs; without it they are fetched
    /// each time.
    media: Option<matty_store::media::MediaCache>,
    /// How far back each open channel's snapshot reaches beyond the
    /// latest [`POSTS_WINDOW`] posts.
    history: std::collections::HashMap<String, History>,
    /// Per channel, the queued send on its way. A channel sends one at a
    /// time so its posts reach the server in order (#194).
    sending: std::collections::HashMap<String, String>,
    /// Per channel, the creation time at or before which stored posts are
    /// left over from a jump into the past that was given up (Jump to
    /// latest, or leaving the channel) and may not join the latest posts;
    /// the latest snapshot leaves them out until older pages reach them.
    floors: std::collections::HashMap<String, i64>,
    /// Thumbnails on their way from the server.
    fetching: BTreeSet<String>,
}

impl Engine {
    pub(crate) fn new(store: Store) -> Self {
        let (loaded, loaded_inbox) = mpsc::unbounded_channel();
        let me = posts::cursor(store.conn(), ME_KEY).ok().flatten();
        Self {
            store,
            me,
            capabilities: None,
            open_channels: BTreeSet::new(),
            open_threads: BTreeSet::new(),
            open_thread_lists: BTreeSet::new(),
            subscribers: Vec::new(),
            sent_users: BTreeSet::new(),
            asked_users: BTreeSet::new(),
            sent_channels: std::collections::BTreeMap::new(),
            session: None,
            loaded,
            loaded_inbox: Some(loaded_inbox),
            typing: TypingThrottle::default(),
            uploads: std::collections::HashMap::new(),
            media: None,
            fetching: BTreeSet::new(),
            history: std::collections::HashMap::new(),
            sending: std::collections::HashMap::new(),
            floors: std::collections::HashMap::new(),
        }
    }

    pub(crate) fn with_media(mut self, media: Option<matty_store::media::MediaCache>) -> Self {
        self.media = media;
        self
    }

    /// Runs until every [`SyncHandle`](crate::SyncHandle) is dropped.
    pub(crate) async fn run(mut self, mut inbox: mpsc::UnboundedReceiver<Message>) {
        let Some(mut loaded_inbox) = self.loaded_inbox.take() else {
            return;
        };
        let mut batch = Vec::with_capacity(BATCH);
        let mut results = Vec::new();
        loop {
            let mut dirty = Dirty::default();
            tokio::select! {
                biased;
                count = inbox.recv_many(&mut batch, BATCH) => {
                    if count == 0 {
                        break;
                    }
                    if let Err(error) = self.apply(batch.drain(..), &mut dirty) {
                        tracing::error!(%error, "sync batch failed");
                    }
                }
                // The engine holds a sender, so this never closes.
                _ = loaded_inbox.recv_many(&mut results, BATCH) => {
                    if let Err(error) = self.write_all(results.drain(..), &mut dirty) {
                        tracing::error!(%error, "could not store loaded data");
                    }
                }
            }
            self.publish(dirty);
        }
        if let Some(session) = self.session.take() {
            session.load.abort();
        }
    }

    fn apply(
        &mut self,
        messages: impl Iterator<Item = Message>,
        dirty: &mut Dirty,
    ) -> Result<(), StoreError> {
        let mut to_fetch = Vec::new();
        let mut uploads = Vec::new();
        let mut images = Vec::new();
        let mut pages = Vec::new();
        let mut downloads = Vec::new();
        let mut fetches = Vec::new();
        let mut sends = Vec::new();
        let mut writes = Vec::new();
        let tx = self.store.conn_mut().transaction()?;
        for message in messages {
            match message {
                Message::Subscribe(sender) => {
                    self.subscribers.push(sender);
                    // The new subscriber has seen no one yet.
                    self.sent_users.clear();
                    self.sent_channels.clear();
                }
                Message::Command(Command::Connect { client }) => {
                    if let Some(old) = self.session.take() {
                        old.load.abort();
                    }
                    // Publish the cache before the first round trip ends.
                    dirty.all_teams = true;
                    dirty.unreads = true;
                    dirty.channels.extend(self.open_channels.iter().cloned());
                    dirty
                        .direct
                        .push(ChangeSet::Connection(Connection::Loading));
                    let load = tokio::spawn(load::run(client.clone(), self.loaded.clone()));
                    // Events from now on queue behind the load's results.
                    let (socket, mut signals) = WsClient::connect(client.clone());
                    let forward = self.loaded.clone();
                    tokio::spawn(async move {
                        while let Some(signal) = signals.recv().await {
                            if forward.send(Loaded::Ws(signal)).is_err() {
                                break;
                            }
                        }
                    });
                    self.session = Some(Session {
                        client,
                        user_id: String::new(),
                        collapsed_threads: cached_crt(&tx)?,
                        fetched: BTreeSet::new(),
                        load,
                        socket,
                        was_connected: false,
                        online: true,
                        resyncing: false,
                        resync_again: false,
                    });
                    // Tell the app what is still waiting from an earlier run.
                    dirty.outbox.extend(queue::channels(&tx)?);
                }
                Message::Command(Command::ShowCache) => {
                    dirty.all_teams = true;
                    dirty.unreads = true;
                    dirty.channels.extend(self.open_channels.iter().cloned());
                }
                Message::Command(Command::SignOut) => match self.session.take() {
                    Some(old) => {
                        old.load.abort();
                        // Dropping the session closes the WebSocket.
                        let client = old.client.clone();
                        drop(old);
                        let loaded = self.loaded.clone();
                        tokio::spawn(async move {
                            if let Err(error) = client.logout().await {
                                tracing::warn!(%error, "the server did not end the session");
                            }
                            let _ = loaded.send(Loaded::SignedOut);
                        });
                    }
                    None => dirty
                        .direct
                        .push(ChangeSet::Connection(Connection::SignedOut)),
                },
                Message::Command(Command::OpenChannel { channel_id }) => {
                    self.open_channels.insert(channel_id.clone());
                    to_fetch.push(channel_id.clone());
                    // Which of its rows failed to send, even from an
                    // earlier run.
                    if !queue::for_channel(&tx, &channel_id)?.is_empty() {
                        dirty.outbox.insert(channel_id.clone());
                    }
                    dirty.channels.insert(channel_id);
                }
                Message::Command(Command::LoadOlder { channel_id }) => {
                    pages.push((channel_id, Page::Older));
                }
                Message::Command(Command::LoadNewer { channel_id }) => {
                    pages.push((channel_id, Page::Newer));
                }
                Message::Command(Command::LoadAround {
                    channel_id,
                    post_id,
                }) => {
                    pages.push((channel_id, Page::Around(post_id)));
                }
                Message::Command(Command::LoadLatest { channel_id }) => {
                    if let Some(history) = self.history.remove(&channel_id) {
                        give_up(&mut self.floors, &channel_id, &history);
                        dirty.direct.push(ChangeSet::History {
                            channel_id: channel_id.clone(),
                            at_start: false,
                            at_end: true,
                            failed: false,
                        });
                        dirty.channels.insert(channel_id);
                    }
                }
                Message::Command(Command::CloseChannel { channel_id }) => {
                    if let Some(history) = self.history.remove(&channel_id) {
                        give_up(&mut self.floors, &channel_id, &history);
                    }
                    self.open_channels.remove(&channel_id);
                    self.sent_channels.remove(&channel_id);
                }
                Message::Command(Command::OpenThread { root_id }) => {
                    self.open_threads.insert(root_id.clone());
                    dirty.threads.insert(root_id);
                }
                Message::Command(Command::CloseThread { root_id }) => {
                    self.open_threads.remove(&root_id);
                }
                Message::Command(Command::OpenThreadList { team_id }) => {
                    self.open_thread_lists.insert(team_id.clone());
                    dirty.thread_lists.insert(team_id);
                }
                Message::Command(Command::CloseThreadList { team_id }) => {
                    self.open_thread_lists.remove(&team_id);
                }
                Message::Command(Command::Event(event)) => {
                    events::apply(&tx, *event, self.me.as_deref(), dirty, &mut fetches)?;
                }
                Message::Command(Command::Resync) => dirty.resync = true,
                Message::Command(Command::SendPost {
                    pending_post_id,
                    channel_id,
                    root_id,
                    message,
                    file_ids,
                }) => {
                    let Some(me) = self.me.clone() else {
                        dirty.direct.push(ChangeSet::Failed(CommandError::Offline));
                        continue;
                    };
                    let now = posts::now_ms();
                    let pending_post_id = if pending_post_id.is_empty() {
                        format!("{me}:{now}")
                    } else {
                        pending_post_id
                    };
                    let outgoing = Outgoing {
                        pending_post_id,
                        channel_id,
                        root_id,
                        message,
                        file_ids,
                        created_at: now,
                        ..Outgoing::default()
                    };
                    outbox::queue(&tx, &outgoing, &me, dirty)?;
                    sends.push(outgoing.channel_id);
                }
                Message::Command(Command::Retry { pending_post_id }) => {
                    if let Some(outgoing) = queue::get(&tx, &pending_post_id)? {
                        queue::unfail(&tx, &pending_post_id)?;
                        dirty.outbox.insert(outgoing.channel_id.clone());
                        sends.push(outgoing.channel_id);
                    }
                }
                Message::Command(Command::Discard { pending_post_id }) => {
                    outbox::discard(&tx, &pending_post_id, dirty)?;
                }
                Message::Command(Command::EditPost { post_id, message }) => {
                    if queue::get(&tx, &post_id)?.is_some() {
                        dirty
                            .direct
                            .push(ChangeSet::Failed(CommandError::NotSentYet { post_id }));
                    } else {
                        writes.push(Write::Edit { post_id, message });
                    }
                }
                Message::Command(Command::DeletePost { post_id }) => {
                    if queue::get(&tx, &post_id)?.is_some() {
                        dirty
                            .direct
                            .push(ChangeSet::Failed(CommandError::NotSentYet { post_id }));
                    } else {
                        writes.push(Write::Delete { post_id });
                    }
                }
                Message::Command(Command::ViewChannel {
                    channel_id,
                    prev_channel_id,
                }) => {
                    if let Some(me) = &self.me {
                        let now = posts::now_ms();
                        for viewed in [&channel_id, &prev_channel_id] {
                            if !viewed.is_empty() {
                                sidebar::mark_viewed(&tx, viewed, me, now)?;
                                events::touch_channel(&tx, viewed, dirty)?;
                            }
                        }
                    }
                    writes.push(Write::View {
                        channel_id,
                        prev_channel_id,
                    });
                }
                Message::Command(Command::MarkUnread { post_id }) => {
                    let collapsed_threads = cached_crt(&tx)?;
                    writes.push(Write::Unread {
                        post_id,
                        collapsed_threads,
                    });
                }
                Message::Command(Command::ToggleReaction {
                    post_id,
                    emoji_name,
                }) => {
                    let Some(me) = self.me.clone() else {
                        dirty.direct.push(ChangeSet::Failed(CommandError::Offline));
                        continue;
                    };
                    let add = !posts::reactions(&tx, &post_id)?
                        .iter()
                        .any(|r| r.user_id == me && r.emoji_name == emoji_name);
                    let reaction = Reaction {
                        user_id: me,
                        post_id,
                        emoji_name,
                        create_at: posts::now_ms(),
                    };
                    react_locally(&tx, &reaction, add, dirty)?;
                    writes.push(Write::React { reaction, add });
                }
                Message::Command(Command::SaveDraft {
                    channel_id,
                    root_id,
                    message,
                    file_ids,
                }) => {
                    let now = posts::now_ms();
                    let draft = Draft {
                        create_at: now,
                        update_at: now,
                        user_id: self.me.clone().unwrap_or_default(),
                        channel_id,
                        root_id,
                        message,
                        file_ids,
                        ..Draft::default()
                    };
                    posts::upsert_draft(&tx, &draft)?;
                    let stored = posts::draft(&tx, &draft.channel_id, &draft.root_id)?;
                    dirty.direct.push(ChangeSet::Draft {
                        channel_id: draft.channel_id.clone(),
                        root_id: draft.root_id.clone(),
                        draft: stored.map(Arc::new),
                    });
                    let synced = self
                        .capabilities
                        .as_ref()
                        .is_some_and(|capabilities| capabilities.synced_drafts);
                    writes.extend(commands::draft_write(synced, draft));
                }
                Message::Command(Command::Typing {
                    channel_id,
                    parent_id,
                }) => {
                    let connection = self.session.as_ref().and_then(|s| s.socket.connection());
                    if let Some(connection) = connection
                        && self.typing.allow(&channel_id, &parent_id, Instant::now())
                    {
                        tokio::spawn(async move {
                            if let Err(error) =
                                connection.user_typing(&channel_id, &parent_id).await
                            {
                                tracing::debug!(%error, "a typing notice was not sent");
                            }
                        });
                    }
                }
                Message::Command(Command::SetStatus {
                    status,
                    dnd_end_time,
                }) => writes.push(Write::Status {
                    status,
                    dnd_end_time,
                }),
                Message::Command(Command::SetFavorite {
                    channel_id,
                    favorite,
                }) => {
                    if let Some(me) = &self.me {
                        let preference = Preference {
                            user_id: me.clone(),
                            category: "favorite_channel".into(),
                            name: channel_id,
                            value: favorite.to_string(),
                        };
                        sidebar::upsert_preferences(&tx, std::slice::from_ref(&preference))?;
                        writes.push(Write::Preferences(vec![preference]));
                    }
                }
                Message::Command(Command::CloseDirect { channel_id }) => {
                    if let Some(me) = &self.me
                        && let Some(channel) = sidebar::channel(&tx, &channel_id)?
                        && let Some((category, name)) =
                            sidebar::show_preference(&channel.kind, &channel.id, &channel.name, me)
                    {
                        let preference = Preference {
                            user_id: me.clone(),
                            category: category.into(),
                            name,
                            value: "false".into(),
                        };
                        sidebar::upsert_preferences(&tx, std::slice::from_ref(&preference))?;
                        dirty.all_teams = true;
                        writes.push(Write::Preferences(vec![preference]));
                    }
                }
                Message::Command(Command::OpenDirect { user_ids }) => {
                    writes.push(Write::OpenDirect { user_ids });
                }
                Message::Command(Command::JoinChannel { channel_id }) => {
                    writes.push(Write::Join { channel_id });
                }
                Message::Command(Command::LeaveChannel { channel_id }) => {
                    writes.push(Write::Leave { channel_id });
                }
                Message::Command(Command::Browse { team_id }) => {
                    writes.push(Write::Browse { team_id });
                }
                Message::Command(Command::Upload {
                    upload_id,
                    channel_id,
                    file_name,
                    bytes,
                }) => uploads.push(matty_api::Upload {
                    channel_id,
                    client_id: upload_id,
                    file_name,
                    bytes,
                }),
                Message::Command(Command::LoadImage { file_id, size }) => {
                    images.push((file_id, size));
                }
                Message::Command(Command::Download {
                    file_id,
                    folder,
                    name,
                }) => downloads.push((file_id, folder, name)),
                Message::Command(Command::CancelUpload { upload_id }) => {
                    if let Some(task) = self.uploads.remove(&upload_id) {
                        task.abort();
                    }
                }
                Message::Command(Command::PatchChannel {
                    channel_id,
                    display_name,
                    header,
                    purpose,
                }) => writes.push(Write::Patch {
                    channel_id,
                    patch: matty_api::ChannelPatch {
                        display_name,
                        header,
                        purpose,
                    },
                }),
                Message::Command(Command::SetChannelNotifications { channel_id, props }) => {
                    writes.push(Write::Notify { channel_id, props });
                }
                Message::Command(Command::AddMembers {
                    channel_id,
                    user_ids,
                }) => writes.push(Write::AddMembers {
                    channel_id,
                    user_ids,
                }),
                Message::Command(Command::Search {
                    team_id,
                    terms,
                    time_zone_offset,
                }) => {
                    let mut query = matty_api::SearchQuery::new(terms);
                    query.time_zone_offset = time_zone_offset;
                    query.per_page = SEARCH_PAGE;
                    writes.push(Write::Search {
                        team_id,
                        query: Box::new(query),
                    });
                }
                Message::Command(Command::FindPeople { team_id }) => {
                    writes.push(Write::People { team_id });
                }
                Message::Command(Command::CreateChannel {
                    team_id,
                    name,
                    display_name,
                    purpose,
                    private,
                }) => {
                    writes.push(Write::Create(Box::new(matty_api::NewChannel {
                        team_id,
                        name,
                        display_name,
                        purpose,
                        kind: if private { "P" } else { "O" }.into(),
                    })));
                }
            }
        }
        tx.commit()?;
        for channel_id in to_fetch {
            self.fetch_channel(&channel_id);
        }
        fetches.dedup();
        for what in fetches {
            self.fetch(what);
        }
        for (file_id, folder, name) in downloads {
            self.download(file_id, folder, name, dirty);
        }
        for (channel_id, page) in pages {
            self.load_page(channel_id, page, dirty);
        }
        for (file_id, size) in images {
            self.image(file_id, size, dirty);
        }
        for upload in uploads {
            self.upload(upload, dirty);
        }
        for channel_id in sends {
            self.drain(&channel_id, dirty)?;
        }
        for write in writes {
            self.write(write, dirty);
        }
        Ok(())
    }

    /// Starts sending a channel's oldest queued post, unless one is on its
    /// way or the socket is down; the rest follow one by one as each is
    /// confirmed (#194). Failed sends wait for Retry and do not hold up
    /// the ones after them.
    fn drain(&mut self, channel_id: &str, dirty: &mut Dirty) -> Result<(), StoreError> {
        if self.sending.contains_key(channel_id) {
            return Ok(());
        }
        let Some(session) = self.session.as_ref().filter(|session| session.online) else {
            return Ok(());
        };
        let conn = self.store.conn();
        let Some(outgoing) = queue::for_channel(conn, channel_id)?
            .into_iter()
            .find(|outgoing| outgoing.last_error.is_none())
        else {
            return Ok(());
        };
        queue::attempt(conn, &outgoing.pending_post_id)?;
        dirty.outbox.insert(outgoing.channel_id.clone());
        self.sending
            .insert(channel_id.to_owned(), outgoing.pending_post_id.clone());
        tokio::spawn(outbox::send(
            session.client.clone(),
            outgoing,
            self.loaded.clone(),
        ));
        Ok(())
    }

    /// A send came back: the channel may send its next one.
    fn sent(&mut self, pending_post_id: &str) -> Option<String> {
        let channel_id = self
            .sending
            .iter()
            .find(|(_, id)| id.as_str() == pending_post_id)
            .map(|(channel_id, _)| channel_id.clone())?;
        self.sending.remove(&channel_id);
        Some(channel_id)
    }

    /// Starts saving a file; it reports back as [`Loaded::Download`].
    fn download(
        &self,
        file_id: String,
        folder: std::path::PathBuf,
        name: String,
        dirty: &mut Dirty,
    ) {
        let Some(session) = self.session.as_ref().filter(|s| !s.user_id.is_empty()) else {
            dirty.direct.push(ChangeSet::Download {
                file_id,
                state: DownloadState::Failed("not connected".into()),
            });
            return;
        };
        tokio::spawn(crate::download::download(
            session.client.clone(),
            file_id,
            folder,
            name,
            self.loaded.clone(),
        ));
    }

    /// Fetches a page of `channel_id`'s history; it comes back as
    /// [`Loaded::Older`], [`Loaded::Newer`] or [`Loaded::Around`].
    fn load_page(&mut self, channel_id: String, page: Page, dirty: &mut Dirty) {
        if !self.open_channels.contains(&channel_id) {
            return;
        }
        let history = self.history.entry(channel_id.clone()).or_default();
        if history.loading {
            return;
        }
        let done = match page {
            Page::Older => history.at_start,
            Page::Newer => history.anchor.is_none(),
            Page::Around(_) => false,
        };
        if done {
            dirty.direct.push(history.change(&channel_id, false));
            return;
        }
        let Some(session) = self.session.as_ref().filter(|s| !s.user_id.is_empty()) else {
            dirty.direct.push(history.change(&channel_id, true));
            return;
        };
        let roots_only = session.collapsed_threads;
        let edge = if matches!(page, Page::Around(_)) {
            None
        } else {
            let floor = self.floors.get(&channel_id).copied();
            match snapshot(
                self.store.conn(),
                &channel_id,
                Some(history),
                floor,
                roots_only,
            ) {
                Ok(window) => {
                    let post = match page {
                        Page::Older => window.first(),
                        _ => window.last(),
                    };
                    let Some(post) = post else {
                        return;
                    };
                    Some(post.id.clone())
                }
                Err(error) => {
                    tracing::error!(%error, "could not read a posts snapshot");
                    return;
                }
            }
        };
        history.loading = true;
        let client = session.client.clone();
        let results = self.loaded.clone();
        tokio::spawn(async move {
            let page_of = |query: matty_api::PostsQuery| matty_api::PostsQuery {
                collapsed_threads: roots_only,
                ..query
            };
            let message = match (page, edge) {
                (Page::Around(post_id), _) => {
                    let around = async {
                        let mut post = client.get_post(&post_id).await?;
                        if roots_only && !post.root_id.is_empty() {
                            post = client.get_post(&post.root_id).await?;
                        }
                        let before = client
                            .channel_posts(
                                &channel_id,
                                &page_of(matty_api::PostsQuery::before(post.id.clone(), HISTORY_PAGE)),
                            )
                            .await?;
                        let after = client
                            .channel_posts(
                                &channel_id,
                                &page_of(matty_api::PostsQuery::after(post.id.clone(), HISTORY_PAGE)),
                            )
                            .await?;
                        Ok::<_, matty_api::ApiError>(Box::new(crate::load::Around {
                            post,
                            before,
                            after,
                        }))
                    }
                    .await
                    .inspect_err(|error| {
                        tracing::warn!(%error, channel_id, "could not load the posts around a post");
                    })
                    .ok();
                    Loaded::Around {
                        channel_id,
                        posts: around,
                    }
                }
                (page, Some(edge)) => {
                    let older = matches!(page, Page::Older);
                    let query = if older {
                        matty_api::PostsQuery::before(edge, HISTORY_PAGE)
                    } else {
                        matty_api::PostsQuery::after(edge, HISTORY_PAGE)
                    };
                    let posts = client
                        .channel_posts(&channel_id, &page_of(query))
                        .await
                        .inspect_err(|error| {
                            tracing::warn!(%error, channel_id, "could not load a page of posts");
                        })
                        .ok();
                    if older {
                        Loaded::Older { channel_id, posts }
                    } else {
                        Loaded::Newer { channel_id, posts }
                    }
                }
                (_, None) => return,
            };
            let _ = results.send(message);
        });
    }

    /// Answers from the media cache, or fetches the image; it comes back
    /// as [`Loaded::Image`].
    fn image(&mut self, file_id: String, size: ImageSize, dirty: &mut Dirty) {
        let key = image_key(&file_id, size);
        if let Some(media) = &self.media {
            match media.get(self.store.conn(), &key) {
                Ok(Some(cached)) => {
                    dirty.direct.push(ChangeSet::Image {
                        file_id,
                        size,
                        bytes: Arc::from(cached.bytes),
                        content_type: cached.content_type,
                    });
                    return;
                }
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, "could not read the media cache"),
            }
        }
        let Some(session) = self.session.as_ref().filter(|s| !s.user_id.is_empty()) else {
            return;
        };
        if !self.fetching.insert(key) {
            return;
        }
        let client = session.client.clone();
        let results = self.loaded.clone();
        tokio::spawn(async move {
            let fetched = match size {
                ImageSize::Thumbnail => client.file_thumbnail(&file_id, None).await,
                ImageSize::Preview => client.file_preview(&file_id, None).await,
            };
            let image = match fetched {
                Ok(matty_api::Fetched::Fresh {
                    bytes,
                    content_type,
                    ..
                }) => Some((bytes, content_type.unwrap_or_else(|| "image/jpeg".into()))),
                Ok(matty_api::Fetched::NotModified) => None,
                Err(error) => {
                    tracing::debug!(%error, file_id, ?size, "no image");
                    None
                }
            };
            let _ = results.send(Loaded::Image {
                file_id,
                size,
                image,
            });
        });
    }

    /// Starts sending a file; it reports back as [`Loaded::Upload`].
    fn upload(&mut self, upload: matty_api::Upload, dirty: &mut Dirty) {
        let upload_id = upload.client_id.clone();
        let Some(session) = self.session.as_ref().filter(|s| !s.user_id.is_empty()) else {
            dirty.direct.push(ChangeSet::Upload {
                upload_id,
                state: UploadState::Failed("not connected".into()),
            });
            return;
        };
        if let Some(old) = self.uploads.remove(&upload_id) {
            old.abort();
        }
        let task = tokio::spawn(outbox::upload(
            session.client.clone(),
            upload,
            self.loaded.clone(),
        ));
        self.uploads.insert(upload_id, task);
    }

    /// Calls the server for a command; the result comes back as a
    /// [`Loaded`] and is applied like the matching event.
    fn write(&self, write: Write, dirty: &mut Dirty) {
        let Some(session) = self.session.as_ref().filter(|s| !s.user_id.is_empty()) else {
            dirty.direct.push(ChangeSet::Failed(CommandError::Offline));
            return;
        };
        tokio::spawn(commands::run(
            session.client.clone(),
            session.user_id.clone(),
            write,
            self.loaded.clone(),
        ));
    }

    /// Fetches an open channel's posts once per session, after the account
    /// has loaded.
    fn fetch_channel(&mut self, channel_id: &str) {
        let Some(session) = &mut self.session else {
            return;
        };
        if session.user_id.is_empty() || !session.fetched.insert(channel_id.to_owned()) {
            return;
        }
        tokio::spawn(load::open_channel(
            session.client.clone(),
            session.user_id.clone(),
            channel_id.to_owned(),
            session.collapsed_threads,
            self.loaded.clone(),
        ));
        tokio::spawn(load::channel_stats(
            session.client.clone(),
            channel_id.to_owned(),
            self.loaded.clone(),
        ));
    }

    /// Writes loaded results in order. Runs of WebSocket events share one
    /// transaction, like a batch of commands.
    fn write_all(
        &mut self,
        results: impl Iterator<Item = Loaded>,
        dirty: &mut Dirty,
    ) -> Result<(), StoreError> {
        let mut events = Vec::new();
        for result in results {
            match result {
                Loaded::Ws(WsSignal::Event(raw)) => events.push(WsEvent::decode(&raw)),
                other => {
                    self.apply_events(std::mem::take(&mut events), dirty)?;
                    self.write_loaded(other, dirty)?;
                }
            }
        }
        self.apply_events(events, dirty)
    }

    fn apply_events(&mut self, events: Vec<WsEvent>, dirty: &mut Dirty) -> Result<(), StoreError> {
        if events.is_empty() {
            return Ok(());
        }
        let mut fetches = Vec::new();
        let tx = self.store.conn_mut().transaction()?;
        for event in events {
            events::apply(&tx, event, self.me.as_deref(), dirty, &mut fetches)?;
        }
        tx.commit()?;
        fetches.dedup();
        for what in fetches {
            self.fetch(what);
        }
        Ok(())
    }

    /// Starts a catch-up, or queues one behind the running one. Needs the
    /// account loaded: the plan names the user and their teams.
    fn resync(&mut self) {
        let Some(session) = &mut self.session else {
            return;
        };
        if session.user_id.is_empty() {
            return;
        }
        if session.resyncing {
            session.resync_again = true;
            return;
        }
        let conn = self.store.conn();
        let plan = (|| -> Result<load::ResyncPlan, StoreError> {
            let teams = cached_team_ids(conn)?;
            // Open channels first, then the rest by their newest post.
            let mut channels: Vec<String> = self.open_channels.iter().cloned().collect();
            for channel_id in posts::cached_channels(conn)? {
                if !self.open_channels.contains(&channel_id) {
                    channels.push(channel_id);
                }
            }
            let since = |key: String| -> Result<Option<i64>, StoreError> {
                Ok(posts::cursor(conn, &key)?.and_then(|value| value.parse().ok()))
            };
            let channels = channels
                .into_iter()
                .map(|channel_id| {
                    let cursor = since(posts::channel_since_key(&channel_id))?;
                    Ok((channel_id, cursor))
                })
                .collect::<Result<_, StoreError>>()?;
            let threads = if session.collapsed_threads {
                teams
                    .iter()
                    .map(|team_id| Ok((team_id.clone(), since(posts::threads_since_key(team_id))?)))
                    .collect::<Result<_, StoreError>>()?
            } else {
                Vec::new()
            };
            Ok(load::ResyncPlan {
                user_id: session.user_id.clone(),
                teams,
                channels,
                threads,
                max_response_bytes: MAX_SINCE_BYTES,
            })
        })();
        match plan {
            Ok(plan) => {
                session.resyncing = true;
                tokio::spawn(load::resync(
                    session.client.clone(),
                    plan,
                    self.loaded.clone(),
                ));
            }
            Err(error) => tracing::error!(%error, "could not plan a resync"),
        }
    }

    /// Runs a fetch an event asked for, when connected. Without a session
    /// the next initial load covers it.
    fn fetch(&self, what: Fetch) {
        if let Some(session) = &self.session {
            tokio::spawn(load::fetch(
                session.client.clone(),
                session.user_id.clone(),
                what,
                self.loaded.clone(),
            ));
        }
    }

    /// Stores a page from [`Engine::load_page`] and widens the snapshot
    /// to take it in.
    fn write_page(
        &mut self,
        channel_id: String,
        posts: Option<matty_api::models::PostList>,
        page: Page,
        dirty: &mut Dirty,
    ) -> Result<(), StoreError> {
        let Some(history) = self.history.get_mut(&channel_id) else {
            return Ok(());
        };
        history.loading = false;
        let Some(list) = posts else {
            dirty.direct.push(history.change(&channel_id, true));
            return Ok(());
        };
        let fetched: Vec<_> = list.posts.into_values().collect();
        let mut latest = false;
        match (page, history.anchor.as_mut()) {
            (Page::Older, anchor) => {
                history.at_start = list.prev_post_id.is_empty();
                match anchor {
                    Some((from, _)) => {
                        *from = fetched.iter().map(|p| p.create_at).fold(*from, i64::min);
                    }
                    None => {
                        history.extra += list.order.len();
                        // The page joins the latest posts, reaching below
                        // a given-up jump's leftovers it overlaps.
                        if let (Some(floor), Some(oldest)) = (
                            self.floors.get_mut(&channel_id),
                            fetched.iter().map(|p| p.create_at).min(),
                        ) {
                            *floor = (*floor).min(oldest - 1);
                        }
                    }
                }
            }
            (_, Some((_, to))) => {
                *to = fetched.iter().map(|p| p.create_at).fold(*to, i64::max);
                latest = list.next_post_id.is_empty();
            }
            (_, None) => {}
        }
        let tx = self.store.conn_mut().transaction()?;
        posts::insert_posts(&tx, &fetched)?;
        tx.commit()?;
        if latest {
            self.reach_latest(&channel_id);
        }
        let history = &self.history[&channel_id];
        dirty.direct.push(history.change(&channel_id, false));
        dirty.channels.insert(channel_id);
        Ok(())
    }

    /// A history window that reached the newest post becomes the latest
    /// posts again, reaching back as far as it did.
    fn reach_latest(&mut self, channel_id: &str) {
        let roots_only = self
            .session
            .as_ref()
            .is_some_and(|session| session.collapsed_threads);
        let Some(history) = self.history.get_mut(channel_id) else {
            return;
        };
        let Some((from, _)) = history.anchor.take() else {
            return;
        };
        // Everything from `from` on now joins the newest post.
        if let Some(floor) = self.floors.get_mut(channel_id) {
            *floor = (*floor).min(from - 1);
        }
        let shown = posts::posts_window(
            self.store.conn(),
            channel_id,
            Window::After(from - 1),
            usize::MAX,
            roots_only,
        )
        .map_or(0, |window| window.len());
        history.extra = shown.saturating_sub(POSTS_WINDOW);
    }

    fn write_loaded(&mut self, result: Loaded, dirty: &mut Dirty) -> Result<(), StoreError> {
        match result {
            Loaded::Account {
                me,
                capabilities,
                preferences,
                teams,
            } => {
                let crt = collapsed_threads(capabilities.collapsed_threads, &preferences);
                let tx = self.store.conn_mut().transaction()?;
                sidebar::upsert_users(&tx, std::slice::from_ref(&*me))?;
                sidebar::upsert_preferences(&tx, &preferences)?;
                // Teams the user left since the last load go.
                let current: BTreeSet<&str> = teams
                    .iter()
                    .filter(|team| team.delete_at == 0)
                    .map(|team| team.id.as_str())
                    .collect();
                for cached in cached_team_ids(&tx)? {
                    if !current.contains(cached.as_str()) {
                        sidebar::delete_team(&tx, &cached)?;
                    }
                }
                sidebar::upsert_teams(&tx, &teams)?;
                if let Some(order) = preferences.iter().find(|p| p.category == "teams_order") {
                    let ids: Vec<String> = order
                        .value
                        .split(',')
                        .filter(|id| !id.is_empty())
                        .map(str::to_owned)
                        .collect();
                    sidebar::set_team_order(&tx, &ids)?;
                }
                posts::set_cursor(&tx, ME_KEY, &me.id)?;
                posts::set_cursor(&tx, CRT_KEY, if crt { "1" } else { "0" })?;
                tx.commit()?;
                if let Some(session) = &mut self.session {
                    session.user_id = me.id.clone();
                    session.collapsed_threads = crt;
                }
                self.me = Some(me.id.clone());
                self.capabilities = Some((*capabilities).clone());
                dirty
                    .direct
                    .push(ChangeSet::Config(Arc::from(capabilities)));
                dirty.direct.push(ChangeSet::Preferences);
                dirty.unreads = true;
                let open: Vec<String> = self.open_channels.iter().cloned().collect();
                for channel_id in open {
                    self.fetch_channel(&channel_id);
                }
            }
            Loaded::Team {
                team_id,
                channels,
                members,
                categories,
                users,
            } => {
                let tx = self.store.conn_mut().transaction()?;
                sidebar::upsert_users(&tx, &users)?;
                sidebar::upsert_channels(&tx, &channels)?;
                sidebar::upsert_channel_members(&tx, &members)?;
                sidebar::replace_categories(&tx, &team_id, &categories)?;
                tx.commit()?;
                dirty.teams.insert(team_id);
                dirty.unreads = true;
                dirty
                    .channels
                    .extend(channels.into_iter().map(|channel| channel.id));
            }
            Loaded::TeamFailed { team_id, error } => {
                tracing::warn!(%team_id, %error, "a team did not load");
                dirty
                    .direct
                    .push(ChangeSet::Connection(Connection::TeamFailed {
                        team_id,
                        error,
                    }));
            }
            Loaded::Expired => {
                tracing::warn!("the server refused the session token");
                dirty
                    .direct
                    .push(ChangeSet::Connection(Connection::Expired));
            }
            Loaded::Failed(error) => {
                tracing::warn!(%error, "the account did not load");
                dirty
                    .direct
                    .push(ChangeSet::Connection(Connection::Failed { error }));
            }
            Loaded::Emoji(emoji) => sidebar::upsert_emoji(self.store.conn(), &emoji)?,
            Loaded::Users(users) => {
                sidebar::upsert_users(self.store.conn(), &users)?;
                self.sent_users.extend(users.iter().map(|u| u.id.clone()));
                if !users.is_empty() {
                    dirty.direct.push(ChangeSet::Users(Arc::from(users)));
                }
            }
            Loaded::Statuses(statuses) => {
                dirty.direct.push(ChangeSet::Presence(Arc::from(statuses)));
            }
            Loaded::Done => dirty.direct.push(ChangeSet::Connection(Connection::Loaded)),
            Loaded::SignedOut => dirty
                .direct
                .push(ChangeSet::Connection(Connection::SignedOut)),
            Loaded::Older { channel_id, posts } => {
                self.write_page(channel_id, posts, Page::Older, dirty)?;
            }
            Loaded::Newer { channel_id, posts } => {
                self.write_page(channel_id, posts, Page::Newer, dirty)?;
            }
            Loaded::Around { channel_id, posts } => {
                let Some(history) = self.history.get_mut(&channel_id) else {
                    return Ok(());
                };
                history.loading = false;
                let Some(around) = posts else {
                    dirty.direct.push(history.change(&channel_id, true));
                    return Ok(());
                };
                let crate::load::Around {
                    post,
                    before,
                    after,
                } = *around;
                let mut fetched = vec![post];
                fetched.extend(before.posts.into_values());
                fetched.extend(after.posts.into_values());
                let from = fetched.iter().map(|p| p.create_at).min().unwrap_or(0);
                let to = fetched.iter().map(|p| p.create_at).max().unwrap_or(0);
                history.at_start = before.prev_post_id.is_empty();
                history.anchor = Some((from, to));
                let tx = self.store.conn_mut().transaction()?;
                posts::insert_posts(&tx, &fetched)?;
                tx.commit()?;
                if after.next_post_id.is_empty() {
                    self.reach_latest(&channel_id);
                }
                let history = &self.history[&channel_id];
                dirty.direct.push(history.change(&channel_id, false));
                dirty.channels.insert(channel_id);
            }
            Loaded::Posts {
                channel_id,
                posts: list,
            } => {
                let tx = self.store.conn_mut().transaction()?;
                let fetched: Vec<_> = list.posts.into_values().collect();
                posts::insert_posts(&tx, &fetched)?;
                tx.commit()?;
                dirty.channels.insert(channel_id);
            }
            Loaded::Directory {
                team_id,
                channels,
                members,
            } => dirty.direct.push(ChangeSet::Directory {
                team_id,
                channels: Arc::from(channels),
                members: Arc::new(members),
            }),
            Loaded::Upload { upload_id, state } => {
                if !matches!(state, UploadState::Sending(_)) {
                    self.uploads.remove(&upload_id);
                }
                dirty.direct.push(ChangeSet::Upload { upload_id, state });
            }
            Loaded::Download { file_id, state } => {
                dirty.direct.push(ChangeSet::Download { file_id, state });
            }
            Loaded::Image {
                file_id,
                size,
                image,
            } => {
                let key = image_key(&file_id, size);
                self.fetching.remove(&key);
                if let Some((bytes, content_type)) = image {
                    if let Some(media) = &self.media
                        && let Err(error) =
                            media.put(self.store.conn(), &key, &bytes, &content_type)
                    {
                        tracing::warn!(%error, "could not keep an image");
                    }
                    dirty.direct.push(ChangeSet::Image {
                        file_id,
                        size,
                        bytes: Arc::from(bytes),
                        content_type,
                    });
                }
            }
            Loaded::Search { terms, posts } => dirty.direct.push(ChangeSet::Search {
                terms,
                posts: Arc::from(posts),
            }),
            Loaded::People { team_id, users } => {
                sidebar::upsert_users(self.store.conn(), &users)?;
                dirty.direct.push(ChangeSet::People {
                    team_id,
                    users: Arc::from(users),
                });
            }
            Loaded::Ready { channel, member } => {
                let channel_id = channel.id.clone();
                if let Some(me) = &self.me
                    && let Some((category, name)) =
                        sidebar::show_preference(&channel.kind, &channel.id, &channel.name, me)
                {
                    sidebar::upsert_preferences(
                        self.store.conn(),
                        &[Preference {
                            user_id: me.clone(),
                            category: category.into(),
                            name,
                            value: "true".into(),
                        }],
                    )?;
                    dirty.all_teams = true;
                }
                self.write_loaded(
                    Loaded::Channel {
                        channel,
                        member: Some(member),
                    },
                    dirty,
                )?;
                dirty.direct.push(ChangeSet::ChannelReady { channel_id });
            }
            Loaded::Left { channel_id } => {
                let tx = self.store.conn_mut().transaction()?;
                events::touch_channel(&tx, &channel_id, dirty)?;
                sidebar::delete_channel(&tx, &channel_id)?;
                tx.commit()?;
                dirty.unreads = true;
            }
            Loaded::Channel { channel, member } => {
                let tx = self.store.conn_mut().transaction()?;
                sidebar::upsert_channels(&tx, std::slice::from_ref(&*channel))?;
                if let Some(member) = member {
                    sidebar::upsert_channel_members(&tx, std::slice::from_ref(&*member))?;
                }
                tx.commit()?;
                // A new channel lands in a category the server picked: for
                // a direct or group message, the messages category of
                // every team.
                if channel.team_id.is_empty() {
                    for team_id in cached_team_ids(self.store.conn())? {
                        self.fetch(Fetch::Categories(team_id));
                    }
                    dirty.all_teams = true;
                } else {
                    self.fetch(Fetch::Categories(channel.team_id.clone()));
                    dirty.teams.insert(channel.team_id.clone());
                }
                dirty.unreads = true;
                dirty.channels.insert(channel.id);
            }
            Loaded::TeamInfo(team) => {
                sidebar::upsert_teams(self.store.conn(), std::slice::from_ref(&*team))?;
                dirty.unreads = true;
            }
            Loaded::Categories {
                team_id,
                categories,
            } => {
                let tx = self.store.conn_mut().transaction()?;
                sidebar::replace_categories(&tx, &team_id, &categories)?;
                tx.commit()?;
                dirty.teams.insert(team_id);
            }
            Loaded::Capabilities(capabilities) => {
                let preferences = all_preferences(self.store.conn())?;
                let crt = collapsed_threads(capabilities.collapsed_threads, &preferences);
                posts::set_cursor(self.store.conn(), CRT_KEY, if crt { "1" } else { "0" })?;
                if let Some(session) = &mut self.session {
                    session.collapsed_threads = crt;
                }
                self.capabilities = Some((*capabilities).clone());
                dirty
                    .direct
                    .push(ChangeSet::Config(Arc::from(capabilities)));
                dirty.all_teams = true;
                dirty.unreads = true;
                dirty.channels.extend(self.open_channels.iter().cloned());
            }
            Loaded::Ws(signal) => match signal {
                WsSignal::State(state) => {
                    let online = matches!(state, ConnectionState::Connected { .. });
                    if let Some(session) = &mut self.session
                        && (online || session.was_connected)
                        && !matches!(state, ConnectionState::Connecting)
                    {
                        session.online = online;
                    }
                    if online && let Some(session) = &mut self.session {
                        // Even a resumed stream can miss channel events, so
                        // every reconnect catches up (WsSignal docs).
                        if session.was_connected {
                            dirty.resync = true;
                        }
                        session.was_connected = true;
                        // What was written while offline, or before a
                        // restart, goes now, channel by channel.
                        for channel_id in queue::channels(self.store.conn())? {
                            self.drain(&channel_id, dirty)?;
                        }
                    }
                    dirty
                        .direct
                        .push(ChangeSet::Connection(Connection::Socket(state)));
                }
                WsSignal::Gap { .. } | WsSignal::ResyncRequired => dirty.resync = true,
                WsSignal::Event(raw) => self.apply_events(vec![WsEvent::decode(&raw)], dirty)?,
            },
            Loaded::Since {
                channel_id,
                posts: list,
            } => {
                let tx = self.store.conn_mut().transaction()?;
                for change in list.changes() {
                    match change {
                        PostChange::Upsert(post) => {
                            posts::insert_posts(&tx, std::slice::from_ref(post))?;
                        }
                        PostChange::Deleted { id, .. } => {
                            let delete_at = list.posts.get(id).map_or(1, |p| p.delete_at);
                            posts::mark_deleted(&tx, id, delete_at)?;
                        }
                    }
                }
                tx.commit()?;
                dirty.channels.insert(channel_id);
                dirty.threads.extend(self.open_threads.iter().cloned());
            }
            Loaded::Reset {
                channel_id,
                posts: list,
            } => {
                let tx = self.store.conn_mut().transaction()?;
                posts::clear_channel(&tx, &channel_id)?;
                let fresh: Vec<_> = list.posts.into_values().collect();
                posts::insert_posts(&tx, &fresh)?;
                tx.commit()?;
                dirty.channels.insert(channel_id);
                dirty.threads.extend(self.open_threads.iter().cloned());
            }
            Loaded::Threads { team_id, threads } => {
                let tx = self.store.conn_mut().transaction()?;
                posts::upsert_threads(&tx, &team_id, &threads)?;
                let newest = threads
                    .iter()
                    .map(|t| t.last_reply_at.max(t.last_viewed_at))
                    .max()
                    .unwrap_or(0);
                if newest > 0 {
                    posts::advance_since(&tx, &posts::threads_since_key(&team_id), newest)?;
                }
                tx.commit()?;
                dirty.thread_lists.insert(team_id);
            }
            Loaded::Sent {
                pending_post_id,
                post,
            } => {
                let tx = self.store.conn_mut().transaction()?;
                outbox::confirm(&tx, &pending_post_id, &post, self.me.as_deref(), dirty)?;
                tx.commit()?;
                self.sent(&pending_post_id);
                self.drain(&post.channel_id, dirty)?;
            }
            Loaded::SendFailed {
                pending_post_id,
                error,
                transient,
            } => {
                tracing::warn!(%pending_post_id, %error, transient, "a send failed");
                let channel_id = self.sent(&pending_post_id);
                let queued = queue::get(self.store.conn(), &pending_post_id)?;
                match (queued, channel_id) {
                    // Still queued, first in line: it goes again after a
                    // pause, or when the socket is back.
                    (Some(outgoing), Some(channel_id)) if transient => {
                        let loaded = self.loaded.clone();
                        let wait = outbox::backoff(outgoing.attempts);
                        tokio::spawn(async move {
                            tokio::time::sleep(wait).await;
                            let _ = loaded.send(Loaded::Drain { channel_id });
                        });
                    }
                    (_, channel_id) => {
                        outbox::fail(self.store.conn(), &pending_post_id, &error, dirty)?;
                        if let Some(channel_id) = channel_id {
                            self.drain(&channel_id, dirty)?;
                        }
                    }
                }
            }
            Loaded::Drain { channel_id } => self.drain(&channel_id, dirty)?,
            Loaded::Edited(post) => {
                let mut fetches = Vec::new();
                events::apply(
                    self.store.conn(),
                    WsEvent::PostEdited(post),
                    self.me.as_deref(),
                    dirty,
                    &mut fetches,
                )?;
            }
            Loaded::Deleted { post_id } => {
                if let Some(post) = posts::post(self.store.conn(), &post_id)? {
                    let mut fetches = Vec::new();
                    events::apply(
                        self.store.conn(),
                        WsEvent::PostDeleted(Box::new(post)),
                        self.me.as_deref(),
                        dirty,
                        &mut fetches,
                    )?;
                }
            }
            Loaded::Unread(unread) => {
                let state = sidebar::ReadState {
                    last_viewed_at: unread.last_viewed_at,
                    msg_count: unread.msg_count,
                    msg_count_root: unread.msg_count_root,
                    mention_count: unread.mention_count,
                    mention_count_root: unread.mention_count_root,
                    urgent_mention_count: unread.urgent_mention_count,
                };
                let conn = self.store.conn();
                sidebar::set_read_state(conn, &unread.channel_id, &unread.user_id, &state)?;
                events::touch_channel(conn, &unread.channel_id, dirty)?;
            }
            Loaded::ReactFailed {
                reaction,
                added,
                error,
            } => {
                react_locally(self.store.conn(), &reaction, !added, dirty)?;
                dirty.direct.push(ChangeSet::Failed(CommandError::Server {
                    action: if added {
                        "add reaction"
                    } else {
                        "remove reaction"
                    },
                    target: reaction.post_id.clone(),
                    error,
                }));
            }
            Loaded::WriteFailed {
                action,
                target,
                error,
            } => dirty.direct.push(ChangeSet::Failed(CommandError::Server {
                action,
                target,
                error,
            })),
            Loaded::Resynced => {
                dirty
                    .direct
                    .push(ChangeSet::Connection(Connection::Resynced));
                if let Some(session) = &mut self.session {
                    session.resyncing = false;
                    if std::mem::take(&mut session.resync_again) {
                        dirty.resync = true;
                    }
                }
            }
            Loaded::Stats(stats) => {
                if self.open_channels.contains(&stats.channel_id) {
                    dirty.direct.push(ChangeSet::Stats(Arc::new(stats)));
                }
            }
            Loaded::PostsFailed { channel_id, error } => {
                if let Some(session) = &mut self.session {
                    session.fetched.remove(&channel_id);
                }
                dirty
                    .direct
                    .push(ChangeSet::Connection(Connection::ChannelFailed {
                        channel_id,
                        error,
                    }));
            }
        }
        Ok(())
    }

    fn publish(&mut self, mut dirty: Dirty) {
        if dirty.preferences {
            self.preferences_changed(&mut dirty);
        }
        if dirty.resync {
            self.resync();
        }
        let mut changes = dirty.direct;
        let conn = self.store.conn();
        let user_id = posts::cursor(conn, ME_KEY).ok().flatten();
        let mode = match cached_crt(conn) {
            Ok(true) => UnreadMode::CollapsedThreads,
            _ => UnreadMode::AllPosts,
        };
        if let Some(user_id) = &user_id {
            let teams = if dirty.all_teams {
                cached_team_ids(conn).unwrap_or_default()
            } else {
                dirty.teams.into_iter().collect()
            };
            for team_id in teams {
                match sidebar::sidebar_snapshot(conn, &team_id, user_id, mode) {
                    Ok(sections) => changes.push(ChangeSet::Sidebar {
                        team_id,
                        sections: Arc::from(sections),
                    }),
                    Err(error) => tracing::error!(%error, "could not read a sidebar snapshot"),
                }
            }
            if dirty.unreads {
                match sidebar::teams_with_unreads(conn, user_id, mode) {
                    Ok(teams) => changes.push(ChangeSet::Unreads {
                        teams: Arc::from(teams),
                    }),
                    Err(error) => tracing::error!(%error, "could not read team unreads"),
                }
            }
        }
        for root_id in dirty.threads {
            if !self.open_threads.contains(&root_id) {
                continue;
            }
            match posts::thread(conn, &root_id) {
                Ok(thread) => changes.push(ChangeSet::Thread {
                    root_id,
                    posts: Arc::from(thread),
                }),
                Err(error) => tracing::error!(%error, "could not read a thread snapshot"),
            }
        }
        for team_id in dirty.thread_lists {
            if !self.open_thread_lists.contains(&team_id) {
                continue;
            }
            match posts::threads(conn, &team_id, false, THREAD_LIST) {
                Ok(threads) => changes.push(ChangeSet::ThreadList {
                    team_id,
                    threads: Arc::from(threads),
                }),
                Err(error) => tracing::error!(%error, "could not read a thread list"),
            }
        }
        for channel_id in dirty.outbox {
            match queue::for_channel(conn, &channel_id) {
                Ok(sends) => changes.push(ChangeSet::Outbox {
                    channel_id,
                    sends: Arc::from(sends),
                }),
                Err(error) => tracing::error!(%error, "could not read the outbox"),
            }
        }
        let roots_only = mode == UnreadMode::CollapsedThreads;
        for channel_id in dirty.channels {
            if !self.open_channels.contains(&channel_id) {
                continue;
            }
            let member = self.me.as_deref().and_then(|me| {
                sidebar::channel_member(conn, &channel_id, me)
                    .inspect_err(|error| tracing::error!(%error, "could not read a membership"))
                    .ok()
                    .flatten()
            });
            match sidebar::channel(conn, &channel_id) {
                Ok(Some(channel)) => {
                    let sent = (
                        channel.update_at,
                        member.as_ref().map(|m| m.notify_props.clone()),
                    );
                    if self.sent_channels.get(&channel_id) != Some(&sent) {
                        self.sent_channels.insert(channel_id.clone(), sent);
                        changes.push(ChangeSet::Channel {
                            channel: Arc::new(channel),
                            member: member.map(Arc::new),
                        });
                    }
                }
                Ok(None) => {}
                Err(error) => tracing::error!(%error, "could not read a channel"),
            }
            let floor = self.floors.get(&channel_id).copied();
            match snapshot(
                conn,
                &channel_id,
                self.history.get(&channel_id),
                floor,
                roots_only,
            ) {
                Ok(window) => changes.push(ChangeSet::Posts {
                    channel_id,
                    posts: Arc::from(window),
                }),
                Err(error) => tracing::error!(%error, "could not read a posts snapshot"),
            }
        }
        if let Some(users) = self.authors(&changes) {
            // Ahead of the posts, so names are there when they draw.
            changes.insert(0, users);
        }
        for change in changes {
            self.subscribers
                .retain(|subscriber| subscriber.try_send(change.clone()).is_ok());
        }
    }

    /// The not yet sent authors of the posts in `changes`: those the cache
    /// has as a change set, and the rest asked of the server once.
    fn authors(&mut self, changes: &[ChangeSet]) -> Option<ChangeSet> {
        let ids: BTreeSet<String> = changes
            .iter()
            .flat_map(|change| match change {
                ChangeSet::Posts { posts, .. }
                | ChangeSet::Thread { posts, .. }
                | ChangeSet::Search { posts, .. } => &posts[..],
                _ => &[],
            })
            .map(|post| post.user_id.clone())
            .filter(|id| !id.is_empty() && !self.sent_users.contains(id))
            .collect();
        if ids.is_empty() {
            return None;
        }
        let ids: Vec<String> = ids.into_iter().collect();
        let found = sidebar::users(self.store.conn(), &ids).unwrap_or_else(|error| {
            tracing::error!(%error, "could not read users");
            Vec::new()
        });
        self.sent_users.extend(found.iter().map(|u| u.id.clone()));
        let missing: Vec<String> = ids
            .into_iter()
            .filter(|id| !self.sent_users.contains(id) && !self.asked_users.contains(id))
            .collect();
        if let Some(session) = self.session.as_ref()
            && !missing.is_empty()
        {
            self.asked_users.extend(missing.iter().cloned());
            let client = session.client.clone();
            let loaded = self.loaded.clone();
            tokio::spawn(async move {
                match client.users_by_ids(&missing).await {
                    Ok(users) => {
                        let _ = loaded.send(Loaded::Users(users));
                    }
                    Err(error) => tracing::warn!(%error, "could not fetch post authors"),
                }
            });
        }
        (!found.is_empty()).then(|| ChangeSet::Users(Arc::from(found)))
    }
}

impl Engine {
    /// Preferences changed: the collapsed-threads choice may have too, which
    /// changes every count and the channel windows.
    fn preferences_changed(&mut self, dirty: &mut Dirty) {
        dirty.direct.push(ChangeSet::Preferences);
        let Some(capabilities) = &self.capabilities else {
            return;
        };
        let conn = self.store.conn();
        let Ok(preferences) = all_preferences(conn) else {
            return;
        };
        let crt = collapsed_threads(capabilities.collapsed_threads, &preferences);
        if cached_crt(conn).ok() == Some(crt) {
            return;
        }
        if let Err(error) = posts::set_cursor(conn, CRT_KEY, if crt { "1" } else { "0" }) {
            tracing::error!(%error, "could not store the collapsed-threads mode");
        }
        if let Some(session) = &mut self.session {
            session.collapsed_threads = crt;
        }
        dirty.all_teams = true;
        dirty.unreads = true;
        dirty.channels.extend(self.open_channels.iter().cloned());
    }
}

fn all_preferences(conn: &Db) -> Result<Vec<Preference>, StoreError> {
    let mut statement = conn.prepare_cached("SELECT category, name, value FROM preferences")?;
    let preferences = statement
        .query_map([], |row| {
            Ok(Preference {
                category: row.get(0)?,
                name: row.get(1)?,
                value: row.get(2)?,
                ..Preference::default()
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(preferences)
}

/// Shows or hides the user's reaction in the store.
fn react_locally(
    conn: &Db,
    reaction: &Reaction,
    add: bool,
    dirty: &mut Dirty,
) -> Result<(), StoreError> {
    let Some(post) = posts::post(conn, &reaction.post_id)? else {
        return Ok(());
    };
    if add {
        posts::add_reaction(conn, reaction)?;
    } else {
        posts::remove_reaction(conn, reaction)?;
    }
    events::touch_posts(&post, dirty);
    Ok(())
}

fn cached_crt(conn: &Db) -> Result<bool, StoreError> {
    Ok(posts::cursor(conn, CRT_KEY)?.as_deref() == Some("1"))
}

fn cached_team_ids(conn: &Db) -> Result<Vec<String>, StoreError> {
    let mut statement = conn.prepare_cached("SELECT id FROM teams WHERE delete_at = 0")?;
    let ids = statement
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// Whether threads are collapsed for this user: the server's mode, then the
/// user's `display_settings/collapsed_reply_threads` where the server lets
/// them choose.
pub(crate) fn collapsed_threads(mode: CollapsedThreads, preferences: &[Preference]) -> bool {
    let chosen = preferences
        .iter()
        .find(|p| p.category == "display_settings" && p.name == "collapsed_reply_threads")
        .map(|p| p.value == "on");
    match mode {
        CollapsedThreads::Disabled => false,
        CollapsedThreads::AlwaysOn => true,
        CollapsedThreads::DefaultOn => chosen.unwrap_or(true),
        CollapsedThreads::DefaultOff => chosen.unwrap_or(false),
    }
}

/// The media cache key for one of a file's images.
fn image_key(file_id: &str, size: ImageSize) -> String {
    use matty_store::media::{FileVariant, file_key};
    file_key(
        file_id,
        match size {
            ImageSize::Thumbnail => FileVariant::Thumb,
            ImageSize::Preview => FileVariant::Preview,
        },
    )
}

/// Posts per page of history (PLAN §4.2).
const HISTORY_PAGE: u32 = 60;

/// How far back an open channel's snapshot reaches.
#[derive(Debug, Default)]
struct History {
    /// Posts beyond [`POSTS_WINDOW`].
    extra: usize,
    /// A stretch away from the newest posts, from and to these creation
    /// times, which replaces the latest posts until it reaches them.
    anchor: Option<(i64, i64)>,
    /// A page is on its way.
    loading: bool,
    /// The channel's first post is loaded.
    at_start: bool,
}

impl History {
    fn change(&self, channel_id: &str, failed: bool) -> ChangeSet {
        ChangeSet::History {
            channel_id: channel_id.to_owned(),
            at_start: self.at_start,
            at_end: self.anchor.is_none(),
            failed,
        }
    }
}

/// Leaving a history window before it reached the newest post: what it
/// loaded stays stored but out of the latest snapshot, which would
/// otherwise show it above a gap.
fn give_up(
    floors: &mut std::collections::HashMap<String, i64>,
    channel_id: &str,
    history: &History,
) {
    if let Some((_, to)) = history.anchor {
        let floor = floors.entry(channel_id.to_owned()).or_insert(to);
        *floor = (*floor).max(to);
    }
}

/// Which page of history to fetch.
#[derive(Debug)]
enum Page {
    Older,
    Newer,
    Around(String),
}

/// An open channel's posts snapshot: its history window, or else the
/// latest posts reaching back as far as history has loaded, newer than
/// `floor` (see `Engine::floors`).
fn snapshot(
    conn: &rusqlite::Connection,
    channel_id: &str,
    history: Option<&History>,
    floor: Option<i64>,
    roots_only: bool,
) -> Result<Vec<matty_api::models::Post>, StoreError> {
    match history.and_then(|h| h.anchor) {
        Some((from, to)) => posts::posts_window(
            conn,
            channel_id,
            Window::Between(from, to),
            usize::MAX,
            roots_only,
        ),
        None => {
            let mut posts = posts::posts_window(
                conn,
                channel_id,
                Window::Latest,
                POSTS_WINDOW + history.map_or(0, |h| h.extra),
                roots_only,
            )?;
            if let Some(floor) = floor {
                posts.retain(|post| post.create_at > floor);
            }
            Ok(posts)
        }
    }
}
