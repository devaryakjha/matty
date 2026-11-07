//! The right pane (DESIGN.md §4): threads first, later search results,
//! saved, pinned, mentions and channel info. It shares the headers' 48px
//! height, separates from the channel with a hairline, and closes on
//! Escape.

mod schedule;
mod slash;

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::base::input::{MoveDown, MoveUp};
use gpui_kit::{
    App, AppContext as _, ClipboardEntry, Context, Entity, EventEmitter, ExternalPaths,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, prelude::FluentBuilder as _, px,
};

use crate::actions::{self, CloseRightPane, UploadFiles};
use crate::attachments::{
    AttachmentStatus, PostFile, SendGate, UploadSource, UploadTray, post_files,
};
use crate::channel::{
    ChannelModel, Post, PostKind, PostStatus, delete_popup, edit_composer, emoji_popup,
    message_density, plain_text, post_editor, post_markers, post_time, queued, queued_marker,
    quick_reaction_buttons, reaction_tooltip, waiting_items, waiting_to_send,
};
use crate::profile::{ProfileActions, UserProfile, profile_card, profile_pane};
use crate::settings::{ClockFormat, SettingsChanged, SettingsStore};
use crate::sidebar::{ChannelKind, Presence};
use crate::style::{self, Palette, TypeStyleExt as _, metrics, scaled};
use crate::t;
use crate::ui::{
    alert_dialog::{alert_dialog, alert_dialog_action, alert_dialog_cancel},
    attachment::Attachment,
    avatar::{Avatar, AvatarSize},
    badge::{Badge, BadgeVariant},
    button::{Button, ButtonSize, ButtonVariant},
    composer::{Composer, ComposerEvent, ComposerState, ComposerSuggestion},
    dialog::{DialogHandle, dialog_description, dialog_popup, dialog_title},
    emoji_picker::{EmojiPicker, EmojiPickerEvent, EmojiPickerState},
    input::{Input, InputEvent, InputState},
    lightbox::{LightboxImage, LightboxState, lightbox},
    menu::{Menu, MenuItem, MenuState},
    message::{Message, MessageHighlight, MessageStatus, message_action, message_actions},
    message_list::{MessageList, MessageListState},
    popover::popover,
    spinner::Spinner,
    theme::UiTheme,
};

/// The reply tray's upload ids start with this, so the window can tell
/// them from the channel tray's.
pub const UPLOAD_PREFIX: &str = "thread-upload";

/// A thread: its root post and the replies loaded for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadModel {
    /// The thread's channel, where its reply draft belongs.
    pub channel_id: SharedString,
    pub channel_name: SharedString,
    pub root: Post,
    pub replies: Vec<Post>,
    pub following: bool,
    /// The thread's replies on the server, loaded or not.
    pub reply_count: usize,
    /// Older replies than the first in `replies` are on the server; the
    /// row above the replies pages them in (#122).
    pub older: bool,
    /// The last older page failed; that row offers Retry.
    pub failed_older: bool,
}

/// A post listed outside its channel: saved, pinned or a mention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedPost {
    pub channel_id: SharedString,
    pub channel_name: SharedString,
    pub post: Post,
}

/// A file a search found (#207), with where and when it was posted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundFile {
    pub file: crate::attachments::PostFile,
    pub channel_id: SharedString,
    pub channel_name: SharedString,
    pub post_id: SharedString,
    pub author: SharedString,
    pub day: SharedString,
}

/// A message search and what came back (PLAN §3 M1 "Search").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchModel {
    /// The files that matched, from the first page (#207).
    pub files: Vec<FoundFile>,
    pub query: SharedString,
    pub results: Vec<ListedPost>,
    /// True while sync runs the query.
    pub searching: bool,
    /// Another page of results may follow.
    pub more: bool,
    /// The next page is on its way.
    pub loading_more: bool,
}

/// What search's `in:` and `from:` suggest: (handle, shown name) pairs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchHints {
    pub channels: Vec<(SharedString, SharedString)>,
    pub people: Vec<(SharedString, SharedString)>,
    /// Today in the user's time zone, for the dates `on:`, `before:` and
    /// `after:` suggest (#207).
    pub today: Option<jiff::civil::Date>,
}

/// One suggestion under the search field: what replaces the word being
/// typed, and how it reads.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Suggestion {
    insert: SharedString,
    label: SharedString,
    detail: SharedString,
}

/// Where the highlight lands after one arrow press among `count` rows:
/// down from nothing starts at the top, up from nothing at the bottom, and
/// both wrap around.
fn step(at: Option<usize>, count: usize, down: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match (at, down) {
        (None, true) => 0,
        (None, false) => count - 1,
        (Some(at), true) => (at + 1) % count,
        (Some(at), false) => (at + count - 1) % count,
    })
}

/// At most this many suggestions show at once.
const SUGGESTIONS: usize = 6;

/// The date modifiers search takes, as the server reads them (#207).
const DATE_MODIFIERS: [&str; 3] = ["on:", "before:", "after:"];

/// The suggestions for a date modifier: today, yesterday and the five days
/// before, by name, each inserting the date as the server reads it
/// (`on:2026-11-05`). What was typed narrows them by name or by date.
/// A weekday's name in the user's language.
fn weekday(day: jiff::civil::Weekday) -> SharedString {
    use jiff::civil::Weekday::*;
    match day {
        Monday => t!("pane-weekday-monday"),
        Tuesday => t!("pane-weekday-tuesday"),
        Wednesday => t!("pane-weekday-wednesday"),
        Thursday => t!("pane-weekday-thursday"),
        Friday => t!("pane-weekday-friday"),
        Saturday => t!("pane-weekday-saturday"),
        Sunday => t!("pane-weekday-sunday"),
    }
}

fn date_suggestions(prefix: &str, typed: &str, today: jiff::civil::Date) -> Vec<Suggestion> {
    let typed = typed.to_lowercase();
    (0..7)
        .filter_map(|back| {
            let date = today.checked_sub(jiff::Span::new().days(back)).ok()?;
            let label = match back {
                0 => t!("pane-today").to_string(),
                1 => t!("pane-yesterday").to_string(),
                _ => weekday(date.weekday()).to_string(),
            };
            let iso = date.strftime("%Y-%m-%d").to_string();
            (iso.starts_with(&typed) || label.to_lowercase().starts_with(&typed)).then(|| {
                Suggestion {
                    insert: format!("{prefix}{iso}").into(),
                    label: label.into(),
                    detail: date.strftime("%B %-d").to_string().into(),
                }
            })
        })
        .filter(|suggestion| suggestion.insert.as_ref() != format!("{prefix}{typed}"))
        .take(SUGGESTIONS)
        .collect()
}

/// The suggestions for `query`'s last word when it starts with `from:` or
/// `in:` (#129), or a date modifier (#207).
fn suggestions(query: &str, hints: &SearchHints) -> Vec<Suggestion> {
    if query.ends_with(char::is_whitespace) {
        return Vec::new();
    }
    let word = query.rsplit(char::is_whitespace).next().unwrap_or("");
    if let Some(today) = hints.today
        && let Some((prefix, typed)) = DATE_MODIFIERS
            .iter()
            .find_map(|prefix| Some((*prefix, word.strip_prefix(prefix)?)))
    {
        return date_suggestions(prefix, typed, today);
    }
    let (prefix, typed, options) = if let Some(typed) = word.strip_prefix("from:") {
        ("from:", typed.trim_start_matches('@'), &hints.people)
    } else if let Some(typed) = word.strip_prefix("in:") {
        ("in:", typed.trim_start_matches('~'), &hints.channels)
    } else {
        return Vec::new();
    };
    let typed = typed.to_lowercase();
    options
        .iter()
        .filter(|(handle, shown)| {
            handle.to_lowercase().starts_with(&typed)
                || shown
                    .to_lowercase()
                    .split_whitespace()
                    .any(|part| part.starts_with(&typed))
        })
        .filter(|(handle, _)| handle.as_ref() != typed)
        .take(SUGGESTIONS)
        .map(|(handle, shown)| Suggestion {
            insert: format!("{prefix}{handle}").into(),
            label: shown.clone(),
            detail: if prefix == "from:" {
                format!("@{handle}").into()
            } else {
                format!("~{handle}").into()
            },
        })
        .collect()
}

/// What the pane shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RightPaneContent {
    Thread(Box<ThreadModel>),
    ChannelInfo(ChannelModel),
    /// The current channel's files, newest first (#208); `None` while
    /// they load.
    ChannelFiles {
        channel_name: SharedString,
        files: Option<Vec<FoundFile>>,
    },
    /// The current channel's pinned posts, newest first.
    Pinned {
        channel_name: SharedString,
        posts: Vec<ListedPost>,
    },
    /// The user's saved posts across channels, newest first.
    Saved(Vec<ListedPost>),
    /// Posts that mention the user, newest first.
    Mentions(Vec<ListedPost>),
    /// The unread digest (#223): each unread channel's first unread
    /// posts, channel by channel.
    Unreads(Vec<ListedPost>),
    Search(SearchModel),
    /// Someone's profile (#212).
    Profile(UserProfile),
}

/// What the pane asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RightPaneEvent {
    /// Offline, the user asked for this action (as sync names it), which
    /// needs the server; nothing changed, and the window says so (#163).
    Offline(&'static str),
    Close,
    /// The user confirmed deleting their sent reply; the pane already
    /// dropped it.
    Delete(SharedString),
    /// "Remove from channel" on a member in the channel info (#208).
    RemoveMember {
        user_id: SharedString,
    },
    /// "Make channel admin" or "Remove channel admin" on a member (#208).
    SetChannelAdmin {
        user_id: SharedString,
        admin: bool,
    },
    /// The user submitted a message search; sync runs it and calls
    /// `set_search` with the results.
    Search(SharedString),
    /// The user asked for the search's next page.
    SearchMore,
    /// The reply composer typed `@` and this much of a name; sync asks
    /// the server and [`RightPaneView::set_suggested`] takes the answer.
    SuggestPeople(SharedString),
    /// Message menu actions on a thread post, as in the channel (#213).
    CopyLink(SharedString),
    SetSaved {
        post: SharedString,
        saved: bool,
    },
    SetPinned {
        post: SharedString,
        pinned: bool,
    },
    Forward(SharedString),
    /// Mark the channel unread from this reply on.
    MarkUnread(SharedString),
    Remind {
        post: SharedString,
        at: crate::channel::RemindAt,
    },
    /// The user replied under `root`; the pane already shows the reply
    /// pending (PLAN §4.3).
    Reply {
        root: SharedString,
        text: SharedString,
        /// Tray ids of the reply's files, all uploaded.
        files: Vec<SharedString>,
        /// False when the user removed the link's preview while replying.
        unfurl: bool,
    },
    /// The user scheduled the reply composer's text and these uploads for
    /// `at` (ms since the epoch) under `root` in `channel` (#220); the
    /// composer is already clear.
    Schedule {
        channel: SharedString,
        root: SharedString,
        text: SharedString,
        files: Vec<SharedString>,
        at: i64,
    },
    /// A file went into the reply tray; sync uploads it to the thread's
    /// channel and reports back through `set_upload_status`.
    Upload {
        id: SharedString,
        name: SharedString,
        source: UploadSource,
    },
    /// A file left the reply tray; sync stops its upload.
    CancelUpload(SharedString),
    /// The user asked to upload a failed reply file again.
    RetryUpload(SharedString),
    /// The reply draft has this link; sync asks the server for its
    /// preview and calls `set_link_preview` (#188).
    PreviewLink(SharedString),
    /// The reply draft changed, after a pause in typing, on leaving the
    /// thread or on sending; sync stores it (#114).
    SaveDraft(crate::composer_draft::DraftSave),
    /// Retry a reply that failed; the pane shows it pending again.
    Retry(SharedString),
    /// The user edited a reply waiting to send; it goes with this text
    /// (#195).
    SaveEdit {
        post: SharedString,
        text: SharedString,
    },
    /// The user discarded a reply the server never got; the row is
    /// already gone and sync drops it from the outbox (#195).
    Discard(SharedString),
    /// The user toggled following the thread under `root`.
    ToggleFollow {
        root: SharedString,
        following: bool,
    },
    /// The user acknowledged a post that asked for it, or took it back
    /// (#220).
    Acknowledge {
        post: SharedString,
        acknowledged: bool,
    },
    ToggleReaction {
        post: SharedString,
        emoji: SharedString,
    },
    /// Jump to `post` in `channel` from a listing.
    Jump {
        channel: SharedString,
        post: SharedString,
    },
    /// The user clicked a file on a post; sync opens or downloads it.
    OpenFile(SharedString),
    /// The lightbox opened on these image files; the window hands their
    /// full size back through `set_full_image`.
    ViewImages(Vec<SharedString>),
    /// The channel info pane asked for the pinned listing.
    OpenPinned,
    /// "Files" in the channel info (#208).
    OpenChannelFiles,
    /// "Add members" in the channel info (#208).
    AddMembers,
    /// The user clicked a link or an entity chip in a message (#103).
    /// A slash command in the reply composer needs the server or the
    /// window (#210).
    Slash(crate::slash::Request),
    OpenLink(SharedString),
    /// View profile on a member's profile card; the window shows it here.
    ViewProfile(UserProfile),
    /// "Add to channel" on a profile card or the profile pane (#212).
    AddToChannel(UserProfile),
    /// Message on a member's profile card.
    OpenDirect {
        user_id: SharedString,
        username: SharedString,
        display_name: SharedString,
    },
    /// Mention on a member's profile card; the window puts `@username `
    /// in the channel composer.
    Mention(SharedString),
    /// The info pane starred or unstarred the channel; it already shows it.
    ToggleFavorite {
        channel: SharedString,
        favorite: bool,
    },
    /// The info pane muted or unmuted the channel; it already shows it.
    ToggleMute {
        channel: SharedString,
        muted: bool,
    },
    /// The user confirmed leaving the channel.
    Leave(SharedString),
    /// The pane lists only the loaded profiles; sync fetches the rest.
    LoadMembers(SharedString),
    /// The thread transcript reached its oldest loaded reply under `root`;
    /// sync pages in the replies before it (#122).
    LoadOlderReplies(SharedString),
    /// "Open in new window" on the thread under `root` (#222).
    PopOut(SharedString),
    /// A button pressed or a menu option picked on an attachment in the
    /// thread (#211); sync runs it as it does from the channel.
    PostAction(crate::interactive::ActionChoice),
    /// The user dismissed an ephemeral reply; the pane already dropped it.
    DismissEphemeral(SharedString),
}

/// Rows of the thread transcript before the replies: the root, the
/// replies divider, and the row that pages in older replies (#122).
const THREAD_HEAD: usize = 3;

/// What a lightbox image is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum LightboxItem {
    /// A file on the post, by id.
    File(SharedString),
    /// An image the message links to, by URL.
    Link(SharedString),
}

pub struct RightPaneView {
    /// `style::composer_cap` as of the last render.
    composer_cap: gpui_kit::Pixels,
    content: Option<RightPaneContent>,
    list: Entity<MessageListState>,
    composer: Entity<ComposerState>,
    focus: FocusHandle,
    /// Focus of the thread transcript while a row is keyboard-selected.
    thread_focus: FocusHandle,
    /// The keyboard-selected row of the thread transcript (PLAN §4.2).
    selection: Option<usize>,
    /// An older page of the thread's replies is on its way (#122).
    loading_older: bool,
    /// The shared right-click menu over thread rows.
    menu: Entity<MenuState>,
    /// The current user's display name, for optimistic rows.
    me: SharedString,
    /// People the thread's authors are looked up in for their presence
    /// dots (#135); the window keeps it in step with the channel's.
    profiles: Vec<UserProfile>,
    /// What the server said about names typed after `@` in a reply.
    people: crate::channel::PeopleAsk,
    /// The channels `~` can complete in a reply.
    directory: Vec<crate::sidebar::ChannelRow>,
    /// Who is typing a reply in the thread (#137).
    typing: Vec<SharedString>,
    draft_len: usize,
    /// Which thread's draft the composer holds, and what was saved of it.
    draft: crate::composer_draft::ComposerDraft,
    _draft_task: Option<gpui_kit::Task<()>>,
    /// This pane's window, to put a thread's draft in the composer once the
    /// pane switches threads.
    window: gpui_kit::AnyWindowHandle,
    /// The preview above the reply composer (#188).
    link: crate::draft_link::DraftLink,
    /// Files waiting to go with the reply, and a reply waiting on them.
    tray: UploadTray,
    _link_task: Option<gpui_kit::Task<()>>,
    pending_seq: usize,
    /// The search field at the top of the Search listing.
    search: Entity<InputState>,
    /// The saved, pinned or mentions listing has yet to come from sync.
    loading: bool,
    /// What `from:` and `in:` can suggest, and what they suggest now.
    hints: SearchHints,
    /// The search shows its Files tab, not its Messages one (#207).
    search_files_tab: bool,
    suggestions: Vec<Suggestion>,
    /// The suggestion the arrow keys point at, which Enter picks.
    suggestion_at: Option<usize>,
    /// The image viewer over a post's images.
    lightbox: Entity<LightboxState>,
    lightbox_handle: DialogHandle,
    /// What each lightbox image is: a file id to download, or the URL of
    /// an image the message links to (#683).
    lightbox_items: Vec<LightboxItem>,
    /// The "Leave channel?" confirmation.
    leave: DialogHandle,
    /// The connection is down, so pending replies wait for it (#195).
    away: bool,
    /// The banner says the user is offline: actions that need the server
    /// are refused before the pane changes, and empty listings say why
    /// (#163).
    offline: bool,
    /// The reply being edited in place, waiting to send (#195).
    editing: Option<ReplyEdit>,
    /// "Discard this message?" and the reply it is about.
    discard: DialogHandle,
    discarding: Option<SharedString>,
    /// The reply a notification opened the thread at, tinted for a few
    /// seconds as the channel tints a permalink's post (#141).
    permalink: Option<SharedString>,
    _permalink_task: Option<gpui_kit::Task<()>>,
    /// A jump waiting for its thread to arrive: the root and the post.
    pending_jump: Option<(SharedString, SharedString)>,
    /// The pane is a pop-out window's whole content (#222): its header
    /// offers neither Expand nor another window.
    popped_out: bool,
    /// The menus on the thread's attachments (#211).
    action_menus: std::rc::Rc<std::cell::RefCell<crate::interactive::Menus>>,
    /// Slash commands typed in the reply composer (#210).
    slash: crate::slash::Slash,
    /// Picks a custom time for "Schedule message" (#220).
    schedule_dialog: Entity<crate::scheduled::ScheduleDialog>,
    /// The full emoji picker, on the reply composer's emoji button, for the
    /// reply or for a reaction, as in the channel (#679).
    picker: Entity<EmojiPickerState>,
    picker_open: bool,
    /// The post the picker reacts to; `None` types into the reply.
    picker_target: Option<SharedString>,
    _subscriptions: Vec<gpui_kit::Subscription>,
}

/// An open reply editor's composer and height cap, for `post_row`.
type ReplyEditor = (Entity<ComposerState>, gpui_kit::Pixels);

/// A reply open in its in-place editor.
struct ReplyEdit {
    post: SharedString,
    composer: Entity<ComposerState>,
    _subscription: gpui_kit::Subscription,
}

impl EventEmitter<RightPaneEvent> for RightPaneView {}

