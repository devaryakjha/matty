//! What the engine tells the views.

use std::sync::Arc;

use matty_api::ServerCapabilities;
use matty_api::models::{Draft, Post, Status};
use matty_api::ws::ConnectionState;
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
    /// A post only this user sees, such as a slash command's reply. It is
    /// never stored; the view shows it until the channel is left.
    Ephemeral(Arc<Post>),
    /// Unread or mention counts changed somewhere: the team rail with its
    /// badges, in rail order.
    Unreads {
        /// Every team the user is on.
        teams: Arc<[TeamWithUnreads]>,
    },
    /// These users' statuses changed.
    Presence(Arc<[Status]>),
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
    /// An opened channel's posts could not load; the cached ones stay.
    ChannelFailed {
        /// The channel.
        channel_id: String,
        /// What went wrong.
        error: String,
    },
}
