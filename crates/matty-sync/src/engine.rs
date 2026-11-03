//! The loop on the sync thread: apply what arrives to the store in batches,
//! then publish what changed.

use std::collections::BTreeSet;
use std::sync::Arc;

use matty_api::Client;
use matty_api::capabilities::CollapsedThreads;
use matty_api::models::Preference;
use matty_api::ws::WsEvent;
use matty_store::posts::{self, Window};
use matty_store::sidebar::{self, UnreadMode};
use matty_store::{Store, StoreError};
use rusqlite::{Connection as Db, Transaction};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::change::{ChangeSet, Connection, POSTS_WINDOW};
use crate::load::{self, Loaded};

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
        }
    }
}

pub(crate) enum Message {
    Command(Command),
    Subscribe(async_channel::Sender<ChangeSet>),
}

/// What a batch touched.
#[derive(Default)]
struct Dirty {
    channels: BTreeSet<String>,
    teams: BTreeSet<String>,
    all_teams: bool,
    unreads: bool,
    direct: Vec<ChangeSet>,
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
}

pub(crate) struct Engine {
    store: Store,
    open_channels: BTreeSet<String>,
    subscribers: Vec<async_channel::Sender<ChangeSet>>,
    session: Option<Session>,
    loaded: mpsc::UnboundedSender<Loaded>,
    loaded_inbox: Option<mpsc::UnboundedReceiver<Loaded>>,
}

impl Engine {
    pub(crate) fn new(store: Store) -> Self {
        let (loaded, loaded_inbox) = mpsc::unbounded_channel();
        Self {
            store,
            open_channels: BTreeSet::new(),
            subscribers: Vec::new(),
            session: None,
            loaded,
            loaded_inbox: Some(loaded_inbox),
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
                    for result in results.drain(..) {
                        if let Err(error) = self.write_loaded(result, &mut dirty) {
                            tracing::error!(%error, "could not store loaded data");
                        }
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
                    self.session = Some(Session {
                        client,
                        user_id: String::new(),
                        collapsed_threads: cached_crt(&tx)?,
                        fetched: BTreeSet::new(),
                        load,
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
                Message::Command(Command::Event(event)) => apply_event(&tx, *event, dirty)?,
            }
        }
        tx.commit()?;
        for channel_id in to_fetch {
            self.fetch_channel(&channel_id);
        }
        Ok(())
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

    fn publish(&mut self, dirty: Dirty) {
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

/// Applies one server event. Events this engine does not handle yet are
/// ignored; the full set comes with #36.
fn apply_event(tx: &Transaction<'_>, event: WsEvent, dirty: &mut Dirty) -> Result<(), StoreError> {
    match event {
        WsEvent::Posted(posted) => {
            dirty.channels.insert(posted.post.channel_id.clone());
            posts::insert_posts(tx, std::slice::from_ref(&posted.post))?;
        }
        WsEvent::PostEdited(post) => {
            dirty.channels.insert(post.channel_id.clone());
            posts::insert_posts(tx, std::slice::from_ref(&post))?;
        }
        WsEvent::PostDeleted(post) => {
            dirty.channels.insert(post.channel_id.clone());
            posts::mark_deleted(tx, &post.id, post.delete_at)?;
        }
        WsEvent::ReactionAdded(reaction) => {
            if let Some(post) = posts::post(tx, &reaction.post_id)? {
                dirty.channels.insert(post.channel_id);
            }
            posts::add_reaction(tx, &reaction)?;
        }
        WsEvent::ReactionRemoved(reaction) => {
            if let Some(post) = posts::post(tx, &reaction.post_id)? {
                dirty.channels.insert(post.channel_id);
            }
            posts::remove_reaction(tx, &reaction)?;
        }
        WsEvent::Typing {
            user_id,
            channel_id,
            parent_id,
        } => dirty.direct.push(ChangeSet::Typing {
            user_id,
            channel_id,
            parent_id,
        }),
        _ => {}
    }
    Ok(())
}
