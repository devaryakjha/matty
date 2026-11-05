//! Channel bookmarks (#226): what the bar under a channel's header shows.
//!
//! The store keeps each channel's bookmarks so the bar shows offline. A
//! channel's list is fetched once per session when it opens (on servers
//! with bookmarks), events keep it current, and every change publishes the
//! channel's whole list as [`ChangeSet::Bookmarks`]. Moving and deleting
//! show at once; a refusal fetches the list again so the bar matches the
//! server, and says so with [`CommandError::Server`].

use std::sync::Arc;

use matty_api::models::{
    BookmarkKind, BookmarkPatch, BookmarkUpdate, ChannelBookmark, NewBookmark,
};
use matty_api::{ApiError, Client};
use matty_store::{StoreError, bookmarks};
use rusqlite::Connection;
use tokio::sync::mpsc::UnboundedSender;

use crate::change::{ChangeSet, CommandError};
use crate::engine::Dirty;
use crate::load::Loaded;

/// The most bookmarks a channel holds, as the server allows.
pub const MAX_BOOKMARKS: usize = 50;

/// A change to a channel's bookmarks the user asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BookmarkCommand {
    /// Adds a link bookmark at the end of the bar.
    AddLink {
        /// The channel.
        channel_id: String,
        /// The name shown.
        display_name: String,
        /// The address.
        link_url: String,
        /// An emoji name without colons, or empty.
        emoji: String,
    },
    /// Changes a bookmark's name, address (for a link) or emoji.
    Edit {
        /// The channel.
        channel_id: String,
        /// The bookmark.
        bookmark_id: String,
        /// The name shown.
        display_name: String,
        /// The address; ignored for a file bookmark.
        link_url: String,
        /// An emoji name without colons, or empty for none.
        emoji: String,
    },
    /// Deletes a bookmark.
    Delete {
        /// The channel.
        channel_id: String,
        /// The bookmark.
        bookmark_id: String,
    },
    /// Moves a bookmark to `index` in the bar, counted once it has left
    /// its place.
    Move {
        /// The channel.
        channel_id: String,
        /// The bookmark.
        bookmark_id: String,
        /// Its new place, from 0.
        index: usize,
    },
}

impl BookmarkCommand {
    /// What the user asked, for [`CommandError`].
    pub fn action(&self) -> &'static str {
        match self {
            Self::AddLink { .. } => "add bookmark",
            Self::Edit { .. } => "edit bookmark",
            Self::Delete { .. } => "delete bookmark",
            Self::Move { .. } => "move bookmark",
        }
    }

    /// The channel it is about.
    pub fn channel_id(&self) -> &str {
        match self {
            Self::AddLink { channel_id, .. }
            | Self::Edit { channel_id, .. }
            | Self::Delete { channel_id, .. }
            | Self::Move { channel_id, .. } => channel_id,
        }
    }
}

/// A server answer about bookmarks, for the engine to write.
#[derive(Debug)]
pub(crate) enum BookmarksLoaded {
    /// A channel's whole list.
    List {
        channel_id: String,
        bookmarks: Vec<ChannelBookmark>,
    },
    /// A new or changed bookmark.
    Saved(ChannelBookmark),
    /// An edit, which may have replaced the bookmark.
    Updated(BookmarkUpdate),
    /// A deleted bookmark.
    Deleted(ChannelBookmark),
    /// Bookmarks whose place changed.
    Sorted {
        channel_id: String,
        bookmarks: Vec<ChannelBookmark>,
    },
    /// The server refused a command.
    Refused {
        channel_id: String,
        action: &'static str,
        error: String,
    },
}

/// Publishes a channel's stored bookmarks.
pub(crate) fn publish(
    conn: &Connection,
    channel_id: &str,
    dirty: &mut Dirty,
) -> Result<(), StoreError> {
    let list = bookmarks::for_channel(conn, channel_id)?;
    dirty.direct.push(ChangeSet::Bookmarks {
        channel_id: channel_id.to_owned(),
        bookmarks: Arc::from(list),
    });
    Ok(())
}

