//! What the engine tells the views.

use std::sync::Arc;

use matty_api::ServerCapabilities;
use matty_api::models::{Channel, ChannelMember, ChannelStats, Draft, Post, Status, User};
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
        /// The threads' root posts, where cached, for their excerpts.
        roots: Arc<[Post]>,
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
    /// An open channel as stored: name, header and purpose, with the
    /// user's membership (notification properties). Sent when it opens and
    /// when either changes.
    Channel {
        /// The channel.
        channel: Arc<Channel>,
        /// The user's membership, once known.
        member: Option<Arc<ChannelMember>>,
    },
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
    /// Older posts came for a channel: the next
    /// [`ChangeSet::Posts`] reaches further back. `at_start` once the
    /// channel's first post is in it, so there is nothing more to load.
    History {
        /// The channel.
        channel_id: String,
        /// The channel's first post is loaded.
        at_start: bool,
        /// The snapshot runs to the newest post. After a
        /// [`Command::LoadAround`](crate::Command::LoadAround) it stops
        /// short of it until [`Command::LoadNewer`](crate::Command::LoadNewer)
        /// pages reach it.
        at_end: bool,
        /// The page failed; asking again retries it.
        failed: bool,
    },
    /// An upload moved on.
    Upload {
        /// The caller's id for it.
        upload_id: String,
        /// Where it is.
        state: UploadState,
    },
    /// A download finished: the answer to
    /// [`Command::Download`](crate::Command::Download).
    Download {
        /// The file.
        file_id: String,
        /// Where it went, or why not.
        state: DownloadState,
    },
    /// A file's image: the answer to
    /// [`Command::LoadImage`](crate::Command::LoadImage).
    Image {
        /// The file.
        file_id: String,
        /// Which rendition.
        size: ImageSize,
        /// The encoded image.
        bytes: Arc<[u8]>,
        /// Its MIME type, such as `image/jpeg`.
        content_type: String,
    },
    /// A link's Open Graph data, or `None` when it has none: the answer to
    /// [`Command::LoadLinkPreview`](crate::Command::LoadLinkPreview).
    LinkPreview {
        /// The URL asked for.
        url: String,
        /// What the server read from it.
        graph: Option<Arc<matty_api::models::OpenGraph>>,
    },
    /// An outside image: the answer to
    /// [`Command::LoadRemoteImage`](crate::Command::LoadRemoteImage).
    RemoteImage {
        /// The URL asked for.
        url: String,
        /// The encoded image.
        bytes: Arc<[u8]>,
        /// Its MIME type, such as `image/png`.
        content_type: String,
    },
    /// A user's profile picture: the answer to
    /// [`Command::LoadAvatar`](crate::Command::LoadAvatar), with no image
    /// when it could not be had.
    Avatar {
        /// The user.
        user_id: String,
        /// The `last_picture_update` the picture is for.
        last_picture_update: i64,
        /// The encoded image and its MIME type.
        image: Option<(Arc<[u8]>, String)>,
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
    /// The signed-in user as the server sent them at sign-in, with their
    /// own `notify_props`; later changes come as [`ChangeSet::Users`].
    Me(Arc<User>),
    /// These users' statuses changed.
    Presence(Arc<[Status]>),
    /// Someone else's post just arrived over the socket, with what the
    /// notification policy needs (#141). Posts loaded by a fetch or a
    /// catch-up never come this way, so they never notify.
    Arrived(Arc<Arrived>),
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
    /// Whether collapsed reply threads are on for the user, at sign-in and
    /// whenever the preferences or the server's config change.
    CollapsedThreads(bool),
    /// Whether link previews show for the user: the server allows them
    /// and the user's `display_settings/link_previews` is not off (#187).
    LinkPreviews(bool),
    /// The server's config or license was read.
    Config(Arc<ServerCapabilities>),
    /// What a link in a message points at: the answer to
    /// [`Command::ResolveLink`](crate::Command::ResolveLink). `None` when
    /// it is gone or the user may not see it.
    LinkResolved {
        /// What was asked.
        query: LinkQuery,
        /// The channel (for a post, the post's channel) or the user.
        answer: Option<LinkAnswer>,
    },
}

/// Something a link names that the app cannot find on its own (#85, #103).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkQuery {
    /// A permalink's post, which may be in any channel.
    Post {
        /// The post.
        post_id: String,
    },
    /// A channel by its team and URL name, as `~name` and channel links
    /// write it.
    Channel {
        /// The team.
        team_id: String,
        /// The channel's URL name.
        name: String,
    },
    /// A person by username, as `@name` writes it.
    User {
        /// The username.
        username: String,
    },
}

/// What a [`LinkQuery`] found.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkAnswer {
    /// The channel, whether or not the user is a member.
    Channel(Arc<Channel>),
    /// The person.
    User(Arc<User>),
}

/// A post that just arrived from someone else.
#[derive(Clone, Debug, PartialEq)]
pub struct Arrived {
    /// The post.
    pub post: Post,
    /// The channel's kind: `O`, `P`, `D` or `G`.
    pub channel_type: String,
    /// The channel's display name, as the server sent it.
    pub channel_display_name: String,
    /// The author's name, as the server formats it.
    pub sender_name: String,
    /// The server listed the user among those the post mentions.
    pub mentioned: bool,
    /// The server listed the user among the thread's followers.
    pub followed: bool,
    /// The user's channel member `notify_props`, empty when not cached.
    pub channel_props: std::collections::HashMap<String, String>,
    /// Collapsed reply threads are on.
    pub collapsed_threads: bool,
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
    /// The server refused the session token: it expired or was revoked,
    /// so the user has to sign in again.
    Expired,
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

/// Which of a file's images: the server scales each image it is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageSize {
    /// About 120 pixels, for the tile on a post.
    Thumbnail,
    /// Up to about 1920 pixels wide, for the lightbox.
    Preview,
}

/// How a download ended.
#[derive(Clone, Debug, PartialEq)]
pub enum DownloadState {
    /// Saved here.
    Done(std::path::PathBuf),
    /// Not saved, and why.
    Failed(String),
}

/// Where an upload is.
#[derive(Clone, Debug, PartialEq)]
pub enum UploadState {
    /// Sending, from 0.0 to 1.0.
    Sending(f32),
    /// On the server; the id goes in the post's `file_ids`.
    Done {
        /// The server's file id.
        file_id: String,
    },
    /// The server refused it or the connection failed.
    Failed(String),
}

/// Why a command was not carried out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    /// The post is on its way to the server: it can be neither edited nor
    /// discarded until the send comes back. A post still waiting in the
    /// outbox can be edited or discarded (#195).
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