impl Focusable for RightPaneView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl RightPaneView {
    pub fn new(me: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.subscribe(
            &SettingsStore::global(cx),
            |this, store, _: &SettingsChanged, cx| {
                let send_on = store.read(cx).get().send_on;
                crate::channel::follow_send_key(&this.composer, send_on, cx);
                this.list.update(cx, |list, cx| list.remeasure(cx));
                cx.notify();
            },
        )
        .detach();
        let send_on = SettingsStore::global(cx).read(cx).get().send_on;
        let list = cx.new(|cx| MessageListState::new(0, cx));
        let keys = cx.weak_entity();
        let paste = cx.weak_entity();
        let composer = cx.new(|cx| {
            let mut state = ComposerState::new(6, window, cx);
            state.set_placeholder(t!("pane-reply-placeholder"), window, cx);
            state.set_triggers(['@', '~', ':', '/'], cx);
            state.set_submit_on_enter(send_on.submits_on_enter(""));
            // Ctrl+Enter (Cmd+Enter on macOS) always sends; see ChannelView.
            state.set_on_key(move |keystroke, window, cx| {
                if let Some(composer) = keys.upgrade().map(|view| view.read(cx).composer.clone())
                    && crate::channel::apply_format(&composer, keystroke, window, cx)
                {
                    return true;
                }
                if keystroke.key == "tab" && keystroke.modifiers.shift {
                    // Shift+Tab leaves the composer for keyboard selection of
                    // the thread's rows, as in the channel (PLAN §4.2).
                    return keys
                        .update(cx, |this, cx| {
                            this.select_newest(window, cx);
                            true
                        })
                        .unwrap_or(false);
                }
                if keystroke.key != "enter"
                    || !(keystroke.modifiers.control || keystroke.modifiers.platform)
                {
                    return false;
                }
                keys.update(cx, |this, cx| {
                    let text = this.composer.read(cx).value(cx);
                    this.reply(text, window, cx);
                    true
                })
                .unwrap_or(false)
            });
            // Images and files on the clipboard go to the tray, as in the
            // channel composer.
            state.set_on_paste(move |item, _, cx| {
                let sources: Vec<UploadSource> = item
                    .entries()
                    .iter()
                    .flat_map(|entry| match entry {
                        ClipboardEntry::Image(image) => {
                            vec![UploadSource::Image(std::sync::Arc::new(image.clone()))]
                        }
                        ClipboardEntry::ExternalPaths(paths) => paths
                            .paths()
                            .iter()
                            .cloned()
                            .map(UploadSource::Path)
                            .collect(),
                        ClipboardEntry::String(_) => vec![],
                    })
                    .collect();
                if sources.is_empty() {
                    return false;
                }
                paste
                    .update(cx, |this, cx| this.add_sources(sources, cx))
                    .is_ok()
            });
            state
        });
        crate::updater::watch_composer(&composer, cx);
        #[cfg(not(target_family = "wasm"))]
        crate::spelling::check(&composer, cx);
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("pane-search-placeholder")));
        let (schedule_dialog, scheduling) = schedule::schedule_dialog(window, cx);
        let composer_input = composer.read(cx).input().clone();
        let picker = cx.new(|cx| EmojiPickerState::new(crate::emoji::categories(), window, cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &picker,
                window,
                |this, _, event: &EmojiPickerEvent, window, cx| match event {
                    EmojiPickerEvent::Pick(name) => this.pick_emoji(name.clone(), window, cx),
                    EmojiPickerEvent::Cancel => this.close_picker(window, cx),
                    EmojiPickerEvent::SkinTone(tone) => crate::emoji::save_skin_tone(*tone, cx),
                },
            ),
            // Typed or set (a draft, a send, a cleared command): the slash
            // command line's suggestions follow the text.
            cx.observe(&composer_input, |this, input, cx| {
                let text = input.read(cx).value().to_string();
                this.slash_line(&text, cx);
            }),
            scheduling,
            cx.subscribe_in(
                &composer,
                window,
                |this, composer, event: &ComposerEvent, window, cx| match event {
                    ComposerEvent::Change => {
                        let send_on = SettingsStore::global(cx).read(cx).get().send_on;
                        crate::channel::follow_send_key(composer, send_on, cx);
                        let text = composer.read(cx).value(cx);
                        this.draft_len = text.chars().count();
                        this.draft_changed(&text, cx);
                        this.slash.edited(&text);
                        // A restored draft is not the user typing.
                        if !this.draft.is_saved(&text) {
                            this.schedule_draft_save(cx);
                        }
                        cx.notify();
                    }
                    ComposerEvent::Submit(text) => this.reply(text.clone(), window, cx),
                    ComposerEvent::Autocomplete {
                        trigger: '/',
                        query,
                    } => this.slash_typing(query, cx),
                    ComposerEvent::AutocompleteClosed => this.slash.closed(),
                    ComposerEvent::Autocomplete { trigger, query } => {
                        this.slash.closed();
                        if let Some(term) = this.people.ask(*trigger, query) {
                            cx.emit(RightPaneEvent::SuggestPeople(term.into()));
                        }
                        let suggestions = this.reply_suggestions(*trigger, query, cx);
                        composer
                            .update(cx, |composer, cx| composer.set_suggestions(suggestions, cx));
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &search,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => {
                        if let Some(pick) =
                            this.suggestion_at.and_then(|at| this.suggestions.get(at))
                        {
                            let insert = pick.insert.clone();
                            this.accept_suggestion(insert, window, cx);
                            return;
                        }
                        this.suggestions.clear();
                        this.submit_search(cx);
                    }
                    InputEvent::Change => {
                        let query = input.read(cx).value().to_string();
                        this.suggestions = suggestions(&query, &this.hints);
                        this.suggestion_at = None;
                        cx.notify();
                    }
                    _ => {}
                },
            ),
        ];
        Self {
            composer_cap: px(320.),
            content: None,
            list,
            composer,
            focus: cx.focus_handle(),
            thread_focus: cx.focus_handle(),
            selection: None,
            loading_older: false,
            menu: cx.new(|cx| MenuState::new([], cx)),
            me: me.into(),
            profiles: Vec::new(),
            people: crate::channel::PeopleAsk::default(),
            directory: Vec::new(),
            typing: Vec::new(),
            draft_len: 0,
            draft: Default::default(),
            _draft_task: None,
            window: window.window_handle(),
            link: Default::default(),
            tray: UploadTray::new(UPLOAD_PREFIX),
            _link_task: None,
            pending_seq: 0,
            search,
            loading: false,
            hints: SearchHints::default(),
            search_files_tab: false,
            suggestions: Vec::new(),
            suggestion_at: None,
            lightbox: cx.new(|_| LightboxState::new([])),
            lightbox_handle: DialogHandle::new(false),
            lightbox_items: Vec::new(),
            leave: DialogHandle::new(false),
            away: crate::preview::connection() != crate::main_window::ConnectionState::Online,
            offline: crate::preview::connection() == crate::main_window::ConnectionState::Offline,
            editing: None,
            discard: DialogHandle::new(false),
            discarding: None,
            permalink: None,
            popped_out: false,
            action_menus: Default::default(),
            slash: Default::default(),
            schedule_dialog,
            picker,
            picker_open: false,
            picker_target: None,
            _permalink_task: None,
            pending_jump: None,
            _subscriptions: subscriptions,
        }
    }

    fn mutate_info(&mut self, change: impl FnOnce(&mut ChannelModel), cx: &mut Context<Self>) {
        if let Some(RightPaneContent::ChannelInfo(channel)) = self.content.as_mut() {
            change(channel);
            cx.notify();
        }
    }

    /// Opens the lightbox over `post`'s images at image `index`: its image
    /// files first, then the images its message links to, as the channel
    /// does.
    fn open_lightbox(
        &mut self,
        post: &Post,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let caption = Self::caption(post, cx);
        let mut items = Vec::new();
        let mut images = Self::file_images(&post.files, &caption, &mut items);
        for embed in &post.embeds {
            if let crate::embeds::Embed::Image(image) = embed
                && let Some(loaded) = &image.loaded
            {
                items.push(LightboxItem::Link(image.url.clone()));
                images.push(
                    LightboxImage::new(
                        gpui_kit::ImageSource::Image(loaded.clone()),
                        crate::embeds::image_name(&image.url),
                    )
                    .dimensions(image.width, image.height)
                    .caption(caption.clone()),
                );
            }
        }
        self.show_images(items, images, index, window, cx);
    }

    /// Opens the lightbox at the image `url` that `post` links to; one
    /// still loading opens in the browser.
    fn open_linked_image(
        &mut self,
        post: &Post,
        url: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let files = post
            .files
            .iter()
            .filter(|file| file.image().is_some())
            .count();
        let index = post
            .embeds
            .iter()
            .filter_map(|embed| match embed {
                crate::embeds::Embed::Image(image) if image.loaded.is_some() => Some(&image.url),
                _ => None,
            })
            .position(|linked| *linked == url);
        match index {
            Some(index) => self.open_lightbox(post, files + index, window, cx),
            None => cx.open_url(&url),
        }
    }

    fn caption(post: &Post, cx: &App) -> SharedString {
        format!(
            "{} · {}",
            post.author,
            ClockFormat::current(cx).format(post.time)
        )
        .into()
    }

    /// `files`' images for the lightbox, each under `caption`, noting each
    /// in `items`.
    fn file_images(
        files: &[crate::attachments::PostFile],
        caption: &SharedString,
        items: &mut Vec<LightboxItem>,
    ) -> Vec<LightboxImage> {
        files
            .iter()
            .filter_map(|file| {
                let image = file.image()?;
                items.push(LightboxItem::File(file.id.clone()));
                Some(
                    LightboxImage::new(image, file.name.clone())
                        .dimensions(file.width, file.height)
                        .caption(caption.clone()),
                )
            })
            .collect()
    }

    /// Opens `files`' images in the lightbox at the `index`th, each under
    /// `caption`.
    fn open_images(
        &mut self,
        files: &[crate::attachments::PostFile],
        caption: SharedString,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut items = Vec::new();
        let images = Self::file_images(files, &caption, &mut items);
        self.show_images(items, images, index, window, cx);
    }

    fn show_images(
        &mut self,
        items: Vec<LightboxItem>,
        images: Vec<LightboxImage>,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if images.is_empty() {
            return;
        }
        let files: Vec<SharedString> = items
            .iter()
            .filter_map(|item| match item {
                LightboxItem::File(id) => Some(id.clone()),
                LightboxItem::Link(_) => None,
            })
            .collect();
        self.lightbox_items = items;
        self.lightbox
            .update(cx, |lightbox, cx| lightbox.set_images(images, index, cx));
        self.lightbox_handle.open(window, cx);
        if !files.is_empty() {
            cx.emit(RightPaneEvent::ViewImages(files));
        }
        cx.notify();
    }

    /// Sync fetched a file's full-size image; the lightbox swaps it in for
    /// the thumbnail when it shows that file.
    pub fn set_full_image(
        &mut self,
        file_id: &SharedString,
        image: std::sync::Arc<gpui_kit::Image>,
        cx: &mut Context<Self>,
    ) {
        let Some(position) = self
            .lightbox_items
            .iter()
            .position(|item| *item == LightboxItem::File(file_id.clone()))
        else {
            return;
        };
        self.lightbox.update(cx, |lightbox, cx| {
            let mut images = lightbox.images().to_vec();
            if let Some(entry) = images.get_mut(position) {
                entry.source = gpui_kit::ImageSource::Image(image);
                let index = lightbox.index();
                lightbox.set_images(images, index, cx);
            }
        });
    }

    fn lightbox(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let open = Button::new("right-pane.lightbox.open")
            .size(ButtonSize::Icon)
            .variant(ButtonVariant::Ghost)
            .aria_label(t!("pane-download"))
            .child(
                lucide(LucideIcon::Download)
                    .size(theme.space(4.5))
                    .text_color(gpui_kit::white()),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                let index = this.lightbox.read(cx).index();
                match this.lightbox_items.get(index) {
                    Some(LightboxItem::File(id)) => cx.emit(RightPaneEvent::OpenFile(id.clone())),
                    Some(LightboxItem::Link(url)) => cx.open_url(url),
                    None => {}
                }
            }))
            .into_any_element();
        lightbox(
            "right-pane.lightbox",
            &self.lightbox_handle,
            &self.lightbox,
            [open],
            window,
            cx,
        )
    }

    fn thread_mut(&mut self) -> Option<&mut ThreadModel> {
        match self.content.as_mut()? {
            RightPaneContent::Thread(thread) => Some(thread),
            _ => None,
        }
    }

    /// Appends the text and the tray's files as a pending reply by the
    /// current user, clears the composer and tells sync. Blank text with no
    /// files stays in the composer; Enter while files upload waits for them.
    fn reply(&mut self, text: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if self.run_slash(&text, window, cx) {
            return;
        }
        let body: SharedString = text.trim().to_string().into();
        if self.thread_mut().is_none() {
            return;
        }
        if self.tray.request_send(!body.is_empty()) != SendGate::Now {
            cx.notify();
            return;
        }
        let files: Vec<PostFile> = self
            .tray
            .take()
            .into_iter()
            .map(PostFile::from_upload)
            .collect();
        let file_ids: Vec<SharedString> = files.iter().map(|file| file.id.clone()).collect();
        self.pending_seq += 1;
        let (me, seq) = (self.me.clone(), self.pending_seq);
        let Some(thread) = self.thread_mut() else {
            return;
        };
        let root = thread.root.id.clone();
        let last = thread.replies.last().unwrap_or(&thread.root);
        let own = crate::channel::own_id(
            &[std::slice::from_ref(&thread.root), &thread.replies[..]].concat(),
        );
        let reply = Post {
            webhook: None,
            id: format!("{root}-pending-{seq}").into(),
            author: me.clone(),
            author_id: own,
            kind: crate::channel::PostKind::Regular,
            time: last.time,
            day: last.day.clone(),
            date: SharedString::default(),
            body: body.clone(),
            reactions: vec![],
            mentions_me: false,
            edited: false,
            // Never a continuation: the row's own "Sending…" must show.
            continues: false,
            mine: true,
            saved: false,
            pinned: false,
            status: PostStatus::Pending,
            replies: None,
            root_id: root.clone(),
            reply_to: None,
            files,
            embeds: vec![],
            image_sizes: vec![],
            priority: None,
        };
        thread.replies.push(reply);
        let content = self.content.clone();
        self.set_content(content, cx);
        self._draft_task = None;
        if let Some(cleared) = self.draft.sent(cx) {
            cx.emit(RightPaneEvent::SaveDraft(cleared));
        }
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.draft_len = 0;
        let unfurl = self.link.sent(&body);
        self._link_task = None;
        cx.emit(RightPaneEvent::Reply {
            root,
            text: body,
            files: file_ids,
            unfurl,
        });
    }

    /// The server's `MaxFileSize`, for checking files before they upload.
    pub fn set_max_file_size(&mut self, bytes: u64) {
        self.tray.set_max_file_size(bytes);
    }

    /// Whether a thread is open, so files can go into its reply.
    pub fn has_thread(&self) -> bool {
        matches!(self.content, Some(RightPaneContent::Thread(_)))
    }

    /// Opens the file picker for the reply; in preview it fills the tray
    /// with fixtures, as the channel composer does.
    pub fn choose_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_thread() {
            return;
        }
        if crate::preview::enabled() {
            self.tray.extend(crate::preview::uploads());
            self.tray.admit(vec![crate::preview::oversize_upload()]);
            cx.notify();
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                this.update(cx, |this, cx| {
                    this.add_sources(paths.into_iter().map(UploadSource::Path), cx)
                })
                .ok();
            }
        })
        .detach();
    }

    /// Adds what passes the size and count checks to the reply tray and
    /// asks sync to upload it.
    fn add_sources(
        &mut self,
        sources: impl IntoIterator<Item = UploadSource>,
        cx: &mut Context<Self>,
    ) {
        if !self.has_thread() {
            return;
        }
        for upload in self.tray.add(sources) {
            cx.emit(RightPaneEvent::Upload {
                id: upload.id,
                name: upload.name,
                source: upload.source,
            });
        }
        cx.notify();
    }

    /// Sync reports progress for a reply file; a reply that waited on it
    /// goes once the last one is up. Ids from another tray are ignored.
    pub fn set_upload_status(
        &mut self,
        id: &str,
        status: AttachmentStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tray.uploads().iter().any(|upload| upload.id == id) {
            // A file in another thread's draft.
            if let Some(save) = crate::composer_draft::DraftBook::set_upload_status(id, status, cx)
            {
                cx.emit(RightPaneEvent::SaveDraft(save));
            }
            return;
        }
        if self.tray.set_status(id, status) {
            self.send_queued(window, cx);
        } else if status == AttachmentStatus::Ready {
            self.schedule_draft_save(cx);
        }
        cx.notify();
    }

    /// Sync is offline: the file waits in the tray, queued (#194).
    pub fn set_upload_queued(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.tray.set_queued(id) {
            self.send_queued(window, cx);
        }
        cx.notify();
    }

    fn remove_upload(&mut self, id: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let due = self.tray.remove(id);
        self.schedule_draft_save(cx);
        cx.emit(RightPaneEvent::CancelUpload(id.clone()));
        if due {
            self.send_queued(window, cx);
        }
        cx.notify();
    }

    fn retry_upload(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        self.tray.set_status(id, AttachmentStatus::Uploading(0.));
        cx.emit(RightPaneEvent::RetryUpload(id.clone()));
        cx.notify();
    }

    fn send_queued(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value(cx);
        self.reply(text, window, cx);
    }

    /// A reply tray chip with its remove and Retry wired to this pane.
    fn upload_chip(
        &self,
        upload: &crate::attachments::Upload,
        cx: &mut Context<Self>,
    ) -> Attachment {
        let (remove, retry) = (upload.id.clone(), upload.id.clone());
        upload.chip(
            "right-pane.upload",
            cx.listener(move |this, _, window, cx| this.remove_upload(&remove, window, cx)),
            cx.listener(move |this, _, _, cx| this.retry_upload(&retry, cx)),
        )
    }

    /// Follows the reply draft's first link, asked about after a pause in
    /// typing like the channel composer's.
    fn draft_changed(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(url) = self.link.follow(text) else {
            if !self.link.has_link() {
                self._link_task = None;
            }
            return;
        };
        self._link_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(crate::draft_link::PAUSE)
                .await;
            this.update(cx, |_, cx| cx.emit(RightPaneEvent::PreviewLink(url)))
                .ok();
        }));
    }

    /// What the server read from `url`; ignored once the draft moved on.
    pub fn set_link_preview(
        &mut self,
        url: &str,
        card: Option<crate::embeds::LinkCard>,
        cx: &mut Context<Self>,
    ) {
        if self.link.answer(url, card) {
            cx.notify();
        }
    }

    /// Applies `change` to the reply with `id`, if it is loaded, and redraws.
    fn mutate_reply(
        &mut self,
        id: &SharedString,
        change: impl FnOnce(&mut Post),
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(thread) = self.thread_mut() else {
            return false;
        };
        let Some(post) = std::iter::once(&mut thread.root)
            .chain(thread.replies.iter_mut())
            .find(|post| &post.id == id)
        else {
            return false;
        };
        change(post);
        cx.notify();
        true
    }

    fn retry(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let changed = self.mutate_reply(&id, |post| post.status = PostStatus::Pending, cx);
        if changed {
            cx.emit(RightPaneEvent::Retry(id));
        }
    }

    /// Removes a reply that never reached the server and tells sync.
    fn discard(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let Some(thread) = self.thread_mut() else {
            return;
        };
        let sent = thread
            .replies
            .iter()
            .any(|post| post.id == id && post.status == PostStatus::Sent);
        thread.replies.retain(|post| post.id != id);
        if self
            .editing
            .as_ref()
            .is_some_and(|editing| editing.post == id)
        {
            self.editing = None;
        }
        let content = self.content.clone();
        self.set_content(content, cx);
        cx.emit(if sent {
            RightPaneEvent::Delete(id)
        } else {
            RightPaneEvent::Discard(id)
        });
    }

    /// Runs a button or menu choice on an attachment (#211), unless
    /// offline.
    fn run_action(&mut self, choice: crate::interactive::ActionChoice, cx: &mut Context<Self>) {
        let key = "run action"; // i18n-ignore: an offline key
        if !self.refused_offline(key, cx) {
            cx.emit(RightPaneEvent::PostAction(choice));
        }
    }

    /// Drops an ephemeral reply the user dismissed and tells the window,
    /// which forgets it.
    fn dismiss(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let Some(thread) = self.thread_mut() else {
            return;
        };
        thread.replies.retain(|post| post.id != id);
        let content = self.content.clone();
        self.set_content(content, cx);
        cx.emit(RightPaneEvent::DismissEphemeral(id));
    }

    /// The banner went to or from "You're offline" (#163).
    pub fn set_offline(&mut self, offline: bool, cx: &mut Context<Self>) {
        if self.offline != offline {
            self.offline = offline;
            // The older-replies row changes, and asks for its page once back.
            self.list.update(cx, |list, cx| list.remeasure(cx));
            cx.notify();
        }
    }

    /// Offline, refuses an action that needs the server before anything
    /// changes here, and asks the window to say so; `true` when refused.
    fn refused_offline(&self, action: &'static str, cx: &mut Context<Self>) -> bool {
        if self.offline {
            cx.emit(RightPaneEvent::Offline(action));
        }
        self.offline
    }

    /// The thread's root or one of its replies.
    fn thread_post(&self, id: &SharedString) -> Option<&Post> {
        match self.content.as_ref()? {
            RightPaneContent::Thread(thread) => std::iter::once(&thread.root)
                .chain(thread.replies.iter())
                .find(|post| &post.id == id),
            _ => None,
        }
    }

    /// The connection went down or came back: waiting replies read "Will
    /// send when back online" meanwhile (#195).
    pub fn set_away(&mut self, away: bool, cx: &mut Context<Self>) {
        if self.away != away {
            self.away = away;
            self.list.update(cx, |list, cx| list.remeasure(cx));
            cx.notify();
        }
    }

    /// Opens "Discard this message?" for a reply waiting to send, or
    /// "Delete this message?" for one sent.
    fn ask_discard(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        // A waiting reply is discarded here; a sent one needs the server.
        let sent = self
            .thread_post(&id)
            .is_some_and(|post| post.status == PostStatus::Sent);
        if sent && self.refused_offline("delete", cx) {
            return;
        }
        self.discarding = Some(id);
        self.discard.open(window, cx);
        cx.notify();
    }

    /// The presence of a post's author among the pane's people.
    fn presence_of(&self, author: &str) -> Option<Presence> {
        self.profiles
            .iter()
            .find(|profile| profile.display_name == author)
            .map(|profile| profile.presence)
    }

    /// [`Self::presence_of`] from a row drawn outside the pane's own render;
    /// inside it the pane is being updated and cannot be read.
    fn presence_in(view: &gpui_kit::WeakEntity<Self>, author: &str, cx: &App) -> Option<Presence> {
        view.upgrade()
            .and_then(|pane| pane.read(cx).presence_of(author))
    }

    fn reply_post(&self, id: &SharedString) -> Option<&Post> {
        match self.content.as_ref()? {
            RightPaneContent::Thread(thread) => thread.replies.iter().find(|post| &post.id == id),
            _ => None,
        }
    }

    /// Turns the user's reply, sent or waiting to send, into a composer
    /// holding its text, as in the channel (#117, #195). Enter saves,
    /// Escape cancels.
    fn start_edit(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(post) = self.reply_post(&id).filter(|post| {
            post.mine
                && post.kind == PostKind::Regular
                && (post.status == PostStatus::Sent || waiting_to_send(post, self.away))
        }) else {
            return;
        };
        let view = cx.weak_entity();
        let composer = edit_composer(
            post.body.clone(),
            move |window, cx| {
                view.update(cx, |this, cx| this.cancel_edit(window, cx))
                    .ok();
            },
            window,
            cx,
        );
        let subscription = cx.subscribe_in(
            &composer,
            window,
            |this, _, event: &ComposerEvent, window, cx| {
                if let ComposerEvent::Submit(text) = event {
                    this.save_edit(text.clone(), window, cx);
                }
            },
        );
        self.editing = Some(ReplyEdit {
            post: id,
            composer,
            _subscription: subscription,
        });
        self.list.update(cx, |list, cx| list.remeasure(cx));
        cx.notify();
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            self.composer
                .update(cx, |composer, cx| composer.focus(window, cx));
            self.list.update(cx, |list, cx| list.remeasure(cx));
            cx.notify();
        }
    }

    /// Keeps the new text and tells sync; an unchanged body just closes the
    /// editor and an emptied one asks to delete.
    fn save_edit(&mut self, text: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = self.editing.as_ref() else {
            return;
        };
        let id = editing.post.clone();
        let body: SharedString = text.trim().to_string().into();
        let unchanged = self.reply_post(&id).is_none_or(|post| post.body == body);
        if body.is_empty() {
            // Emptying a message asks to delete it, as the webapp does.
            self.cancel_edit(window, cx);
            self.ask_discard(id, window, cx);
            return;
        }
        if unchanged {
            self.cancel_edit(window, cx);
            return;
        }
        // Offline a sent reply keeps its editor open for another try.
        let sent = self
            .thread_post(&id)
            .is_some_and(|post| post.status == PostStatus::Sent);
        if sent && self.refused_offline("edit", cx) {
            return;
        }
        // A waiting send just goes with the new text, without "(edited)".
        self.mutate_reply(
            &id,
            |post| {
                post.body = body.clone();
                post.edited = post.status == PostStatus::Sent;
            },
            cx,
        );
        cx.emit(RightPaneEvent::SaveEdit {
            post: id,
            text: body,
        });
        self.cancel_edit(window, cx);
    }

    fn discard_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let thread = match self.content.as_ref() {
            Some(RightPaneContent::Thread(thread)) => Some(thread),
            _ => None,
        };
        let unsent = self.discarding.as_ref().is_none_or(|id| {
            self.reply_post(id)
                .is_none_or(|post| post.status != PostStatus::Sent)
        });
        let place: SharedString = thread
            .map(|thread| format!("#{}", thread.channel_name).into())
            .unwrap_or_else(|| t!("pane-discard-place"));
        let popup = delete_popup("thread.discard", unsent, &place, 0, cx);
        let view = cx.entity().downgrade();
        alert_dialog("thread.discard", &self.discard, popup, window, cx).on_ok(move |_, _, cx| {
            view.update(cx, |this, cx| {
                if let Some(id) = this.discarding.take() {
                    this.discard(id, cx);
                }
            })
            .ok();
            true
        })
    }

    fn toggle_reaction(&mut self, id: SharedString, emoji: SharedString, cx: &mut Context<Self>) {
        let mine = self.thread_post(&id).is_some_and(|post| {
            post.reactions
                .iter()
                .any(|reaction| reaction.emoji == emoji && reaction.mine)
        });
        if self.refused_offline(
            if mine {
                "remove reaction" // i18n-ignore: an offline key
            } else {
                "add reaction" // i18n-ignore: an offline key
            },
            cx,
        ) {
            return;
        }
        let changed = self.mutate_reply(
            &id,
            |post| {
                if let Some(at) = post.reactions.iter().position(|r| r.emoji == emoji) {
                    let reaction = &mut post.reactions[at];
                    if reaction.mine {
                        reaction.count -= 1;
                        reaction.mine = false;
                        if reaction.count == 0 {
                            post.reactions.remove(at);
                        }
                    } else {
                        reaction.count += 1;
                        reaction.mine = true;
                    }
                } else {
                    // A new emoji on this post (#620).
                    post.reactions.push(crate::channel::PostReaction {
                        emoji: emoji.clone(),
                        count: 1,
                        mine: true,
                        users: vec![],
                    });
                }
            },
            cx,
        );
        if changed {
            cx.emit(RightPaneEvent::ToggleReaction { post: id, emoji });
        }
    }

    fn toggle_follow(&mut self, cx: &mut Context<Self>) {
        let following = match self.content.as_ref() {
            Some(RightPaneContent::Thread(thread)) => thread.following,
            _ => return,
        };
        if self.refused_offline(
            if following {
                "unfollow thread" // i18n-ignore: an offline key
            } else {
                "follow thread" // i18n-ignore: an offline key
            },
            cx,
        ) {
            return;
        }
        let Some(thread) = self.thread_mut() else {
            return;
        };
        thread.following = !thread.following;
        let event = RightPaneEvent::ToggleFollow {
            root: thread.root.id.clone(),
            following: thread.following,
        };
        cx.notify();
        cx.emit(event);
    }

    pub fn content(&self) -> Option<&RightPaneContent> {
        self.content.as_ref()
    }

    /// Replaces what the pane shows and resets the transcript to its end.
    /// Sets what search's `from:` and `in:` suggest.
    pub fn set_search_hints(&mut self, hints: SearchHints) {
        self.hints = hints;
    }

    /// Puts a suggestion in place of the word being typed.
    fn accept_suggestion(
        &mut self,
        insert: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search.update(cx, |input, cx| {
            let value = input.value().to_string();
            let keep = value
                .rfind(char::is_whitespace)
                .map_or("", |at| &value[..=at]);
            input.set_value(format!("{keep}{insert} "), window, cx);
            input.focus(window, cx);
        });
        self.suggestions.clear();
        self.suggestion_at = None;
        cx.notify();
    }

    /// Moves the highlight through the suggestions with the arrow keys,
    /// leaving the keys to the field when nothing is suggested.
    fn step_suggestion(&mut self, down: bool, cx: &mut Context<Self>) {
        let count = self.suggestions.len();
        if count == 0 {
            cx.propagate();
            return;
        }
        self.suggestion_at = step(self.suggestion_at, count, down);
        cx.stop_propagation();
        cx.notify();
    }

    /// Marks the listing on screen as still loading, so an empty one reads
    /// "Loading…" rather than its empty state.
    pub fn set_loading(&mut self, loading: bool, cx: &mut Context<Self>) {
        self.loading = loading;
        cx.notify();
    }

    /// Shows a search: the field takes the query and focus, the listing the
    /// results. Sync calls this again when results arrive.
    pub fn set_search(&mut self, model: SearchModel, window: &mut Window, cx: &mut Context<Self>) {
        let focus = model.query.is_empty() || model.searching;
        // A new search starts on its messages, or on its files when only
        // files matched.
        if model.searching {
            self.search_files_tab = false;
        } else if model.results.is_empty() && !model.files.is_empty() {
            self.search_files_tab = true;
        }
        self.search.update(cx, |input, cx| {
            if input.value() != model.query {
                input.set_value(model.query.clone(), window, cx);
            }
        });
        self.set_content(Some(RightPaneContent::Search(model)), cx);
        if focus {
            let handle = self.search.read(cx).focus_handle(cx);
            window.on_next_frame(move |window, cx| window.focus(&handle, cx));
        }
    }

    fn submit_search(&mut self, cx: &mut Context<Self>) {
        let query: SharedString = self.search.read(cx).value().trim().to_string().into();
        if query.is_empty() {
            return;
        }
        if let Some(RightPaneContent::Search(model)) = self.content.as_mut() {
            model.query = query.clone();
            model.searching = true;
            cx.notify();
        }
        cx.emit(RightPaneEvent::Search(query));
    }

    pub fn set_content(&mut self, content: Option<RightPaneContent>, cx: &mut Context<Self>) {
        self.loading = false;
        let rows = match &content {
            Some(RightPaneContent::Thread(thread)) => thread.replies.len() + THREAD_HEAD,
            _ => 0,
        };
        self.list = cx.new(|cx| MessageListState::new(rows, cx));
        let same_thread = match (&self.content, &content) {
            (Some(RightPaneContent::Thread(a)), Some(RightPaneContent::Thread(b))) => {
                a.root.id == b.root.id
            }
            _ => false,
        };
        if !same_thread {
            self.link = Default::default();
            self._link_task = None;
            self.editing = None;
            self.permalink = None;
            self._permalink_task = None;
        }
        // A jump waits through an empty pane, not past another thread.
        if let (Some(RightPaneContent::Thread(thread)), Some((root, _))) =
            (&content, &self.pending_jump)
            && thread.root.id != *root
        {
            self.pending_jump = None;
        }
        self.content = content;
        self.selection = None;
        self.loading_older = false;
        self.apply_pending_jump(cx);
        let key = self.draft_key();
        let moved = self.draft.key() != key.as_ref();
        if moved
            && self
                .draft
                .switches_quietly(key.as_ref(), &self.composer, cx)
        {
            // Nothing typed and nothing to restore: switch now, without
            // touching the window (an update there redraws it).
            self.switch_draft(key, None, cx);
        } else if moved {
            // The composer needs the window to change its text.
            let window = self.window;
            let this = cx.weak_entity();
            cx.defer(move |cx| {
                window
                    .update(cx, |_, window, cx| {
                        this.update(cx, |this, cx| this.sync_draft(window, cx))
                    })
                    .ok();
            });
        }
        cx.notify();
    }

    /// Where the composer's draft belongs: the shown thread.
    fn draft_key(&self) -> Option<crate::composer_draft::DraftKey> {
        match &self.content {
            Some(RightPaneContent::Thread(thread)) => {
                Some(crate::composer_draft::DraftKey::thread(
                    thread.channel_id.clone(),
                    thread.root.id.clone(),
                ))
            }
            _ => None,
        }
    }

    /// Puts the composer on the shown thread's draft, saving the one it
    /// leaves.
    fn sync_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.draft_key();
        if self.draft.key() == key.as_ref() {
            return;
        }
        self.switch_draft(key, Some(window), cx);
    }

    fn switch_draft(
        &mut self,
        key: Option<crate::composer_draft::DraftKey>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        self._draft_task = None;
        self.slash.reset();
        let left = self
            .draft
            .switch(key, &self.composer, &mut self.tray, window, cx);
        if let Some(left) = left {
            cx.emit(RightPaneEvent::SaveDraft(left));
        }
        self.draft_len = self.composer.read(cx).value(cx).chars().count();
        cx.notify();
    }

    /// Saves the reply draft once typing pauses (#114).
    fn schedule_draft_save(&mut self, cx: &mut Context<Self>) {
        self._draft_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(crate::composer_draft::SAVE_PAUSE)
                .await;
            this.update(cx, |this, cx| this.save_draft(cx)).ok();
        }));
    }

    fn save_draft(&mut self, cx: &mut Context<Self>) {
        self._draft_task = None;
        if let Some(save) = self.draft.save(&self.composer, &self.tray, cx) {
            cx.emit(RightPaneEvent::SaveDraft(save));
        }
    }

    /// The thread's draft changed elsewhere; the composer follows unless
    /// the user has typed since it was saved.
    pub fn refresh_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .draft
            .refresh(&self.composer, &mut self.tray, window, cx)
        {
            self.draft_len = self.composer.read(cx).value(cx).chars().count();
            cx.notify();
        }
    }

    /// Sync's latest of the open thread. The same thread keeps its scroll
    /// and its composer; new replies join the end of the list, and an
    /// older page goes in above the first reply without moving what the
    /// reader sees (#122).
    pub fn set_thread(&mut self, thread: ThreadModel, cx: &mut Context<Self>) {
        let Some(RightPaneContent::Thread(shown)) = &self.content else {
            self.set_content(Some(RightPaneContent::Thread(Box::new(thread))), cx);
            return;
        };
        if shown.root.id != thread.root.id {
            self.set_content(Some(RightPaneContent::Thread(Box::new(thread))), cx);
            return;
        }
        // Replies before the one that was first came from an older page.
        let older = match shown.replies.first() {
            Some(first) => thread.replies.iter().position(|reply| reply.id == first.id),
            None => Some(0),
        };
        let Some(older) = older.filter(|older| shown.replies.len() + older <= thread.replies.len())
        else {
            self.set_content(Some(RightPaneContent::Thread(Box::new(thread))), cx);
            return;
        };
        let newer = thread.replies.len() - shown.replies.len() - older;
        let paged = older > 0 || !thread.older || thread.failed_older;
        self.content = Some(RightPaneContent::Thread(Box::new(thread)));
        if paged {
            self.loading_older = false;
        }
        if older > 0 {
            self.selection = self
                .selection
                .map(|row| if row >= THREAD_HEAD { row + older } else { row });
        }
        self.list.update(cx, |list, cx| {
            if older > 0 {
                // The list keeps the row at the top of the viewport where
                // it was, so the reader's place holds.
                list.splice(THREAD_HEAD..THREAD_HEAD, older, cx);
                list.remeasure_items(0..THREAD_HEAD, cx);
            } else {
                list.remeasure(cx);
            }
            if newer > 0 {
                list.append(newer, cx);
            }
        });
        self.apply_pending_jump(cx);
        cx.notify();
    }

    /// Scrolls the thread under `root` to `post` and tints it for a few
    /// seconds, as a notification's click asks (#141). Until that thread
    /// is shown with the post in it, the jump waits for sync to bring it.
    pub fn jump_to(&mut self, root: SharedString, post: SharedString, cx: &mut Context<Self>) {
        self.pending_jump = Some((root, post));
        self.apply_pending_jump(cx);
    }

    fn apply_pending_jump(&mut self, cx: &mut Context<Self>) {
        let Some((root, post)) = self.pending_jump.clone() else {
            return;
        };
        let Some(RightPaneContent::Thread(thread)) = &self.content else {
            return;
        };
        if thread.root.id != root {
            return;
        }
        let row = if thread.root.id == post {
            Some(0)
        } else {
            thread
                .replies
                .iter()
                .position(|reply| reply.id == post)
                .map(|index| index + THREAD_HEAD)
        };
        let Some(row) = row else {
            return;
        };
        self.pending_jump = None;
        self.list
            .update(cx, |list, cx| list.scroll_to_item(row, cx));
        self.permalink = Some(post);
        self._permalink_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(4))
                .await;
            this.update(cx, |this, cx| {
                this.permalink = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Makes the pane a pop-out window's content (#222).
    pub fn set_popped_out(&mut self, popped_out: bool, cx: &mut Context<Self>) {
        self.popped_out = popped_out;
        cx.notify();
    }

    /// The post a notification opened the thread at, while it is tinted.
    pub fn permalink(&self) -> Option<&SharedString> {
        self.permalink.as_ref()
    }

    /// Asks for the replies before the oldest loaded one when the row
    /// above the replies shows and nothing stops it.
    fn request_older(&mut self, cx: &mut Context<Self>) {
        let Some(RightPaneContent::Thread(thread)) = &self.content else {
            return;
        };
        // Offline the page could only fail: the row waits instead (#163).
        if !thread.older || thread.failed_older || self.loading_older || self.offline {
            return;
        }
        self.loading_older = true;
        cx.emit(RightPaneEvent::LoadOlderReplies(thread.root.id.clone()));
        // This runs while the list lays out the row, so the remeasure
        // waits until the list has let go of its state.
        let view = cx.entity().downgrade();
        cx.defer(move |cx| {
            view.update(cx, |this, cx| {
                this.list.update(cx, |list, cx| {
                    list.remeasure_items(THREAD_HEAD - 1..THREAD_HEAD, cx)
                });
                cx.notify();
            })
            .ok();
        });
    }

    /// The row between the divider and the replies: nothing once every
    /// reply is loaded, otherwise the older page's progress.
    fn older_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let Some(RightPaneContent::Thread(thread)) = &self.content else {
            return div().into_any_element();
        };
        if !thread.older {
            return div().into_any_element();
        }
        let row = div()
            .flex()
            .items_center()
            .justify_center()
            .gap(theme.space(2.))
            .px(theme.space(4.))
            .py(theme.space(3.))
            .text_size(theme.text(12.))
            .text_color(theme.colors.muted_foreground)
            .debug_selector(|| "right-pane.older".into());
        if self.offline && !self.loading_older {
            // Not a spinner that never ends: what is cached shows, and the
            // rest when the connection is back (#163).
            row.child(
                lucide(LucideIcon::WifiOff)
                    .size(theme.space(3.5))
                    .text_color(theme.colors.muted_foreground),
            )
            .child(t!("pane-older-offline"))
            .into_any_element()
        } else if thread.failed_older {
            row.child(t!("pane-older-failed"))
                .child(
                    div()
                        .debug_selector(|| "right-pane.older.retry".into())
                        .child(
                            Button::new("right-pane.older.retry")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Sm)
                                .label(t!("pane-older-retry"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(RightPaneContent::Thread(thread)) =
                                        &mut this.content
                                    {
                                        thread.failed_older = false;
                                    }
                                    this.request_older(cx);
                                })),
                        ),
                )
                .into_any_element()
        } else {
            row.child(
                Spinner::new("right-pane.older.spinner")
                    .label(t!("pane-older-loading"))
                    .size(gpui_kit::px(14.)),
            )
            .child(t!("pane-older-loading-text"))
            .into_any_element()
        }
    }

    /// Rows of the thread transcript: the root, the divider, then replies.
    fn thread_rows(&self) -> usize {
        match &self.content {
            Some(RightPaneContent::Thread(thread)) => thread.replies.len() + THREAD_HEAD,
            _ => 0,
        }
    }

    fn select_row(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.selection = index;
        if index.is_none() {
            self.composer
                .update(cx, |composer, cx| composer.focus(window, cx));
        }
        cx.notify();
    }

    /// The post on thread row `row`: the root, or a reply after the
    /// divider.
    fn row_post(&self, row: usize) -> Option<&Post> {
        let Some(RightPaneContent::Thread(thread)) = &self.content else {
            return None;
        };
        match row {
            0 => Some(&thread.root),
            row => thread.replies.get(row.checked_sub(THREAD_HEAD)?),
        }
    }

    /// Single keys on the selected row, as in the channel (#107): R
    /// replies, E edits and Delete removes the user's own reply, P pins, S
    /// saves, U marks unread, F forwards, T follows the thread, C copies
    /// the text and L the link.
    fn selection_key(
        &mut self,
        keystroke: &gpui_kit::Keystroke,
        selected: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let modifiers = keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt {
            return false;
        }
        let Some(row) = selected else {
            return false;
        };
        let Some(post) = self.row_post(row).cloned() else {
            return false;
        };
        let reply = row >= THREAD_HEAD;
        let own = reply && post.mine && post.kind == PostKind::Regular;
        let sent = post.status == PostStatus::Sent;
        let id = post.id.clone();
        match keystroke.key.as_str() {
            "r" => self.select_row(None, window, cx),
            "e" if own && (sent || waiting_to_send(&post, self.away)) => {
                self.selection = None;
                self.start_edit(id, window, cx);
            }
            "delete" | "backspace" if own && (sent || waiting_to_send(&post, self.away)) => {
                self.ask_discard(id, window, cx)
            }
            "p" if sent => cx.emit(RightPaneEvent::SetPinned {
                post: id,
                pinned: !post.pinned,
            }),
            "s" if sent => cx.emit(RightPaneEvent::SetSaved {
                post: id,
                saved: !post.saved,
            }),
            "u" if sent => cx.emit(RightPaneEvent::MarkUnread(id)),
            "f" if sent => cx.emit(RightPaneEvent::Forward(id)),
            "t" => self.toggle_follow(cx),
            "c" => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(plain_text(&post.body)))
            }
            "l" if sent => cx.emit(RightPaneEvent::CopyLink(id)),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Selects the newest reply, or the root when there is none.
    fn select_newest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.thread_rows();
        if rows == 0 {
            return;
        }
        self.selection = Some(if rows > THREAD_HEAD { rows - 1 } else { 0 });
        window.focus(&self.thread_focus, cx);
        cx.notify();
    }

    /// Selects the row showing `id`.
    fn select_post(&mut self, id: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(RightPaneContent::Thread(thread)) = &self.content else {
            return;
        };
        let row = if thread.root.id == *id {
            Some(0)
        } else {
            thread
                .replies
                .iter()
                .position(|post| post.id == *id)
                .map(|index| index + THREAD_HEAD)
        };
        if row.is_some() {
            self.selection = row;
            window.focus(&self.thread_focus, cx);
            cx.notify();
        }
    }

    /// Opens the shared menu at the pointer with `items`.
    fn open_menu(
        &mut self,
        items: Vec<MenuItem>,
        at: gpui_kit::Point<gpui_kit::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu.update(cx, |menu, cx| {
            menu.set_items(items, cx);
            menu.open_at(Some(at), window, cx);
        });
    }

    /// Opens the picker for `target` (a post to react to, or the reply).
    fn open_picker(&mut self, target: Option<SharedString>, cx: &mut Context<Self>) {
        self.picker.update(cx, crate::emoji::prepare_picker);
        self.picker_target = target;
        self.picker_open = true;
        cx.notify();
    }

    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker_open {
            self.picker_open = false;
            self.picker_target = None;
            self.composer
                .update(cx, |composer, cx| composer.focus(window, cx));
            cx.notify();
        }
    }

    fn pick_emoji(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let glyph = crate::emoji::glyph(&name).unwrap_or_else(|| format!(":{name}:").into());
        match self.picker_target.take() {
            // Reacting counts the pick itself.
            Some(post) => self.toggle_reaction(post, glyph, cx),
            None => {
                crate::emoji::record_pick(&name, cx);
                self.composer.update(cx, |composer, cx| {
                    composer.insert(format!("{glyph} "), window, cx);
                })
            }
        }
        self.close_picker(window, cx);
    }

    /// The right-click menu of a thread row: a reaction submenu and copy;
    /// on a reply the server does not have yet, copy, Retry if it failed,
    /// and the channel's Edit and Discard… while it waits to send (#195).
    fn thread_items_for(
        view: &gpui_kit::WeakEntity<Self>,
        post: &Post,
        away: bool,
        reply: bool,
    ) -> Vec<MenuItem> {
        let body = post.body.clone();
        let markdown_body = post.body.clone();
        // The server does not have an unsent reply yet: nothing to react to.
        let mut items = if post.status == PostStatus::Sent {
            let (view, id) = (view.clone(), post.id.clone());
            vec![
                MenuItem::new("thread.context.react", t!("pane-add-reaction-menu")).on_click(
                    move |_, _, cx| {
                        let id = id.clone();
                        view.update(cx, |this, cx| this.open_picker(Some(id), cx))
                            .ok();
                    },
                ),
                MenuItem::separator(),
            ]
        } else {
            Vec::new()
        };
        let copy_items = [
            MenuItem::new("thread.context.copy", t!("pane-copy-text")).on_click(move |_, _, cx| {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(plain_text(&body)))
            }),
            MenuItem::new("thread.context.copy-markdown", t!("pane-copy-markdown")).on_click(
                move |_, _, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                        markdown_body.to_string(),
                    ))
                },
            ),
        ];
        let sent = post.status == PostStatus::Sent;
        // A sent post puts copying with the owner's changes, as the
        // channel's menu does (#213); others start with it.
        if !sent {
            items.extend(copy_items.iter().cloned());
        }
        if post.status == PostStatus::Ephemeral {
            let (view, id) = (view.clone(), post.id.clone());
            items.push(
                MenuItem::new("thread.context.dismiss", t!("pane-dismiss")).on_click(
                    move |_, _, cx| {
                        let id = id.clone();
                        view.update(cx, |this, cx| this.dismiss(id, cx)).ok();
                    },
                ),
            );
        }
        if sent {
            let emit = |name: &str, label: SharedString, event: RightPaneEvent| {
                let view = view.clone();
                MenuItem::new(
                    gpui_kit::ElementId::Name(format!("thread.context.{name}").into()),
                    label,
                )
                .on_click(move |_, _, cx| {
                    let event = event.clone();
                    view.update(cx, |_, cx| cx.emit(event)).ok();
                })
            };
            let id = post.id.clone();
            items.extend([
                emit(
                    "forward",
                    t!("pane-forward"),
                    RightPaneEvent::Forward(id.clone()),
                ),
                emit(
                    "unread",
                    t!("pane-mark-unread"),
                    RightPaneEvent::MarkUnread(id.clone()),
                ),
                MenuItem::submenu(
                    "thread.context.remind",
                    t!("pane-remind"),
                    crate::channel::RemindAt::ALL.map(|at| {
                        emit(
                            &format!("remind.{at:?}"),
                            at.label(),
                            RightPaneEvent::Remind {
                                post: id.clone(),
                                at,
                            },
                        )
                    }),
                ),
                emit(
                    "save",
                    if post.saved {
                        t!("pane-unsave")
                    } else {
                        t!("pane-save")
                    },
                    RightPaneEvent::SetSaved {
                        post: id.clone(),
                        saved: !post.saved,
                    },
                ),
                emit(
                    "pin",
                    if post.pinned {
                        t!("pane-unpin")
                    } else {
                        t!("pane-pin")
                    },
                    RightPaneEvent::SetPinned {
                        post: id.clone(),
                        pinned: !post.pinned,
                    },
                ),
                emit(
                    "copy-link",
                    t!("pane-copy-link"),
                    RightPaneEvent::CopyLink(id.clone()),
                ),
                MenuItem::separator(),
            ]);
        }
        if post.status == PostStatus::Failed {
            let (retry_view, retry_id) = (view.clone(), post.id.clone());
            items.push(MenuItem::separator());
            items.push(
                MenuItem::new("thread.context.retry", t!("pane-retry")).on_click(
                    move |_, _, cx| {
                        let id = retry_id.clone();
                        retry_view.update(cx, |this, cx| this.retry(id, cx)).ok();
                    },
                ),
            );
        }
        // The user's own sent replies are edited and deleted here, as in
        // the channel (#117, #118); the root is the channel's to change.
        let own_sent =
            reply && post.mine && post.kind == PostKind::Regular && post.status == PostStatus::Sent;
        if own_sent {
            let (edit_view, edit_id) = (view.clone(), post.id.clone());
            items.push(
                MenuItem::new("thread.context.edit", t!("pane-edit")).on_click(
                    move |_, window, cx| {
                        edit_view
                            .update(cx, |this, cx| this.start_edit(edit_id.clone(), window, cx))
                            .ok();
                    },
                ),
            );
        }
        if sent {
            items.extend(copy_items);
        }
        if own_sent {
            let (delete_view, delete_id) = (view.clone(), post.id.clone());
            items.push(
                MenuItem::new("thread.context.delete", t!("pane-delete")).on_click(
                    move |_, window, cx| {
                        delete_view
                            .update(cx, |this, cx| {
                                this.ask_discard(delete_id.clone(), window, cx)
                            })
                            .ok();
                    },
                ),
            );
        }
        let (edit_view, edit_id) = (view.clone(), post.id.clone());
        let (discard_view, discard_id) = (view.clone(), post.id.clone());
        items.extend(waiting_items(
            "thread.context",
            post,
            away,
            move |window, cx| {
                edit_view
                    .update(cx, |this, cx| this.start_edit(edit_id.clone(), window, cx))
                    .ok();
            },
            move |window, cx| {
                discard_view
                    .update(cx, |this, cx| {
                        this.ask_discard(discard_id.clone(), window, cx)
                    })
                    .ok();
            },
        ));
        items
    }

    fn header(
        &self,
        title: SharedString,
        subtitle: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let muted = theme.colors.muted_foreground;
        // A thread can move to its own window, unless it is in one (#222).
        let thread_root = match &self.content {
            Some(RightPaneContent::Thread(thread)) if !self.popped_out => {
                Some(thread.root.id.clone())
            }
            _ => None,
        };
        let action = |id: &'static str, label: SharedString, glyph: LucideIcon| {
            Button::new(id)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::IconSm)
                .aria_label(label)
                .child(
                    lucide(glyph)
                        .size(scaled(&theme, metrics::ICON))
                        .text_color(muted),
                )
        };
        div()
            .flex_shrink_0()
            .h(scaled(&theme, metrics::HEADER_HEIGHT))
            .pl(theme.space(4.))
            .pr(theme.space(2.))
            .flex()
            .items_center()
            .justify_between()
            .gap(theme.space(2.))
            .border_b(style::hairline())
            .border_color(theme.colors.border)
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(theme.space(2.))
                    .min_w_0()
                    .child(
                        div()
                            .type_style(style::TITLE, &theme)
                            .text_color(theme.colors.foreground)
                            .child(title),
                    )
                    .children(subtitle.map(|text| {
                        div()
                            .min_w_0()
                            .truncate()
                            .type_style(style::META, &theme)
                            .text_color(muted)
                            .child(text)
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap(theme.space(0.5))
                    .when_some(thread_root, |row, root| {
                        row.child(style::tipped(
                            "right-pane.pop-out.tip",
                            actions::tooltip_text(&t!("pane-open-in-window"), "Open in new window"),
                            action(
                                "right-pane.pop-out",
                                t!("pane-open-in-window"),
                                LucideIcon::SquareArrowOutUpRight,
                            )
                            .on_click(cx.listener(
                                move |_, _, _, cx| cx.emit(RightPaneEvent::PopOut(root.clone())),
                            )),
                        ))
                    })
                    .when(!self.popped_out, |row| {
                        row.child(style::tipped(
                            "right-pane.expand.tip",
                            actions::tooltip_text(&t!("pane-expand"), "Expand right pane"),
                            action(
                                "right-pane.expand",
                                t!("pane-expand"),
                                LucideIcon::Maximize2,
                            ),
                        ))
                    })
                    .child(style::tipped(
                        "right-pane.close.tip",
                        actions::tooltip_text(&t!("pane-close"), "Close right pane"),
                        action("right-pane.close", t!("pane-close"), LucideIcon::X)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(RightPaneEvent::Close))),
                    )),
            )
    }

    /// Who is typing a reply in the open thread.
    pub fn set_typing(&mut self, names: Vec<SharedString>, cx: &mut Context<Self>) {
        if self.typing != names {
            self.typing = names;
            cx.notify();
        }
    }

    /// Keeps the people whose presence the thread's avatars show.
    /// Suggestions for the reply composer's triggers, as in the channel's;
    /// the server's matches stand in for the thread's people.
    fn reply_suggestions(&self, trigger: char, query: &str, cx: &App) -> Vec<ComposerSuggestion> {
        crate::channel::composer_suggestions(
            trigger,
            query,
            std::iter::empty(),
            &self.people,
            &self.directory,
            true,
            &crate::emoji::frequent(cx),
        )
    }

    /// Takes the server's matches for `term` and, while the reply composer
    /// still asks for it, shows them.
    pub fn set_suggested(
        &mut self,
        term: String,
        people: Vec<UserProfile>,
        cx: &mut Context<Self>,
    ) {
        if self.people.answer(&term, people) {
            let suggestions = self.reply_suggestions('@', &term, cx);
            self.composer
                .update(cx, |composer, cx| composer.set_suggestions(suggestions, cx));
        }
    }

    /// The channels `~` can complete in a reply.
    pub fn set_directory(&mut self, rows: Vec<crate::sidebar::ChannelRow>) {
        self.directory = rows;
    }

    pub fn set_profiles(&mut self, profiles: Vec<UserProfile>, cx: &mut Context<Self>) {
        if self.profiles != profiles {
            self.profiles = profiles.clone();
            cx.notify();
        }
        self.mutate_info(|channel| channel.profiles = profiles, cx);
    }

    /// The open channel's admins, once its full member list loaded (#208).
    pub fn set_admins(&mut self, admins: Vec<SharedString>, cx: &mut Context<Self>) {
        self.mutate_info(|channel| channel.admins = admins, cx);
    }

    /// A post's avatar, with the author's presence dot when the author is
    /// a known person (#135).
    fn avatar(
        id: &str,
        post: &Post,
        presence: Option<Presence>,
        initial: String,
        cx: &App,
    ) -> gpui_kit::AnyElement {
        if let Some(webhook) = &post.webhook {
            let id = format!("thread.{id}.avatar").into();
            return crate::webhooks::avatar(id, webhook, &post.author, cx);
        }
        let avatar = crate::avatars::with_picture(
            Avatar::new(gpui_kit::ElementId::Name(
                format!("thread.{id}.avatar").into(),
            ))
            .size(AvatarSize::Default)
            .fallback(initial),
            &post.author_id,
            cx,
        );
        let Some(presence) = presence else {
            return avatar.aria_label(post.author.clone()).into_any_element();
        };
        let theme = UiTheme::read(cx);
        avatar
            .aria_label(post.author.clone())
            .status(style::avatar_status(presence, &Palette::read(cx)))
            .ring_color(theme.colors.background)
            .into_any_element()
    }

    fn post_row(
        view: &gpui_kit::WeakEntity<Self>,
        post: &Post,
        root: bool,
        highlight: MessageHighlight,
        (away, editor, menus): (
            bool,
            Option<&ReplyEditor>,
            Option<&crate::interactive::Menus>,
        ),
        presence: Option<Presence>,
        cx: &mut App,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let time = post_time(post, away, cx);
        let id = post.id.clone();
        let initial: String = post.author.chars().take(1).collect();
        let (retry_view, retry_id) = (view.clone(), id.clone());
        let (delete_view, delete_id) = (view.clone(), id.clone());
        let (react_view, react_id) = (view.clone(), id.clone());
        // Only the thread passes card menus; its sent posts can be reacted
        // to from the hover toolbar, as in the channel (#620). Listings
        // can't: their posts are not the pane's thread.
        let reactable = menus.is_some() && post.status == PostStatus::Sent;
        let mut row = Message::new(gpui_kit::ElementId::Name(format!("thread.{id}").into()))
            .copy_prefix(crate::channel::copy_prefix(post, cx))
            .status(match post.status {
                PostStatus::Sent | PostStatus::Ephemeral => MessageStatus::Sent,
                PostStatus::Pending => MessageStatus::Pending,
                PostStatus::Failed => MessageStatus::Failed,
            })
            .on_retry(move |_, _, cx| {
                let id = retry_id.clone();
                retry_view.update(cx, |this, cx| this.retry(id, cx)).ok();
            })
            .on_delete(move |_, _, cx| {
                let id = delete_id.clone();
                delete_view.update(cx, |this, cx| this.discard(id, cx)).ok();
            })
            .on_reaction(move |emoji, _, cx| {
                let (id, emoji) = (react_id.clone(), emoji.clone());
                react_view
                    .update(cx, |this, cx| this.toggle_reaction(id, emoji, cx))
                    .ok();
            })
            .author(post.author.clone())
            .timestamp(time.clone())
            .when(
                post.status == PostStatus::Sent && !post.date.is_empty(),
                |row| {
                    row.timestamp_tooltip(t!(
                        "pane-sent-at",
                        date = post.date.to_string(),
                        time = time.to_string()
                    ))
                },
            )
            .gutter_time(time)
            // A queued reply keeps its header so the clock shows (#195).
            .continuation(post.continues && !root && !queued(post, away))
            .density(message_density(cx))
            .highlight(highlight)
            .avatar(Self::avatar(&id, post, presence, initial, cx))
            .reactions(post.reactions.iter().map(|reaction| {
                crate::emoji::reaction(&reaction.emoji, reaction.count, reaction.mine, cx)
            }))
            .reaction_tooltip(reaction_tooltip(post.reactions.clone()))
            // The hover toolbar is built only for the hovered or selected
            // row (#674).
            .when(reactable, |row| {
                let (view, post) = (view.clone(), post.clone());
                row.actions_with(move |_, cx| {
                    Self::hover_actions(&view, &post, away, !root, cx).into_any_element()
                })
            })
            .when(post.status == PostStatus::Ephemeral, |row| {
                let (view, id) = (view.clone(), id.clone());
                row.actions(
                    message_actions(cx).child(
                        message_action(
                            gpui_kit::ElementId::Name(format!("thread.{id}.dismiss").into()),
                            LucideIcon::X,
                            t!("pane-dismiss"),
                            cx,
                        )
                        .on_click(move |_, _, cx| {
                            let id = id.clone();
                            view.update(cx, |this, cx| this.dismiss(id, cx)).ok();
                        }),
                    ),
                )
            })
            .px(theme.space(4.))
            .child(match editor {
                Some((composer, cap)) => {
                    let (cancel, save) = (view.clone(), view.clone());
                    post_editor(
                        "thread.edit",
                        composer,
                        *cap,
                        move |window, cx| {
                            cancel
                                .update(cx, |this, cx| this.cancel_edit(window, cx))
                                .ok();
                        },
                        move |text, window, cx| {
                            save.update(cx, |this, cx| this.save_edit(text, window, cx))
                                .ok();
                        },
                        cx,
                    )
                    .into_any_element()
                }
                None => {
                    let link_view = view.clone();
                    crate::links::message_body(
                        gpui_kit::ElementId::Name(format!("thread.{id}.body").into()),
                        &post.body,
                        &post.image_sizes,
                        move |url, _, cx| {
                            let event = RightPaneEvent::OpenLink(url.clone());
                            link_view.update(cx, |_, cx| cx.emit(event)).ok();
                        },
                        cx,
                    )
                    .into_any_element()
                }
            })
            .children(
                menus
                    .filter(|_| !post.embeds.is_empty())
                    .map(|menus| Self::embeds(view, post, menus, cx).into_any_element()),
            )
            .children((!post.files.is_empty()).then(|| {
                let (image_view, image_post) = (view.clone(), post.clone());
                let file_view = view.clone();
                post_files(
                    &format!("thread.{id}"),
                    &post.files,
                    move |index, window, cx| {
                        let post = image_post.clone();
                        image_view
                            .update(cx, |this, cx| this.open_lightbox(&post, index, window, cx))
                            .ok();
                    },
                    move |file, _, cx| {
                        file_view
                            .update(cx, |_, cx| cx.emit(RightPaneEvent::OpenFile(file)))
                            .ok();
                    },
                    cx,
                )
            }));
        if queued(post, away) {
            // Queued: a clock marks "Will send when back online" (#195).
            row = row.header_child(queued_marker(&theme));
        }
        if let Some(label) = crate::priority::label(post.priority.as_ref(), cx) {
            row = row.header_child(label);
        }
        for marker in post_markers(post, &theme) {
            row = row.header_child(marker);
        }
        let (ack_view, ack_post) = (view.clone(), id.clone());
        row = row.children(crate::priority::ack_bar(
            &format!("thread.{id}"),
            post.priority.as_ref(),
            post.status == PostStatus::Sent,
            std::rc::Rc::new(move |acknowledged, _, cx| {
                let post = ack_post.clone();
                ack_view
                    .update(cx, |_, cx| {
                        cx.emit(RightPaneEvent::Acknowledge { post, acknowledged })
                    })
                    .ok();
            }),
            cx,
        ));
        if reactable && !post.reactions.is_empty() {
            // The chip row gets an add button, as in the channel (#620).
            let (add_view, add_id) = (view.clone(), post.id.clone());
            row = row.on_add_reaction(move |_, _, cx| {
                let id = add_id.clone();
                add_view
                    .update(cx, |this, cx| this.open_picker(Some(id), cx))
                    .ok();
            });
        }
        // Right-click selects the row and opens its menu (design-42).
        // Menus are built when they open, not for every row on every frame
        // (#674).
        let menu_post = std::rc::Rc::new(post.clone());
        let menu_view = view.clone();
        let selector = format!("thread.{id}");
        div()
            .debug_selector(move || selector.clone())
            .on_mouse_down(
                gpui_kit::MouseButton::Right,
                move |event: &gpui_kit::MouseDownEvent, window, cx| {
                    let items = Self::thread_items_for(&menu_view, &menu_post, away, !root);
                    menu_view
                        .update(cx, |this, cx| {
                            this.select_post(&id, window, cx);
                            this.open_menu(items, event.position, window, cx);
                        })
                        .ok();
                },
            )
            .child(row)
    }

    /// A sent thread post's hover toolbar (#620): add a reaction, and
    /// More with the right-click menu's actions.
    fn hover_actions(
        view: &gpui_kit::WeakEntity<Self>,
        post: &Post,
        away: bool,
        reply: bool,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let id = post.id.clone();
        // A tooltip would sit over the open menu.
        let menu_open = view
            .upgrade()
            .is_some_and(|pane| pane.read(cx).menu.read(cx).is_open());
        let quick_view = view.clone();
        let quick_id = id.clone();
        let quick = quick_reaction_buttons(
            &id,
            move |emoji, cx| {
                let id = quick_id.clone();
                quick_view
                    .update(cx, |this, cx| this.toggle_reaction(id, emoji, cx))
                    .ok();
            },
            cx,
        );
        let post = std::rc::Rc::new(post.clone());
        let (react_view, react_id) = (view.clone(), id.clone());
        let react_selector = format!("thread.{id}.react");
        let (more_view, more_post) = (view.clone(), post);
        message_actions(cx)
            .children(quick)
            .child(
                style::tipped_when(
                    !menu_open,
                    gpui_kit::ElementId::Name(format!("thread.{id}.react.tip").into()),
                    t!("pane-add-reaction"),
                    message_action(
                        gpui_kit::ElementId::Name(format!("thread.{id}.react").into()),
                        LucideIcon::FaceSlightlySmilingPlus,
                        t!("pane-add-reaction"),
                        cx,
                    )
                    .on_click(move |_, _, cx| {
                        let id = react_id.clone();
                        react_view
                            .update(cx, |this, cx| this.open_picker(Some(id), cx))
                            .ok();
                    }),
                )
                .debug_selector(move || react_selector.clone()),
            )
            .child(style::tipped_when(
                !menu_open,
                gpui_kit::ElementId::Name(format!("thread.{id}.more.tip").into()),
                t!("pane-more-actions"),
                message_action(
                    gpui_kit::ElementId::Name(format!("thread.{id}.more").into()),
                    LucideIcon::Ellipsis,
                    t!("pane-more-actions"),
                    cx,
                )
                .on_click(move |event: &gpui_kit::ClickEvent, window, cx| {
                    let items = Self::thread_items_for(&more_view, &more_post, away, reply);
                    more_view
                        .update(cx, |this, cx| {
                            this.open_menu(items, event.position(), window, cx)
                        })
                        .ok();
                }),
            ))
    }

    /// The cards under a thread post (#187), with an attachment's buttons
    /// and menus (#211), as the channel draws them.
    fn embeds(
        view: &gpui_kit::WeakEntity<Self>,
        post: &Post,
        menus: &crate::interactive::Menus,
        cx: &App,
    ) -> impl IntoElement + use<> {
        // The real id: a choice names its post by it.
        let id = post.id.clone();
        let collapsed = SettingsStore::global(cx)
            .read(cx)
            .get()
            .previews_collapsed(&id);
        let toggle_id = id.clone();
        let on_toggle: crate::embeds::ToggleHandler = std::rc::Rc::new(move |collapsed, _, cx| {
            // Remembered with the channel's choice for the same post; the
            // pane remeasures on the settings change.
            let store = SettingsStore::global(cx);
            if store.read(cx).get().previews_collapsed(&toggle_id) != collapsed {
                store.update(cx, |store, cx| {
                    store.update(cx, |settings| {
                        settings.choose_previews(&toggle_id, collapsed)
                    })
                });
            }
        });
        // A linked image opens in the image viewer, as in the channel (#683).
        let (image_view, image_post) = (view.clone(), post.clone());
        let on_image: crate::embeds::ImageHandler = std::rc::Rc::new(move |url, window, cx| {
            image_view
                .update(cx, |this, cx| {
                    this.open_linked_image(&image_post, url, window, cx)
                })
                .ok();
        });
        let jump_view = view.clone();
        let on_jump: crate::embeds::OnJump = std::rc::Rc::new(move |channel, post, _, cx| {
            let event = RightPaneEvent::Jump {
                channel: channel.clone(),
                post: post.clone(),
            };
            jump_view.update(cx, |_, cx| cx.emit(event)).ok();
        });
        let action_view = view.clone();
        let on_action: crate::interactive::ActionHandler =
            std::rc::Rc::new(move |choice, _, cx| {
                action_view
                    .update(cx, |this, cx| this.run_action(choice, cx))
                    .ok();
            });
        crate::embeds::post_embeds(
            &id,
            &post.embeds,
            collapsed,
            on_toggle,
            on_image,
            on_jump,
            (menus, on_action),
            cx,
        )
        .into_any_element()
    }

    fn replies_divider(
        view: &gpui_kit::WeakEntity<Self>,
        thread: &ThreadModel,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let view = view.clone();
        // Older replies may not be loaded yet; the server's count holds.
        let count = thread.reply_count.max(thread.replies.len());
        let label = t!("pane-replies", count = count);
        div()
            .flex()
            .items_center()
            .gap(theme.space(3.))
            .px(theme.space(4.))
            .py(theme.space(2.))
            .child(
                div()
                    .type_style(style::META, &theme)
                    .text_color(theme.colors.muted_foreground)
                    .child(label),
            )
            .child(div().flex_1().h(style::hairline()).bg(theme.colors.border))
            .child(
                Button::new("right-pane.follow")
                    .variant(if thread.following {
                        ButtonVariant::Secondary
                    } else {
                        ButtonVariant::Outline
                    })
                    .size(ButtonSize::Xs)
                    .label(if thread.following {
                        t!("pane-following")
                    } else {
                        t!("pane-follow")
                    })
                    .on_click(move |_, _, cx| {
                        view.update(cx, |this, cx| this.toggle_follow(cx)).ok();
                    }),
            )
    }

    fn thread(&self, thread: &ThreadModel, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let model = thread.clone();
        let view = cx.entity().downgrade();
        let selection = self.selection;
        let permalink = self.permalink.clone();
        let highlight = move |index: usize, post: &Post| {
            if selection == Some(index) {
                MessageHighlight::Selected
            } else if permalink.as_ref() == Some(&post.id) {
                MessageHighlight::Permalink
            } else {
                MessageHighlight::None
            }
        };
        let away = self.away;
        let editing = self.editing.as_ref().map(|editing| {
            (
                editing.post.clone(),
                (editing.composer.clone(), self.composer_cap),
            )
        });
        let menus = self.action_menus.clone();
        let transcript =
            MessageList::new(
                "right-pane.thread",
                &self.list,
                move |index, _, cx| match index {
                    0 => Self::post_row(
                        &view,
                        &model.root,
                        true,
                        highlight(0, &model.root),
                        (away, None, Some(&menus.borrow())),
                        Self::presence_in(&view, &model.root.author, cx),
                        cx,
                    )
                    .into_any_element(),
                    1 => Self::replies_divider(&view, &model, cx).into_any_element(),
                    2 => view
                        .update(cx, |this, cx| {
                            this.request_older(cx);
                            this.older_row(cx).into_any_element()
                        })
                        .unwrap_or_else(|_| div().into_any_element()),
                    index => match model.replies.get(index - THREAD_HEAD) {
                        Some(reply) => {
                            let editor = editing
                                .as_ref()
                                .filter(|(post, _)| *post == reply.id)
                                .map(|(_, editor)| editor);
                            Self::post_row(
                                &view,
                                reply,
                                false,
                                highlight(index, reply),
                                (away, editor, Some(&menus.borrow())),
                                Self::presence_in(&view, &reply.author, cx),
                                cx,
                            )
                            .into_any_element()
                        }
                        None => div().into_any_element(),
                    },
                },
            )
            .size_full()
            .pt(theme.space(2.))
            .track_focus(&self.thread_focus)
            .selected(self.selection)
            .selectable(|index, _| index == 0 || index >= THREAD_HEAD)
            .on_select({
                let view = cx.entity().downgrade();
                move |index, window, cx| {
                    view.update(cx, |this, cx| this.select_row(index, window, cx))
                        .ok();
                }
            })
            .on_key({
                let view = cx.entity().downgrade();
                move |event, selected, window, cx| {
                    view.update(cx, |this, cx| {
                        this.selection_key(&event.keystroke, selected, window, cx)
                    })
                    .unwrap_or(false)
                }
            });
        let send_on = SettingsStore::global(cx).read(cx).get().send_on;
        let send = Button::new("right-pane.send")
            .variant(ButtonVariant::Default)
            .size(ButtonSize::IconSm)
            .aria_label(t!("pane-reply"))
            .disabled(self.tray.gate(self.draft_len > 0) != SendGate::Now)
            .on_click(cx.listener(|this, _, window, cx| {
                let text = this.composer.read(cx).value(cx);
                this.reply(text, window, cx)
            }))
            .child(if self.tray.is_queued() {
                Spinner::new("right-pane.send.spinner")
                    .color(theme.colors.primary_foreground)
                    .into_any_element()
            } else {
                lucide(LucideIcon::SendHorizontal)
                    .size(scaled(&theme, metrics::ICON))
                    .text_color(theme.colors.primary_foreground)
                    .into_any_element()
            });
        let chips: Vec<Attachment> = self
            .tray
            .uploads()
            .iter()
            .map(|upload| self.upload_chip(upload, cx))
            .collect();
        let tray = crate::attachments::upload_tray("right-pane", &self.tray, chips, cx);
        // Files dropped anywhere on the thread go into the reply (#150).
        let overlay = crate::attachments::drop_overlay(
            "right-pane.drop",
            crate::attachments::drop_title(&thread.channel_name),
            t!("pane-drop"),
            crate::attachments::preview_drop(),
            cx,
        );
        let tool = |id: &'static str, label: SharedString, glyph: LucideIcon| {
            Button::new(id)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::IconSm)
                .aria_label(label)
                .child(
                    lucide(glyph)
                        .size(scaled(&theme, metrics::ICON))
                        .text_color(theme.colors.muted_foreground),
                )
        };
        // The full picker, for the reply or a reaction, as in the channel
        // (#679).
        let picker = self.picker.clone();
        let picker_open = self.picker_open;
        let emoji_tool = tool(
            "right-pane.emoji",
            t!("pane-emoji"),
            LucideIcon::FaceSlightlySmiling,
        );
        let emoji_popover = popover("right-pane.emoji.popover")
            .open(picker_open)
            .offset(px(6.))
            .trigger_with(move |_, _, _| {
                style::tipped_when(
                    !picker_open,
                    "right-pane.emoji.tip",
                    t!("pane-emoji"),
                    emoji_tool,
                )
                .into_any_element()
            })
            .on_open_change(cx.listener(|this, open: &bool, window, cx| {
                if *open {
                    this.open_picker(None, cx);
                } else {
                    this.close_picker(window, cx);
                }
            }))
            .content(move |_, window, cx| {
                picker.update(cx, |picker, cx| picker.open(window, cx));
                emoji_popup(EmojiPicker::new(&picker).bordered(false), cx)
            });
        let format_bar = SettingsStore::global(cx).read(cx).get().format_bar;
        let format_tool =
            |id: &'static str,
             label: SharedString,
             action: &'static str,
             glyph: LucideIcon,
             (before, after): (&'static str, &'static str)| {
                let composer = self.composer.clone();
                style::tipped(
                    SharedString::from(format!("{id}.tip")),
                    actions::tooltip_text(&label, action),
                    tool(id, label, glyph).on_click(move |_, window, cx| {
                        composer.update(cx, |composer, cx| {
                            crate::channel::format_bar::toggle_format(
                                composer, before, after, window, cx,
                            );
                            composer.focus(window, cx);
                        })
                    }),
                )
            };
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.add_sources(paths.paths().iter().cloned().map(UploadSource::Path), cx)
            }))
            .child(div().flex_1().min_h_0().child(transcript))
            .child(
                div()
                    .flex_shrink_0()
                    .on_action(cx.listener(|this, _: &UploadFiles, window, cx| {
                        this.choose_files(window, cx)
                    }))
                    .px(theme.space(4.))
                    .pt(theme.space(1.))
                    .children(self.link.preview(
                        "right-pane.link-preview",
                        cx,
                        cx.listener(|this, _, _, cx| {
                            this.link.remove();
                            cx.notify();
                        }),
                    ))
                    .children(tray)
                    .children(crate::channel::format_bar::format_row(&self.composer, cx))
                    .children(crate::slash::error_line(
                        "right-pane.command-error",
                        self.slash.error(),
                        cx,
                    ))
                    .child(
                        Composer::new(&self.composer)
                            .max_height(self.composer_cap)
                            .rounded(theme.radius.lg)
                            .child(style::tipped(
                                "right-pane.attach.tip",
                                actions::tooltip_text(&t!("pane-attach"), "Upload files"),
                                tool("right-pane.attach", t!("pane-attach"), LucideIcon::Plus)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.choose_files(window, cx)
                                    })),
                            ))
                            // The channel composer's formats, so a reply
                            // writes the same way (#683).
                            .child(format_tool(
                                "right-pane.bold",
                                t!("chan-bold"),
                                "Bold", // i18n-ignore: the shortcut table's action name
                                LucideIcon::Bold,
                                ("**", "**"),
                            ))
                            .child(format_tool(
                                "right-pane.italic",
                                t!("chan-italic"),
                                "Italic", // i18n-ignore: the shortcut table's action name
                                LucideIcon::Italic,
                                ("_", "_"),
                            ))
                            .child(format_tool(
                                "right-pane.code",
                                t!("chan-code"),
                                "Inline code", // i18n-ignore: the shortcut table's action name
                                LucideIcon::Code,
                                ("`", "`"),
                            ))
                            .child(style::tipped(
                                "right-pane.formatting.tip",
                                if format_bar {
                                    t!("chan-hide-formatting")
                                } else {
                                    t!("chan-show-formatting")
                                },
                                tool(
                                    "right-pane.formatting",
                                    if format_bar {
                                        t!("chan-hide-formatting")
                                    } else {
                                        t!("chan-show-formatting")
                                    },
                                    LucideIcon::Type,
                                )
                                .variant(if format_bar {
                                    ButtonVariant::Secondary
                                } else {
                                    ButtonVariant::Ghost
                                })
                                .on_click(|_, _, cx| {
                                    crate::channel::format_bar::toggle_format_bar(cx)
                                }),
                            ))
                            .child(emoji_popover)
                            .child(div().flex_1())
                            .children(self.schedule_button(cx))
                            .child(style::tipped(
                                "right-pane.send.tip",
                                actions::send_tooltip(&t!("pane-reply"), send_on),
                                send,
                            )),
                    ),
            )
            .child(crate::channel::typing_line(&self.typing, cx))
            .children(overlay)
    }

    /// Nothing open: one line on how to get a thread here.
    fn empty(cx: &App) -> impl IntoElement + use<> {
        style::centered_empty(
            LucideIcon::MessageSquareReply,
            t!("pane-no-thread"),
            t!("pane-no-thread-detail"),
            cx,
        )
    }

    /// The search's message results: a count, then the listing.
    fn search_messages(
        &self,
        body: gpui_kit::Div,
        model: &SearchModel,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let theme = UiTheme::read(cx).clone();
        body.when(
            !model.results.is_empty() && model.files.is_empty(),
            |body| {
                body.child(
                    div()
                        .px(theme.space(4.))
                        .pt(theme.space(2.))
                        .type_style(style::META, &theme)
                        .text_color(theme.colors.muted_foreground)
                        .child(t!(
                            "pane-search-count",
                            count = model.results.len(),
                            query = model.query.to_string(),
                        )),
                )
            },
        )
        .child(self.listing(
            "right-pane.search",
            &model.results,
            (
                LucideIcon::SearchX,
                t!("pane-search-nothing"),
                t!("pane-search-nothing-detail"),
            ),
            model.more.then_some(model.loading_more),
            cx,
        ))
    }

    /// Messages and Files, with their counts, once a search found files
    /// (#207).
    fn search_tabs(&self, model: &SearchModel, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let view = cx.entity().downgrade();
        let more = if model.more { "+" } else { "" };
        div().px(theme.space(4.)).pt(theme.space(2.)).child(
            crate::ui::tabs::Tabs::new("right-pane.search.tabs")
                .aria_label(t!("pane-search-results"))
                .selected(if self.search_files_tab {
                    "files"
                } else {
                    "messages"
                })
                .item(crate::ui::tabs::Tab::new(
                    "right-pane.search.tabs.messages",
                    "messages",
                    t!(
                        "pane-search-messages-tab",
                        count = model.results.len(),
                        more = more,
                    ),
                ))
                .item(crate::ui::tabs::Tab::new(
                    "right-pane.search.tabs.files",
                    "files",
                    t!("pane-search-files-tab", count = model.files.len()),
                ))
                .on_change(move |value, _, cx| {
                    view.update(cx, |this, cx| {
                        this.search_files_tab = value == "files";
                        cx.notify();
                    })
                    .ok();
                }),
        )
    }

    /// Channel files (#208): loading, empty, or the files under who posted
    /// them and when.
    fn channel_files(
        &self,
        files: Option<&[FoundFile]>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = UiTheme::read(cx).clone();
        match files {
            None if self.offline => {
                let (title, text) = crate::offline::pane_empty("files");
                style::centered_empty(LucideIcon::WifiOff, title, text, cx).into_any_element()
            }
            None => div()
                .flex_1()
                .px(theme.space(4.))
                .py(theme.space(3.))
                .type_style(style::META, &theme)
                .text_color(theme.colors.muted_foreground)
                .child(t!("pane-loading"))
                .into_any_element(),
            Some([]) => style::centered_empty(
                LucideIcon::Paperclip,
                t!("pane-files-empty"),
                t!("pane-files-empty-detail"),
                cx,
            )
            .into_any_element(),
            Some(files) => self.found_files(files, false, cx).into_any_element(),
        }
    }

    /// The files a search found (#207): each under its channel, who posted
    /// it and when, with Jump to its message. An image opens the lightbox
    /// and any other file downloads, as in the transcript.
    /// Channel files leaves out the channel, which its header names.
    fn found_files(
        &self,
        files: &[FoundFile],
        with_channel: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let view = cx.entity().downgrade();
        div()
            .id("right-pane.search.files")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(theme.space(2.))
            .children(files.iter().map(|found| {
                let channel = match found.channel_name.as_ref() {
                    name if name.contains(' ') => name.to_string(),
                    name => format!("#{name}"),
                };
                let (jump_channel, jump_post) = (found.channel_id.clone(), found.post_id.clone());
                let (jump_view, image_view, file_view) = (view.clone(), view.clone(), view.clone());
                let shown = vec![found.file.clone()];
                let image_files = shown.clone();
                let caption: SharedString = format!("{} · {}", found.author, found.day).into();
                div()
                    .pb(theme.space(2.))
                    .mb(theme.space(2.))
                    .border_b(style::hairline())
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .px(theme.space(4.))
                            .pb(theme.space(1.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(theme.space(2.))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .type_style(style::META, &theme)
                                    .text_color(theme.colors.muted_foreground)
                                    .child(if with_channel {
                                        format!("{channel} · {} · {}", found.author, found.day)
                                    } else {
                                        format!("{} · {}", found.author, found.day)
                                    }),
                            )
                            .child(
                                Button::new(gpui_kit::ElementId::Name(
                                    format!("right-pane.search.files.{}.jump", found.file.id)
                                        .into(),
                                ))
                                .variant(ButtonVariant::Link)
                                .size(ButtonSize::Xs)
                                .label(t!("pane-jump"))
                                .on_click(move |_, _, cx| {
                                    let event = RightPaneEvent::Jump {
                                        channel: jump_channel.clone(),
                                        post: jump_post.clone(),
                                    };
                                    jump_view.update(cx, |_, cx| cx.emit(event)).ok();
                                }),
                            ),
                    )
                    .child(div().px(theme.space(4.)).child(post_files(
                        "right-pane.search.files",
                        &shown,
                        move |_, window, cx| {
                            let (files, caption) = (image_files.clone(), caption.clone());
                            image_view
                                .update(cx, |this, cx| {
                                    this.open_images(&files, caption, 0, window, cx)
                                })
                                .ok();
                        },
                        move |file, _, cx| {
                            file_view
                                .update(cx, |_, cx| cx.emit(RightPaneEvent::OpenFile(file)))
                                .ok();
                        },
                        cx,
                    )))
            }))
    }

    /// Saved, pinned and mention listings: each post under a caption with
    /// its channel and a Jump link, separated by hairlines.
    fn listing(
        &self,
        id: &'static str,
        items: &[ListedPost],
        empty: (LucideIcon, SharedString, SharedString),
        more: Option<bool>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = UiTheme::read(cx).clone();
        if items.is_empty() && self.loading {
            return div()
                .flex_1()
                .px(theme.space(4.))
                .py(theme.space(3.))
                .type_style(style::META, &theme)
                .text_color(theme.colors.muted_foreground)
                .child(t!("pane-loading"))
                .into_any_element();
        }
        if items.is_empty() && self.offline {
            let (title, text) = crate::offline::pane_empty(id.rsplit('.').next().unwrap_or(id));
            return style::centered_empty(LucideIcon::WifiOff, title, text, cx).into_any_element();
        }
        if items.is_empty() {
            return style::centered_empty(empty.0, empty.1, empty.2, cx).into_any_element();
        }
        let view = cx.entity().downgrade();
        div()
            .id(id)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(theme.space(2.))
            .children(items.iter().map(|item| {
                let channel = match item.channel_name.as_ref() {
                    name if name.contains(' ') => name.to_string(),
                    name => format!("#{name}"),
                };
                let (jump_channel, jump_post) = (item.channel_id.clone(), item.post.id.clone());
                let jump_view = view.clone();
                div()
                    .pb(theme.space(2.))
                    .mb(theme.space(2.))
                    .border_b(style::hairline())
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .px(theme.space(4.))
                            .pb(theme.space(1.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .type_style(style::META, &theme)
                                    .text_color(theme.colors.muted_foreground)
                                    .child(format!("{channel} · {}", item.post.day)),
                            )
                            .child(
                                Button::new(gpui_kit::ElementId::Name(
                                    format!("{id}.{}.jump", item.post.id).into(),
                                ))
                                .variant(ButtonVariant::Link)
                                .size(ButtonSize::Xs)
                                .label(t!("pane-jump"))
                                .on_click(move |_, _, cx| {
                                    let event = RightPaneEvent::Jump {
                                        channel: jump_channel.clone(),
                                        post: jump_post.clone(),
                                    };
                                    jump_view.update(cx, |_, cx| cx.emit(event)).ok();
                                }),
                            ),
                    )
                    .child(Self::post_row(
                        &view,
                        &Post {
                            continues: false,
                            ..item.post.clone()
                        },
                        true,
                        MessageHighlight::None,
                        // Listings show no cards, as before.
                        (false, None, None),
                        self.presence_of(&item.post.author),
                        cx,
                    ))
            }))
            // `Some(loading)` when another page may follow.
            .when_some(more, |list, loading| {
                list.child(
                    div().px(theme.space(4.)).pb(theme.space(2.)).child(
                        Button::new(gpui_kit::ElementId::Name(format!("{id}.more").into()))
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Sm)
                            .w_full()
                            .disabled(loading)
                            .label(if loading {
                                t!("pane-search-loading-more")
                            } else {
                                t!("pane-search-more")
                            })
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(RightPaneEvent::SearchMore)),
                            ),
                    ),
                )
            })
            .into_any_element()
    }

    /// The Search listing: the field, a hint about modifiers, then the
    /// results under a count, or the state the search is in.
    fn search(&self, model: &SearchModel, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let muted = theme.colors.muted_foreground;
        let recent = SettingsStore::global(cx)
            .read(cx)
            .get()
            .recent_searches
            .clone();
        let body = if model.query.is_empty() && !recent.is_empty() {
            self.recent_searches(&recent, cx).into_any_element()
        } else if model.query.is_empty() {
            style::centered_empty(
                LucideIcon::Search,
                t!("pane-search-empty"),
                t!("pane-search-empty-detail"),
                cx,
            )
            .into_any_element()
        } else if model.searching {
            div()
                .flex_1()
                .px(theme.space(4.))
                .py(theme.space(3.))
                .type_style(style::META, &theme)
                .text_color(muted)
                .child(t!("pane-searching", query = model.query.to_string()))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .when(!model.files.is_empty(), |body| {
                    body.child(self.search_tabs(model, cx))
                })
                .when(self.search_files_tab && !model.files.is_empty(), |body| {
                    body.child(self.found_files(&model.files, true, cx))
                })
                .when(!self.search_files_tab || model.files.is_empty(), |body| {
                    self.search_messages(body, model, cx)
                })
                .into_any_element()
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_shrink_0()
                    .px(theme.space(4.))
                    .pt(theme.space(3.))
                    .pb(theme.space(2.))
                    .flex()
                    .flex_col()
                    .gap(theme.space(1.5))
                    .border_b(style::hairline())
                    .border_color(theme.colors.border)
                    // Caught on the way down, before the field moves its cursor.
                    .capture_action(
                        cx.listener(|this, _: &MoveDown, _, cx| this.step_suggestion(true, cx)),
                    )
                    .capture_action(
                        cx.listener(|this, _: &MoveUp, _, cx| this.step_suggestion(false, cx)),
                    )
                    .child(Input::new(&self.search).aria_label(t!("pane-search-placeholder")))
                    .when(!self.suggestions.is_empty(), |field| {
                        field.child(self.suggestion_list(cx))
                    })
                    .child(
                        div()
                            .type_style(style::META, &theme)
                            .text_color(muted)
                            .child(t!("pane-search-hint")),
                    ),
            )
            .child(body)
    }

    /// The people or channels the word being typed could name.
    fn suggestion_list(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let muted = theme.colors.muted_foreground;
        div()
            .flex()
            .flex_col()
            .children(
                self.suggestions
                    .iter()
                    .enumerate()
                    .map(|(index, suggestion)| {
                        let insert = suggestion.insert.clone();
                        Button::new(gpui_kit::ElementId::Name(
                            format!("right-pane.search.suggestion.{index}").into(),
                        ))
                        .variant(if self.suggestion_at == Some(index) {
                            ButtonVariant::Secondary
                        } else {
                            ButtonVariant::Ghost
                        })
                        .w_full()
                        .justify_start()
                        .aria_label(t!(
                            "pane-search-suggestion",
                            query = suggestion.insert.to_string()
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme.space(2.))
                                .min_w_0()
                                .child(div().truncate().child(suggestion.label.clone()))
                                .child(
                                    div()
                                        .type_style(style::META, &theme)
                                        .text_color(muted)
                                        .child(suggestion.detail.clone()),
                                ),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.accept_suggestion(insert.clone(), window, cx)
                            },
                        ))
                    }),
            )
    }

    /// The last searches, newest first, each one click from running again.
    fn recent_searches(
        &self,
        recent: &[String],
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let muted = theme.colors.muted_foreground;
        div()
            .id("right-pane.recent-searches")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(theme.space(2.))
            .py(theme.space(2.))
            .flex()
            .flex_col()
            .child(
                div()
                    .px(theme.space(2.))
                    .pb(theme.space(1.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .type_style(style::LABEL, &theme)
                            .text_color(muted)
                            .child(t!("pane-recent-searches")),
                    )
                    .child(
                        Button::new("right-pane.recent-searches.clear")
                            .variant(ButtonVariant::Link)
                            .size(ButtonSize::Xs)
                            .label(t!("pane-recent-clear"))
                            .on_click(cx.listener(|_, _, _, cx| {
                                SettingsStore::global(cx).update(cx, |store, cx| {
                                    store.update(cx, |settings| settings.recent_searches.clear())
                                });
                                cx.notify();
                            })),
                    ),
            )
            .children(recent.iter().enumerate().map(|(index, query)| {
                let run: SharedString = query.clone().into();
                Button::new(gpui_kit::ElementId::Name(
                    format!("right-pane.recent-searches.{index}").into(),
                ))
                .variant(ButtonVariant::Ghost)
                .w_full()
                .justify_start()
                .aria_label(t!("pane-recent-again", query = query.clone()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space(2.))
                        .min_w_0()
                        .child(
                            lucide(LucideIcon::Clock)
                                .size(theme.space(4.))
                                .text_color(muted),
                        )
                        .child(div().truncate().child(query.clone())),
                )
                .on_click(
                    cx.listener(move |_, _, _, cx| cx.emit(RightPaneEvent::Search(run.clone()))),
                )
            }))
    }

    /// Channel info (PLAN §3 M1 navigation): what the channel is for, the
    /// actions on it, then its members with their presence.
    fn channel_info(
        &self,
        channel: &ChannelModel,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let muted = theme.colors.muted_foreground;
        let label = |text: String| {
            div()
                .type_style(style::LABEL, &theme)
                .text_color(muted)
                .child(text.to_uppercase())
        };
        let kind_icon = match channel.kind {
            ChannelKind::Public => Some(LucideIcon::Hash),
            ChannelKind::Private => Some(LucideIcon::Lock),
            ChannelKind::Direct { .. } | ChannelKind::Group { .. } => None,
        };
        let chip = |id: &'static str, text: SharedString, glyph: LucideIcon, on: bool| {
            Button::new(id)
                .variant(if on {
                    ButtonVariant::Secondary
                } else {
                    ButtonVariant::Outline
                })
                .size(ButtonSize::Sm)
                .aria_label(text.clone())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space(1.5))
                        .child(
                            lucide(glyph)
                                .size(scaled(&theme, metrics::ICON_SMALL))
                                .text_color(if on { theme.colors.foreground } else { muted }),
                        )
                        .child(text),
                )
        };
        let channel_id = channel.id.clone();
        let (favorite_id, mute_id) = (channel_id.clone(), channel_id.clone());
        let (favorite, is_muted) = (channel.favorite, channel.muted);
        let leave = self.leave.clone();
        let about = div()
            .flex()
            .flex_col()
            .gap(theme.space(2.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space(1.5))
                    .type_style(style::TITLE, &theme)
                    .text_color(theme.colors.foreground)
                    .children(kind_icon.map(|glyph| {
                        lucide(glyph)
                            .size(scaled(&theme, metrics::ICON))
                            .text_color(muted)
                    }))
                    .child(channel.name.clone()),
            )
            .children(channel.purpose.clone().map(|text| {
                div()
                    .type_style(style::BODY, &theme)
                    .text_color(theme.colors.foreground)
                    .child(text)
            }))
            .children(channel.header.clone().map(|text| {
                div()
                    .type_style(style::UI, &theme)
                    .text_color(muted)
                    .child(text)
            }));
        let actions = div()
            .flex()
            .flex_wrap()
            .gap(theme.space(2.))
            .child(
                chip(
                    "info.favorite",
                    if favorite {
                        t!("pane-favorited")
                    } else {
                        t!("pane-favorite")
                    },
                    LucideIcon::Star,
                    favorite,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let favorite = !favorite;
                    this.mutate_info(|channel| channel.favorite = favorite, cx);
                    cx.emit(RightPaneEvent::ToggleFavorite {
                        channel: favorite_id.clone(),
                        favorite,
                    });
                })),
            )
            .child(
                chip(
                    "info.mute",
                    if is_muted {
                        t!("pane-muted")
                    } else {
                        t!("pane-mute")
                    },
                    if is_muted {
                        LucideIcon::BellOff
                    } else {
                        LucideIcon::Bell
                    },
                    is_muted,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let muted = !is_muted;
                    this.mutate_info(|channel| channel.muted = muted, cx);
                    cx.emit(RightPaneEvent::ToggleMute {
                        channel: mute_id.clone(),
                        muted,
                    });
                })),
            )
            .child(
                chip(
                    "info.pinned",
                    t!("pane-pinned-count", count = channel.pinned),
                    LucideIcon::Pin,
                    false,
                )
                .on_click(cx.listener(|_, _, _, cx| cx.emit(RightPaneEvent::OpenPinned))),
            )
            .child(
                chip("info.files", t!("pane-files"), LucideIcon::Paperclip, false)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(RightPaneEvent::OpenChannelFiles))),
            );
        // Leaving sits on its own line, away from the toggles.
        let actions = div()
            .flex()
            .flex_col()
            .gap(theme.space(2.))
            .items_start()
            .child(actions)
            .when(kind_icon.is_some(), |actions| {
                actions.child(
                    Button::new("info.leave")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Sm)
                        .aria_label(t!("pane-leave-channel"))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme.space(1.5))
                                .text_color(theme.colors.destructive)
                                .child(
                                    lucide(LucideIcon::LogOut)
                                        .size(scaled(&theme, metrics::ICON_SMALL))
                                        .text_color(theme.colors.destructive),
                                )
                                .child(t!("pane-leave")),
                        )
                        .on_click(move |_, window, cx| leave.open(window, cx)),
                )
            });
        let is_channel = kind_icon.is_some();
        let manage_members = is_channel && channel.rights.members;
        let view = cx.entity().downgrade();
        let me = self.me.clone();
        let can_manage = channel.rights.roles;
        let members = div()
            .flex()
            .flex_col()
            .gap(theme.space(1.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pb(theme.space(1.))
                    .child(label(
                        t!("pane-members", count = channel.members).to_string(),
                    ))
                    .when(manage_members, |row| {
                        row.child(
                            Button::new("info.members.add")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Xs)
                                .aria_label(t!("pane-add-members"))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(theme.space(1.))
                                        .child(
                                            lucide(LucideIcon::UserPlus)
                                                .size(scaled(&theme, metrics::ICON_SMALL))
                                                .text_color(theme.colors.foreground),
                                        )
                                        .child(t!("pane-add")),
                                )
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(RightPaneEvent::AddMembers)),
                                ),
                        )
                    }),
            )
            .children(channel.profiles.iter().map(|profile| {
                let admin = channel.admins.contains(&profile.user_id);
                let others = profile.display_name != me;
                let roles = can_manage && others;
                let removable = manage_members && others;
                Self::member_row(profile, admin, roles, removable, &view, cx)
            }))
            .when(channel.profiles.len() < channel.members, |members| {
                members.child(
                    Button::new("info.members.all")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Sm)
                        .label(t!("pane-all-members", count = channel.members))
                        .on_click(move |_, _, cx| {
                            let event = RightPaneEvent::LoadMembers(channel_id.clone());
                            view.update(cx, |_, cx| cx.emit(event)).ok();
                        }),
                )
            });
        div()
            .id("right-pane.info")
            .overflow_y_scroll()
            .size_full()
            .flex()
            .flex_col()
            .gap(theme.space(5.))
            .p(theme.space(4.))
            .child(about)
            .child(actions)
            .child(members)
    }

    /// One member: avatar with presence, name and handle, opening the
    /// profile card.
    /// The right-click menu of a member the user manages: their admin role
    /// and removing them (#208).
    fn member_items(
        view: &gpui_kit::WeakEntity<Self>,
        profile: &UserProfile,
        admin: bool,
        (roles, removable): (bool, bool),
    ) -> Vec<MenuItem> {
        let (role_view, role_user) = (view.clone(), profile.user_id.clone());
        let (view, user_id) = (view.clone(), profile.user_id.clone());
        let role = if admin {
            t!("pane-remove-admin")
        } else {
            t!("pane-make-admin")
        };
        let mut items = Vec::new();
        if roles {
            items.push(
                MenuItem::new("info.member.admin", role).on_click(move |_, _, cx| {
                    let event = RightPaneEvent::SetChannelAdmin {
                        user_id: role_user.clone(),
                        admin: !admin,
                    };
                    role_view.update(cx, |_, cx| cx.emit(event)).ok();
                }),
            );
        }
        if removable {
            items.push(
                MenuItem::new("info.member.remove", t!("pane-remove-member")).on_click(
                    move |_, _, cx| {
                        let event = RightPaneEvent::RemoveMember {
                            user_id: user_id.clone(),
                        };
                        view.update(cx, |_, cx| cx.emit(event)).ok();
                    },
                ),
            );
        }
        items
    }

    /// One member, with an Admin badge for a channel admin. `roles` (a
    /// channel admin's) and `removable` (managing members) fill its
    /// right-click menu, [`Self::member_items`].
    fn member_row(
        profile: &UserProfile,
        admin: bool,
        roles: bool,
        removable: bool,
        view: &gpui_kit::WeakEntity<Self>,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let menu_view = view.clone();
        let menu_profile = profile.clone();
        let theme = UiTheme::read(cx).clone();
        let palette = Palette::read(cx);
        let initial: String = profile.display_name.chars().take(1).collect();
        let username = profile.username.clone();
        let (row_profile, card_profile) = (profile.clone(), profile.clone());
        let (message_view, mention_view) = (view.clone(), view.clone());
        popover(gpui_kit::ElementId::Name(
            format!("info.member.{username}.popover").into(),
        ))
        .offset(px(4.))
        .trigger_with(move |_, _, cx| {
            let (menu_view, menu_profile) = (menu_view.clone(), menu_profile.clone());
            div()
                .id(gpui_kit::ElementId::Name(
                    format!("info.member.{}", row_profile.username).into(),
                ))
                .when(roles || removable, |row| {
                    row.on_mouse_down(
                        gpui_kit::MouseButton::Right,
                        move |event: &gpui_kit::MouseDownEvent, window, cx| {
                            let items = Self::member_items(
                                &menu_view,
                                &menu_profile,
                                admin,
                                (roles, removable),
                            );
                            let at = event.position;
                            menu_view
                                .update(cx, |this, cx| this.open_menu(items, at, window, cx))
                                .ok();
                        },
                    )
                })
                .h(scaled(&theme, metrics::ROW_HEIGHT + 8.))
                .px(theme.space(2.))
                .mx(theme.space(-2.))
                .rounded(theme.radius.md)
                .flex()
                .items_center()
                .gap(theme.space(3.))
                .cursor_pointer()
                .hover(|style| style.bg(theme.colors.accent))
                .child(
                    div()
                        .relative()
                        .flex_shrink_0()
                        .child(crate::avatars::with_picture(
                            Avatar::new(gpui_kit::ElementId::Name(
                                format!("info.member.{}.avatar", row_profile.username).into(),
                            ))
                            .size(AvatarSize::Sm)
                            .fallback(initial.clone())
                            .aria_label(row_profile.display_name.clone())
                            .status(style::avatar_status(row_profile.presence, &palette))
                            .ring_color(theme.colors.background),
                            &row_profile.user_id,
                            cx,
                        )),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .type_style(style::UI, &theme)
                        .text_color(theme.colors.foreground)
                        .child(row_profile.display_name.clone()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .type_style(style::META, &theme)
                        .text_color(theme.colors.muted_foreground)
                        .child(format!("@{}", row_profile.username)),
                )
                .when(admin, |row| {
                    row.child(
                        Badge::new(t!("pane-admin"))
                            .variant(BadgeVariant::Secondary)
                            .flex_shrink_0()
                            .h(theme.space(4.5))
                            .px(theme.space(1.5))
                            .text_size(theme.text(10.)),
                    )
                })
                .into_any_element()
        })
        .content(move |_, _, cx| {
            let (message_profile, mention_profile) = (card_profile.clone(), card_profile.clone());
            let (message_view, mention_view) = (message_view.clone(), mention_view.clone());
            let (close_message, close_mention, close_view) =
                (cx.entity(), cx.entity(), cx.entity());
            let (view_view, view_profile) = (mention_view.clone(), card_profile.clone());
            let (add_view, add_profile, close_add) =
                (mention_view.clone(), card_profile.clone(), cx.entity());
            let own = add_view
                .upgrade()
                .is_some_and(|this| this.read(cx).me == card_profile.display_name);
            profile_card(
                &username,
                &card_profile,
                ProfileActions {
                    on_message: Box::new(move |window, cx| {
                        close_message.update(cx, |state, cx| state.dismiss(window, cx));
                        let event = RightPaneEvent::OpenDirect {
                            user_id: message_profile.user_id.clone(),
                            username: message_profile.username.clone(),
                            display_name: message_profile.display_name.clone(),
                        };
                        message_view.update(cx, |_, cx| cx.emit(event)).ok();
                    }),
                    on_mention: Box::new(move |window, cx| {
                        close_mention.update(cx, |state, cx| state.dismiss(window, cx));
                        let event = RightPaneEvent::Mention(mention_profile.username.clone());
                        mention_view.update(cx, |_, cx| cx.emit(event)).ok();
                    }),
                    on_view: Some(Box::new(move |window, cx| {
                        close_view.update(cx, |state, cx| state.dismiss(window, cx));
                        let event = RightPaneEvent::ViewProfile(view_profile.clone());
                        view_view.update(cx, |_, cx| cx.emit(event)).ok();
                    })),
                    on_add_to_channel: (!own).then(|| -> crate::profile::ProfileAction {
                        Box::new(move |window, cx| {
                            close_add.update(cx, |state, cx| state.dismiss(window, cx));
                            let event = RightPaneEvent::AddToChannel(add_profile.clone());
                            add_view.update(cx, |_, cx| cx.emit(event)).ok();
                        })
                    }),
                },
                cx,
            )
        })
    }

    /// "Leave #design?" with the action named in the button.
    fn leave_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let name: SharedString = match self.content.as_ref() {
            Some(RightPaneContent::ChannelInfo(channel)) => format!("#{}", channel.name).into(),
            _ => t!("pane-leave-this-channel"),
        };
        let popup = dialog_popup("channel.leave.popup", t!("pane-leave-channel"), cx)
            .child(
                dialog_title("channel.leave.title", cx)
                    .child(t!("pane-leave-title", name = name.to_string())),
            )
            .child(
                dialog_description("channel.leave.description", cx)
                    .child(t!("pane-leave-description")),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        alert_dialog_cancel("channel.leave.cancel")
                            .variant(ButtonVariant::Outline)
                            .label(t!("pane-leave-cancel")),
                    )
                    .child(
                        alert_dialog_action("channel.leave.confirm")
                            .variant(ButtonVariant::Destructive)
                            .label(t!("pane-leave")),
                    ),
            );
        let view = cx.entity().downgrade();
        alert_dialog("channel.leave", &self.leave, popup, window, cx).on_ok(move |_, _, cx| {
            view.update(cx, |this, cx| {
                if let Some(RightPaneContent::ChannelInfo(channel)) = this.content.as_ref() {
                    cx.emit(RightPaneEvent::Leave(channel.id.clone()));
                }
            })
            .ok();
            true
        })
    }
}

