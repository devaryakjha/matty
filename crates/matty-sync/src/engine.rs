//! The loop on the sync thread: apply what arrives to the store in batches,
//! then publish what changed.

use std::collections::BTreeSet;
use std::sync::Arc;
use web_time::Instant;

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
    ChangeSet, CommandError, Connection, DownloadState, ImageSize, POSTS_WINDOW, Picture,
    UploadState,
};
use crate::commands::{self, Write};
use crate::events::{self, Fetch};
use crate::load::{self, Loaded};
use crate::outbox;
use crate::presence::StatusBatcher;
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
    /// Adds, edits, deletes or moves a channel bookmark (#226);
    /// [`ChangeSet::Bookmarks`](crate::ChangeSet::Bookmarks) follows, or a
    /// [`ChangeSet::Failed`](crate::ChangeSet::Failed).
    Bookmark(crate::BookmarkCommand),
    /// Asks installed Apps for a channel's bindings, or runs one (#226).
    App(crate::AppCommand),
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
    /// Loads the page of replies before the oldest one the open thread's
    /// snapshot holds (#122); the next [`ChangeSet::Thread`] reaches back
    /// that far, or says the page failed.
    LoadOlderReplies {
        /// The root post's id.
        root_id: String,
    },
    /// Loads the page of followed threads after the last one the team's
    /// Threads list holds (#122); the next [`ChangeSet::ThreadList`]
    /// reaches that far, or says the page failed.
    LoadMoreThreads {
        /// The team.
        team_id: String,
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
    /// The user read a followed thread (#124).
    ReadThread {
        /// The team whose Threads view lists it.
        team_id: String,
        /// The root post's id.
        root_id: String,
    },
    /// "Mark all as read" in a team's Threads view.
    ReadAllThreads {
        /// The team.
        team_id: String,
    },
    /// Follow or unfollow a thread.
    FollowThread {
        /// The team whose Threads view lists it.
        team_id: String,
        /// The root post's id.
        root_id: String,
        /// True to follow, false to stop.
        follow: bool,
    },
    /// Catch up with the server now, as after a reconnect: memberships,
    /// each cached channel since its cursor, and thread lists.
    Resync,
    /// Reconnects the socket at once, skipping the backoff wait: the
    /// offline banner's Retry now (#162).
    Reconnect,
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
        /// Files attached while offline, by local path: kept in the outbox
        /// and uploaded before the post goes (#194).
        local_files: Vec<std::path::PathBuf>,
        /// Post properties, such as `remove_link_preview` when the user
        /// removed the link preview while composing (#188).
        props: std::collections::HashMap<String, serde_json::Value>,
    },
    /// Sends a failed post again.
    Retry {
        /// Its pending id.
        pending_post_id: String,
    },
    /// Drops an unsent post: a queued or failed one. One on its way to the
    /// server is refused with
    /// [`CommandError::NotSentYet`](crate::CommandError::NotSentYet).
    Discard {
        /// Its pending id.
        pending_post_id: String,
    },
    /// Changes a post's text. A post waiting in the outbox takes the new
    /// text before it goes, keeping its place (#195); one on its way to the
    /// server is refused with
    /// [`CommandError::NotSentYet`](crate::CommandError::NotSentYet).
    EditPost {
        /// The post.
        post_id: String,
        /// The new text.
        message: String,
    },
    /// Deletes a sent post. A post waiting in the outbox is discarded, and
    /// one on its way is refused like [`Command::EditPost`].
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
    /// Replaces the user's notification preferences, such as
    /// `mention_keys`, with this whole map;
    /// [`ChangeSet::Users`](crate::ChangeSet::Users) follows.
    SetNotifyProps {
        /// Every key, changed or not: the server replaces the map.
        notify_props: std::collections::HashMap<String, String>,
    },
    /// Moves a category one place up or down in its team's sidebar;
    /// [`ChangeSet::Sidebar`](crate::ChangeSet::Sidebar) follows.
    MoveCategory {
        /// The category to move.
        category_id: String,
        /// Up (towards the top) or down.
        up: bool,
    },
    /// Moves a channel one place up or down within its category, which then
    /// sorts by hand; [`ChangeSet::Sidebar`](crate::ChangeSet::Sidebar)
    /// follows.
    MoveChannel {
        /// The category the channel is in.
        category_id: String,
        /// The channel to move.
        channel_id: String,
        /// Up (towards the top) or down.
        up: bool,
    },
    /// Puts a category at `index` in its team's sidebar, as dragging its
    /// header does; [`ChangeSet::Sidebar`](crate::ChangeSet::Sidebar)
    /// follows.
    PlaceCategory {
        /// The category to move.
        category_id: String,
        /// Its place once moved, counted from the top.
        index: usize,
    },
    /// Puts a channel at `index` in a category, its own or another, as
    /// dragging its row does. The category then sorts by hand;
    /// [`ChangeSet::Sidebar`](crate::ChangeSet::Sidebar) follows.
    PlaceChannel {
        /// The channel.
        channel_id: String,
        /// The category it ends up in.
        category_id: String,
        /// Its place there once moved, counted from the top.
        index: usize,
    },
    /// Changes the user's own names, nickname, position or username;
    /// [`ChangeSet::Users`](crate::ChangeSet::Users) follows.
    UpdateProfile {
        /// The fields to change.
        patch: matty_api::ProfilePatch,
    },
    /// Sets the user's profile picture from an image file's bytes and
    /// name, or with `None` goes back to the generated one.
    SetPicture {
        /// The image's bytes and file name, or `None` to remove it.
        picture: Option<(Vec<u8>, String)>,
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
    /// Finds what a link names, from the cache or else the server;
    /// [`ChangeSet::LinkResolved`](crate::ChangeSet::LinkResolved) answers.
    ResolveLink {
        /// What the link names.
        query: crate::LinkQuery,
    },
    /// Asks the server who matches a typed `@name`, channel members first;
    /// [`ChangeSet::Suggested`](crate::ChangeSet::Suggested) answers.
    SuggestPeople {
        /// What follows the `@`.
        term: String,
        /// The team to look in.
        team_id: Option<String>,
        /// The channel whose members come first.
        channel_id: Option<String>,
    },
    /// Leaves a channel; it drops out of the sidebar once the server agrees.
    LeaveChannel {
        /// The channel.
        channel_id: String,
    },
    /// Archives a channel (#208); it leaves the sidebar as a left channel
    /// does.
    ArchiveChannel {
        /// The channel.
        channel_id: String,
    },
    /// Makes a public channel private (#208); the channel comes back in
    /// [`ChangeSet::Channel`](crate::ChangeSet::Channel).
    ConvertToPrivate {
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
    /// Turns collapsed reply threads on or off for the user, where the
    /// server lets users choose (#157). Every count and window follows;
    /// [`ChangeSet::CollapsedThreads`](crate::ChangeSet::CollapsedThreads)
    /// answers.
    SetCollapsedThreads {
        /// On.
        on: bool,
    },
    /// Saves account preferences Preferences mirrors, such as
    /// `display_settings/message_display` (#156) or
    /// `advanced_settings/send_on_ctrl_enter` (#157), as the user's.
    /// [`ChangeSet::AccountPreferences`](crate::ChangeSet::AccountPreferences)
    /// answers with every such preference.
    SetPreferences {
        /// Category, name and value of each; the user id is filled in.
        preferences: Vec<Preference>,
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
    /// Asks the server for a link's Open Graph data, for the preview while
    /// composing (#188); [`ChangeSet::LinkPreview`] answers, at once when
    /// the link was asked about before.
    ///
    /// [`ChangeSet::LinkPreview`]: crate::ChangeSet::LinkPreview
    LoadLinkPreview {
        /// The link.
        url: String,
    },
    /// Fetches an image from outside the server, such as a link preview's:
    /// through the server's image proxy when it has one, else directly and
    /// without the session (#159). [`ChangeSet::RemoteImage`] answers; a URL
    /// that gives no image gets no answer.
    ///
    /// [`ChangeSet::RemoteImage`]: crate::ChangeSet::RemoteImage
    LoadRemoteImage {
        /// The image's URL.
        url: String,
    },
    /// Loads a profile picture, a custom emoji's image or a team's icon
    /// (#153): from the media cache, else from the server, keeping it for
    /// next time under its version. [`ChangeSet::Picture`] always answers,
    /// with no image when there is none to show.
    ///
    /// [`ChangeSet::Picture`]: crate::ChangeSet::Picture
    LoadPicture {
        /// Which kind of picture.
        picture: Picture,
        /// The user, emoji or team.
        id: String,
        /// Its version as the app knows it (`last_picture_update`, the
        /// emoji's `update_at` or `last_team_icon_update`); `None` uses the
        /// stored one.
        version: Option<i64>,
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
    /// A new custom sidebar category named `name`, with `channel_id`
    /// moved into it when given (#208). The team's categories come back
    /// in the sidebar change sets.
    CreateCategory {
        /// The team whose sidebar gets it.
        team_id: String,
        /// Its title.
        name: String,
        /// A channel to move into it.
        channel_id: Option<String>,
    },
    /// Renames a category.
    RenameCategory {
        /// The category.
        category_id: String,
        /// Its new title.
        name: String,
    },
    /// Deletes a custom category; the server moves its channels back to
    /// their default categories.
    DeleteCategory {
        /// The category.
        category_id: String,
    },
    /// Moves a channel to the top of another category.
    MoveToCategory {
        /// The channel.
        channel_id: String,
        /// Where it goes.
        category_id: String,
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
    /// Removes someone from a channel (#208); the new counts come back in
    /// [`ChangeSet::Stats`](crate::ChangeSet::Stats).
    RemoveMember {
        /// The channel.
        channel_id: String,
        /// The person.
        user_id: String,
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
        /// Which page of results, from 0.
        page: u32,
    },
    /// Presses an interactive message's button, or picks `selected_option`
    /// from its menu (#211);
    /// [`ChangeSet::PostActionDone`](crate::ChangeSet::PostActionDone) or a
    /// failure answers.
    RunPostAction {
        /// The post.
        post_id: String,
        /// The button or menu.
        action_id: String,
        /// The menu option picked; `None` for a button.
        selected_option: Option<String>,
        /// The action's cookie, which an ephemeral post needs.
        cookie: String,
    },
    /// Sends an interactive dialog's values to its integration, or with
    /// `cancelled` set and no values, that the user closed it (#211);
    /// [`ChangeSet::DialogSubmitted`](crate::ChangeSet::DialogSubmitted) or
    /// a failure answers.
    SubmitDialog {
        /// The dialog's integration URL.
        url: String,
        /// The dialog's callback id.
        callback_id: String,
        /// The dialog's state, sent back unchanged.
        state: String,
        /// The channel the user was in.
        channel_id: String,
        /// That channel's team.
        team_id: String,
        /// The values by field name.
        submission: std::collections::HashMap<String, serde_json::Value>,
        /// The user closed it instead.
        cancelled: bool,
    },
    /// Asks the server what may come next in a slash command the user is
    /// typing (#210): command names, subcommands or a plugin's argument
    /// values; [`ChangeSet::CommandSuggestions`](crate::ChangeSet::CommandSuggestions)
    /// answers, empty when the server has none or could not be asked.
    SuggestCommands {
        /// The channel's team.
        team_id: String,
        /// The channel the user is typing in.
        channel_id: String,
        /// The thread when typing a reply; empty in the channel.
        root_id: String,
        /// The line so far, from its `/`.
        user_input: String,
    },
    /// Runs a slash command on the server (#210);
    /// [`ChangeSet::CommandRan`](crate::ChangeSet::CommandRan) answers, or
    /// offline a failure with the action `run command`.
    ExecuteCommand {
        /// The channel's team; empty for a direct or group message.
        team_id: String,
        /// The channel the user typed it in.
        channel_id: String,
        /// The thread when typed in a reply; empty in the channel.
        root_id: String,
        /// The whole line, from its `/`.
        command: String,
    },
    /// Asks the server to remind the user of a post at `target_time`
    /// (Unix seconds); only a failure answers.
    SetReminder {
        /// The post.
        post_id: String,
        /// When, in Unix seconds.
        target_time: i64,
    },
    /// Loads a listing; [`ChangeSet::Listing`](crate::ChangeSet::Listing)
    /// answers.
    LoadListing(crate::Listing),
    /// Saves or unsaves a post for the user (a `flagged_post` preference);
    /// [`ChangeSet::Saved`](crate::ChangeSet::Saved) follows at once.
    SetSaved {
        /// The post.
        post_id: String,
        /// Save it, or unsave it.
        saved: bool,
    },
    /// Pins or unpins a post in its channel; the post comes back edited
    /// over the WebSocket.
    SetPinned {
        /// The post.
        post_id: String,
        /// Pin it, or unpin it.
        pinned: bool,
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
            Self::Bookmark(command) => f.debug_tuple("Bookmark").field(command).finish(),
            Self::App(command) => f.debug_tuple("App").field(command).finish(),
            Self::SignOut => f.write_str("SignOut"),
            Self::Event(event) => f.debug_tuple("Event").field(event).finish(),
            Self::SetCollapsedThreads { on } => f
                .debug_struct("SetCollapsedThreads")
                .field("on", on)
                .finish(),
            Self::SetPreferences { preferences } => f
                .debug_struct("SetPreferences")
                .field("preferences", preferences)
                .finish(),
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
            Self::LoadOlderReplies { root_id } => f
                .debug_struct("LoadOlderReplies")
                .field("root_id", root_id)
                .finish(),
            Self::LoadMoreThreads { team_id } => f
                .debug_struct("LoadMoreThreads")
                .field("team_id", team_id)
                .finish(),
            Self::OpenThreadList { team_id } => f
                .debug_struct("OpenThreadList")
                .field("team_id", team_id)
                .finish(),
            Self::CloseThreadList { team_id } => f
                .debug_struct("CloseThreadList")
                .field("team_id", team_id)
                .finish(),
            Self::ReadThread { team_id, root_id } => f
                .debug_struct("ReadThread")
                .field("team_id", team_id)
                .field("root_id", root_id)
                .finish(),
            Self::ReadAllThreads { team_id } => f
                .debug_struct("ReadAllThreads")
                .field("team_id", team_id)
                .finish(),
            Self::FollowThread {
                team_id,
                root_id,
                follow,
            } => f
                .debug_struct("FollowThread")
                .field("team_id", team_id)
                .field("root_id", root_id)
                .field("follow", follow)
                .finish(),
            Self::Resync => f.write_str("Resync"),
            Self::Reconnect => f.write_str("Reconnect"),
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
            // Keywords are the user's own words: keep them out of logs.
            Self::SetNotifyProps { .. } => f.write_str("SetNotifyProps"),
            Self::UpdateProfile { .. } => f.write_str("UpdateProfile"),
            Self::SetPicture { picture } => f
                .debug_struct("SetPicture")
                .field("bytes", &picture.as_ref().map(|(bytes, _)| bytes.len()))
                .finish(),
            Self::MoveCategory { .. } => f.write_str("MoveCategory"),
            Self::MoveChannel { .. } => f.write_str("MoveChannel"),
            Self::PlaceCategory { .. } => f.write_str("PlaceCategory"),
            Self::PlaceChannel { .. } => f.write_str("PlaceChannel"),
            Self::OpenDirect { user_ids } => f
                .debug_struct("OpenDirect")
                .field("user_ids", user_ids)
                .finish(),
            Self::JoinChannel { channel_id } => f
                .debug_struct("JoinChannel")
                .field("channel_id", channel_id)
                .finish(),
            Self::ResolveLink { query } => {
                f.debug_struct("ResolveLink").field("query", query).finish()
            }
            // A half-typed name is the user's own words: keep it out of logs.
            Self::SuggestPeople { .. } => f.write_str("SuggestPeople"),
            Self::LeaveChannel { channel_id } => f
                .debug_struct("LeaveChannel")
                .field("channel_id", channel_id)
                .finish(),
            Self::ArchiveChannel { channel_id } => f
                .debug_struct("ArchiveChannel")
                .field("channel_id", channel_id)
                .finish(),
            Self::ConvertToPrivate { channel_id } => f
                .debug_struct("ConvertToPrivate")
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
            Self::LoadLinkPreview { .. } => {
                f.debug_struct("LoadLinkPreview").finish_non_exhaustive()
            }
            Self::LoadRemoteImage { .. } => {
                f.debug_struct("LoadRemoteImage").finish_non_exhaustive()
            }
            Self::LoadPicture {
                picture,
                id,
                version,
            } => f
                .debug_struct("LoadPicture")
                .field("picture", picture)
                .field("id", id)
                .field("version", version)
                .finish(),
            Self::Download { file_id, .. } => f
                .debug_struct("Download")
                .field("file_id", file_id)
                .finish_non_exhaustive(),
            Self::CancelUpload { upload_id } => f
                .debug_struct("CancelUpload")
                .field("upload_id", upload_id)
                .finish(),
            Self::CreateCategory {
                team_id,
                channel_id,
                ..
            } => f
                .debug_struct("CreateCategory")
                .field("team_id", team_id)
                .field("channel_id", channel_id)
                .finish_non_exhaustive(),
            Self::RenameCategory { category_id, .. } => f
                .debug_struct("RenameCategory")
                .field("category_id", category_id)
                .finish_non_exhaustive(),
            Self::DeleteCategory { category_id } => f
                .debug_struct("DeleteCategory")
                .field("category_id", category_id)
                .finish(),
            Self::MoveToCategory {
                channel_id,
                category_id,
            } => f
                .debug_struct("MoveToCategory")
                .field("channel_id", channel_id)
                .field("category_id", category_id)
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
            Self::RemoveMember {
                channel_id,
                user_id,
            } => f
                .debug_struct("RemoveMember")
                .field("channel_id", channel_id)
                .field("user_id", user_id)
                .finish(),
            Self::Search { team_id, .. } => f
                .debug_struct("Search")
                .field("team_id", team_id)
                .finish_non_exhaustive(),
            Self::RunPostAction {
                post_id, action_id, ..
            } => f
                .debug_struct("RunPostAction")
                .field("post_id", post_id)
                .field("action_id", action_id)
                .finish_non_exhaustive(),
            // The values may hold a password: only the dialog is named.
            Self::SuggestCommands { user_input, .. } => f
                .debug_struct("SuggestCommands")
                .field("user_input", user_input)
                .finish_non_exhaustive(),
            // The line may hold anything the user typed.
            Self::ExecuteCommand { channel_id, .. } => f
                .debug_struct("ExecuteCommand")
                .field("channel_id", channel_id)
                .finish_non_exhaustive(),
            Self::SubmitDialog {
                callback_id,
                cancelled,
                ..
            } => f
                .debug_struct("SubmitDialog")
                .field("callback_id", callback_id)
                .field("cancelled", cancelled)
                .finish_non_exhaustive(),
            Self::SetReminder {
                post_id,
                target_time,
            } => f
                .debug_struct("SetReminder")
                .field("post_id", post_id)
                .field("target_time", target_time)
                .finish(),
            Self::LoadListing(listing) => f.debug_tuple("LoadListing").field(listing).finish(),
            Self::SetSaved { post_id, saved } => f
                .debug_struct("SetSaved")
                .field("post_id", post_id)
                .field("saved", saved)
                .finish(),
            Self::SetPinned { post_id, pinned } => f
                .debug_struct("SetPinned")
                .field("post_id", post_id)
                .field("pinned", pinned)
                .finish(),
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
    /// The custom emoji list changed, or a new subscriber needs it.
    pub(crate) emoji: bool,
    /// Preferences changed.
    pub(crate) preferences: bool,
    /// The saved posts may have changed.
    pub(crate) saved: bool,
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
    /// Post authors whose statuses are still to be asked for (#135).
    statuses: StatusBatcher,
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
    /// What the server said about each link asked for while composing.
    link_previews: std::collections::HashMap<String, Option<Arc<matty_api::models::OpenGraph>>>,
    /// How far back each open thread's replies are loaded (#122).
    thread_pages: std::collections::HashMap<String, ThreadPages>,
    /// How far each team's Threads list is loaded (#122).
    thread_list_pages: std::collections::HashMap<String, ListPages>,
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
            statuses: StatusBatcher::default(),
            uploads: std::collections::HashMap::new(),
            media: None,
            fetching: BTreeSet::new(),
            link_previews: std::collections::HashMap::new(),
            history: std::collections::HashMap::new(),
            sending: std::collections::HashMap::new(),
            floors: std::collections::HashMap::new(),
            thread_pages: std::collections::HashMap::new(),
            thread_list_pages: std::collections::HashMap::new(),
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
        let mut remote_images = Vec::new();
        let mut pictures = Vec::new();
        let mut threads_to_fetch = Vec::new();
        let mut thread_lists_to_fetch = Vec::new();
        let mut thread_pages_to_fetch = Vec::new();
        let mut link_previews = Vec::new();
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
                    self.statuses.clear();
                    // Publish the cache before the first round trip ends.
                    dirty.all_teams = true;
                    dirty.emoji = true;
                    dirty.unreads = true;
                    dirty.channels.extend(self.open_channels.iter().cloned());
                    dirty
                        .direct
                        .push(ChangeSet::Connection(Connection::Loading));
                    let load = matty_api::rt::spawn(load::run(client.clone(), self.loaded.clone()));
                    // Events from now on queue behind the load's results.
                    let (socket, mut signals) = WsClient::connect(client.clone());
                    let forward = self.loaded.clone();
                    matty_api::rt::spawn(async move {
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
                    publish_drafts(&tx, dirty)?;
                }
                Message::Command(Command::ShowCache) => {
                    dirty.all_teams = true;
                    dirty.saved = true;
                    dirty.unreads = true;
                    dirty.channels.extend(self.open_channels.iter().cloned());
                    publish_drafts(&tx, dirty)?;
                }
                Message::Command(Command::SignOut) => {
                    // Nothing queued in this session may go out under the
                    // next, even if the cache outlives the sign out (#194).
                    let dropped = queue::clear(&tx)?;
                    if dropped > 0 {
                        tracing::info!(dropped, "dropped queued sends at sign out");
                    }
                    match self.session.take() {
                        Some(old) => {
                            old.load.abort();
                            // Dropping the session closes the WebSocket.
                            let client = old.client.clone();
                            drop(old);
                            let loaded = self.loaded.clone();
                            matty_api::rt::spawn(async move {
                                if let Err(error) = client.logout().await {
                                    tracing::warn!(%error, "the server did not end the session");
                                }
                                let _ = loaded.send(Loaded::SignedOut);
                            });
                        }
                        None => dirty
                            .direct
                            .push(ChangeSet::Connection(Connection::SignedOut)),
                    }
                }
                Message::Command(Command::Bookmark(command)) => {
                    let client = self
                        .session
                        .as_ref()
                        .filter(|session| !session.user_id.is_empty())
                        .map(|session| session.client.clone());
                    crate::bookmarks::command(&tx, client, &self.loaded, command, dirty)?;
                }
                Message::Command(Command::App(command)) => {
                    if let Some(session) = &self.session {
                        let client = session.client.clone();
                        crate::apps::command(client, command, self.loaded.clone(), dirty);
                    }
                }
                Message::Command(Command::OpenChannel { channel_id }) => {
                    self.open_channels.insert(channel_id.clone());
                    let bookmarks = self.capabilities.as_ref().map(|c| c.channel_bookmarks);
                    crate::bookmarks::opened(&tx, &channel_id, bookmarks, dirty)?;
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
                    self.thread_pages.remove(&root_id);
                    dirty.threads.insert(root_id.clone());
                    threads_to_fetch.push((root_id, None));
                }
                Message::Command(Command::CloseThread { root_id }) => {
                    self.open_threads.remove(&root_id);
                    self.thread_pages.remove(&root_id);
                }
                Message::Command(Command::LoadOlderReplies { root_id }) => {
                    let floor = self.thread_pages.get_mut(&root_id).and_then(|pages| {
                        let floor = pages.floor.clone().filter(|_| !pages.at_start)?;
                        (!pages.loading).then_some(floor)
                    });
                    if let Some(floor) = floor {
                        threads_to_fetch.push((root_id, Some(floor)));
                    }
                }
                Message::Command(Command::LoadMoreThreads { team_id }) => {
                    let pages = self.thread_list_pages.entry(team_id.clone()).or_default();
                    if !pages.loading && !pages.complete {
                        let shown = THREAD_LIST + pages.extra;
                        let last = posts::threads(&tx, &team_id, false, shown)?
                            .last()
                            .map(|thread| thread.id.clone());
                        thread_pages_to_fetch.push((team_id, last));
                    }
                }
                Message::Command(Command::OpenThreadList { team_id }) => {
                    self.open_thread_lists.insert(team_id.clone());
                    let since = posts::cursor(&tx, &posts::threads_since_key(&team_id))?
                        .and_then(|value| value.parse().ok());
                    thread_lists_to_fetch.push((team_id.clone(), since));
                    dirty.thread_lists.insert(team_id);
                }
                Message::Command(Command::ReadThread { team_id, root_id }) => {
                    let at = posts::now_ms();
                    posts::mark_thread_read(&tx, &root_id, at, 0, 0)?;
                    dirty.thread_lists.insert(team_id.clone());
                    writes.push(Write::ReadThread {
                        team_id,
                        thread_id: root_id,
                        at,
                    });
                }
                Message::Command(Command::ReadAllThreads { team_id }) => {
                    posts::mark_team_threads_read(&tx, &team_id, posts::now_ms())?;
                    dirty.thread_lists.insert(team_id.clone());
                    writes.push(Write::ReadAllThreads { team_id });
                }
                Message::Command(Command::FollowThread {
                    team_id,
                    root_id,
                    follow,
                }) => {
                    posts::set_following(&tx, &root_id, follow)?;
                    dirty.thread_lists.insert(team_id.clone());
                    writes.push(Write::Follow {
                        team_id,
                        thread_id: root_id,
                        follow,
                    });
                }
                Message::Command(Command::CloseThreadList { team_id }) => {
                    self.open_thread_lists.remove(&team_id);
                }
                Message::Command(Command::Event(event)) => {
                    events::apply(&tx, *event, self.me.as_deref(), dirty, &mut fetches)?;
                }
                Message::Command(Command::Resync) => dirty.resync = true,
                Message::Command(Command::Reconnect) => {
                    if let Some(session) = &self.session {
                        session.socket.reconnect_now();
                    }
                }
                Message::Command(Command::SendPost {
                    pending_post_id,
                    channel_id,
                    root_id,
                    message,
                    file_ids,
                    local_files,
                    props,
                }) => {
                    let Some(me) = self.me.clone() else {
                        dirty
                            .direct
                            .push(ChangeSet::Failed(CommandError::Offline { action: "send" }));
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
                        local_files: local_files
                            .iter()
                            .map(|path| path.to_string_lossy().into_owned())
                            .collect(),
                        props,
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
                    if in_flight(&self.sending, &pending_post_id) {
                        // The server may be creating it right now; the row
                        // comes back and reads "Sending…".
                        if let Some(outgoing) = queue::get(&tx, &pending_post_id)? {
                            dirty.channels.insert(outgoing.channel_id);
                        }
                        dirty
                            .direct
                            .push(ChangeSet::Failed(CommandError::NotSentYet {
                                post_id: pending_post_id,
                            }));
                    } else {
                        outbox::discard(&tx, &pending_post_id, dirty)?;
                    }
                }
                Message::Command(Command::EditPost { post_id, message }) => {
                    match queue::get(&tx, &post_id)? {
                        // Still on its way: the server may already have
                        // it, so the view goes back to what is stored.
                        Some(outgoing) if in_flight(&self.sending, &post_id) => {
                            dirty.channels.insert(outgoing.channel_id);
                            dirty
                                .direct
                                .push(ChangeSet::Failed(CommandError::NotSentYet { post_id }));
                        }
                        // Waiting: the queued send takes the new text (#195).
                        Some(outgoing) => {
                            queue::edit(&tx, &post_id, &message)?;
                            outbox::touch(&outgoing, dirty);
                        }
                        None => writes.push(Write::Edit { post_id, message }),
                    }
                }
                Message::Command(Command::DeletePost { post_id }) => {
                    match queue::get(&tx, &post_id)? {
                        Some(outgoing) if in_flight(&self.sending, &post_id) => {
                            dirty.channels.insert(outgoing.channel_id);
                            dirty
                                .direct
                                .push(ChangeSet::Failed(CommandError::NotSentYet { post_id }));
                        }
                        // Never sent, so nobody else saw it: it just goes.
                        Some(_) => {
                            outbox::discard(&tx, &post_id, dirty)?;
                        }
                        None => writes.push(Write::Delete { post_id }),
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
                    let me = self.me.clone().unwrap_or_default();
                    let add = !posts::reactions(&tx, &post_id)?
                        .iter()
                        .any(|r| r.user_id == me && r.emoji_name == emoji_name);
                    // Offline it would show here and never reach the
                    // server, so nothing changes (#163).
                    if me.is_empty() || !connected(self.session.as_ref()) {
                        dirty.direct.push(ChangeSet::Failed(CommandError::Offline {
                            action: reaction_action(add),
                        }));
                        continue;
                    }
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
                        matty_api::rt::spawn(async move {
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
                Message::Command(Command::SetNotifyProps { notify_props }) => {
                    writes.push(Write::NotifyProps(notify_props))
                }
                Message::Command(Command::UpdateProfile { patch }) => {
                    writes.push(Write::Profile(patch))
                }
                Message::Command(Command::SetPicture { picture }) => {
                    writes.push(Write::Picture(picture))
                }
                Message::Command(Command::SetCollapsedThreads { on }) => {
                    if let Some(me) = &self.me {
                        let preference = Preference {
                            user_id: me.clone(),
                            category: "display_settings".into(),
                            name: "collapsed_reply_threads".into(),
                            value: if on { "on" } else { "off" }.into(),
                        };
                        sidebar::upsert_preferences(&tx, std::slice::from_ref(&preference))?;
                        dirty.preferences = true;
                        writes.push(Write::Preferences(vec![preference]));
                    }
                }
                Message::Command(Command::SetPreferences { preferences }) => {
                    if let Some(me) = &self.me {
                        let preferences: Vec<Preference> = preferences
                            .into_iter()
                            .map(|preference| Preference {
                                user_id: me.clone(),
                                ..preference
                            })
                            .collect();
                        sidebar::upsert_preferences(&tx, &preferences)?;
                        dirty.preferences = true;
                        writes.push(Write::Preferences(preferences));
                    }
                }
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
                Message::Command(Command::ArchiveChannel { channel_id }) => {
                    writes.push(Write::Archive { channel_id });
                }
                Message::Command(Command::ConvertToPrivate { channel_id }) => {
                    writes.push(Write::Privacy {
                        channel_id,
                        private: true,
                    });
                }
                Message::Command(Command::ResolveLink { query }) => {
                    match cached_link(&tx, &query)? {
                        Some(answer) => dirty.direct.push(ChangeSet::LinkResolved {
                            query,
                            answer: Some(answer),
                        }),
                        None => writes.push(Write::Resolve(query)),
                    }
                }
                Message::Command(Command::SuggestPeople {
                    term,
                    team_id,
                    channel_id,
                }) => {
                    writes.push(Write::Suggest {
                        term,
                        team_id,
                        channel_id,
                    });
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
                Message::Command(Command::LoadRemoteImage { url }) => {
                    remote_images.push(url);
                }
                Message::Command(Command::LoadPicture {
                    picture,
                    id,
                    version,
                }) => pictures.push((picture, id, version)),
                Message::Command(Command::LoadLinkPreview { url }) => {
                    link_previews.push(url);
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
                Message::Command(Command::CreateCategory {
                    team_id,
                    name,
                    channel_id,
                }) => {
                    let category = matty_api::models::SidebarCategory {
                        user_id: self.me.clone().unwrap_or_default(),
                        team_id: team_id.clone(),
                        kind: "custom".into(),
                        display_name: name,
                        channel_ids: channel_id.into_iter().collect(),
                        ..Default::default()
                    };
                    writes.push(Write::CreateCategory {
                        team_id,
                        category: Box::new(category),
                    });
                }
                Message::Command(Command::RenameCategory { category_id, name }) => {
                    if let Some(mut category) = sidebar::category(&tx, &category_id)? {
                        category.display_name = name;
                        writes.push(category_write(self.me.as_deref(), category));
                    }
                }
                Message::Command(Command::MoveToCategory {
                    channel_id,
                    category_id,
                }) => {
                    if let Some(mut category) = sidebar::category(&tx, &category_id)? {
                        category.channel_ids.retain(|id| *id != channel_id);
                        category.channel_ids.insert(0, channel_id);
                        writes.push(category_write(self.me.as_deref(), category));
                    }
                }
                Message::Command(Command::MoveCategory { category_id, up }) => {
                    if let Some(category) = sidebar::category(&tx, &category_id)? {
                        let mut order = sidebar::category_order(&tx, &category.team_id)?;
                        if step(&mut order, &category_id, up) {
                            writes.push(Write::CategoryOrder {
                                team_id: category.team_id,
                                order,
                            });
                        }
                    }
                }
                Message::Command(Command::MoveChannel {
                    category_id,
                    channel_id,
                    up,
                }) => {
                    if let Some(mut category) = sidebar::category(&tx, &category_id)?
                        && step(&mut category.channel_ids, &channel_id, up)
                    {
                        category.sorting = "manual".into();
                        writes.push(category_write(self.me.as_deref(), category));
                    }
                }
                Message::Command(Command::PlaceCategory { category_id, index }) => {
                    if let Some(category) = sidebar::category(&tx, &category_id)? {
                        let mut order = sidebar::category_order(&tx, &category.team_id)?;
                        if place(&mut order, category_id, index) {
                            writes.push(Write::CategoryOrder {
                                team_id: category.team_id,
                                order,
                            });
                        }
                    }
                }
                Message::Command(Command::PlaceChannel {
                    channel_id,
                    category_id,
                    index,
                }) => {
                    if let Some(mut category) = sidebar::category(&tx, &category_id)?
                        && place(&mut category.channel_ids, channel_id, index)
                    {
                        category.sorting = "manual".into();
                        writes.push(category_write(self.me.as_deref(), category));
                    }
                }
                Message::Command(Command::DeleteCategory { category_id }) => {
                    if let Some(category) = sidebar::category(&tx, &category_id)? {
                        writes.push(Write::DeleteCategory {
                            team_id: category.team_id,
                            category_id,
                        });
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
                Message::Command(Command::RemoveMember {
                    channel_id,
                    user_id,
                }) => writes.push(Write::RemoveMember {
                    channel_id,
                    user_id,
                }),
                Message::Command(Command::Search {
                    team_id,
                    terms,
                    time_zone_offset,
                    page,
                }) => {
                    let mut query = matty_api::SearchQuery::new(terms);
                    query.time_zone_offset = time_zone_offset;
                    query.page = page;
                    query.per_page = SEARCH_PAGE;
                    writes.push(Write::Search {
                        team_id,
                        query: Box::new(query),
                    });
                }
                Message::Command(Command::FindPeople { team_id }) => {
                    writes.push(Write::People { team_id });
                }
                Message::Command(Command::RunPostAction {
                    post_id,
                    action_id,
                    selected_option,
                    cookie,
                }) => writes.push(Write::PostAction {
                    post_id,
                    action_id,
                    selected_option,
                    cookie,
                }),
                Message::Command(Command::SuggestCommands {
                    team_id,
                    channel_id,
                    root_id,
                    user_input,
                }) => writes.push(Write::SuggestCommands {
                    team_id,
                    channel_id,
                    root_id,
                    user_input,
                }),
                Message::Command(Command::ExecuteCommand {
                    team_id,
                    channel_id,
                    root_id,
                    command,
                }) => writes.push(Write::ExecuteCommand(matty_api::models::ExecuteCommand {
                    command,
                    channel_id,
                    team_id,
                    root_id,
                })),
                Message::Command(Command::SubmitDialog {
                    url,
                    callback_id,
                    state,
                    channel_id,
                    team_id,
                    submission,
                    cancelled,
                }) => writes.push(Write::SubmitDialog(Box::new(
                    matty_api::models::SubmitDialogRequest {
                        kind: "dialog_submission".into(),
                        url,
                        callback_id,
                        state,
                        user_id: String::new(),
                        channel_id,
                        team_id,
                        submission,
                        cancelled,
                    },
                ))),
                Message::Command(Command::SetReminder {
                    post_id,
                    target_time,
                }) => writes.push(Write::Reminder {
                    post_id,
                    target_time,
                }),
                Message::Command(Command::LoadListing(listing)) => {
                    if connected(self.session.as_ref()) {
                        writes.push(Write::Listing(listing));
                    } else {
                        dirty.direct.push(cached_listing(&tx, listing)?);
                    }
                }
                Message::Command(Command::SetSaved { post_id, saved }) => {
                    if !connected(self.session.as_ref()) {
                        dirty.direct.push(ChangeSet::Failed(CommandError::Offline {
                            action: if saved {
                                "save message"
                            } else {
                                "unsave message"
                            },
                        }));
                    } else if let Some(me) = &self.me {
                        let preference = Preference {
                            user_id: me.clone(),
                            category: SAVED_CATEGORY.into(),
                            name: post_id,
                            value: "true".into(),
                        };
                        let preferences = vec![preference];
                        if saved {
                            sidebar::upsert_preferences(&tx, &preferences)?;
                            writes.push(Write::Preferences(preferences));
                        } else {
                            sidebar::delete_preferences(&tx, &preferences)?;
                            writes.push(Write::DeletePreferences(preferences));
                        }
                        dirty.saved = true;
                    }
                }
                Message::Command(Command::SetPinned { post_id, pinned }) => {
                    writes.push(Write::Pin { post_id, pinned });
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
        for url in remote_images {
            self.remote_image(url, dirty);
        }
        for (picture, id, version) in pictures {
            self.picture(picture, id, version, dirty);
        }
        for (team_id, since) in thread_lists_to_fetch {
            self.fetch_thread_list(team_id, since);
        }
        for (root_id, before) in threads_to_fetch {
            self.fetch_thread(root_id, before, dirty);
        }
        for (team_id, last) in thread_pages_to_fetch {
            self.fetch_more_threads(team_id, last, dirty);
        }
        for url in link_previews {
            self.link_preview(url, dirty);
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
        let Some(outgoing) = queue::next(conn, channel_id)? else {
            return Ok(());
        };
        queue::attempt(conn, &outgoing.pending_post_id)?;
        dirty.outbox.insert(outgoing.channel_id.clone());
        self.sending
            .insert(channel_id.to_owned(), outgoing.pending_post_id.clone());
        matty_api::rt::spawn(outbox::send(
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
        matty_api::rt::spawn(crate::download::download(
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
        matty_api::rt::spawn(async move {
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
        matty_api::rt::spawn(async move {
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

    /// Answers from what the server already said about `url`, or asks it;
    /// the answer comes back as [`Loaded::LinkPreview`]. Each link is asked
    /// about once a run.
    fn link_preview(&mut self, url: String, dirty: &mut Dirty) {
        if let Some(graph) = self.link_previews.get(&url) {
            dirty.direct.push(ChangeSet::LinkPreview {
                url,
                graph: graph.clone(),
            });
            return;
        }
        let Some(session) = self.session.as_ref().filter(|s| !s.user_id.is_empty()) else {
            return;
        };
        if !self.fetching.insert(format!("opengraph/{url}")) {
            return;
        }
        let client = session.client.clone();
        let results = self.loaded.clone();
        matty_api::rt::spawn(async move {
            let graph = match client.opengraph(&url).await {
                Ok(graph) if !graph.title.is_empty() || !graph.description.is_empty() => {
                    Some(graph)
                }
                Ok(_) => None,
                Err(error) => {
                    tracing::debug!(%error, "no link preview");
                    None
                }
            };
            let _ = results.send(Loaded::LinkPreview { url, graph });
        });
    }

    /// Reads a thread's newest page of replies from the server, as the
    /// cache may hold only its root (collapsed threads) or nothing (a
    /// thread opened from a search), or with `before` the page before that
    /// reply (#122); it comes back as [`Loaded::Thread`].
    fn fetch_thread(&mut self, root_id: String, before: Option<(i64, String)>, dirty: &mut Dirty) {
        let Some(session) = self.session.as_ref().filter(|s| !s.user_id.is_empty()) else {
            return;
        };
        if !self.fetching.insert(format!("thread/{root_id}")) {
            return;
        }
        let older = before.is_some();
        if older {
            let pages = self.thread_pages.entry(root_id.clone()).or_default();
            pages.loading = true;
            pages.failed = false;
            dirty.threads.insert(root_id.clone());
        }
        let query = match before {
            Some((at, post_id)) => {
                matty_api::ThreadQuery::from(post_id, at, matty_api::Direction::Up, THREAD_PAGE)
            }
            None => matty_api::ThreadQuery::latest(THREAD_PAGE),
        };
        let client = session.client.clone();
        let results = self.loaded.clone();
        matty_api::rt::spawn(async move {
            let posts = client
                .post_thread(&root_id, &query)
                .await
                .inspect_err(|error| tracing::debug!(%error, "could not read a thread"))
                .ok();
            let _ = results.send(Loaded::Thread {
                root_id,
                posts,
                older,
            });
        });
    }

    /// Reads the page of a team's followed threads after `last`, the
    /// oldest one listed (#122); it comes back as [`Loaded::MoreThreads`].
    fn fetch_more_threads(&mut self, team_id: String, last: Option<String>, dirty: &mut Dirty) {
        let Some(session) = self.session.as_ref().filter(|s| !s.user_id.is_empty()) else {
            return;
        };
        let pages = self.thread_list_pages.entry(team_id.clone()).or_default();
        pages.loading = true;
        pages.failed = false;
        dirty.thread_lists.insert(team_id.clone());
        let client = session.client.clone();
        let user_id = session.user_id.clone();
        let results = self.loaded.clone();
        matty_api::rt::spawn(async move {
            let query = matty_api::ThreadsQuery {
                before: last,
                per_page: THREADS_PAGE,
                ..matty_api::ThreadsQuery::default()
            };
            let threads = client
                .user_threads(&user_id, &team_id, &query)
                .await
                .inspect_err(|error| tracing::debug!(%error, "could not read more threads"))
                .ok();
            let _ = results.send(Loaded::MoreThreads { team_id, threads });
        });
    }

    /// Reads a team's followed threads from the server when its Threads
    /// view opens, since the last time it did; they come back as
    /// [`Loaded::Threads`]. Offline, the view shows what is cached.
    fn fetch_thread_list(&mut self, team_id: String, since: Option<i64>) {
        let Some(session) = self.session.as_ref().filter(|s| !s.user_id.is_empty()) else {
            return;
        };
        if !self.fetching.insert(format!("threads/{team_id}")) {
            return;
        }
        let client = session.client.clone();
        let user_id = session.user_id.clone();
        let results = self.loaded.clone();
        matty_api::rt::spawn(async move {
            let query = matty_api::ThreadsQuery {
                since,
                deleted: since.is_some(),
                ..matty_api::ThreadsQuery::default()
            };
            let (threads, total) = match client.user_threads(&user_id, &team_id, &query).await {
                Ok(threads) => (threads.threads, Some(threads.total)),
                Err(error) => {
                    tracing::debug!(%error, "could not read the thread list");
                    (Vec::new(), None)
                }
            };
            let _ = results.send(Loaded::Threads {
                team_id,
                threads,
                total,
            });
        });
    }

    /// Answers from the media cache, or fetches an outside image; it comes
    /// back as [`Loaded::RemoteImage`].
    fn remote_image(&mut self, url: String, dirty: &mut Dirty) {
        let key = matty_store::media::remote_key(&url);
        if let Some(media) = &self.media {
            match media.get(self.store.conn(), &key) {
                Ok(Some(cached)) => {
                    dirty.direct.push(ChangeSet::RemoteImage {
                        url,
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
        let proxy = self
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.image_proxy);
        let client = session.client.clone();
        let results = self.loaded.clone();
        matty_api::rt::spawn(async move {
            // Through the server's image proxy when it has one (#159).
            let fetched = client.remote_image(&url, proxy).await;
            let image = match fetched {
                Ok(matty_api::Fetched::Fresh {
                    bytes,
                    content_type: Some(content_type),
                    ..
                }) if content_type.starts_with("image/") => Some((bytes, content_type)),
                Ok(_) => None,
                Err(error) => {
                    tracing::debug!(%error, proxy, "no outside image");
                    None
                }
            };
            let _ = results.send(Loaded::RemoteImage { url, image });
        });
    }

    /// Answers a picture from the media cache, or fetches it; it comes
    /// back as [`Loaded::Picture`]. Without a session, or for a version that
    /// says there is no picture, the app hears an empty answer.
    fn picture(&mut self, picture: Picture, id: String, version: Option<i64>, dirty: &mut Dirty) {
        let version = version.unwrap_or_else(|| self.stored_version(picture, &id));
        let empty = |id: String| ChangeSet::Picture {
            picture,
            id,
            version,
            image: None,
        };
        // A team that never set an icon has none to fetch.
        if picture == Picture::TeamIcon && version <= 0 {
            dirty.direct.push(empty(id));
            return;
        }
        let key = picture.key(&id, version);
        if let Some(media) = &self.media {
            match media.get(self.store.conn(), &key) {
                Ok(Some(cached)) => {
                    dirty.direct.push(ChangeSet::Picture {
                        picture,
                        id,
                        version,
                        image: Some((Arc::from(cached.bytes), cached.content_type)),
                    });
                    return;
                }
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, "could not read the media cache"),
            }
        }
        let Some(session) = self.session.as_ref().filter(|s| !s.user_id.is_empty()) else {
            dirty.direct.push(empty(id));
            return;
        };
        if !self.fetching.insert(key) {
            return;
        }
        let client = session.client.clone();
        let results = self.loaded.clone();
        matty_api::rt::spawn(async move {
            let fetched = match picture {
                Picture::Avatar => client.avatar(&id, None).await,
                Picture::Emoji => client.emoji_image(&id, None).await,
                Picture::TeamIcon => client.team_icon(&id, None).await,
            };
            let image = match fetched {
                Ok(matty_api::Fetched::Fresh {
                    bytes,
                    content_type,
                    ..
                }) => Some((bytes, content_type.unwrap_or_else(|| "image/png".into()))),
                Ok(matty_api::Fetched::NotModified) => None,
                Err(error) => {
                    tracing::debug!(%error, ?picture, id, "no picture");
                    None
                }
            };
            let _ = results.send(Loaded::Picture {
                picture,
                id,
                version,
                image,
            });
        });
    }

    /// The version of `id`'s picture the store knows, or 0.
    fn stored_version(&self, picture: Picture, id: &str) -> i64 {
        let conn = self.store.conn();
        match picture {
            Picture::Avatar => sidebar::users(conn, &[id.to_owned()])
                .ok()
                .and_then(|users| users.first().map(|user| user.last_picture_update)),
            Picture::Emoji => sidebar::custom_emoji(conn).ok().and_then(|emoji| {
                emoji
                    .into_iter()
                    .find(|one| one.id == id)
                    .map(|one| one.update_at)
            }),
            Picture::TeamIcon => None,
        }
        .unwrap_or(0)
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
        let task = matty_api::rt::spawn(outbox::upload(
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
            dirty.direct.push(ChangeSet::Failed(CommandError::Offline {
                action: write.action(),
            }));
            return;
        };
        matty_api::rt::spawn(commands::run(
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
        matty_api::rt::spawn(load::open_channel(
            session.client.clone(),
            session.user_id.clone(),
            channel_id.to_owned(),
            session.collapsed_threads,
            self.loaded.clone(),
        ));
        matty_api::rt::spawn(load::channel_stats(
            session.client.clone(),
            channel_id.to_owned(),
            self.loaded.clone(),
        ));
        if self
            .capabilities
            .as_ref()
            .is_some_and(|c| c.channel_bookmarks)
        {
            matty_api::rt::spawn(crate::bookmarks::fetch(
                session.client.clone(),
                channel_id.to_owned(),
                self.loaded.clone(),
            ));
        }
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
                matty_api::rt::spawn(load::resync(
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
            matty_api::rt::spawn(load::fetch(
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
                dirty.direct.push(ChangeSet::CollapsedThreads(crt));
                dirty.direct.push(ChangeSet::LinkPreviews(link_previews(
                    capabilities.link_previews,
                    &preferences,
                )));
                self.me = Some(me.id.clone());
                self.capabilities = Some((*capabilities).clone());
                // The window reads the user's own notification preferences
                // (mention keywords) from here.
                dirty.direct.push(ChangeSet::Me(Arc::from(me)));
                dirty
                    .direct
                    .push(ChangeSet::Config(Arc::from(capabilities)));
                dirty.direct.push(ChangeSet::Preferences);
                dirty.saved = true;
                dirty
                    .direct
                    .push(ChangeSet::AccountPreferences(account_preferences(
                        &preferences,
                    )));
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
            Loaded::Emoji(emoji) => {
                sidebar::upsert_emoji(self.store.conn(), &emoji)?;
                dirty.emoji = true;
            }
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
            Loaded::Drafts(drafts) => {
                // The newer copy wins, so a draft typed here while the load
                // ran stays.
                let tx = self.store.conn_mut().transaction()?;
                for draft in drafts.into_iter().filter(|draft| draft.delete_at == 0) {
                    posts::upsert_draft(&tx, &draft)?;
                    let stored = posts::draft(&tx, &draft.channel_id, &draft.root_id)?;
                    dirty.direct.push(ChangeSet::Draft {
                        channel_id: draft.channel_id,
                        root_id: draft.root_id,
                        draft: stored.map(Arc::new),
                    });
                }
                tx.commit()?;
            }
            Loaded::StatusesDue => self.fetch_statuses(),
            Loaded::Done => {
                // The server answered the whole load, so what waits from an
                // earlier run goes now, even before the socket is up (#194).
                for channel_id in queue::channels(self.store.conn())? {
                    self.drain(&channel_id, dirty)?;
                }
                dirty.direct.push(ChangeSet::Connection(Connection::Loaded));
            }
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
            Loaded::Thread {
                root_id,
                posts: list,
                older,
            } => {
                self.fetching.remove(&format!("thread/{root_id}"));
                let open = self.open_threads.contains(&root_id);
                let Some(list) = list else {
                    // A failed first page leaves the cached thread as it
                    // is; a failed older page offers Retry.
                    if let Some(pages) = self.thread_pages.get_mut(&root_id).filter(|_| older) {
                        pages.loading = false;
                        pages.failed = true;
                        dirty.threads.insert(root_id);
                    }
                    return Ok(());
                };
                if open {
                    let pages = self.thread_pages.entry(root_id.clone()).or_default();
                    pages.loading = false;
                    pages.at_start = !list.has_next;
                    let oldest = list
                        .posts
                        .values()
                        .filter(|post| post.id != root_id)
                        .map(|post| (post.create_at, post.id.clone()))
                        .min();
                    if let Some(oldest) = oldest {
                        pages.floor = Some(match pages.floor.take() {
                            Some(floor) if older => floor.min(oldest),
                            _ => oldest,
                        });
                    }
                    dirty.threads.insert(root_id.clone());
                }
                if !list.posts.is_empty() {
                    let tx = self.store.conn_mut().transaction()?;
                    let fetched: Vec<_> = list.posts.into_values().collect();
                    posts::insert_posts(&tx, &fetched)?;
                    tx.commit()?;
                    dirty.threads.insert(root_id);
                }
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
            Loaded::LinkPreview { url, graph } => {
                self.fetching.remove(&format!("opengraph/{url}"));
                let graph = graph.map(Arc::new);
                self.link_previews.insert(url.clone(), graph.clone());
                dirty.direct.push(ChangeSet::LinkPreview { url, graph });
            }
            Loaded::RemoteImage { url, image } => {
                let key = matty_store::media::remote_key(&url);
                self.fetching.remove(&key);
                if let Some((bytes, content_type)) = image {
                    if let Some(media) = &self.media
                        && let Err(error) =
                            media.put(self.store.conn(), &key, &bytes, &content_type)
                    {
                        tracing::warn!(%error, "could not keep an image");
                    }
                    dirty.direct.push(ChangeSet::RemoteImage {
                        url,
                        bytes: Arc::from(bytes),
                        content_type,
                    });
                }
            }
            Loaded::Picture {
                picture,
                id,
                version,
                image,
            } => {
                let key = picture.key(&id, version);
                self.fetching.remove(&key);
                if let Some((bytes, content_type)) = &image
                    && let Some(media) = &self.media
                    && let Err(error) = media.put(self.store.conn(), &key, bytes, content_type)
                {
                    tracing::warn!(%error, ?picture, "could not keep a picture");
                }
                dirty.direct.push(ChangeSet::Picture {
                    picture,
                    id,
                    version,
                    image: image.map(|(bytes, content_type)| (Arc::from(bytes), content_type)),
                });
            }
            Loaded::Suggested { term, users } => {
                sidebar::upsert_users(self.store.conn(), &users)?;
                self.sent_users.extend(users.iter().map(|u| u.id.clone()));
                let users: Arc<[matty_api::models::User]> = Arc::from(users);
                if !users.is_empty() {
                    dirty.direct.push(ChangeSet::Users(users.clone()));
                }
                dirty.direct.push(ChangeSet::Suggested { term, users });
            }
            Loaded::Resolved { query, answer } => {
                dirty.direct.push(ChangeSet::LinkResolved { query, answer })
            }
            Loaded::Bookmarks(loaded) => {
                let tx = self.store.conn_mut().transaction()?;
                crate::bookmarks::write(&tx, loaded, dirty)?;
                tx.commit()?;
            }
            Loaded::Changes(changes) => dirty.direct.extend(changes),
            Loaded::CommandSuggestions {
                channel_id,
                root_id,
                user_input,
                suggestions,
            } => dirty.direct.push(ChangeSet::CommandSuggestions {
                channel_id,
                root_id,
                user_input,
                suggestions: Arc::from(suggestions),
            }),
            Loaded::CommandRan { command, outcome } => dirty.direct.push(ChangeSet::CommandRan {
                channel_id: command.channel_id,
                root_id: command.root_id,
                command: command.command,
                outcome,
            }),
            Loaded::PostActionDone { post_id, action_id } => dirty
                .direct
                .push(ChangeSet::PostActionDone { post_id, action_id }),
            Loaded::DialogSubmitted {
                callback_id,
                cancelled,
                answer,
            } => dirty.direct.push(ChangeSet::DialogSubmitted {
                callback_id,
                cancelled,
                answer,
            }),
            Loaded::Search {
                terms,
                page,
                posts,
                more,
            } => dirty.direct.push(ChangeSet::Search {
                terms,
                page,
                posts: Arc::from(posts),
                more,
            }),
            Loaded::Listing { listing, posts } => dirty.direct.push(ChangeSet::Listing {
                listing,
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
                dirty.direct.push(ChangeSet::CollapsedThreads(crt));
                dirty.direct.push(ChangeSet::LinkPreviews(link_previews(
                    capabilities.link_previews,
                    &preferences,
                )));
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
                        self.refresh_statuses();
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
            Loaded::MoreThreads { team_id, threads } => {
                let pages = self.thread_list_pages.entry(team_id.clone()).or_default();
                pages.loading = false;
                dirty.thread_lists.insert(team_id.clone());
                let Some(page) = threads else {
                    pages.failed = true;
                    return Ok(());
                };
                pages.extra += page.threads.len();
                pages.total = Some(page.total);
                pages.complete = page.threads.len() < THREADS_PAGE as usize;
                let tx = self.store.conn_mut().transaction()?;
                posts::upsert_threads(&tx, &team_id, &page.threads)?;
                tx.commit()?;
            }
            Loaded::Threads {
                team_id,
                threads,
                total,
            } => {
                self.fetching.remove(&format!("threads/{team_id}"));
                if let Some(total) = total {
                    self.thread_list_pages
                        .entry(team_id.clone())
                        .or_default()
                        .total = Some(total);
                }
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
            Loaded::FileUp {
                pending_post_id,
                path,
                file_id,
            } => {
                queue::file_up(self.store.conn(), &pending_post_id, &path, &file_id)?;
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
                        matty_api::rt::spawn(async move {
                            matty_api::rt::sleep(wait).await;
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
                let action = reaction_action(added);
                dirty.direct.push(ChangeSet::Failed(match error {
                    None => CommandError::Offline { action },
                    Some(error) => CommandError::Server {
                        action,
                        target: reaction.post_id.clone(),
                        error,
                    },
                }));
            }
            Loaded::WriteFailed {
                action,
                target,
                error,
            } => {
                // An edit or delete the view already showed goes back to
                // what the cache holds.
                if matches!(action, "edit" | "delete")
                    && let Some(post) = posts::post(self.store.conn(), &target)?
                {
                    dirty.channels.insert(post.channel_id);
                    if self.open_threads.contains(&post.root_id) {
                        dirty.threads.insert(post.root_id);
                    } else if self.open_threads.contains(&post.id) {
                        dirty.threads.insert(post.id);
                    }
                }
                dirty.direct.push(ChangeSet::Failed(match error {
                    None => CommandError::Offline { action },
                    Some(error) => CommandError::Server {
                        action,
                        target,
                        error,
                    },
                }));
            }
            Loaded::ListingOffline(listing) => {
                let change = cached_listing(self.store.conn(), listing)?;
                dirty.direct.push(change);
            }
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
        if (dirty.saved || dirty.preferences)
            && let Ok(saved) = saved_post_ids(self.store.conn())
        {
            dirty.direct.push(ChangeSet::Saved(Arc::from(saved)));
        }
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
            if dirty.emoji {
                match sidebar::custom_emoji(conn) {
                    Ok(emoji) => changes.push(ChangeSet::CustomEmoji(Arc::from(emoji))),
                    Err(error) => tracing::error!(%error, "could not read custom emoji"),
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
            let pages = self.thread_pages.get(&root_id);
            match posts::thread(conn, &root_id) {
                Ok(mut thread) => {
                    let older =
                        pages.and_then(|pages| pages.floor.as_ref().filter(|_| !pages.at_start));
                    // Cached replies older than the pages loaded would show
                    // above a gap; they wait until a page reaches them.
                    if let Some((at, id)) = older {
                        thread.retain(|post| {
                            post.id == root_id || (post.create_at, &post.id) >= (*at, id)
                        });
                    }
                    changes.push(ChangeSet::Thread {
                        root_id,
                        posts: Arc::from(thread),
                        older: older.is_some(),
                        failed: pages.is_some_and(|pages| pages.failed),
                    });
                }
                Err(error) => tracing::error!(%error, "could not read a thread snapshot"),
            }
        }
        for team_id in dirty.thread_lists {
            if !self.open_thread_lists.contains(&team_id) {
                continue;
            }
            let pages = self.thread_list_pages.get(&team_id);
            let limit = THREAD_LIST + pages.map_or(0, |pages| pages.extra);
            let list = posts::threads(conn, &team_id, false, limit).and_then(|threads| {
                let roots = threads
                    .iter()
                    .filter_map(|thread| posts::post(conn, &thread.id).transpose())
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((threads, roots))
            });
            match list {
                Ok((threads, roots)) => {
                    let more = pages.is_some_and(|pages| {
                        !pages.complete
                            && pages
                                .total
                                .is_some_and(|total| total > threads.len() as i64)
                    });
                    changes.push(ChangeSet::ThreadList {
                        team_id,
                        threads: Arc::from(threads),
                        roots: Arc::from(roots),
                        more,
                        failed: pages.is_some_and(|pages| pages.failed),
                    });
                }
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
                Ok(window) => {
                    if !roots_only {
                        match reply_roots(conn, &window) {
                            Ok(roots) if !roots.is_empty() => {
                                changes.push(ChangeSet::ReplyRoots {
                                    channel_id: channel_id.clone(),
                                    roots: Arc::from(roots),
                                });
                            }
                            Ok(_) => {}
                            Err(error) => tracing::error!(%error, "could not read reply roots"),
                        }
                    }
                    changes.push(ChangeSet::Posts {
                        channel_id,
                        posts: Arc::from(window),
                    });
                }
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
        let mut ids = BTreeSet::new();
        for change in changes {
            match change {
                ChangeSet::Posts { posts, .. }
                | ChangeSet::ReplyRoots { roots: posts, .. }
                | ChangeSet::Thread { posts, .. }
                | ChangeSet::Search { posts, .. }
                | ChangeSet::Listing { posts, .. } => {
                    ids.extend(posts.iter().map(|post| post.user_id.clone()));
                    // Forwards quote another post, named by its author.
                    ids.extend(posts.iter().flat_map(quoted_authors));
                }
                // The Threads view names each root's author and who replied.
                ChangeSet::ThreadList { threads, roots, .. } => {
                    ids.extend(roots.iter().map(|post| post.user_id.clone()));
                    ids.extend(threads.iter().flat_map(|t| t.participants.iter().cloned()));
                }
                _ => {}
            }
        }
        self.watch_statuses(&ids);
        ids.retain(|id| !id.is_empty() && !self.sent_users.contains(id));
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
            matty_api::rt::spawn(async move {
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
    /// Queues the statuses of `ids` (but the user's own, which the account
    /// load and `status_change` keep) and wakes the engine when the batch
    /// window closes.
    fn watch_statuses(&mut self, ids: &BTreeSet<String>) {
        if self.session.is_none() {
            return;
        }
        let me = self.me.as_deref();
        let ids = ids.iter().map(String::as_str).filter(|id| Some(*id) != me);
        let Some(due) = self.statuses.add(ids, Instant::now()) else {
            return;
        };
        let loaded = self.loaded.clone();
        matty_api::rt::spawn(async move {
            matty_api::rt::sleep(due.saturating_duration_since(Instant::now())).await;
            let _ = loaded.send(Loaded::StatusesDue);
        });
    }

    /// The batch window closed: asks for its statuses in one request.
    fn fetch_statuses(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Some(ids) = self.statuses.take(Instant::now()) else {
            return;
        };
        let client = session.client.clone();
        let loaded = self.loaded.clone();
        matty_api::rt::spawn(async move {
            match client.statuses(&ids).await {
                Ok(statuses) => {
                    let _ = loaded.send(Loaded::Statuses(statuses));
                }
                Err(error) => tracing::warn!(%error, "could not fetch statuses"),
            }
        });
    }

    /// After a reconnect: statuses changed while the socket was down went
    /// unannounced, so asks again for everyone shown, over the socket.
    fn refresh_statuses(&self) {
        let Some(connection) = self.session.as_ref().and_then(|s| s.socket.connection()) else {
            return;
        };
        let ids: Vec<String> = self.statuses.known().map(str::to_owned).collect();
        if ids.is_empty() {
            return;
        }
        let loaded = self.loaded.clone();
        matty_api::rt::spawn(async move {
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            match connection.get_statuses_by_ids(&ids).await {
                Ok(reply) if reply.is_ok() => {
                    let statuses = crate::presence::statuses_reply(reply.data);
                    if !statuses.is_empty() {
                        let _ = loaded.send(Loaded::Statuses(statuses));
                    }
                }
                Ok(reply) => tracing::debug!(?reply.error, "statuses were refused"),
                Err(error) => tracing::debug!(%error, "could not refresh statuses"),
            }
        });
    }

    /// Preferences changed: the collapsed-threads choice may have too, which
    /// changes every count and the channel windows.
    fn preferences_changed(&mut self, dirty: &mut Dirty) {
        dirty.direct.push(ChangeSet::Preferences);
        let conn = self.store.conn();
        let Ok(preferences) = all_preferences(conn) else {
            return;
        };
        dirty
            .direct
            .push(ChangeSet::AccountPreferences(account_preferences(
                &preferences,
            )));
        let Some(capabilities) = &self.capabilities else {
            return;
        };
        let crt = collapsed_threads(capabilities.collapsed_threads, &preferences);
        dirty.direct.push(ChangeSet::CollapsedThreads(crt));
        dirty.direct.push(ChangeSet::LinkPreviews(link_previews(
            capabilities.link_previews,
            &preferences,
        )));
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
        if !crt {
            self.fetch_replies();
        }
    }

    /// Collapsed reply threads went off: the open channels were fetched
    /// without their replies, so their newest page comes again with them
    /// (#123).
    fn fetch_replies(&self) {
        let Some(session) = self.session.as_ref().filter(|s| !s.user_id.is_empty()) else {
            return;
        };
        for channel_id in self.open_channels.iter().cloned() {
            let client = session.client.clone();
            let results = self.loaded.clone();
            matty_api::rt::spawn(async move {
                match client
                    .channel_posts(&channel_id, &matty_api::PostsQuery::latest(HISTORY_PAGE))
                    .await
                {
                    Ok(posts) => {
                        let _ = results.send(Loaded::Posts { channel_id, posts });
                    }
                    Err(error) => tracing::debug!(%error, "could not read the replies"),
                }
            });
        }
    }
}

/// The authors of the posts a post quotes through permalinks.
fn quoted_authors(post: &matty_api::models::Post) -> impl Iterator<Item = String> + '_ {
    post.metadata
        .embeds
        .iter()
        .filter(|embed| embed.kind == matty_api::models::EmbedKind::Permalink)
        .filter_map(|embed| {
            let id = embed.data.as_ref()?.get("post")?.get("user_id")?.as_str()?;
            (!id.is_empty()).then(|| id.to_owned())
        })
}

/// Whether there is a signed-in session to call the server with.
/// `matty --offline` has none; a dropped network still has one, and its
/// calls fail as [`CommandError::Offline`].
fn connected(session: Option<&Session>) -> bool {
    session.is_some_and(|s| !s.user_id.is_empty())
}

/// What toggling a reaction asked, for [`CommandError`].
fn reaction_action(add: bool) -> &'static str {
    if add {
        "add reaction"
    } else {
        "remove reaction"
    }
}

/// A listing answered from the cache, for when the server cannot be
/// reached (#163): the saved posts and a channel's pins this computer
/// holds, newest first. Recent mentions need the server's search.
fn cached_listing(conn: &Db, listing: crate::Listing) -> Result<ChangeSet, StoreError> {
    let mut found = match &listing {
        crate::Listing::Saved => {
            let mut found = Vec::new();
            for id in saved_post_ids(conn)? {
                if let Some(post) = posts::post(conn, &id)?
                    && post.delete_at == 0
                {
                    found.push(post);
                }
            }
            found
        }
        crate::Listing::Pinned { channel_id } => posts::pinned(conn, channel_id)?,
        crate::Listing::Mentions { .. } => {
            return Ok(ChangeSet::Failed(CommandError::Offline {
                action: "load recent mentions",
            }));
        }
    };
    found.sort_by(|a, b| (b.create_at, &b.id).cmp(&(a.create_at, &a.id)));
    Ok(ChangeSet::Listing {
        listing,
        posts: Arc::from(found),
    })
}

/// The preference category Mattermost keeps saved ("flagged") posts in.
const SAVED_CATEGORY: &str = "flagged_post";

/// The posts the user saved, as the cache knows them.
fn saved_post_ids(conn: &Db) -> Result<Vec<String>, StoreError> {
    Ok(all_preferences(conn)?
        .into_iter()
        .filter(|preference| preference.category == SAVED_CATEGORY && preference.value == "true")
        .map(|preference| preference.name)
        .collect())
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

pub(crate) fn cached_crt(conn: &Db) -> Result<bool, StoreError> {
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

/// What the cache already knows about a link's target; `None` sends the
/// question to the server.
fn cached_link(db: &Db, query: &crate::LinkQuery) -> Result<Option<crate::LinkAnswer>, StoreError> {
    use crate::{LinkAnswer, LinkQuery};
    Ok(match query {
        LinkQuery::Post { post_id } => match posts::post(db, post_id)? {
            Some(post) => sidebar::channel(db, &post.channel_id)?
                .map(|channel| LinkAnswer::Channel(Arc::new(channel))),
            None => None,
        },
        LinkQuery::Channel { team_id, name } => sidebar::channel_by_name(db, team_id, name)?
            .map(|channel| LinkAnswer::Channel(Arc::new(channel))),
        LinkQuery::User { username } => {
            sidebar::user_by_username(db, username)?.map(|user| LinkAnswer::User(Arc::new(user)))
        }
    })
}

/// Whether link previews show: the server's `EnableLinkPreviews`, then the
/// user's `display_settings/link_previews`, on unless set to "false".
/// The categories of account preferences Preferences mirrors (#156, #157),
/// and the user's Mattermost themes (#214).
const ACCOUNT_CATEGORIES: [&str; 3] = ["display_settings", "advanced_settings", "theme"];

/// The user's display and advanced settings and themes, for
/// [`ChangeSet::AccountPreferences`].
pub(crate) fn account_preferences(preferences: &[Preference]) -> Arc<[Preference]> {
    preferences
        .iter()
        .filter(|p| ACCOUNT_CATEGORIES.contains(&p.category.as_str()))
        .cloned()
        .collect()
}

pub(crate) fn link_previews(enabled: bool, preferences: &[Preference]) -> bool {
    enabled
        && !preferences.iter().any(|p| {
            p.category == "display_settings" && p.name == "link_previews" && p.value == "false"
        })
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

/// Replies per page of a long thread (#122).
const THREAD_PAGE: u32 = 50;

/// Followed threads per page of the Threads list (#122).
const THREADS_PAGE: u32 = 25;

/// How far back an open thread's replies are loaded.
#[derive(Debug, Default)]
struct ThreadPages {
    /// The oldest reply fetched, by creation time then id; while older
    /// replies remain, the snapshot leaves out cached ones before it.
    floor: Option<(i64, String)>,
    /// Every reply back to the root is loaded.
    at_start: bool,
    /// An older page is on its way.
    loading: bool,
    /// The last older page failed.
    failed: bool,
}

/// How far a team's Threads list is loaded.
#[derive(Debug, Default)]
struct ListPages {
    /// Threads listed beyond [`THREAD_LIST`], one page at a time.
    extra: usize,
    /// The server's count of the team's followed threads, once asked.
    total: Option<i64>,
    /// A page came back short: there are no more.
    complete: bool,
    /// A page is on its way.
    loading: bool,
    /// The last page failed.
    failed: bool,
}

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

/// Saves `category` back to the server as the user's (`me`).
/// Swaps `id` with its neighbor above (`up`) or below; false when it is
/// missing or already at that end.
fn step(ids: &mut [String], id: &str, up: bool) -> bool {
    let Some(at) = ids.iter().position(|other| other == id) else {
        return false;
    };
    let to = if up { at.checked_sub(1) } else { Some(at + 1) };
    match to {
        Some(to) if to < ids.len() => {
            ids.swap(at, to);
            true
        }
        _ => false,
    }
}

/// Puts `id` at `index` in `ids` (clamped to the end), taking it out of
/// wherever it was; false when it is already there.
fn place(ids: &mut Vec<String>, id: String, index: usize) -> bool {
    let before = ids.clone();
    ids.retain(|other| *other != id);
    let index = index.min(ids.len());
    ids.insert(index, id);
    *ids != before
}

fn category_write(me: Option<&str>, mut category: matty_api::models::SidebarCategory) -> Write {
    category.user_id = me.unwrap_or_default().to_owned();
    Write::UpdateCategory {
        team_id: category.team_id.clone(),
        category: Box::new(category),
    }
}

/// The cached roots of the replies in `window` that it does not hold.
fn reply_roots(
    conn: &rusqlite::Connection,
    window: &[matty_api::models::Post],
) -> Result<Vec<matty_api::models::Post>, StoreError> {
    let shown: BTreeSet<&str> = window.iter().map(|post| post.id.as_str()).collect();
    let missing: BTreeSet<&str> = window
        .iter()
        .map(|post| post.root_id.as_str())
        .filter(|root| !root.is_empty() && !shown.contains(root))
        .collect();
    let mut roots = Vec::new();
    for root_id in missing {
        if let Some(root) = posts::post(conn, root_id)? {
            roots.push(root);
        }
    }
    Ok(roots)
}

/// Whether the send is on its way to the server now, given each channel's
/// send in flight.
fn in_flight(sending: &std::collections::HashMap<String, String>, pending_post_id: &str) -> bool {
    sending.values().any(|id| id == pending_post_id)
}

/// Every stored draft as a [`ChangeSet::Draft`], so a subscriber restores
/// the composers and marks the sidebar before the server answers.
fn publish_drafts(conn: &Db, dirty: &mut Dirty) -> Result<(), StoreError> {
    for draft in posts::drafts(conn)? {
        dirty.direct.push(ChangeSet::Draft {
            channel_id: draft.channel_id.clone(),
            root_id: draft.root_id.clone(),
            draft: Some(Arc::new(draft)),
        });
    }
    Ok(())
}
