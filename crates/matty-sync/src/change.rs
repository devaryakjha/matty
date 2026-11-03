//! What the engine tells the views.

use std::sync::Arc;

use matty_api::models::Post;
use matty_api::ws::ConnectionState;
use matty_store::sidebar::SidebarSection;

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
    /// Unread or mention counts changed somewhere; team badges included.
    Unreads,
    /// These users' statuses changed.
    Presence(Arc<[String]>),
    /// Someone started typing.
    Typing {
        /// Who.
        user_id: String,
        /// Where.
        channel_id: String,
        /// The thread root when typing a reply, else empty.
        parent_id: String,
    },
    /// The connection to the server changed.
    Connection(ConnectionState),
    /// The user's preferences changed.
    Preferences,
    /// The server's config or license changed.
    Config,
}
