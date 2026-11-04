//! The right pane (DESIGN.md §4): threads first, later search results,
//! saved, pinned, mentions and channel info. It shares the headers' 48px
//! height, separates from the channel with a hairline, and closes on
//! Escape.

use gpui_icons::{LucideIcon, lucide};
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
    ChannelModel, Post, PostKind, PostStatus, QUICK_REACTIONS, delete_popup, edit_composer,
    message_density, plain_text, post_editor, post_markers, post_time, queued, queued_marker,
    reaction_tooltip, waiting_items, waiting_to_send,
};
use crate::profile::{ProfileActions, UserProfile, profile_card};
use crate::settings::{ClockFormat, SettingsChanged, SettingsStore};
use crate::sidebar::{ChannelKind, Presence};
use crate::style::{self, Palette, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    alert_dialog::{alert_dialog, alert_dialog_action, alert_dialog_cancel},
    attachment::Attachment,
    avatar::{Avatar, AvatarSize},
    button::{Button, ButtonSize, ButtonVariant},
    composer::{Composer, ComposerEvent, ComposerState},
    dialog::{DialogHandle, dialog_description, dialog_popup, dialog_title},
    input::{Input, InputEvent, InputState},
    lightbox::{LightboxImage, LightboxState, lightbox},
    menu::{Menu, MenuItem, MenuState},
    message::{Message, MessageHighlight, MessageStatus, Reaction},
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

/// A message search and what came back (PLAN §3 M1 "Search").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchModel {
    pub query: SharedString,
    pub results: Vec<ListedPost>,
    /// True while sync runs the query.
    pub searching: bool,
}

/// What the pane shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RightPaneContent {
    Thread(Box<ThreadModel>),
    ChannelInfo(ChannelModel),
    /// The current channel's pinned posts, newest first.
    Pinned {
        channel_name: SharedString,
        posts: Vec<ListedPost>,
    },
    /// The user's saved posts across channels, newest first.
    Saved(Vec<ListedPost>),
    /// Posts that mention the user, newest first.
    Mentions(Vec<ListedPost>),
    Search(SearchModel),
}

/// What the pane asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RightPaneEvent {
    Close,
    /// The user submitted a message search; sync runs it and calls
    /// `set_search` with the results.
    Search(SharedString),
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
    /// The user clicked a link or an entity chip in a message (#103).
    OpenLink(SharedString),
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
}