/// A channel opened: shows its stored bookmarks when the server has
/// bookmarks, or when it is not known yet and some are stored.
pub(crate) fn opened(
    conn: &Connection,
    channel_id: &str,
    supported: Option<bool>,
    dirty: &mut Dirty,
) -> Result<(), StoreError> {
    match supported {
        Some(false) => Ok(()),
        Some(true) => publish(conn, channel_id, dirty),
        None if bookmarks::for_channel(conn, channel_id)?.is_empty() => Ok(()),
        None => publish(conn, channel_id, dirty),
    }
}

/// Fetches a channel's bookmarks. A failure leaves the stored ones.
pub(crate) async fn fetch(client: Client, channel_id: String, results: UnboundedSender<Loaded>) {
    match client.channel_bookmarks(&channel_id).await {
        Ok(bookmarks) => {
            let _ = results.send(Loaded::Bookmarks(BookmarksLoaded::List {
                channel_id,
                bookmarks,
            }));
        }
        Err(error) => tracing::warn!(%error, "could not load a channel's bookmarks"),
    }
}

/// Applies a command: moves and deletes show at once, then the server is
/// asked. Without a session it is refused as offline.
pub(crate) fn command(
    conn: &Connection,
    client: Option<Client>,
    results: &UnboundedSender<Loaded>,
    command: BookmarkCommand,
    dirty: &mut Dirty,
) -> Result<(), StoreError> {
    let Some(client) = client else {
        dirty.direct.push(ChangeSet::Failed(CommandError::Offline {
            action: command.action(),
        }));
        return Ok(());
    };
    match &command {
        BookmarkCommand::Delete {
            channel_id,
            bookmark_id,
        } => {
            bookmarks::remove(conn, bookmark_id)?;
            publish(conn, channel_id, dirty)?;
        }
        BookmarkCommand::Move {
            channel_id,
            bookmark_id,
            index,
        } => {
            let mut list = bookmarks::for_channel(conn, channel_id)?;
            if let Some(from) = list.iter().position(|b| b.id == *bookmark_id) {
                let moved = list.remove(from);
                list.insert((*index).min(list.len()), moved);
                for (order, bookmark) in list.iter_mut().enumerate() {
                    bookmark.sort_order = i64::try_from(order).unwrap_or(i64::MAX);
                    bookmarks::upsert(conn, bookmark)?;
                }
                publish(conn, channel_id, dirty)?;
            }
        }
        BookmarkCommand::AddLink { .. } | BookmarkCommand::Edit { .. } => {}
    }
    matty_api::rt::spawn(run(client, command, results.clone()));
    Ok(())
}

/// Asks the server, answering with what it said.
async fn run(client: Client, command: BookmarkCommand, results: UnboundedSender<Loaded>) {
    let action = command.action();
    let channel_id = command.channel_id().to_owned();
    let outcome: Result<BookmarksLoaded, ApiError> = async {
        Ok(match command {
            BookmarkCommand::AddLink {
                channel_id,
                display_name,
                link_url,
                emoji,
            } => BookmarksLoaded::Saved(
                client
                    .create_bookmark(&NewBookmark {
                        channel_id,
                        display_name,
                        kind: BookmarkKind::Link,
                        link_url,
                        file_id: String::new(),
                        emoji,
                    })
                    .await?,
            ),
            BookmarkCommand::Edit {
                channel_id,
                bookmark_id,
                display_name,
                link_url,
                emoji,
            } => {
                let patch = BookmarkPatch {
                    display_name: Some(display_name),
                    link_url: (!link_url.is_empty()).then_some(link_url),
                    emoji: Some(emoji),
                };
                BookmarksLoaded::Updated(
                    client
                        .patch_bookmark(&channel_id, &bookmark_id, &patch)
                        .await?,
                )
            }
            BookmarkCommand::Delete {
                channel_id,
                bookmark_id,
            } => BookmarksLoaded::Deleted(
                client.delete_bookmark(&channel_id, &bookmark_id).await?,
            ),
            BookmarkCommand::Move {
                channel_id,
                bookmark_id,
                index,
            } => BookmarksLoaded::Sorted {
                bookmarks: client
                    .sort_bookmark(
                        &channel_id,
                        &bookmark_id,
                        i64::try_from(index).unwrap_or(i64::MAX),
                    )
                    .await?,
                channel_id,
            },
        })
    }
    .await;
    let loaded = match outcome {
        Ok(loaded) => loaded,
        Err(error) => {
            tracing::warn!(%error, action, "the server refused a bookmark change");
            BookmarksLoaded::Refused {
                channel_id: channel_id.clone(),
                action,
                error: refusal(&error),
            }
        }
    };
    let refused = matches!(loaded, BookmarksLoaded::Refused { .. });
    let _ = results.send(Loaded::Bookmarks(loaded));
    if refused {
        // Undo what showed at once: the server's list is the truth.
        fetch(client, channel_id, results).await;
    }
}

