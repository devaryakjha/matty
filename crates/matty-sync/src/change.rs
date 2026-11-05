//! What the engine tells the views.

use std::sync::Arc;

use matty_api::ServerCapabilities;
use matty_api::models::{
    Channel, ChannelMember, ChannelStats, Draft, Post, Preference, Status, User,
};
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
    /// The roots of the replies in an open channel's posts that the posts
    /// leave out, for the "Commented on" line above a reply while collapsed
    /// reply threads are off (#123). Sent ahead of [`ChangeSet::Posts`].
    ReplyRoots {
        /// The channel.
        channel_id: String,
        /// The roots that are cached.
        roots: Arc<[Post]>,
    },
    /// An open thread changed.
    Thread {
        /// The root post's id.
        root_id: String,
        /// The root, then its replies.
        posts: Arc<[Post]>,
        /// Older replies are on the server and not in `posts` yet;
        /// [`Command::LoadOlderReplies`](crate::Command::LoadOlderReplies)
        /// pages them in (#122).
        older: bool,
        /// The last older page failed.
        failed: bool,
    },
    /// A team's followed threads changed: the Threads view's list.
    ThreadList {
        /// The team.
        team_id: String,
        /// Followed threads, latest reply first.
        threads: Arc<[ThreadSummary]>,
        /// The threads' root posts, where cached, for their excerpts.
        roots: Arc<[Post]>,
        /// The server follows more threads than `threads` lists;
        /// [`Command::LoadMoreThreads`](crate::Command::LoadMoreThreads)
        /// pages them in (#122).
        more: bool,
        /// The last page failed.
        failed: bool,
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
    /// The user's scheduled messages in every team, soonest first (#220),
    /// whenever they change. Only on servers that allow scheduling.
    Scheduled(Arc<[matty_api::models::ScheduledPost]>),
    /// A post only this user sees, such as a slash command's reply. It is
    /// never stored; the view shows it until the channel is left. The
    /// server changes one with `post_edited` and removes one with
    /// `post_deleted`, which come here too, the removal with `delete_at`
    /// set.
    Ephemeral(Arc<Post>),
    /// The server took an interactive message's button press or menu
    /// choice: the answer to
    /// [`Command::RunPostAction`](crate::Command::RunPostAction). A refusal
    /// comes as [`ChangeSet::Failed`] with the action `run action`.
    PostActionDone {
        /// The post.
        post_id: String,
        /// The button or menu.
        action_id: String,
    },
    /// What the server suggests for a slash command being typed: the answer
    /// to [`Command::SuggestCommands`](crate::Command::SuggestCommands),
    /// for that `user_input` where it was typed.
    CommandSuggestions {
        /// The channel.
        channel_id: String,
        /// The thread; empty for the channel.
        root_id: String,
        /// The line asked about.
        user_input: String,
        /// Its suggestions, in the server's order.
        suggestions: Arc<[matty_api::models::CommandSuggestion]>,
    },
    /// The server ran a slash command, or refused it: the answer to
    /// [`Command::ExecuteCommand`](crate::Command::ExecuteCommand). The
    /// refusal is the server's words, such as an unknown command's. A path
    /// in `goto_location` is made a URL on this server. An ephemeral answer
    /// also comes as [`ChangeSet::Ephemeral`] from the socket.
    CommandRan {
        /// The channel.
        channel_id: String,
        /// The thread; empty for the channel.
        root_id: String,
        /// The line that ran.
        command: String,
        /// The answer, or what the server said instead.
        outcome: Result<matty_api::models::CommandResponse, String>,
    },
    /// An integration opened an interactive dialog (#211), in answer to a
    /// button press or slash command of this user's.
    OpenDialog(Arc<matty_api::models::OpenDialogRequest>),
    /// The server passed on a dialog's submission or cancellation: the
    /// answer to [`Command::SubmitDialog`](crate::Command::SubmitDialog).
    /// The integration's complaints are in `answer`; empty, it took it. A
    /// failure to reach it comes as [`ChangeSet::Failed`] with the action
    /// `submit dialog`.
    DialogSubmitted {
        /// The dialog's callback id.
        callback_id: String,
        /// Whether this was the cancellation.
        cancelled: bool,
        /// What the integration said.
        answer: matty_api::models::SubmitDialogResponse,
    },
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
    /// A download's progress or end: the answer to
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
    /// A profile picture, custom emoji image or team icon: the answer to
    /// [`Command::LoadPicture`](crate::Command::LoadPicture), with no
    /// image when it could not be had.
    Picture {
        /// Which kind of picture.
        picture: Picture,
        /// The user, emoji or team.
        id: String,
        /// The version the picture is for.
        version: i64,
        /// The encoded image and its MIME type.
        image: Option<(Arc<[u8]>, String)>,
    },
    /// The server's custom emoji, by name, from the cache and again after
    /// each load. Their images come through
    /// [`Command::LoadPicture`](crate::Command::LoadPicture).
    CustomEmoji(Arc<[matty_api::models::Emoji]>),
    /// Search results, best first: the answer to
    /// [`Command::Search`](crate::Command::Search). Their authors come
    /// ahead in [`ChangeSet::Users`].
    Search {
        /// What was searched for.
        terms: String,
        /// Which page of results, from 0.
        page: u32,
        /// The posts that matched on this page.
        posts: Arc<[Post]>,
        /// A full page came back, so another may follow.
        more: bool,
    },
    /// A listing of posts outside their channel: the answer to
    /// [`Command::LoadListing`](crate::Command::LoadListing). Their authors
    /// come ahead in [`ChangeSet::Users`].
    Listing {
        /// Which listing.
        listing: Listing,
        /// Its posts, newest first.
        posts: Arc<[Post]>,
    },
    /// The ids of the posts the user saved, whenever they may have changed.
    Saved(Arc<[String]>),
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
    /// The user's `display_settings`, `advanced_settings` and `theme`
    /// preferences, at sign-in and whenever they change, for Preferences to
    /// mirror: message display and name format (#156), send on Ctrl+Enter
    /// (#157), and the Mattermost theme for every team and per team (#214).
    /// A preference the user never set is missing.
    AccountPreferences(Arc<[Preference]>),
    /// Whether collapsed reply threads are on for the user, at sign-in and
    /// whenever the preferences or the server's config change.
    CollapsedThreads(bool),
    /// Whether link previews show for the user: the server allows them
    /// and the user's `display_settings/link_previews` is not off (#187).
    LinkPreviews(bool),
    /// The server's config or license was read.
    Config(Arc<ServerCapabilities>),
    /// Who matches a typed `@name`: the answer to
    /// [`Command::SuggestPeople`](crate::Command::SuggestPeople), channel
    /// members first. Empty when no one does or the server could not say.
    Suggested {
        /// What was asked.
        term: String,
        /// The matches.
        users: Arc<[User]>,
    },
    /// What a link in a message points at: the answer to
    /// [`Command::ResolveLink`](crate::Command::ResolveLink). `None` when
    /// it is gone or the user may not see it.
    LinkResolved {
        /// What was asked.
        query: LinkQuery,
        /// The channel (for a post, the post's channel) or the user.
        answer: Option<LinkAnswer>,
    },
    /// A channel's bookmarks changed, or were read from the cache when it
    /// opened (#226): the whole list, in bar order.
    Bookmarks {
        /// The channel.
        channel_id: String,
        /// Its bookmarks.
        bookmarks: Arc<[matty_api::models::ChannelBookmark]>,
    },
    /// What installed Apps add in a channel: the answer to
    /// [`AppCommand::Bindings`](crate::AppCommand::Bindings) (#226).
    AppBindings {
        /// The channel.
        channel_id: String,
        /// The top-level bindings: post menu, channel header, commands.
        bindings: Arc<[matty_api::models::AppBinding]>,
    },
    /// How an App's call ended: the answer to
    /// [`AppCommand::Run`](crate::AppCommand::Run) (#226).
    AppAnswered {
        /// The caller's id for the call.
        call_id: String,
        /// What happened.
        answer: crate::AppAnswer,
    },
    /// A dynamic select's options: the answer to
    /// [`AppCommand::Lookup`](crate::AppCommand::Lookup) (#226). Empty when
    /// the App found none or the lookup failed.
    AppLookup {
        /// The form's dialog, by its callback id.
        callback_id: String,
        /// The field.
        field: String,
        /// What the user had typed.
        query: String,
        /// The options, in the App's order.
        options: Vec<matty_api::models::AppSelectOption>,
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

/// A small picture Matty keeps in the media cache under a version (#153).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Picture {
    /// A user's profile picture, versioned by `last_picture_update`.
    Avatar,
    /// A custom emoji's image, versioned by its `update_at`.
    Emoji,
    /// A team's icon, versioned by `last_team_icon_update`.
    TeamIcon,
}

impl Picture {
    /// The media cache key for `id`'s picture at `version`.
    pub(crate) fn key(self, id: &str, version: i64) -> String {
        use matty_store::media::{avatar_key, emoji_key, team_icon_key};
        match self {
            Self::Avatar => avatar_key(id, version),
            Self::Emoji => emoji_key(id, version),
            Self::TeamIcon => team_icon_key(id, version),
        }
    }
}

/// Which of a file's images: the server scales each image it is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageSize {
    /// About 120 pixels, for the tile on a post.
    Thumbnail,
    /// Up to about 1920 pixels wide, for the lightbox.
    Preview,
}

