//! The loop on the sync thread: apply what arrives to the store in batches,
//! then publish what changed.

use std::collections::BTreeSet;
use std::sync::Arc;

use matty_api::ws::WsEvent;
use matty_store::posts::{self, Window};
use matty_store::{Store, StoreError};
use rusqlite::Transaction;
use tokio::sync::mpsc;

use crate::change::{ChangeSet, POSTS_WINDOW};

/// Most messages applied in one transaction, so a flood still publishes.
const BATCH: usize = 512;

/// What the app asks of the engine.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Command {
    /// A decoded server event to apply, from the WebSocket.
    Event(Box<WsEvent>),
    /// A channel is on screen: publish its posts now and whenever they change.
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

pub(crate) enum Message {
    Command(Command),
    Subscribe(async_channel::Sender<ChangeSet>),
}

/// What a batch touched.
#[derive(Default)]
struct Dirty {
    channels: BTreeSet<String>,
    direct: Vec<ChangeSet>,
}

pub(crate) struct Engine {
    store: Store,
    open_channels: BTreeSet<String>,
    subscribers: Vec<async_channel::Sender<ChangeSet>>,
}

impl Engine {
    pub(crate) fn new(store: Store) -> Self {
        Self {
            store,
            open_channels: BTreeSet::new(),
            subscribers: Vec::new(),
        }
    }

    /// Runs until every [`SyncHandle`](crate::SyncHandle) is dropped.
    pub(crate) async fn run(mut self, mut inbox: mpsc::UnboundedReceiver<Message>) {
        let mut batch = Vec::with_capacity(BATCH);
        while inbox.recv_many(&mut batch, BATCH).await > 0 {
            let mut dirty = Dirty::default();
            if let Err(error) = self.apply(batch.drain(..), &mut dirty) {
                tracing::error!(%error, "sync batch failed");
            }
            self.publish(dirty);
        }
    }

    fn apply(
        &mut self,
        messages: impl Iterator<Item = Message>,
        dirty: &mut Dirty,
    ) -> Result<(), StoreError> {
        let tx = self.store.conn_mut().transaction()?;
        for message in messages {
            match message {
                Message::Subscribe(sender) => self.subscribers.push(sender),
                Message::Command(Command::OpenChannel { channel_id }) => {
                    self.open_channels.insert(channel_id.clone());
                    dirty.channels.insert(channel_id);
                }
                Message::Command(Command::CloseChannel { channel_id }) => {
                    self.open_channels.remove(&channel_id);
                }
                Message::Command(Command::Event(event)) => apply_event(&tx, *event, dirty)?,
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn publish(&mut self, dirty: Dirty) {
        let mut changes = dirty.direct;
        for channel_id in dirty.channels {
            if !self.open_channels.contains(&channel_id) {
                continue;
            }
            match posts::posts_window(
                self.store.conn(),
                &channel_id,
                Window::Latest,
                POSTS_WINDOW,
                false,
            ) {
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
