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

use crate::change::{ChangeSet, CommandError, Connection, POSTS_WINDOW};
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
    /// A decoded server event to apply, from the WebSocket.
    Event(Box<WsEvent>),
    /// A channel is on screen: publish its posts now and whenever they
    /// change, and fetch them once per connection.
    OpenChannel {
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
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect { .. } => f.write_str("Connect"),
            Self::Event(event) => f.debug_tuple("Event").field(event).finish(),
            Self::OpenChannel { channel_id } => f
                .debug_struct("OpenChannel")
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
        }
    }
}

pub(crate) enum Message {
    Command(Command),
    Subscribe(async_channel::Sender<ChangeSet>),
}

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
    session: Option<Session>,
    loaded: mpsc::UnboundedSender<Loaded>,
    loaded_inbox: Option<mpsc::UnboundedReceiver<Loaded>>,
    typing: TypingThrottle,
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
            session: None,
            loaded,
            loaded_inbox: Some(loaded_inbox),
            typing: TypingThrottle::default(),
        }
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
        let mut fetches = Vec::new();
        let mut sends = Vec::new();
        let mut writes = Vec::new();
        let tx = self.store.conn_mut().transaction()?;
        for message in messages {
            match message {
                Message::Subscribe(sender) => self.subscribers.push(sender),
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
                        resyncing: false,
                        resync_again: false,
                    });
                }
                Message::Command(Command::OpenChannel { channel_id }) => {
                    self.open_channels.insert(channel_id.clone());
                    to_fetch.push(channel_id.clone());
                    dirty.channels.insert(channel_id);
                }
                Message::Command(Command::CloseChannel { channel_id }) => {
                    self.open_channels.remove(&channel_id);
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
                    sends.push(outgoing.pending_post_id);
                }
                Message::Command(Command::Retry { pending_post_id }) => {
                    if queue::get(&tx, &pending_post_id)?.is_some() {
                        sends.push(pending_post_id);
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
        for pending_post_id in sends {
            self.send(&pending_post_id, dirty)?;
        }
        for write in writes {
            self.write(write, dirty);
        }
        Ok(())
    }

    /// Starts sending a queued post, or fails it when offline.
    fn send(&mut self, pending_post_id: &str, dirty: &mut Dirty) -> Result<(), StoreError> {
        let conn = self.store.conn();
        let Some(outgoing) = queue::get(conn, pending_post_id)? else {
            return Ok(());
        };
        let Some(session) = &self.session else {
            return outbox::fail(conn, pending_post_id, "not connected", dirty);
        };
        queue::attempt(conn, pending_post_id)?;
        dirty.outbox.insert(outgoing.channel_id.clone());
        tokio::spawn(outbox::send(
            session.client.clone(),
            outgoing,
            self.loaded.clone(),
        ));
        Ok(())
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
            Loaded::Failed(error) => {
                tracing::warn!(%error, "the account did not load");
                dirty
                    .direct
                    .push(ChangeSet::Connection(Connection::Failed { error }));
            }
            Loaded::Emoji(emoji) => sidebar::upsert_emoji(self.store.conn(), &emoji)?,
            Loaded::Statuses(statuses) => {
                dirty.direct.push(ChangeSet::Presence(Arc::from(statuses)));
            }
            Loaded::Done => dirty.direct.push(ChangeSet::Connection(Connection::Loaded)),
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
            Loaded::Channel { channel, member } => {
                let tx = self.store.conn_mut().transaction()?;
                sidebar::upsert_channels(&tx, std::slice::from_ref(&*channel))?;
                if let Some(member) = member {
                    sidebar::upsert_channel_members(&tx, std::slice::from_ref(&*member))?;
                }
                tx.commit()?;
                // A new channel lands in a category the server picked.
                if channel.team_id.is_empty() {
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
                    if let ConnectionState::Connected { .. } = state
                        && let Some(session) = &mut self.session
                    {
                        // Even a resumed stream can miss channel events, so
                        // every reconnect catches up (WsSignal docs).
                        if session.was_connected {
                            dirty.resync = true;
                        }
                        session.was_connected = true;
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
            }
            Loaded::SendFailed {
                pending_post_id,
                error,
            } => {
                tracing::warn!(%pending_post_id, %error, "a send failed");
                outbox::fail(self.store.conn(), &pending_post_id, &error, dirty)?;
            }
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
            match posts::posts_window(conn, &channel_id, Window::Latest, POSTS_WINDOW, roots_only) {
                Ok(window) => changes.push(ChangeSet::Posts {
                    channel_id,
                    posts: Arc::from(window),
                }),
                Err(error) => tracing::error!(%error, "could not read a posts snapshot"),
            }
        }
        for change in changes {
            self.subscribers
                .retain(|subscriber| subscriber.try_send(change.clone()).is_ok());
        }
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