/// Where a download is.
#[derive(Clone, Debug, PartialEq)]
pub enum DownloadState {
    /// Arriving: bytes so far, and the size when the server gave it.
    Progress {
        /// Bytes written so far.
        received: u64,
        /// The file's size, when known.
        total: Option<u64>,
    },
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
    /// The command needs the server and Matty could not reach it: it is
    /// offline (`matty --offline`, or the network dropped). Nothing changed
    /// here or on the server, so the user can try again once back (#163).
    Offline {
        /// What was asked, as in [`CommandError::Server`].
        action: &'static str,
    },
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

/// A list of posts the right pane shows outside their channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    /// The user's saved posts across teams.
    Saved,
    /// A channel's pinned posts.
    Pinned {
        /// The channel.
        channel_id: String,
    },
    /// Recent posts mentioning the user in a team: an "or" search for the
    /// user's mention keys.
    Mentions {
        /// The team.
        team_id: String,
        /// The mention keys, space separated, such as `@ada ada deploy`.
        terms: String,
    },
    /// The unread digest (#223): the first unread posts of each channel,
    /// channel by channel in the order given, oldest first within each.
    Unreads {
        /// The unread channels, in the order the digest lists them.
        channel_ids: Vec<String>,
        /// Whether collapsed reply threads are on, so replies stay out.
        collapsed_threads: bool,
    },
}
