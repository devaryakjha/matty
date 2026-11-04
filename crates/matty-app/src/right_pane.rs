//! The right pane (DESIGN.md §4): threads first, later search results,
//! saved, pinned, mentions and channel info. It shares the headers' 48px
//! height, separates from the channel with a hairline, and closes on
//! Escape.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::actions::{self, CloseRightPane};
use crate::attachments::post_files;
use crate::channel::{
    ChannelModel, Post, PostStatus, QUICK_REACTIONS, message_density, plain_text, post_markers,
    reaction_tooltip,
};
use crate::profile::{ProfileActions, UserProfile, profile_card};
use crate::settings::{ClockFormat, SendKey, SettingsChanged, SettingsStore};
use crate::sidebar::{ChannelKind, Presence};
use crate::style::{self, Palette, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    alert_dialog::{alert_dialog, alert_dialog_action, alert_dialog_cancel},
    avatar::{Avatar, AvatarSize},
    button::{Button, ButtonSize, ButtonVariant},
    composer::{Composer, ComposerEvent, ComposerState},
    dialog::{DialogHandle, dialog_description, dialog_popup, dialog_title},
    input::{Input, InputEvent, InputState},
    lightbox::{LightboxImage, LightboxState, lightbox},
    markdown::markdown,
    menu::{Menu, MenuItem, MenuState},
    message::{Message, MessageHighlight, MessageStatus, Reaction},
    message_list::{MessageList, MessageListState},
    popover::popover,
    theme::UiTheme,
};

/// A thread: its root post and the replies loaded for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadModel {
    pub channel_name: SharedString,
    pub root: Post,
    pub replies: Vec<Post>,
    pub following: bool,
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
    Thread(ThreadModel),
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
    },
    /// Retry a reply that failed; the pane shows it pending again.
    Retry(SharedString),
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
}