/// Rows of the thread transcript before the replies: the root, the
/// replies divider, and the row that pages in older replies (#122).
const THREAD_HEAD: usize = 3;

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
    draft_len: usize,
    /// The preview above the reply composer (#188).
    link: crate::draft_link::DraftLink,
    /// Files waiting to go with the reply, and a reply waiting on them.
    tray: UploadTray,
    _link_task: Option<gpui_kit::Task<()>>,
    pending_seq: usize,
    /// The search field at the top of the Search listing.
    search: Entity<InputState>,
    /// The image viewer over a post's images.
    lightbox: Entity<LightboxState>,
    lightbox_handle: DialogHandle,
    lightbox_files: Vec<SharedString>,
    /// The "Leave channel?" confirmation.
    leave: DialogHandle,
    /// The connection is down, so pending replies wait for it (#195).
    away: bool,
    /// The reply being edited in place, waiting to send (#195).
    editing: Option<ReplyEdit>,
    /// "Discard this message?" and the reply it is about.
    discard: DialogHandle,
    discarding: Option<SharedString>,
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
            state.set_placeholder("Reply…", window, cx);
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
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search messages"));
        let subscriptions = vec![
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
                        cx.notify();
                    }
                    ComposerEvent::Submit(text) => this.reply(text.clone(), window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(&search, window, |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.submit_search(cx);
                }
            }),
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
            draft_len: 0,
            link: Default::default(),
            tray: UploadTray::new(UPLOAD_PREFIX),
            _link_task: None,
            pending_seq: 0,
            search,
            lightbox: cx.new(|_| LightboxState::new([])),
            lightbox_handle: DialogHandle::new(false),
            lightbox_files: Vec::new(),
            leave: DialogHandle::new(false),
            away: crate::preview::connection() != crate::main_window::ConnectionState::Online,
            editing: None,
            discard: DialogHandle::new(false),
            discarding: None,
            _subscriptions: subscriptions,
        }
    }

    fn mutate_info(&mut self, change: impl FnOnce(&mut ChannelModel), cx: &mut Context<Self>) {
        if let Some(RightPaneContent::ChannelInfo(channel)) = self.content.as_mut() {
            change(channel);
            cx.notify();
        }
    }

    /// Opens the lightbox over `post`'s images at image `index`.
    fn open_lightbox(
        &mut self,
        post: &Post,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let caption: SharedString = format!(
            "{} · {}",
            post.author,
            ClockFormat::current(cx).format(post.time)
        )
        .into();
        let mut ids = Vec::new();
        let images: Vec<LightboxImage> = post
            .files
            .iter()
            .filter_map(|file| {
                let image = file.image()?;
                ids.push(file.id.clone());
                Some(LightboxImage::new(image, file.name.clone()).caption(caption.clone()))
            })
            .collect();
        if images.is_empty() {
            return;
        }
        self.lightbox_files = ids.clone();
        self.lightbox
            .update(cx, |lightbox, cx| lightbox.set_images(images, index, cx));
        self.lightbox_handle.open(window, cx);
        cx.emit(RightPaneEvent::ViewImages(ids));
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
        let Some(position) = self.lightbox_files.iter().position(|id| id == file_id) else {
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
            .aria_label("Open file")
            .child(
                lucide(LucideIcon::Download)
                    .size(theme.space(4.5))
                    .text_color(gpui_kit::white()),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                let index = this.lightbox.read(cx).index();
                if let Some(id) = this.lightbox_files.get(index) {
                    cx.emit(RightPaneEvent::OpenFile(id.clone()));
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
        };
        thread.replies.push(reply);
        let content = self.content.clone();
        self.set_content(content, cx);
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
        if self.tray.set_status(id, status) {
            self.send_queued(window, cx);
        }
        cx.notify();
    }

    fn remove_upload(&mut self, id: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let due = self.tray.remove(id);
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
        cx.emit(RightPaneEvent::Discard(id));
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

    /// Opens "Discard this message?" for a reply waiting to send.
    fn ask_discard(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.discarding = Some(id);
        self.discard.open(window, cx);
        cx.notify();
    }

    fn reply_post(&self, id: &SharedString) -> Option<&Post> {
        match self.content.as_ref()? {
            RightPaneContent::Thread(thread) => thread.replies.iter().find(|post| &post.id == id),
            _ => None,
        }
    }

    /// Turns a reply waiting to send into a composer holding its text, as
    /// in the channel (#195). Enter saves, Escape cancels.
    fn start_edit(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(post) = self.reply_post(&id).filter(|post| {
            post.mine && post.kind == PostKind::Regular && waiting_to_send(post, self.away)
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

    /// Keeps the new text and tells sync; an unchanged or emptied body just
    /// closes the editor, since discarding has its own confirmation.
    fn save_edit(&mut self, text: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = self.editing.as_ref() else {
            return;
        };
        let id = editing.post.clone();
        let body: SharedString = text.trim().to_string().into();
        let unchanged = self.reply_post(&id).is_none_or(|post| post.body == body);
        if body.is_empty() || unchanged {
            self.cancel_edit(window, cx);
            return;
        }
        // A waiting send just goes with the new text, without "(edited)".
        self.mutate_reply(&id, |post| post.body = body.clone(), cx);
        cx.emit(RightPaneEvent::SaveEdit {
            post: id,
            text: body,
        });
        self.cancel_edit(window, cx);
    }

    fn discard_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let popup = delete_popup("thread.discard", true, "the thread", cx);
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
                }
            },
            cx,
        );
        if changed {
            cx.emit(RightPaneEvent::ToggleReaction { post: id, emoji });
        }
    }

    fn toggle_follow(&mut self, cx: &mut Context<Self>) {
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
    /// Shows a search: the field takes the query and focus, the listing the
    /// results. Sync calls this again when results arrive.
    pub fn set_search(&mut self, model: SearchModel, window: &mut Window, cx: &mut Context<Self>) {
        let focus = model.query.is_empty() || model.searching;
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
            // The files were for the other thread's channel.
            for upload in self.tray.take() {
                cx.emit(RightPaneEvent::CancelUpload(upload.id));
            }
            self.editing = None;
        }
        self.content = content;
        self.selection = None;
        self.loading_older = false;
        cx.notify();
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
        cx.notify();
    }

    /// Asks for the replies before the oldest loaded one when the row
    /// above the replies shows and nothing stops it.
    fn request_older(&mut self, cx: &mut Context<Self>) {
        let Some(RightPaneContent::Thread(thread)) = &self.content else {
            return;
        };
        if !thread.older || thread.failed_older || self.loading_older {
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
        if thread.failed_older {
            row.child("Couldn't load older replies.")
                .child(
                    div()
                        .debug_selector(|| "right-pane.older.retry".into())
                        .child(
                            Button::new("right-pane.older.retry")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Sm)
                                .label("Retry")
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
                    .label("Loading older replies")
                    .size(gpui_kit::px(14.)),
            )
            .child("Loading older replies…")
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

    /// The right-click menu of a thread row: a reaction submenu and copy;
    /// on a reply the server does not have yet, copy, Retry if it failed,
    /// and the channel's Edit and Discard… while it waits to send (#195).
    fn thread_items_for(
        view: &gpui_kit::WeakEntity<Self>,
        post: &Post,
        away: bool,
    ) -> Vec<MenuItem> {
        let body = post.body.clone();
        let reactions = QUICK_REACTIONS.iter().map(|(emoji, name)| {
            let (view, id) = (view.clone(), post.id.clone());
            let emoji: SharedString = (*emoji).into();
            MenuItem::new(
                gpui_kit::ElementId::Name(format!("thread.context.react.{name}").into()),
                format!("{emoji}  {name}"),
            )
            .checked(post.reactions.iter().any(|r| r.emoji == emoji && r.mine))
            .on_click(move |_, _, cx| {
                let (id, emoji) = (id.clone(), emoji.clone());
                view.update(cx, |this, cx| this.toggle_reaction(id, emoji, cx))
                    .ok();
            })
        });
        let markdown_body = post.body.clone();
        // The server does not have an unsent reply yet: nothing to react to.
        let mut items = if post.status == PostStatus::Sent {
            vec![
                MenuItem::submenu("thread.context.react", "Add reaction", reactions),
                MenuItem::separator(),
            ]
        } else {
            Vec::new()
        };
        items.extend([
            MenuItem::new("thread.context.copy", "Copy text").on_click(move |_, _, cx| {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(plain_text(&body)))
            }),
            MenuItem::new("thread.context.copy-markdown", "Copy as Markdown").on_click(
                move |_, _, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                        markdown_body.to_string(),
                    ))
                },
            ),
        ]);
        if post.status == PostStatus::Failed {
            let (retry_view, retry_id) = (view.clone(), post.id.clone());
            items.push(MenuItem::separator());
            items.push(
                MenuItem::new("thread.context.retry", "Retry").on_click(move |_, _, cx| {
                    let id = retry_id.clone();
                    retry_view.update(cx, |this, cx| this.retry(id, cx)).ok();
                }),
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
        let action = |id: &'static str, label: &'static str, glyph: LucideIcon| {
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
                    .child(style::tipped(
                        "right-pane.expand.tip",
                        actions::tooltip_text("Expand", "Expand right pane"),
                        action("right-pane.expand", "Expand", LucideIcon::Maximize2),
                    ))
                    .child(style::tipped(
                        "right-pane.close.tip",
                        actions::tooltip_text("Close", "Close right pane"),
                        action("right-pane.close", "Close", LucideIcon::X)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(RightPaneEvent::Close))),
                    )),
            )
    }

    fn post_row(
        view: &gpui_kit::WeakEntity<Self>,
        post: &Post,
        root: bool,
        highlight: MessageHighlight,
        (away, editor): (bool, Option<&ReplyEditor>),
        cx: &mut App,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let time = post_time(post, away, cx);
        let id = post.id.clone();
        let initial: String = post.author.chars().take(1).collect();
        let (retry_view, retry_id) = (view.clone(), id.clone());
        let (delete_view, delete_id) = (view.clone(), id.clone());
        let (react_view, react_id) = (view.clone(), id.clone());
        let mut row = Message::new(gpui_kit::ElementId::Name(format!("thread.{id}").into()))
            .status(match post.status {
                PostStatus::Sent => MessageStatus::Sent,
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
                |row| row.timestamp_tooltip(format!("{} at {time}", post.date)),
            )
            .gutter_time(time)
            // A queued reply keeps its header so the clock shows (#195).
            .continuation(post.continues && !root && !queued(post, away))
            .density(message_density(cx))
            .highlight(highlight)
            .avatar(crate::avatars::with_picture(
                Avatar::new(gpui_kit::ElementId::Name(
                    format!("thread.{id}.avatar").into(),
                ))
                .size(AvatarSize::Default)
                .fallback(initial)
                .aria_label(post.author.clone()),
                &post.author_id,
                cx,
            ))
            .reactions(post.reactions.iter().map(|reaction| {
                Reaction::new(reaction.emoji.clone(), reaction.count, reaction.mine)
            }))
            .reaction_tooltip(reaction_tooltip(post.reactions.clone()))
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
                        move |url, _, cx| {
                            let event = RightPaneEvent::OpenLink(url.clone());
                            link_view.update(cx, |_, cx| cx.emit(event)).ok();
                        },
                        cx,
                    )
                    .into_any_element()
                }
            })
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
        for marker in post_markers(post, &theme) {
            row = row.header_child(marker);
        }
        // Right-click selects the row and opens its menu (design-42).
        let items = Self::thread_items_for(view, post, away);
        let menu_view = view.clone();
        div()
            .on_mouse_down(
                gpui_kit::MouseButton::Right,
                move |event: &gpui_kit::MouseDownEvent, window, cx| {
                    let items = items.clone();
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

    fn replies_divider(
        view: &gpui_kit::WeakEntity<Self>,
        thread: &ThreadModel,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let view = view.clone();
        // Older replies may not be loaded yet; the server's count holds.
        let count = thread.reply_count.max(thread.replies.len());
        let label = match count {
            1 => "1 reply".to_string(),
            n => format!("{n} replies"),
        };
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
                        "Following"
                    } else {
                        "Follow"
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
        let highlight = move |index| {
            if selection == Some(index) {
                MessageHighlight::Selected
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
        let transcript =
            MessageList::new(
                "right-pane.thread",
                &self.list,
                move |index, _, cx| match index {
                    0 => Self::post_row(&view, &model.root, true, highlight(0), (away, None), cx)
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
                                highlight(index),
                                (away, editor),
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
            });
        let send_on = SettingsStore::global(cx).read(cx).get().send_on;
        let send = Button::new("right-pane.send")
            .variant(ButtonVariant::Default)
            .size(ButtonSize::IconSm)
            .aria_label("Reply")
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
            "Files go into your reply.",
            crate::attachments::preview_drop(),
            cx,
        );
        let tool = |id: &'static str, label: &'static str, glyph: LucideIcon| {
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
                    .pb(theme.space(4.))
                    .children(self.link.preview(
                        "right-pane.link-preview",
                        cx,
                        cx.listener(|this, _, _, cx| {
                            this.link.remove();
                            cx.notify();
                        }),
                    ))
                    .children(tray)
                    .child(
                        Composer::new(&self.composer)
                            .max_height(self.composer_cap)
                            .rounded(theme.radius.lg)
                            .child(style::tipped(
                                "right-pane.attach.tip",
                                actions::tooltip_text("Attach files", "Upload files"),
                                tool("right-pane.attach", "Attach files", LucideIcon::Plus)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.choose_files(window, cx)
                                    })),
                            ))
                            .child(style::tipped(
                                "right-pane.emoji.tip",
                                "Emoji",
                                tool("right-pane.emoji", "Emoji", LucideIcon::FaceSlightlySmiling)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.composer.update(cx, |composer, cx| {
                                            composer.insert(":", window, cx);
                                            composer.focus(window, cx);
                                        })
                                    })),
                            ))
                            .child(div().flex_1())
                            .child(style::tipped(
                                "right-pane.send.tip",
                                actions::send_tooltip("Reply", send_on),
                                send,
                            )),
                    ),
            )
            .children(overlay)
    }

    /// Nothing open: one line on how to get a thread here.
    fn empty(cx: &App) -> impl IntoElement + use<> {
        style::centered_empty(
            LucideIcon::MessageSquareReply,
            "No thread open",
            "Hover a message and choose Reply in thread to follow it here.",
            cx,
        )
    }

    /// Saved, pinned and mention listings: each post under a caption with
    /// its channel and a Jump link, separated by hairlines.
    fn listing(
        &self,
        id: &'static str,
        items: &[ListedPost],
        empty: (LucideIcon, &'static str, &'static str),
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        if items.is_empty() {
            return style::centered_empty(empty.0, empty.1, empty.2, cx).into_any_element();
        }
        let theme = UiTheme::read(cx).clone();
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
                                .label("Jump")
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
                        (false, None),
                        cx,
                    ))
            }))
            .into_any_element()
    }

    /// The Search listing: the field, a hint about modifiers, then the
    /// results under a count, or the state the search is in.
    fn search(&self, model: &SearchModel, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let muted = theme.colors.muted_foreground;
        let body = if model.query.is_empty() {
            style::centered_empty(
                LucideIcon::Search,
                "Search messages",
                "Words, \u{201c}exact phrases\u{201d}, from:someone and in:channel.",
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
                .child(format!(
                    "Searching for \u{201c}{}\u{201d}\u{2026}",
                    model.query
                ))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .when(!model.results.is_empty(), |body| {
                    body.child(
                        div()
                            .px(theme.space(4.))
                            .pt(theme.space(2.))
                            .type_style(style::META, &theme)
                            .text_color(muted)
                            .child(match model.results.len() {
                                1 => format!("1 result for \u{201c}{}\u{201d}", model.query),
                                n => format!("{n} results for \u{201c}{}\u{201d}", model.query),
                            }),
                    )
                })
                .child(self.listing(
                    "right-pane.search",
                    &model.results,
                    (
                        LucideIcon::SearchX,
                        "Nothing matched",
                        "Try fewer words, or narrow with from: and in:.",
                    ),
                    cx,
                ))
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
                    .child(Input::new(&self.search).aria_label("Search messages"))
                    .child(
                        div()
                            .type_style(style::META, &theme)
                            .text_color(muted)
                            .child("from:name  in:channel  \u{201c}exact phrase\u{201d}"),
                    ),
            )
            .child(body)
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
                    if favorite { "Favorited" } else { "Favorite" }.into(),
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
                    if is_muted { "Muted" } else { "Mute" }.into(),
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
                    format!("{} pinned", channel.pinned).into(),
                    LucideIcon::Pin,
                    false,
                )
                .on_click(cx.listener(|_, _, _, cx| cx.emit(RightPaneEvent::OpenPinned))),
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
                        .aria_label("Leave channel")
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
                                .child("Leave"),
                        )
                        .on_click(move |_, window, cx| leave.open(window, cx)),
                )
            });
        let view = cx.entity().downgrade();
        let members = div()
            .flex()
            .flex_col()
            .gap(theme.space(1.))
            .child(label(format!("Members · {}", channel.members)).pb(theme.space(1.)))
            .children(
                channel
                    .profiles
                    .iter()
                    .map(|profile| Self::member_row(profile, &view, cx)),
            )
            .when(channel.profiles.len() < channel.members, |members| {
                members.child(
                    Button::new("info.members.all")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Sm)
                        .label(format!("Show all {} members", channel.members))
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
    fn member_row(
        profile: &UserProfile,
        view: &gpui_kit::WeakEntity<Self>,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let palette = Palette::read(cx);
        let dot = match profile.presence {
            Presence::Online => palette.online,
            Presence::Away => palette.away,
            Presence::Dnd => palette.dnd,
            Presence::Offline => theme.colors.muted_foreground,
        };
        let initial: String = profile.display_name.chars().take(1).collect();
        let username = profile.username.clone();
        let (row_profile, card_profile) = (profile.clone(), profile.clone());
        let (message_view, mention_view) = (view.clone(), view.clone());
        popover(gpui_kit::ElementId::Name(
            format!("info.member.{username}.popover").into(),
        ))
        .offset(px(4.))
        .trigger_with(move |_, _, cx| {
            div()
                .id(gpui_kit::ElementId::Name(
                    format!("info.member.{}", row_profile.username).into(),
                ))
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
                            .aria_label(row_profile.display_name.clone()),
                            &row_profile.user_id,
                            cx,
                        ))
                        .child(
                            div()
                                .absolute()
                                .bottom(px(-1.))
                                .right(px(-1.))
                                .size(px(9.))
                                .rounded_full()
                                .bg(dot)
                                .border_2()
                                .border_color(theme.colors.background),
                        ),
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
                .into_any_element()
        })
        .content(move |_, _, cx| {
            let (message_profile, mention_profile) = (card_profile.clone(), card_profile.clone());
            let (message_view, mention_view) = (message_view.clone(), mention_view.clone());
            let (close_message, close_mention) = (cx.entity(), cx.entity());
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
            _ => "this channel".into(),
        };
        let popup = dialog_popup("channel.leave.popup", "Leave channel", cx)
            .child(dialog_title("channel.leave.title", cx).child(format!("Leave {name}?")))
            .child(
                dialog_description("channel.leave.description", cx).child(
                    "You stop receiving its messages and it leaves your sidebar. You can join again later.",
                ),
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
                            .label("Cancel"),
                    )
                    .child(
                        alert_dialog_action("channel.leave.confirm")
                            .variant(ButtonVariant::Destructive)
                            .label("Leave"),
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
            .child(
                Menu::new(&self.menu, "Message actions")
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
        match self.content.clone() {
            Some(RightPaneContent::Thread(thread)) => root
                .child(self.header(
                    "Thread".into(),
                    Some(match thread.channel_name.as_ref() {
                        name if name.contains(' ') => name.to_string().into(),
                        name => format!("#{name}").into(),
                    }),
                    cx,
                ))
                .child(div().flex_1().min_h_0().child(self.thread(&thread, cx))),
            Some(RightPaneContent::ChannelInfo(channel)) => root
                .child(self.header("Channel info".into(), None, cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(self.channel_info(&channel, cx)),
                ),
            Some(RightPaneContent::Pinned {
                channel_name,
                posts,
            }) => root
                .child(self.header(
                    "Pinned messages".into(),
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
                        "Nothing pinned yet",
                        "Pin a message from its More menu to keep it here for everyone.",
                    ),
                    cx,
                )),
            Some(RightPaneContent::Saved(posts)) => root
                .child(self.header("Saved messages".into(), None, cx))
                .child(self.listing(
                    "right-pane.saved",
                    &posts,
                    (
                        LucideIcon::Bookmark,
                        "Nothing saved yet",
                        "Save a message from its hover toolbar to find it here later.",
                    ),
                    cx,
                )),
            Some(RightPaneContent::Mentions(posts)) => root
                .child(self.header("Recent mentions".into(), None, cx))
                .child(self.listing(
                    "right-pane.mentions",
                    &posts,
                    (
                        LucideIcon::AtSign,
                        "No mentions yet",
                        "Messages that mention you or your keywords show up here.",
                    ),
                    cx,
                )),
            Some(RightPaneContent::Search(model)) => root
                .child(self.header("Search".into(), None, cx))
                .child(self.search(&model, cx)),
            None => root
                .child(self.header("Thread".into(), None, cx))
                .child(Self::empty(cx)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn post(id: &str) -> Post {
        Post {
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
        }
    }

    /// The thread with replies `r{from}` to `r{to - 1}` of 500 loaded.
    fn thread(replies: std::ops::Range<usize>, older: bool, failed: bool) -> ThreadModel {
        ThreadModel {
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
}
