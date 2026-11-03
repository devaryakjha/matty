//! What the engine tells the views.

use std::sync::Arc;

use matty_api::ServerCapabilities;
use matty_api::models::{Channel, ChannelStats, Draft, Post, Status, User};
use matty_api::ws::ConnectionState;
use matty_store::outbox::Outgoing;
use matty_store::posts::ThreadSummary;
use matty_store::sidebar::{SidebarSection, TeamWithUnreads};

/// Posts kept in an open channel's snapshot.
pub const POSTS_WINDOW: usize = 200;

/// Something the views should redraw from. Snapshots are built on the sync
/// thread and shared, never copied, on the way to the UI.
#[derive(Clone, Debug, PartialEq)]
pub enum ChangeSet {
    /// A team's sidebar changed.
    Sidebar {
        /// The team; empty for direct and group messages.
        team_id: String,
        /// The sidebar as it now stands.
        sections: Arc<[SidebarSection]>,
    },
    /// An open channel's posts changed.
    Posts {
        /// The channel.
        channel_id: String,
        /// The newest posts, oldest first, at most [`POSTS_WINDOW`].
        posts: Arc<[Post]>,
    },
    /// An open thread changed.
    Thread {
        /// The root post's id.
        root_id: String,
        /// The root, then its replies.
        posts: Arc<[Post]>,
    },
    /// A team's followed threads changed: the Threads view's list.
    ThreadList {
        /// The team.
        team_id: String,
        /// Followed threads, latest reply first.
        threads: Arc<[ThreadSummary]>,
    },
    /// A draft changed on this or another device; `None` when deleted.
    Draft {
        /// The channel.
        channel_id: String,
        /// The thread root, or empty for the channel's own draft.
        root_id: String,
        /// The draft now.
        draft: Option<Arc<Draft>>,
    },
    /// A channel's unsent messages changed. Their rows are in the posts
    /// snapshot with `id == pending_post_id`; this says which are still
    /// sending and which failed (`last_error`).
    Outbox {
        /// The channel.
        channel_id: String,
        /// Queued sends, oldest first.
        sends: Arc<[Outgoing]>,
    },
    /// A command could not be carried out.
    Failed(CommandError),
    /// A post only this user sees, such as a slash command's reply. It is
    /// never stored; the view shows it until the channel is left.
    Ephemeral(Arc<Post>),
    /// Unread or mention counts changed somewhere: the team rail with its
    /// badges, in rail order.
    Unreads {
        /// Every team the user is on.
        teams: Arc<[TeamWithUnreads]>,
    },
    /// People whose posts are on screen, each once per subscriber as they
    /// become known: from the cache at once, from the server when missing.
    Users(Arc<[User]>),
    /// An open channel as stored: name, header and purpose. Sent when it
    /// opens and when the server changes it.
    Channel(Arc<Channel>),
    /// An open channel's member and pinned-post counts, fetched as it
    /// opens.
    Stats(Arc<ChannelStats>),
    /// A team's public channels, joined or not, with member counts by
    /// channel id: the answer to [`Command::Browse`](crate::Command::Browse).
    Directory {
        /// The team.
        team_id: String,
        /// Its public channels.
        channels: Arc<[Channel]>,
        /// Members by channel id.
        members: Arc<std::collections::HashMap<String, i64>>,
    },
    /// Search results, best first: the answer to
    /// [`Command::Search`](crate::Command::Search). Their authors come
    /// ahead in [`ChangeSet::Users`].
    Search {
        /// What was searched for.
        terms: String,
        /// The posts that matched.
        posts: Arc<[Post]>,
    },
    /// A team's active people: the answer to
    /// [`Command::FindPeople`](crate::Command::FindPeople).
    People {
        /// The team.
        team_id: String,
        /// Its people.
        users: Arc<[User]>,
    },
    /// These users' statuses changed.
    Presence(Arc<[Status]>),
    /// A channel the user asked to open ([`Command::OpenDirect`] or
    /// [`Command::JoinChannel`](crate::Command::JoinChannel)) is ready: it is
    /// in the sidebar, so the view can switch to it.
    ///
    /// [`Command::OpenDirect`]: crate::Command::OpenDirect
    ChannelReady {
        /// The channel.
        channel_id: String,
    },
    /// Someone started typing.
    Typing {
        /// Who.
        user_id: String,
        /// Where.
        channel_id: String,
        /// The thread root when typing a reply, else empty.
        parent_id: String,
    },
    /// The connection to the server, or the load over it, changed.
    Connection(Connection),
    /// The user's preferences changed.
    Preferences,
    /// The server's config or license was read.
    Config(Arc<ServerCapabilities>),
}

/// Where syncing with the server stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Connection {
    /// The WebSocket's state.
    Socket(ConnectionState),
    /// The initial load started; the views show what the cache had.
    Loading,
    /// Every team has loaded, or failed to.
    Loaded,
    /// A catch-up after missed events finished.
    Resynced,
    /// One team could not load; the others did.
    TeamFailed {
        /// The team.
        team_id: String,
        /// What went wrong, for the log and a retry banner.
        error: String,
    },
    /// The account could not load at all.
    Failed {
        /// What went wrong.
        error: String,
    },
    /// [`Command::SignOut`](crate::Command::SignOut) finished: the session
    /// is gone here and, when the server could be reached, there too.
    SignedOut,
    /// An opened channel's posts could not load; the cached ones stay.
    ChannelFailed {
        /// The channel.
        channel_id: String,
        /// What went wrong.
        error: String,
    },
}

/// Why a command was not carried out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    /// The post is still in the outbox: it can be retried or discarded, but
    /// not edited or deleted until the server has it.
    NotSentYet {
        /// The post's pending id.
        post_id: String,
    },
    /// The command needs a connection and there is none.
    Offline,
    /// The server refused or failed.
    Server {
        /// What was asked, such as `edit`.
        action: &'static str,
        /// The post, channel or user it was about.
        target: String,
        /// What the server said.
        error: String,
    },
}