pub struct RightPaneView {
    content: Option<RightPaneContent>,
    list: Entity<MessageListState>,
    composer: Entity<ComposerState>,
    focus: FocusHandle,
    /// Focus of the thread transcript while a row is keyboard-selected.
    thread_focus: FocusHandle,
    /// The keyboard-selected row of the thread transcript (PLAN §4.2).
    selection: Option<usize>,
    /// The shared right-click menu over thread rows.
    menu: Entity<MenuState>,
    /// The current user's display name, for optimistic rows.
    me: SharedString,
    draft_len: usize,
    pending_seq: usize,
    /// The search field at the top of the Search listing.
    search: Entity<InputState>,
    /// The image viewer over a post's images.
    lightbox: Entity<LightboxState>,
    lightbox_handle: DialogHandle,
    lightbox_files: Vec<SharedString>,
    /// The "Leave channel?" confirmation.
    leave: DialogHandle,
    _subscriptions: Vec<gpui_kit::Subscription>,
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
                this.composer.update(cx, |composer, _| {
                    composer.set_submit_on_enter(send_on == SendKey::Enter)
                });
                this.list.update(cx, |list, cx| list.remeasure(cx));
                cx.notify();
            },
        )
        .detach();
        let send_on = SettingsStore::global(cx).read(cx).get().send_on;
        let list = cx.new(|cx| MessageListState::new(0, cx));
        let keys = cx.weak_entity();
        let composer = cx.new(|cx| {
            let mut state = ComposerState::new(6, window, cx);
            state.set_placeholder("Reply…", window, cx);
            state.set_submit_on_enter(send_on == SendKey::Enter);
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
            state
        });
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search messages"));
        let subscriptions = vec![
            cx.subscribe_in(
                &composer,
                window,
                |this, composer, event: &ComposerEvent, window, cx| match event {
                    ComposerEvent::Change => {
                        this.draft_len = composer.read(cx).value(cx).chars().count();
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
            content: None,
            list,
            composer,
            focus: cx.focus_handle(),
            thread_focus: cx.focus_handle(),
            selection: None,
            menu: cx.new(|cx| MenuState::new([], cx)),
            me: me.into(),
            draft_len: 0,
            pending_seq: 0,
            search,
            lightbox: cx.new(|_| LightboxState::new([])),
            lightbox_handle: DialogHandle::new(false),
            lightbox_files: Vec::new(),
            leave: DialogHandle::new(false),
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

    /// Appends the text as a pending reply by the current user, clears the
    /// composer and tells sync. Blank text stays in the composer.
    fn reply(&mut self, text: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let body: SharedString = text.trim().to_string().into();
        if body.is_empty() {
            return;
        }
        self.pending_seq += 1;
        let (me, seq) = (self.me.clone(), self.pending_seq);
        let Some(thread) = self.thread_mut() else {
            return;
        };
        let root = thread.root.id.clone();
        let last = thread.replies.last().unwrap_or(&thread.root);
        let reply = Post {
            id: format!("{root}-pending-{seq}").into(),
            author: me.clone(),
            time: last.time,
            day: last.day.clone(),
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
            files: vec![],
        };
        thread.replies.push(reply);
        let content = self.content.clone();
        self.set_content(content, cx);
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.draft_len = 0;
        cx.emit(RightPaneEvent::Reply { root, text: body });
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

    /// Removes a reply that never reached the server.
    fn discard(&mut self, id: SharedString, cx: &mut Context<Self>) {
        let Some(thread) = self.thread_mut() else {
            return;
        };
        thread.replies.retain(|post| post.id != id);
        let content = self.content.clone();
        self.set_content(content, cx);
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
            // Root, the replies divider, then replies.
            Some(RightPaneContent::Thread(thread)) => thread.replies.len() + 2,
            _ => 0,
        };
        self.list = cx.new(|cx| MessageListState::new(rows, cx));
        self.content = content;
        self.selection = None;
        cx.notify();
    }

    /// Rows of the thread transcript: the root, the divider, then replies.
    fn thread_rows(&self) -> usize {
        match &self.content {
            Some(RightPaneContent::Thread(thread)) => thread.replies.len() + 2,
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
        self.selection = Some(if rows > 2 { rows - 1 } else { 0 });
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
                .map(|index| index + 2)
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

    /// The right-click menu of a thread row: copy, a reaction submenu, and
    /// Retry and Delete on a reply that never reached the server.
    fn thread_items_for(view: &gpui_kit::WeakEntity<Self>, post: &Post) -> Vec<MenuItem> {
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
        let mut items = vec![
            MenuItem::submenu("thread.context.react", "Add reaction", reactions),
            MenuItem::separator(),
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
        ];
        if post.status == PostStatus::Failed {
            let (retry_view, retry_id) = (view.clone(), post.id.clone());
            let (delete_view, delete_id) = (view.clone(), post.id.clone());
            items.push(MenuItem::separator());
            items.push(
                MenuItem::new("thread.context.retry", "Retry").on_click(move |_, _, cx| {
                    let id = retry_id.clone();
                    retry_view.update(cx, |this, cx| this.retry(id, cx)).ok();
                }),
            );
            items.push(MenuItem::new("thread.context.delete", "Delete").on_click(
                move |_, _, cx| {
                    let id = delete_id.clone();
                    delete_view.update(cx, |this, cx| this.discard(id, cx)).ok();
                },
            ));
        }
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
        cx: &mut App,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let time: SharedString = match post.status {
            PostStatus::Sent => ClockFormat::current(cx).format(post.time).into(),
            PostStatus::Pending => "Sending…".into(),
            PostStatus::Failed => "Not sent".into(),
        };
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
            .gutter_time(time)
            .continuation(post.continues && !root)
            .density(message_density(cx))
            .highlight(highlight)
            .avatar(
                Avatar::new(gpui_kit::ElementId::Name(
                    format!("thread.{id}.avatar").into(),
                ))
                .size(AvatarSize::Default)
                .fallback(initial)
                .aria_label(post.author.clone()),
            )
            .reactions(post.reactions.iter().map(|reaction| {
                Reaction::new(reaction.emoji.clone(), reaction.count, reaction.mine)
            }))
            .reaction_tooltip(reaction_tooltip(post.reactions.clone()))
            .px(theme.space(4.))
            .child(markdown(
                gpui_kit::ElementId::Name(format!("thread.{id}.body").into()),
                post.body.clone(),
                cx,
            ))
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
        for marker in post_markers(post, &theme) {
            row = row.header_child(marker);
        }
        // Right-click selects the row and opens its menu (design-42).
        let items = Self::thread_items_for(view, post);
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
        let count = thread.replies.len();
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
                    .child(if thread.following {
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
        let transcript =
            MessageList::new(
                "right-pane.thread",
                &self.list,
                move |index, _, cx| match index {
                    0 => Self::post_row(&view, &model.root, true, highlight(0), cx)
                        .into_any_element(),
                    1 => Self::replies_divider(&view, &model, cx).into_any_element(),
                    index => match model.replies.get(index - 2) {
                        Some(reply) => Self::post_row(&view, reply, false, highlight(index), cx)
                            .into_any_element(),
                        None => div().into_any_element(),
                    },
                },
            )
            .size_full()
            .pt(theme.space(2.))
            .track_focus(&self.thread_focus)
            .selected(self.selection)
            .selectable(|index, _| index != 1)
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
            .disabled(self.draft_len == 0)
            .on_click(cx.listener(|this, _, window, cx| {
                let text = this.composer.read(cx).value(cx);
                this.reply(text, window, cx)
            }))
            .child(
                lucide(LucideIcon::SendHorizontal)
                    .size(scaled(&theme, metrics::ICON))
                    .text_color(theme.colors.primary_foreground),
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
            .flex()
            .flex_col()
            .child(div().flex_1().min_h_0().child(transcript))
            .child(
                div()
                    .flex_shrink_0()
                    .px(theme.space(4.))
                    .pt(theme.space(1.))
                    .pb(theme.space(4.))
                    .child(
                        Composer::new(&self.composer)
                            .rounded(theme.radius.lg)
                            .child(style::tipped(
                                "right-pane.attach.tip",
                                actions::tooltip_text("Attach files", "Upload files"),
                                tool("right-pane.attach", "Attach files", LucideIcon::Plus),
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
                                .child("Jump")
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
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space(1.5))
                        .child(lucide(glyph).size(scaled(&theme, 14.)).text_color(if on {
                            theme.colors.foreground
                        } else {
                            muted
                        }))
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
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme.space(1.5))
                                .text_color(theme.colors.destructive)
                                .child(
                                    lucide(LucideIcon::LogOut)
                                        .size(scaled(&theme, 14.))
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
        .trigger_with(move |_, _, _| {
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
                        .child(
                            Avatar::new(gpui_kit::ElementId::Name(
                                format!("info.member.{}.avatar", row_profile.username).into(),
                            ))
                            .size(AvatarSize::Sm)
                            .fallback(initial.clone())
                            .aria_label(row_profile.display_name.clone()),
                        )
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
        let theme = UiTheme::read(cx).clone();
        let root = div()
            .child(self.lightbox(window, cx))
            .child(self.leave_dialog(window, cx))
            .child(Menu::new(&self.menu, "Message actions").context_area(div()))
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