impl Render for RightPaneView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.composer_cap = style::composer_cap(window);
        let theme = UiTheme::read(cx).clone();
        let root = div()
            .child(self.lightbox(window, cx))
            .child(self.leave_dialog(window, cx))
            .child(self.discard_dialog(window, cx))
            .child(self.schedule_dialog.clone())
            .child(
                Menu::new(&self.menu, t!("pane-message-actions"))
                    .context_area(div())
                    .disabled(true),
            )
            .key_context(actions::RIGHT_PANE)
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &CloseRightPane, _, cx| cx.emit(RightPaneEvent::Close)))
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(theme.colors.background)
            .border_l(style::hairline())
            .border_color(theme.colors.border);
        if let Some(RightPaneContent::Thread(thread)) = &self.content {
            let posts: Vec<Post> = std::iter::once(&thread.root)
                .chain(&thread.replies)
                .filter(|post| !post.embeds.is_empty())
                .cloned()
                .collect();
            self.action_menus
                .borrow_mut()
                .sync(&posts, window, cx, |this, choice, cx| {
                    this.run_action(choice, cx)
                });
        }
        match self.content.clone() {
            Some(RightPaneContent::Thread(thread)) => root
                .child(self.header(
                    t!("pane-thread"),
                    Some(match thread.channel_name.as_ref() {
                        name if name.contains(' ') => name.to_string().into(),
                        name => format!("#{name}").into(),
                    }),
                    cx,
                ))
                .child(div().flex_1().min_h_0().child(self.thread(&thread, cx))),
            Some(RightPaneContent::ChannelInfo(channel)) => root
                .child(self.header(t!("pane-channel-info"), None, cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(self.channel_info(&channel, cx)),
                ),
            Some(RightPaneContent::ChannelFiles {
                channel_name,
                files,
            }) => root
                .child(self.header(
                    t!("pane-files"),
                    Some(match channel_name.as_ref() {
                        name if name.contains(' ') => name.to_string().into(),
                        name => format!("#{name}").into(),
                    }),
                    cx,
                ))
                .child(self.channel_files(files.as_deref(), cx)),
            Some(RightPaneContent::Pinned {
                channel_name,
                posts,
            }) => root
                .child(self.header(
                    t!("pane-pinned"),
                    Some(match channel_name.as_ref() {
                        name if name.contains(' ') => name.to_string().into(),
                        name => format!("#{name}").into(),
                    }),
                    cx,
                ))
                .child(self.listing(
                    "right-pane.pinned",
                    &posts,
                    (
                        LucideIcon::Pin,
                        t!("pane-pinned-empty"),
                        t!("pane-pinned-empty-detail"),
                    ),
                    None,
                    cx,
                )),
            Some(RightPaneContent::Saved(posts)) => root
                .child(self.header(t!("pane-saved"), None, cx))
                .child(self.listing(
                    "right-pane.saved",
                    &posts,
                    (
                        LucideIcon::Bookmark,
                        t!("pane-saved-empty"),
                        t!("pane-saved-empty-detail"),
                    ),
                    None,
                    cx,
                )),
            Some(RightPaneContent::Mentions(posts)) => root
                .child(self.header(t!("pane-mentions"), None, cx))
                .child(self.listing(
                    "right-pane.mentions",
                    &posts,
                    (
                        LucideIcon::AtSign,
                        t!("pane-mentions-empty"),
                        t!("pane-mentions-empty-detail"),
                    ),
                    None,
                    cx,
                )),
            Some(RightPaneContent::Unreads(posts)) => root
                .child(self.header(t!("pane-unreads"), None, cx))
                .child(self.listing(
                    "right-pane.unreads",
                    &posts,
                    (
                        LucideIcon::Inbox,
                        t!("pane-unreads-empty"),
                        t!("pane-unreads-empty-detail"),
                    ),
                    None,
                    cx,
                )),
            Some(RightPaneContent::Search(model)) => root
                .child(self.header(t!("pane-search"), None, cx))
                .child(self.search(&model, cx)),
            Some(RightPaneContent::Profile(profile)) => {
                let (message_view, mention_view) =
                    (cx.entity().downgrade(), cx.entity().downgrade());
                let (message_profile, username) = (profile.clone(), profile.username.clone());
                root.child(self.header(t!("pane-profile"), None, cx)).child(
                    div().flex_1().min_h_0().child(profile_pane(
                        &profile,
                        Box::new(move |_, cx| {
                            let event = RightPaneEvent::OpenDirect {
                                user_id: message_profile.user_id.clone(),
                                username: message_profile.username.clone(),
                                display_name: message_profile.display_name.clone(),
                            };
                            message_view.update(cx, |_, cx| cx.emit(event)).ok();
                        }),
                        Box::new(move |_, cx| {
                            let event = RightPaneEvent::Mention(username.clone());
                            mention_view.update(cx, |_, cx| cx.emit(event)).ok();
                        }),
                        (profile.display_name != self.me).then(
                            || -> crate::profile::ProfileAction {
                                let (add_view, add_profile) =
                                    (cx.entity().downgrade(), profile.clone());
                                Box::new(move |_, cx| {
                                    let event = RightPaneEvent::AddToChannel(add_profile.clone());
                                    add_view.update(cx, |_, cx| cx.emit(event)).ok();
                                })
                            },
                        ),
                        cx,
                    )),
                )
            }
            // Offline, a thread that is not cached never arrives (#163).
            None if self.offline => {
                let (title, text) = crate::offline::pane_empty("thread");
                root.child(self.header(t!("pane-thread"), None, cx))
                    .child(style::centered_empty(LucideIcon::WifiOff, title, text, cx))
            }
            None => root
                .child(self.header(t!("pane-thread"), None, cx))
                .child(Self::empty(cx)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_walk_the_suggestions_and_wrap() {
        assert_eq!(step(None, 3, true), Some(0));
        assert_eq!(step(None, 3, false), Some(2));
        assert_eq!(step(Some(2), 3, true), Some(0));
        assert_eq!(step(Some(0), 3, false), Some(2));
        assert_eq!(step(Some(1), 3, true), Some(2));
        assert_eq!(step(None, 0, true), None);
    }

    #[test]
    fn from_and_in_suggest_people_and_channels_for_the_last_word() {
        let hints = SearchHints {
            channels: vec![
                ("town-square".into(), "Town Square".into()),
                ("random".into(), "Random".into()),
            ],
            people: vec![
                ("ada".into(), "Ada Lovelace".into()),
                ("alan".into(), "Alan Turing".into()),
            ],
            today: None,
        };
        let inserts = |query: &str| {
            suggestions(query, &hints)
                .into_iter()
                .map(|s| s.insert.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(inserts("deploy from:a"), ["from:ada", "from:alan"]);
        assert_eq!(inserts("from:@tur"), ["from:alan"]);
        assert_eq!(inserts("in:squ"), ["in:town-square"]);
        assert_eq!(inserts("in:"), ["in:town-square", "in:random"]);
        // A finished word, a plain word and an exact match suggest nothing.
        assert!(inserts("from:ada ").is_empty());
        assert!(inserts("lunch").is_empty());
        assert!(inserts("in:random").is_empty());
    }

    #[test]
    fn date_modifiers_suggest_recent_days() {
        let hints = SearchHints {
            today: Some(jiff::civil::date(2026, 11, 5)),
            ..SearchHints::default()
        };
        let shown = |query: &str| {
            suggestions(query, &hints)
                .into_iter()
                .map(|s| format!("{} {}", s.insert, s.label))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            shown("deploy on:")[..3],
            [
                "on:2026-11-05 Today",
                "on:2026-11-04 Yesterday",
                "on:2026-11-03 Tuesday"
            ]
        );
        assert_eq!(shown("on:").len(), SUGGESTIONS);
        // By name or by date, for each modifier.
        assert_eq!(shown("before:yes"), ["before:2026-11-04 Yesterday"]);
        // November 5 back to November 1.
        assert_eq!(shown("after:2026-11-0").len(), 5);
        assert_eq!(shown("on:mon"), ["on:2026-11-02 Monday"]);
        // A whole date, or one further back, suggests nothing.
        assert!(shown("on:2026-11-05").is_empty());
        assert!(shown("on:2025").is_empty());
        // Without today, the modifiers suggest nothing.
        assert!(suggestions("on:", &SearchHints::default()).is_empty());
    }

    use std::cell::RefCell;
    use std::rc::Rc;

    fn post(id: &str) -> Post {
        Post {
            webhook: None,
            id: id.to_string().into(),
            author: "Ada".into(),
            author_id: SharedString::default(),
            kind: crate::channel::PostKind::Regular,
            time: 600,
            day: "Today".into(),
            date: SharedString::default(),
            body: format!("Reply {id}").into(),
            reactions: vec![],
            mentions_me: false,
            edited: false,
            continues: false,
            mine: false,
            saved: false,
            pinned: false,
            status: PostStatus::Sent,
            replies: None,
            root_id: "root".into(),
            reply_to: None,
            files: vec![],
            embeds: vec![],
            image_sizes: vec![],
            priority: None,
        }
    }

    /// The thread with replies `r{from}` to `r{to - 1}` of 500 loaded.
    fn thread(replies: std::ops::Range<usize>, older: bool, failed: bool) -> ThreadModel {
        ThreadModel {
            channel_id: "c-town".into(),
            channel_name: "town".into(),
            root: Post {
                root_id: SharedString::default(),
                ..post("root")
            },
            replies: replies.map(|i| post(&format!("r{i}"))).collect(),
            following: false,
            reply_count: 500,
            older,
            failed_older: failed,
        }
    }

    fn pane(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (
        Entity<RightPaneView>,
        Rc<RefCell<Vec<RightPaneEvent>>>,
        gpui_kit::VisualTestContext,
    ) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let window = cx.add_window(|window, cx| RightPaneView::new("me", window, cx));
        let view = window.root(cx).unwrap();
        let events = Rc::new(RefCell::new(Vec::new()));
        cx.update(|cx| {
            let events = events.clone();
            cx.subscribe(&view, move |_, event: &RightPaneEvent, _| {
                events.borrow_mut().push(event.clone());
            })
            .detach();
        });
        let cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        (view, events, cx)
    }

    fn draw(cx: &mut gpui_kit::VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// A listing draws inside the pane's own render, where the pane cannot
    /// read itself: its authors' presence comes from the pane directly.
    /// Before, opening Saved, Pinned, Mentions or a search with results
    /// panicked.
    #[gpui_kit::test]
    fn a_listing_draws_with_its_authors_presence(cx: &mut gpui_kit::TestAppContext) {
        let (view, _, mut cx) = pane(cx);
        cx.update(|_, cx| {
            view.update(cx, |pane, cx| {
                pane.set_profiles(
                    vec![UserProfile {
                        user_id: "ada".into(),
                        username: "ada".into(),
                        display_name: "Ada".into(),
                        position: None,
                        presence: Presence::Online,
                        status: None,
                        time_zone: None,
                    }],
                    cx,
                );
                pane.set_content(
                    Some(RightPaneContent::Saved(vec![ListedPost {
                        channel_id: "c-town".into(),
                        channel_name: "town".into(),
                        post: post("s1"),
                    }])),
                    cx,
                );
            })
        });
        draw(&mut cx);
        assert_eq!(
            view.read_with(&cx, |pane, _| pane.presence_of("Ada")),
            Some(Presence::Online)
        );
    }

    /// #620, #680: a thread post shows the channel's hover toolbar, and
    /// its add-reaction button opens the full emoji picker.
    #[gpui_kit::test]
    fn a_thread_post_can_be_reacted_to_from_its_hover_toolbar(cx: &mut gpui_kit::TestAppContext) {
        let (view, _, mut cx) = pane(cx);
        cx.update(|_, cx| {
            view.update(cx, |pane, cx| {
                pane.set_thread(thread(0..2, false, false), cx)
            })
        });
        draw(&mut cx);
        let row = cx.debug_bounds("thread.r1").expect("the reply is drawn");
        cx.simulate_mouse_move(row.center(), None, Default::default());
        draw(&mut cx);
        let react = cx
            .debug_bounds("thread.r1.react")
            .expect("hovering shows the add-reaction button");
        cx.simulate_click(react.center(), Default::default());
        assert!(view.read_with(&cx, |pane, _| pane.picker_open));
    }

    /// #620: a reaction a reply doesn't have yet shows at once and is
    /// sent. Before, only an emoji already on the reply could be added.
    #[gpui_kit::test]
    fn a_new_reaction_on_a_reply_shows_and_is_sent(cx: &mut gpui_kit::TestAppContext) {
        let (view, events, mut cx) = pane(cx);
        cx.update(|_, cx| {
            view.update(cx, |pane, cx| {
                pane.set_thread(thread(0..2, false, false), cx);
                pane.toggle_reaction("r1".into(), "👍".into(), cx);
            })
        });
        let reactions = view.read_with(&cx, |pane, _| {
            pane.thread_post(&"r1".into())
                .map(|post| post.reactions.clone())
                .unwrap_or_default()
        });
        assert_eq!(reactions.len(), 1);
        assert_eq!(
            (
                reactions[0].emoji.as_ref(),
                reactions[0].count,
                reactions[0].mine
            ),
            ("👍", 1, true)
        );
        assert_eq!(
            *events.borrow(),
            [RightPaneEvent::ToggleReaction {
                post: "r1".into(),
                emoji: "👍".into()
            }]
        );
    }

    /// #163: offline, a reaction or an edit to a sent reply is refused
    /// before the thread changes, and older replies wait for the
    /// connection instead of a spinner that never ends.
    #[gpui_kit::test]
    fn offline_the_thread_refuses_server_actions_and_waits(cx: &mut gpui_kit::TestAppContext) {
        let (view, events, mut cx) = pane(cx);
        cx.update(|_, cx| {
            view.update(cx, |pane, cx| {
                pane.set_offline(true, cx);
                pane.set_thread(thread(0..3, true, false), cx);
            })
        });
        draw(&mut cx);
        cx.update(|window, cx| {
            view.update(cx, |pane, cx| {
                pane.toggle_reaction("r1".into(), "👍".into(), cx);
                pane.toggle_follow(cx);
                pane.ask_discard("r2".into(), window, cx);
                assert_eq!(pane.discarding, None);
            })
        });
        assert_eq!(
            *events.borrow(),
            [
                RightPaneEvent::Offline("add reaction"),
                RightPaneEvent::Offline("follow thread"),
                RightPaneEvent::Offline("delete"),
            ],
            "no page of older replies was asked for"
        );
        assert!(view.read_with(&cx, |pane, _| !pane.loading_older));

        // Back online, the older page is asked for.
        events.borrow_mut().clear();
        cx.update(|_, cx| view.update(cx, |pane, cx| pane.set_offline(false, cx)));
        draw(&mut cx);
        assert_eq!(
            *events.borrow(),
            [RightPaneEvent::LoadOlderReplies("root".into())]
        );
    }

    /// #122, with S1's measurement: a 500-reply thread opens on its newest
    /// 50, and the 450 older ones go in above without moving the reply the
    /// reader is looking at.
    #[gpui_kit::test]
    fn an_older_page_goes_in_without_moving_the_reader(cx: &mut gpui_kit::TestAppContext) {
        let (view, events, mut cx) = pane(cx);
        cx.update(|_, cx| {
            view.update(cx, |pane, cx| {
                pane.set_thread(thread(450..500, true, false), cx)
            })
        });
        draw(&mut cx);
        // Opened at the newest reply, far from the oldest loaded one.
        assert!(events.borrow().is_empty(), "{:?}", events.borrow());

        let list = view.read_with(&cx, |pane, _| pane.list.clone());
        cx.update(|_, cx| list.update(cx, |list, cx| list.scroll_to_item(THREAD_HEAD + 20, cx)));
        draw(&mut cx);
        let before = list.read_with(&cx, |list, _| list.logical_scroll_top());
        assert_eq!(before.item_ix, THREAD_HEAD + 20);

        cx.update(|_, cx| {
            view.update(cx, |pane, cx| {
                pane.set_thread(thread(0..500, false, false), cx)
            })
        });
        draw(&mut cx);
        let after = list.read_with(&cx, |list, _| list.logical_scroll_top());
        assert_eq!(after.item_ix, before.item_ix + 450);
        assert_eq!(after.offset_in_item, before.offset_in_item);
        let shown = view.read_with(&cx, |pane, _| match &pane.content {
            Some(RightPaneContent::Thread(thread)) => {
                thread.replies[after.item_ix - THREAD_HEAD].id.clone()
            }
            _ => SharedString::default(),
        });
        assert_eq!(shown.as_ref(), "r470", "the same reply is at the top");
        assert_eq!(list.read_with(&cx, |list, _| list.item_count()), 503);
    }

    /// Reaching the oldest loaded reply asks once for the page before it;
    /// a failed page offers Retry instead of asking again.
    /// #141: a notification's click opens the thread before sync has
    /// sent it; once it arrives the pane scrolls to the reply and tints it.
    #[gpui_kit::test]
    fn a_jump_waits_for_its_thread_and_tints_the_reply(cx: &mut gpui_kit::TestAppContext) {
        let (view, _, mut cx) = pane(cx);
        cx.update(|_, cx| {
            view.update(cx, |pane, cx| {
                pane.set_content(None, cx);
                pane.jump_to("root".into(), "r10".into(), cx);
            })
        });
        draw(&mut cx);
        assert_eq!(
            view.read_with(&cx, |pane, _| pane.permalink().cloned()),
            None
        );

        // Another thread does not take the jump.
        cx.update(|_, cx| {
            view.update(cx, |pane, cx| {
                pane.set_thread(
                    ThreadModel {
                        root: Post {
                            root_id: SharedString::default(),
                            ..post("other")
                        },
                        ..thread(0..50, false, false)
                    },
                    cx,
                )
            })
        });
        assert_eq!(
            view.read_with(&cx, |pane, _| pane.permalink().cloned()),
            None
        );

        cx.update(|_, cx| {
            view.update(cx, |pane, cx| {
                pane.set_content(None, cx);
                pane.jump_to("root".into(), "r10".into(), cx);
                pane.set_thread(thread(0..200, false, false), cx);
            })
        });
        draw(&mut cx);
        assert_eq!(
            view.read_with(&cx, |pane, _| pane.permalink().cloned()),
            Some("r10".into())
        );
        let list = view.read_with(&cx, |pane, _| pane.list.clone());
        let top = list.read_with(&cx, |list, _| list.logical_scroll_top());
        assert_eq!(top.item_ix, THREAD_HEAD + 10);
    }

    #[gpui_kit::test]
    fn the_oldest_reply_asks_for_the_page_before_it(cx: &mut gpui_kit::TestAppContext) {
        let (view, events, mut cx) = pane(cx);
        cx.update(|_, cx| {
            view.update(cx, |pane, cx| {
                pane.set_thread(thread(498..500, true, false), cx)
            })
        });
        draw(&mut cx);
        draw(&mut cx);
        let asked = |events: &[RightPaneEvent]| {
            events
                .iter()
                .filter(|event| matches!(event, RightPaneEvent::LoadOlderReplies(root) if root == "root"))
                .count()
        };
        assert_eq!(asked(&events.borrow()), 1);
        assert!(view.read_with(&cx, |pane, _| pane.loading_older));
        assert!(cx.debug_bounds("right-pane.older").is_some());

        cx.update(|_, cx| {
            view.update(cx, |pane, cx| {
                pane.set_thread(thread(498..500, true, true), cx)
            })
        });
        draw(&mut cx);
        assert_eq!(asked(&events.borrow()), 1, "a failed page waits for Retry");
        let retry = cx
            .debug_bounds("right-pane.older.retry")
            .expect("Retry shows");
        cx.simulate_click(retry.center(), Default::default());
        draw(&mut cx);
        assert_eq!(asked(&events.borrow()), 2);

        cx.update(|_, cx| {
            view.update(cx, |pane, cx| {
                pane.set_thread(thread(0..500, false, false), cx)
            })
        });
        draw(&mut cx);
        assert_eq!(
            asked(&events.borrow()),
            2,
            "at the root: nothing more to ask"
        );
        assert!(cx.debug_bounds("right-pane.older").is_none());
    }

    /// #107: single keys act on the selected thread row as in the channel.
    #[gpui_kit::test]
    fn single_keys_act_on_the_selected_reply(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let thread = ThreadModel {
            channel_id: "c-town".into(),
            channel_name: "town".into(),
            root: post("root"),
            replies: vec![Post {
                mine: true,
                status: PostStatus::Sent,
                ..post("r1")
            }],
            following: true,
            reply_count: 1,
            older: false,
            failed_older: false,
        };
        let window = cx.add_window(|window, cx| RightPaneView::new("me", window, cx));
        let view = window.root(cx).unwrap();
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|cx| {
            let events = events.clone();
            cx.subscribe(&view, move |_, event: &RightPaneEvent, _| {
                if matches!(
                    event,
                    RightPaneEvent::SetPinned { .. }
                        | RightPaneEvent::SetSaved { .. }
                        | RightPaneEvent::MarkUnread(_)
                ) {
                    events.borrow_mut().push(event.clone());
                }
            })
            .detach();
        });
        let mut cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let key = |k: &'static str| gpui_kit::Keystroke::parse(k).unwrap();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.set_content(Some(RightPaneContent::Thread(Box::new(thread))), cx);
                let reply = Some(THREAD_HEAD);
                assert!(view.selection_key(&key("p"), reply, window, cx));
                assert!(view.selection_key(&key("s"), reply, window, cx));
                assert!(view.selection_key(&key("u"), Some(0), window, cx));
                // Someone else's root is not the user's to edit; a modifier
                // leaves the key to the app.
                assert!(!view.selection_key(&key("e"), Some(0), window, cx));
                assert!(!view.selection_key(&key("ctrl-p"), reply, window, cx));
                assert!(view.selection_key(&key("e"), reply, window, cx));
                assert!(view.editing.is_some());
                assert_eq!(view.selection, None);
            })
        });
        let events = events.borrow();
        assert!(
            matches!(&events[0], RightPaneEvent::SetPinned { post, pinned: true } if post == "r1")
        );
        assert!(
            matches!(&events[1], RightPaneEvent::SetSaved { post, saved: true } if post == "r1")
        );
        assert!(matches!(&events[2], RightPaneEvent::MarkUnread(post) if post == "root"));
        assert_eq!(events.len(), 3);
    }

    /// #117, #118: the user's sent reply is edited in place, marked
    /// edited, and emptying it asks to delete it for everyone.
    #[gpui_kit::test]
    fn a_sent_reply_is_edited_or_deleted(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let mine = Post {
            mine: true,
            status: PostStatus::Sent,
            body: "Shipping today.".into(),
            ..post("r1")
        };
        let thread = ThreadModel {
            channel_id: "c-town".into(),
            channel_name: "town".into(),
            root: Post {
                mine: true,
                ..post("root")
            },
            replies: vec![mine],
            following: true,
            reply_count: 1,
            older: false,
            failed_older: false,
        };
        let window = cx.add_window(|window, cx| RightPaneView::new("me", window, cx));
        let view = window.root(cx).unwrap();
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|cx| {
            let events = events.clone();
            cx.subscribe(&view, move |_, event: &RightPaneEvent, _| {
                if matches!(
                    event,
                    RightPaneEvent::SaveEdit { .. }
                        | RightPaneEvent::Delete(_)
                        | RightPaneEvent::Discard(_)
                ) {
                    events.borrow_mut().push(event.clone());
                }
            })
            .detach();
        });
        let mut cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.set_content(Some(RightPaneContent::Thread(Box::new(thread))), cx);
                // The root is the channel's to edit; the reply is the pane's.
                view.start_edit("root".into(), window, cx);
                assert!(view.editing.is_none());
                view.start_edit("r1".into(), window, cx);
                assert!(view.editing.is_some());
                view.save_edit("Shipping tomorrow.".into(), window, cx);
                let reply = view.reply_post(&"r1".into()).unwrap();
                assert_eq!(reply.body.as_ref(), "Shipping tomorrow.");
                assert!(reply.edited);

                view.start_edit("r1".into(), window, cx);
                view.save_edit("  ".into(), window, cx);
                assert!(view.editing.is_none());
                assert_eq!(view.discarding.as_deref(), Some("r1"));
                let id = view.discarding.take().unwrap();
                view.discard(id, cx);
                assert!(view.reply_post(&"r1".into()).is_none());
            })
        });
        let events = events.borrow();
        assert!(matches!(&events[0], RightPaneEvent::SaveEdit { post, .. } if post == "r1"));
        assert!(matches!(&events[1], RightPaneEvent::Delete(post) if post == "r1"));
        assert_eq!(events.len(), 2);
    }

    /// #195: a reply queued while offline is edited in place or discarded
    /// from the thread, as in the channel, and sync hears of both.
    #[gpui_kit::test]
    fn a_queued_reply_can_be_edited_or_discarded(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let waiting = |id: &str, body: &str| Post {
            mine: true,
            status: PostStatus::Pending,
            body: body.to_owned().into(),
            // Grouped under the reply before it, which is also the user's.
            continues: true,
            ..post(id)
        };
        let thread = ThreadModel {
            channel_id: "c-town".into(),
            channel_name: "town".into(),
            root: post("root"),
            replies: vec![
                waiting("me:1", "First."),
                waiting("me:2", "Pushing the fix now."),
            ],
            following: true,
            reply_count: 2,
            older: false,
            failed_older: false,
        };
        let window = cx.add_window(|window, cx| RightPaneView::new("me", window, cx));
        let view = window.root(cx).unwrap();
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|cx| {
            let events = events.clone();
            cx.subscribe(&view, move |_, event: &RightPaneEvent, _| {
                if matches!(
                    event,
                    RightPaneEvent::SaveEdit { .. } | RightPaneEvent::Discard(_)
                ) {
                    events.borrow_mut().push(event.clone());
                }
            })
            .detach();
        });
        let mut cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let body = |view: &RightPaneView, id: &str| {
            view.reply_post(&id.into())
                .map(|post| (post.body.to_string(), post.edited))
        };
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.set_content(Some(RightPaneContent::Thread(Box::new(thread))), cx);
                // Online, it is on its way: no menu items, no editor.
                view.set_away(false, cx);
                let reply = view.reply_post(&"me:2".into()).unwrap().clone();
                assert!(!queued(&reply, view.away));
                assert!(waiting_items("t", &reply, view.away, |_, _| {}, |_, _| {}).is_empty());
                view.start_edit("me:2".into(), window, cx);
                assert!(view.editing.is_none());

                // Offline, it waits: the menu offers Edit and Discard…,
                // and the row keeps its header for the clock.
                view.set_away(true, cx);
                assert!(queued(&reply, view.away));
                assert_eq!(
                    waiting_items("t", &reply, view.away, |_, _| {}, |_, _| {}).len(),
                    3
                );
                view.start_edit("me:2".into(), window, cx);
                assert!(view.editing.is_some());
                view.save_edit("Pushing the fix tonight.".into(), window, cx);
                assert!(view.editing.is_none());
                assert_eq!(
                    body(view, "me:2"),
                    Some(("Pushing the fix tonight.".to_owned(), false))
                );

                // Discard asks first, then the row goes and sync drops it.
                view.ask_discard("me:1".into(), window, cx);
                assert_eq!(view.discarding.as_deref(), Some("me:1"));
                let id = view.discarding.take().unwrap();
                view.discard(id, cx);
                assert_eq!(body(view, "me:1"), None);
            })
        });
        // The thread draws with the queued reply in it.
        cx.run_until_parked();
        assert_eq!(
            *events.borrow(),
            [
                RightPaneEvent::SaveEdit {
                    post: "me:2".into(),
                    text: "Pushing the fix tonight.".into(),
                },
                RightPaneEvent::Discard("me:1".into()),
            ]
        );
    }

    /// #114: each thread keeps its own reply draft; switching threads saves
    /// the one left and restores the one opened, and replying clears it.
    #[gpui_kit::test]
    fn a_threads_reply_draft_survives_switching_threads(cx: &mut gpui_kit::TestAppContext) {
        use crate::composer_draft::SAVE_PAUSE;
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let posts = crate::preview::channel("general").unwrap().posts;
        let thread = |index: usize| {
            RightPaneContent::Thread(Box::new(
                crate::preview::thread("general", &posts[index].id).unwrap(),
            ))
        };
        let (first, second) = (thread(0), thread(1));
        let (pane, cx) = cx.add_window_view(|window, cx| RightPaneView::new("me", window, cx));
        let saves = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|_, cx| {
            let saves = saves.clone();
            cx.subscribe(&pane, move |_, event: &RightPaneEvent, _| {
                if let RightPaneEvent::SaveDraft(save) = event {
                    saves
                        .borrow_mut()
                        .push((save.key.root_id.to_string(), save.text.to_string()));
                }
            })
            .detach();
        });
        let show = |content: &RightPaneContent, cx: &mut gpui_kit::VisualTestContext| {
            let content = content.clone();
            cx.update(|_, cx| pane.update(cx, |pane, cx| pane.set_content(Some(content), cx)));
            cx.run_until_parked();
        };
        let focus = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| {
                let composer = pane.read(cx).composer.clone();
                composer.update(cx, |composer, cx| composer.focus(window, cx));
            });
        };
        let text = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|_, cx| pane.read(cx).composer.read(cx).value(cx).to_string())
        };
        let root = |content: &RightPaneContent| match content {
            RightPaneContent::Thread(thread) => thread.root.id.to_string(),
            _ => unreachable!(),
        };

        show(&first, cx);
        focus(cx);
        cx.simulate_input("on it");
        cx.executor().advance_clock(SAVE_PAUSE);
        cx.run_until_parked();
        assert_eq!(
            saves.borrow_mut().drain(..).collect::<Vec<_>>(),
            [(root(&first), "on it".to_owned())]
        );

        show(&second, cx);
        assert_eq!(text(cx), "");
        show(&first, cx);
        assert_eq!(text(cx), "on it");
        assert!(saves.borrow().is_empty(), "restoring saves nothing");

        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| {
                let text = pane.composer.read(cx).value(cx);
                pane.reply(text, window, cx);
            })
        });
        cx.run_until_parked();
        assert_eq!(text(cx), "");
        assert_eq!(
            saves.borrow_mut().drain(..).collect::<Vec<_>>(),
            [(root(&first), String::new())]
        );
    }

    /// #188: the reply composer asks about its link once typing pauses,
    /// and a reply whose preview was removed goes without one.
    #[gpui_kit::test]
    fn a_reply_drafts_link_is_previewed_once_and_can_be_removed(cx: &mut gpui_kit::TestAppContext) {
        use crate::draft_link::PAUSE;
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let posts = crate::preview::channel("general").unwrap().posts;
        let thread = RightPaneContent::Thread(Box::new(
            crate::preview::thread("general", &posts[0].id).unwrap(),
        ));
        let (pane, cx) = cx.add_window_view(|window, cx| RightPaneView::new("me", window, cx));
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|window, cx| {
            let events = events.clone();
            cx.subscribe(&pane, move |_, event: &RightPaneEvent, _| match event {
                RightPaneEvent::PreviewLink(_) | RightPaneEvent::Reply { .. } => {
                    events.borrow_mut().push(event.clone());
                }
                _ => {}
            })
            .detach();
            pane.update(cx, |pane, cx| pane.set_content(Some(thread), cx));
            let composer = pane.read(cx).composer.clone();
            composer.update(cx, |composer, cx| composer.focus(window, cx));
        });
        cx.run_until_parked();
        let url = "https://github.com/karyalabs/matty";

        cx.simulate_input("see https://github.com/karyalabs/matty");
        cx.executor().advance_clock(PAUSE / 2);
        cx.run_until_parked();
        assert!(events.borrow().is_empty(), "nothing before the pause");
        cx.executor().advance_clock(PAUSE);
        cx.run_until_parked();
        cx.simulate_input(" too");
        cx.executor().advance_clock(PAUSE * 2);
        cx.run_until_parked();
        assert_eq!(
            events.borrow_mut().drain(..).collect::<Vec<_>>(),
            [RightPaneEvent::PreviewLink(url.into())],
            "one ask for the link"
        );

        let card = crate::embeds::LinkCard {
            url: url.into(),
            site_name: "GitHub".into(),
            title: "karyalabs/matty".into(),
            description: SharedString::default(),
            image: None,
        };
        let shown = cx.update(|_, cx| {
            pane.update(cx, |pane, cx| {
                pane.set_link_preview(url, Some(card), cx);
                pane.link
                    .preview("thread.link-preview", cx, |_, _, _| {})
                    .is_some()
            })
        });
        assert!(shown, "the card shows above the reply composer");

        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| {
                pane.link.remove();
                let text = pane.composer.read(cx).value(cx);
                pane.reply(text, window, cx);
            })
        });
        cx.run_until_parked();
        assert!(matches!(
            events.borrow().as_slice(),
            [RightPaneEvent::Reply { unfurl: false, .. }]
        ));
    }

    /// A reply whose attachment has an Approve button and a region menu.
    fn interactive_reply() -> Post {
        let api: matty_api::models::Post = serde_json::from_value(serde_json::json!({
            "id": "r1",
            "props": {"attachments": [{
                "text": "Deploy?",
                "actions": [
                    {"id": "go", "type": "button", "name": "Approve", "cookie": "k1"},
                    {"id": "where", "type": "select", "name": "Region",
                     "options": [{"text": "EU", "value": "eu"}]}
                ]
            }]}
        }))
        .unwrap();
        Post {
            embeds: crate::embeds::from_post(&api),
            ..post("r1")
        }
    }

    /// #211: a thread reply draws its attachment's buttons, and a press
    /// asks the window to run the action, as in the channel; offline it is
    /// refused first.
    #[gpui_kit::test]
    fn a_button_in_the_thread_runs_its_action(cx: &mut gpui_kit::TestAppContext) {
        let (view, events, mut cx) = pane(cx);
        cx.update(|_, cx| {
            view.update(cx, |pane, cx| {
                let mut thread = thread(0..0, false, false);
                thread.replies = vec![interactive_reply()];
                pane.set_thread(thread, cx);
            })
        });
        draw(&mut cx);
        draw(&mut cx);
        let press = |cx: &mut gpui_kit::VisualTestContext| {
            let bounds = cx
                .debug_bounds("post-action-go")
                .expect("the thread draws the button");
            cx.simulate_click(bounds.center(), Default::default());
        };
        press(&mut cx);
        assert_eq!(
            *events.borrow(),
            [RightPaneEvent::PostAction(
                crate::interactive::ActionChoice {
                    post_id: "r1".into(),
                    action_id: "go".into(),
                    selected: None,
                    cookie: "k1".into(),
                }
            )]
        );
        events.borrow_mut().clear();
        cx.update(|_, cx| view.update(cx, |pane, cx| pane.set_offline(true, cx)));
        draw(&mut cx);
        press(&mut cx);
        assert_eq!(*events.borrow(), [RightPaneEvent::Offline("run action")]);
    }

    /// #211: dismissing an ephemeral reply drops it here and tells the
    /// window.
    #[gpui_kit::test]
    fn an_ephemeral_reply_can_be_dismissed(cx: &mut gpui_kit::TestAppContext) {
        let (view, events, mut cx) = pane(cx);
        cx.update(|_, cx| {
            view.update(cx, |pane, cx| {
                let mut thread = thread(0..1, false, false);
                thread.replies.push(Post {
                    status: PostStatus::Ephemeral,
                    ..post("e1")
                });
                pane.set_thread(thread, cx);
                pane.dismiss("e1".into(), cx);
            })
        });
        draw(&mut cx);
        let replies = view.read_with(&cx, |pane, _| match pane.content() {
            Some(RightPaneContent::Thread(thread)) => thread.replies.len(),
            _ => 0,
        });
        assert_eq!(replies, 1);
        assert_eq!(
            *events.borrow(),
            [RightPaneEvent::DismissEphemeral("e1".into())]
        );
    }
}