/// What to tell the user about a refusal.
fn refusal(error: &ApiError) -> String {
    if error.is_forbidden() {
        "You don't have permission to change this channel's bookmarks.".into()
    } else {
        error.to_string()
    }
}

/// Writes a server answer and publishes the channel's list.
pub(crate) fn write(
    conn: &Connection,
    loaded: BookmarksLoaded,
    dirty: &mut Dirty,
) -> Result<(), StoreError> {
    let channel_id = match loaded {
        BookmarksLoaded::List {
            channel_id,
            bookmarks: list,
        } => {
            bookmarks::replace(conn, &channel_id, &list)?;
            channel_id
        }
        BookmarksLoaded::Saved(bookmark) => {
            bookmarks::upsert(conn, &bookmark)?;
            bookmark.channel_id
        }
        BookmarksLoaded::Updated(update) => {
            bookmarks::apply_update(conn, &update)?;
            let Some(channel_id) = update
                .updated
                .or(update.deleted)
                .map(|bookmark| bookmark.channel_id)
            else {
                return Ok(());
            };
            channel_id
        }
        BookmarksLoaded::Deleted(bookmark) => {
            bookmarks::remove(conn, &bookmark.id)?;
            bookmark.channel_id
        }
        BookmarksLoaded::Sorted {
            channel_id,
            bookmarks: list,
        } => {
            for bookmark in &list {
                bookmarks::upsert(conn, bookmark)?;
            }
            channel_id
        }
        BookmarksLoaded::Refused {
            channel_id,
            action,
            error,
        } => {
            dirty.direct.push(ChangeSet::Failed(CommandError::Server {
                action,
                target: channel_id,
                error,
            }));
            return Ok(());
        }
    };
    publish(conn, &channel_id, dirty)
}

/// Applies a bookmark event from the WebSocket.
pub(crate) fn event(
    conn: &Connection,
    event: matty_api::ws::WsEvent,
    dirty: &mut Dirty,
) -> Result<(), StoreError> {
    use matty_api::ws::WsEvent;
    let loaded = match event {
        WsEvent::ChannelBookmarkCreated { bookmark, .. } => BookmarksLoaded::Saved(*bookmark),
        WsEvent::ChannelBookmarkUpdated { channel_id, update } => {
            bookmarks::apply_update(conn, &update)?;
            return publish(conn, &channel_id, dirty);
        }
        WsEvent::ChannelBookmarkDeleted { bookmark, .. } => BookmarksLoaded::Deleted(*bookmark),
        WsEvent::ChannelBookmarkSorted {
            channel_id,
            bookmarks,
        } => BookmarksLoaded::Sorted {
            channel_id,
            bookmarks,
        },
        _ => return Ok(()),
    };
    write(conn, loaded, dirty)
}
