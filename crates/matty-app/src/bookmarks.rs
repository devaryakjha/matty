//! The bookmarks bar under a channel's header (#226): the channel's links
//! and files, as Mattermost 9.5 and later keep them.
//!
//! A link opens through Matty's link routing, so a Mattermost link stays
//! in Matty; a file opens through the download path. Right-click a
//! bookmark, or use the bar's More menu, to edit, move or delete it; Add
//! a bookmark asks for a link. The bar only shows on servers with
//! bookmarks, and sync keeps it in the store so it shows offline. What the
//! user changes leaves as [`BookmarkAsk`], which the window hands to
//! [`crate::live::Live::bookmark`]; the server's refusal comes back as a
//! toast and the bar returns to the server's list.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use matty_api::models::{BookmarkKind, ChannelBookmark};

use crate::style::{self, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    alert_dialog::{alert_dialog, alert_dialog_action, alert_dialog_cancel},
    button::{Button, ButtonSize, ButtonVariant},
    dialog::{DialogHandle, dialog, dialog_description, dialog_popup, dialog_title},
    field::Field,
    input::{InputEvent, InputState},
    menu::{Menu, MenuItem, MenuState},
    theme::UiTheme,
};

/// The widest a bookmark's name shows before it is cut short.
const MAX_CHIP_WIDTH: f32 = 200.;
/// Mattermost's limit on a bookmark's name.
const MAX_NAME: usize = 64;

/// What a bookmark opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Link(SharedString),
    File {
        file_id: SharedString,
        extension: SharedString,
    },
}

/// One bookmark as the bar shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bookmark {
    pub id: SharedString,
    pub name: SharedString,
    pub target: Target,
    /// An emoji name, shown instead of the icon.
    pub emoji: Option<SharedString>,
    /// The link's favicon, when the server found one.
    pub icon_url: Option<SharedString>,
}

impl Bookmark {
    fn link(&self) -> Option<&SharedString> {
        match &self.target {
            Target::Link(url) => Some(url),
            Target::File { .. } => None,
        }
    }
}

/// A channel's bookmarks bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BookmarksModel {
    pub channel_id: SharedString,
    pub bookmarks: Vec<Bookmark>,
    /// The user may add, edit, move and delete bookmarks here. The server
    /// has the last word.
    pub can_edit: bool,
}

/// The bar for `channel_id` from sync's bookmarks.
pub fn model(channel_id: &str, list: &[ChannelBookmark], can_edit: bool) -> BookmarksModel {
    let text =
        |value: &str| (!value.trim().is_empty()).then(|| SharedString::from(value.to_owned()));
    BookmarksModel {
        channel_id: channel_id.to_owned().into(),
        bookmarks: list
            .iter()
            .map(|bookmark| Bookmark {
                id: bookmark.id.clone().into(),
                name: text(&bookmark.display_name)
                    .or_else(|| text(&bookmark.link_url))
                    .unwrap_or_else(|| "Untitled".into()),
                target: match bookmark.kind {
                    BookmarkKind::Link => Target::Link(bookmark.link_url.clone().into()),
                    BookmarkKind::File => Target::File {
                        file_id: bookmark.file_id.clone().into(),
                        extension: bookmark
                            .file
                            .as_ref()
                            .map(|file| file.extension.clone())
                            .unwrap_or_default()
                            .into(),
                    },
                },
                emoji: text(&bookmark.emoji),
                icon_url: text(&bookmark.image_url),
            })
            .collect(),
        can_edit,
    }
}

/// A change the user asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BookmarkAsk {
    Add {
        channel_id: SharedString,
        name: SharedString,
        url: SharedString,
        emoji: SharedString,
    },
    Edit {
        channel_id: SharedString,
        bookmark_id: SharedString,
        name: SharedString,
        url: SharedString,
        emoji: SharedString,
    },
    Delete {
        channel_id: SharedString,
        bookmark_id: SharedString,
    },
    Move {
        channel_id: SharedString,
        bookmark_id: SharedString,
        index: usize,
    },
}

impl BookmarkAsk {
    /// The sync command for it.
    pub fn command(&self) -> matty_sync::Command {
        use matty_sync::BookmarkCommand;
        matty_sync::Command::Bookmark(match self {
            Self::Add {
                channel_id,
                name,
                url,
                emoji,
            } => BookmarkCommand::AddLink {
                channel_id: channel_id.to_string(),
                display_name: name.to_string(),
                link_url: url.to_string(),
                emoji: emoji.to_string(),
            },
            Self::Edit {
                channel_id,
                bookmark_id,
                name,
                url,
                emoji,
            } => BookmarkCommand::Edit {
                channel_id: channel_id.to_string(),
                bookmark_id: bookmark_id.to_string(),
                display_name: name.to_string(),
                link_url: url.to_string(),
                emoji: emoji.to_string(),
            },
            Self::Delete {
                channel_id,
                bookmark_id,
            } => BookmarkCommand::Delete {
                channel_id: channel_id.to_string(),
                bookmark_id: bookmark_id.to_string(),
            },
            Self::Move {
                channel_id,
                bookmark_id,
                index,
            } => BookmarkCommand::Move {
                channel_id: channel_id.to_string(),
                bookmark_id: bookmark_id.to_string(),
                index: *index,
            },
        })
    }
}

/// What the bar asks of the channel view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BarEvent {
    Ask(BookmarkAsk),
    OpenLink(SharedString),
    OpenFile(SharedString),
}

/// A link as typed, made whole: `example.com` becomes
/// `https://example.com`. `None` when it is no web link.
pub fn normalize_link(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.contains(char::is_whitespace) {
        return None;
    }
    let full = if text.contains("://") {
        text.to_owned()
    } else {
        format!("https://{text}")
    };
    let url = url::Url::parse(&full).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some_and(|h| !h.is_empty()))
        .then_some(full)
}

/// The add and edit dialog's body.
pub struct BookmarkEditor {
    /// The bookmark being edited, or `None` when adding.
    editing: Option<Bookmark>,
    name: Entity<InputState>,
    link: Entity<InputState>,
    emoji: Entity<InputState>,
    handle: DialogHandle,
    focus: bool,
    /// Save was pressed with a link that is not one.
    tried: bool,
    _subscriptions: Vec<gpui_kit::Subscription>,
}

/// What the editor decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorSave {
    pub bookmark_id: Option<SharedString>,
    pub name: SharedString,
    pub url: SharedString,
    pub emoji: SharedString,
}

impl EventEmitter<EditorSave> for BookmarkEditor {}

impl BookmarkEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let name = input("A short name", window, cx);
        let link = input("https://example.com", window, cx);
        let emoji = input("Emoji name, such as book", window, cx);
        let subscriptions = [&name, &link, &emoji]
            .into_iter()
            .map(|input| {
                cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.save(window, cx);
                    } else {
                        cx.notify();
                    }
                })
            })
            .collect();
        Self {
            editing: None,
            name,
            link,
            emoji,
            handle: DialogHandle::new(false),
            focus: false,
            tried: false,
            _subscriptions: subscriptions,
        }
    }

    pub fn handle(&self) -> &DialogHandle {
        &self.handle
    }

    pub fn title(&self) -> &'static str {
        if self.editing.is_some() {
            "Edit bookmark"
        } else {
            "Add a bookmark"
        }
    }

    /// Opens the dialog to add a link, or to edit `bookmark`.
    pub fn open(
        &mut self,
        bookmark: Option<Bookmark>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (name, link, emoji) = match &bookmark {
            Some(bookmark) => (
                bookmark.name.to_string(),
                bookmark.link().map(ToString::to_string).unwrap_or_default(),
                bookmark
                    .emoji
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            ),
            None => Default::default(),
        };
        self.name
            .update(cx, |input, cx| input.set_value(&name, window, cx));
        self.link
            .update(cx, |input, cx| input.set_value(&link, window, cx));
        self.emoji
            .update(cx, |input, cx| input.set_value(&emoji, window, cx));
        self.editing = bookmark;
        self.tried = false;
        self.focus = true;
        self.handle.open(window, cx);
        cx.notify();
    }

    fn is_file(&self) -> bool {
        self.editing
            .as_ref()
            .is_some_and(|bookmark| bookmark.link().is_none())
    }

    fn values(&self, cx: &App) -> (String, Option<String>, String) {
        let name = self.name.read(cx).value().trim().to_owned();
        let link = normalize_link(self.link.read(cx).value().as_ref());
        let emoji = self
            .emoji
            .read(cx)
            .value()
            .trim()
            .trim_matches(':')
            .to_owned();
        (name, link, emoji)
    }

    fn can_save(&self, cx: &App) -> bool {
        let (name, link, _) = self.values(cx);
        name.chars().count() <= MAX_NAME
            && (self.is_file() && !name.is_empty() || !self.is_file() && link.is_some())
    }

    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_save(cx) {
            self.tried = true;
            cx.notify();
            return;
        }
        let (name, link, emoji) = self.values(cx);
        let url = link.unwrap_or_default();
        // An empty name reads as the link's host, as the web app names it.
        let name = if name.is_empty() {
            crate::remote_images::host(&url).to_string()
        } else {
            name
        };
        self.handle.close(window, cx);
        cx.emit(EditorSave {
            bookmark_id: self.editing.as_ref().map(|bookmark| bookmark.id.clone()),
            name: name.into(),
            url: url.into(),
            emoji: emoji.into(),
        });
    }
}

impl Render for BookmarkEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        if self.focus {
            self.focus = false;
            let first = if self.is_file() {
                &self.name
            } else {
                &self.link
            };
            let focus = first.read(cx).focus_handle(cx);
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        let (name, link, _) = self.values(cx);
        let bad_link = self.tried && link.is_none() && !self.is_file();
        let mut link_field = Field::new("bookmark.edit.link", &self.link).label("Link");
        if bad_link {
            link_field = link_field.error("Enter a web address, such as example.com.");
        }
        let mut name_field = Field::new("bookmark.edit.name", &self.name).label("Name");
        name_field = if name.chars().count() > MAX_NAME {
            name_field.error(format!("At most {MAX_NAME} characters."))
        } else if self.is_file() {
            name_field.description("The file keeps its own name on the server.")
        } else {
            name_field.description("Leave it empty to use the link's address.")
        };
        let emoji_field = Field::new("bookmark.edit.emoji", &self.emoji)
            .label("Emoji")
            .description("Shown instead of the link's icon. Optional.");
        div()
            .flex()
            .flex_col()
            .gap(theme.space(4.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space(1.))
                    .child(dialog_title("bookmark.edit.title", cx).child(self.title()))
                    .child(
                        dialog_description("bookmark.edit.description", cx)
                            .child("Everyone in the channel sees it in the bar under the header."),
                    ),
            )
            .when(!self.is_file(), |this| this.child(link_field))
            .child(name_field)
            .child(emoji_field)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        Button::new("bookmark.edit.cancel")
                            .variant(ButtonVariant::Outline)
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.handle.close(window, cx);
                            })),
                    )
                    .child(
                        div().debug_selector(|| "bookmark-edit-save".into()).child(
                            Button::new("bookmark.edit.save")
                                .variant(ButtonVariant::Default)
                                .label(if self.editing.is_some() {
                                    "Save"
                                } else {
                                    "Add"
                                })
                                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                        ),
                    ),
            )
    }
}

/// The bookmarks bar: one entity per channel view.
pub struct BookmarksBar {
    model: Option<BookmarksModel>,
    /// The right-click menu, its items set when it opens.
    menu: Entity<MenuState>,
    /// The bar's More menu: every bookmark's actions, for the keyboard.
    more: Entity<MenuState>,
    editor: Entity<BookmarkEditor>,
    delete: DialogHandle,
    deleting: Option<Bookmark>,
    _subscription: gpui_kit::Subscription,
}

impl EventEmitter<BarEvent> for BookmarksBar {}

impl BookmarksBar {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| BookmarkEditor::new(window, cx));
        let subscription = cx.subscribe(&editor, |this, _, save: &EditorSave, cx| {
            this.saved(save.clone(), cx);
        });
        Self {
            model: None,
            menu: cx.new(|cx| MenuState::new([], cx)),
            more: cx.new(|cx| MenuState::new([], cx)),
            editor,
            delete: DialogHandle::new(false),
            deleting: None,
            _subscription: subscription,
        }
    }

    pub fn model(&self) -> Option<&BookmarksModel> {
        self.model.as_ref()
    }

    /// Whether the bar has something to show for `channel_id`: bookmarks,
    /// or Add a bookmark for someone who may add one.
    pub fn shows(&self, channel_id: &str) -> bool {
        self.model.as_ref().is_some_and(|model| {
            model.channel_id == channel_id && (!model.bookmarks.is_empty() || model.can_edit)
        })
    }

    /// The channel's bookmarks, or `None` where the server has none.
    pub fn set_model(&mut self, model: Option<BookmarksModel>, cx: &mut Context<Self>) {
        if self.model != model {
            self.model = model;
            self.refresh_more(cx);
            cx.notify();
        }
    }

    fn channel_id(&self) -> SharedString {
        self.model
            .as_ref()
            .map(|model| model.channel_id.clone())
            .unwrap_or_default()
    }

    /// Opens the dialog to add a link.
    pub fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.open(None, window, cx));
    }

    fn edit(&mut self, bookmark: Bookmark, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.open(Some(bookmark), window, cx));
    }

    fn saved(&mut self, save: EditorSave, cx: &mut Context<Self>) {
        let channel_id = self.channel_id();
        let ask = match save.bookmark_id {
            Some(bookmark_id) => {
                // Shown at once; sync's answer replaces it.
                if let Some(bookmark) = self.find_mut(&bookmark_id) {
                    bookmark.name = save.name.clone();
                    bookmark.emoji = (!save.emoji.is_empty()).then(|| save.emoji.clone());
                    if let Target::Link(url) = &mut bookmark.target
                        && !save.url.is_empty()
                    {
                        *url = save.url.clone();
                    }
                }
                BookmarkAsk::Edit {
                    channel_id,
                    bookmark_id,
                    name: save.name,
                    url: save.url,
                    emoji: save.emoji,
                }
            }
            None => BookmarkAsk::Add {
                channel_id,
                name: save.name,
                url: save.url,
                emoji: save.emoji,
            },
        };
        self.refresh_more(cx);
        cx.emit(BarEvent::Ask(ask));
        cx.notify();
    }

    fn find_mut(&mut self, id: &SharedString) -> Option<&mut Bookmark> {
        self.model
            .as_mut()?
            .bookmarks
            .iter_mut()
            .find(|bookmark| &bookmark.id == id)
    }

    /// Moves a bookmark one place left (`-1`) or right (`1`), at once
    /// here and then on the server.
    pub fn shift(&mut self, id: &SharedString, by: isize, cx: &mut Context<Self>) {
        let Some(model) = self.model.as_mut() else {
            return;
        };
        let Some(from) = model.bookmarks.iter().position(|b| &b.id == id) else {
            return;
        };
        let Some(to) = from
            .checked_add_signed(by)
            .filter(|to| *to < model.bookmarks.len())
        else {
            return;
        };
        let moved = model.bookmarks.remove(from);
        model.bookmarks.insert(to, moved);
        let ask = BookmarkAsk::Move {
            channel_id: model.channel_id.clone(),
            bookmark_id: id.clone(),
            index: to,
        };
        self.refresh_more(cx);
        cx.emit(BarEvent::Ask(ask));
        cx.notify();
    }

    fn ask_delete(&mut self, bookmark: Bookmark, window: &mut Window, cx: &mut Context<Self>) {
        self.deleting = Some(bookmark);
        self.delete.open(window, cx);
        cx.notify();
    }

    fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(bookmark) = self.deleting.take() else {
            return;
        };
        let channel_id = self.channel_id();
        if let Some(model) = self.model.as_mut() {
            model.bookmarks.retain(|b| b.id != bookmark.id);
        }
        self.refresh_more(cx);
        cx.emit(BarEvent::Ask(BookmarkAsk::Delete {
            channel_id,
            bookmark_id: bookmark.id,
        }));
        cx.notify();
    }

    fn open(&self, bookmark: &Bookmark, cx: &mut Context<Self>) {
        cx.emit(match &bookmark.target {
            Target::Link(url) => BarEvent::OpenLink(url.clone()),
            Target::File { file_id, .. } => BarEvent::OpenFile(file_id.clone()),
        });
    }

    /// A bookmark's actions, as both menus list them.
    fn actions(&self, bookmark: &Bookmark, prefix: &str, cx: &Context<Self>) -> Vec<MenuItem> {
        let Some(model) = &self.model else {
            return Vec::new();
        };
        let view = cx.entity().downgrade();
        let at = model.bookmarks.iter().position(|b| b.id == bookmark.id);
        let last = model.bookmarks.len().saturating_sub(1);
        let id = |name: &str| {
            gpui_kit::ElementId::Name(format!("{prefix}.{}.{name}", bookmark.id).into())
        };
        let item =
            |name: &str,
             label: &'static str,
             run: Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)>| {
                let view = view.clone();
                MenuItem::new(id(name), label).on_click(move |_, window, cx| {
                    view.update(cx, |this, cx| run(this, window, cx)).ok();
                })
            };
        let mut items = Vec::new();
        let open = bookmark.clone();
        items.push(item(
            "open",
            if bookmark.link().is_some() {
                "Open link"
            } else {
                "Open file"
            },
            Box::new(move |this, _, cx| this.open(&open, cx)),
        ));
        if let Some(url) = bookmark.link().cloned() {
            items.push(item(
                "copy",
                "Copy link",
                Box::new(move |_, _, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(url.to_string()))
                }),
            ));
        }
        if !model.can_edit {
            return items;
        }
        items.push(MenuItem::separator());
        let edit = bookmark.clone();
        items.push(item(
            "edit",
            "Edit\u{2026}",
            Box::new(move |this, window, cx| this.edit(edit.clone(), window, cx)),
        ));
        let (left, right) = (bookmark.id.clone(), bookmark.id.clone());
        items.push(
            item(
                "left",
                "Move left",
                Box::new(move |this, _, cx| this.shift(&left, -1, cx)),
            )
            .disabled(at == Some(0)),
        );
        items.push(
            item(
                "right",
                "Move right",
                Box::new(move |this, _, cx| this.shift(&right, 1, cx)),
            )
            .disabled(at == Some(last)),
        );
        items.push(MenuItem::separator());
        let delete = bookmark.clone();
        items.push(item(
            "delete",
            "Delete\u{2026}",
            Box::new(move |this, window, cx| this.ask_delete(delete.clone(), window, cx)),
        ));
        items
    }

    /// The More menu: Add a bookmark, then each bookmark's actions.
    fn refresh_more(&mut self, cx: &mut Context<Self>) {
        let mut items = Vec::new();
        if let Some(model) = &self.model {
            if model.can_edit {
                let view = cx.entity().downgrade();
                items.push(
                    MenuItem::new("bookmarks.more.add", "Add a bookmark\u{2026}")
                        .disabled(model.bookmarks.len() >= matty_sync::MAX_BOOKMARKS)
                        .on_click(move |_, window, cx| {
                            view.update(cx, |this, cx| this.add(window, cx)).ok();
                        }),
                );
                if !model.bookmarks.is_empty() {
                    items.push(MenuItem::separator());
                }
            }
            for bookmark in &model.bookmarks {
                items.push(MenuItem::submenu(
                    gpui_kit::ElementId::Name(format!("bookmarks.more.{}", bookmark.id).into()),
                    bookmark.name.clone(),
                    self.actions(bookmark, "bookmarks.more", cx),
                ));
            }
        }
        self.more.update(cx, |menu, cx| menu.set_items(items, cx));
    }

    fn icon(bookmark: &Bookmark, theme: &UiTheme, cx: &App) -> gpui_kit::AnyElement {
        let size = scaled(theme, metrics::ICON_SMALL);
        if let Some(glyph) = bookmark.emoji.as_deref().and_then(crate::emoji::glyph) {
            return div()
                .text_size(size)
                .line_height(size)
                .child(glyph)
                .into_any_element();
        }
        if let Some(source) = bookmark
            .icon_url
            .as_deref()
            .and_then(|url| crate::remote_images::source(url, cx))
        {
            return gpui_kit::img(source)
                .size(size)
                .rounded(px(2.))
                .into_any_element();
        }
        let glyph = match &bookmark.target {
            Target::Link(_) => LucideIcon::Link,
            Target::File { extension, .. }
                if crate::attachments::is_image(&format!("x.{extension}")) =>
            {
                LucideIcon::Image
            }
            Target::File { .. } => LucideIcon::FileText,
        };
        lucide(glyph)
            .size(size)
            .text_color(theme.colors.muted_foreground)
            .into_any_element()
    }

    fn chip(&self, bookmark: &Bookmark, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let open = bookmark.clone();
        let menu_for = bookmark.clone();
        let tip: SharedString = match &bookmark.target {
            Target::Link(url) => url.clone(),
            Target::File { .. } => format!("{} (file)", bookmark.name).into(),
        };
        let label = match &bookmark.target {
            Target::Link(_) => format!("{}, link", bookmark.name),
            Target::File { .. } => format!("{}, file", bookmark.name),
        };
        style::tipped(
            gpui_kit::ElementId::Name(format!("bookmark.{}.tip", bookmark.id).into()),
            tip,
            div()
                .debug_selector({
                    let id = bookmark.id.clone();
                    move || format!("bookmark-{id}")
                })
                .on_mouse_down(
                    gpui_kit::MouseButton::Right,
                    cx.listener(move |this, event: &gpui_kit::MouseDownEvent, window, cx| {
                        let items = this.actions(&menu_for, "bookmark.context", cx);
                        this.menu.update(cx, |menu, cx| {
                            menu.set_items(items, cx);
                            menu.open_at(Some(event.position), window, cx);
                        });
                    }),
                )
                .child(
                    Button::new(gpui_kit::ElementId::Name(
                        format!("bookmark.{}", bookmark.id).into(),
                    ))
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Sm)
                    .aria_label(label)
                    .px(theme.space(2.))
                    .gap(theme.space(1.5))
                    .type_style(style::UI, &theme)
                    .text_color(theme.colors.foreground)
                    .on_click(cx.listener(move |this, _, _, cx| this.open(&open, cx)))
                    .child(Self::icon(bookmark, &theme, cx))
                    .child(
                        div()
                            .max_w(px(MAX_CHIP_WIDTH))
                            .truncate()
                            .child(bookmark.name.clone()),
                    ),
                ),
        )
    }

    fn editor_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let handle = self.editor.read(cx).handle().clone();
        let title = self.editor.read(cx).title();
        let popup = dialog_popup("bookmark.edit.popup", title, cx).child(self.editor.clone());
        dialog("bookmark.edit", &handle, popup, window, cx)
    }

    fn delete_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let name = self.deleting.as_ref().map_or_else(
            || "this bookmark".to_owned(),
            |b| format!("\u{201c}{}\u{201d}", b.name),
        );
        let popup = dialog_popup("bookmark.delete.popup", "Delete bookmark", cx)
            .child(dialog_title("bookmark.delete.title", cx).child(format!("Delete {name}?")))
            .child(
                dialog_description("bookmark.delete.description", cx)
                    .child("It leaves the bar for everyone in the channel."),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        alert_dialog_cancel("bookmark.delete.cancel")
                            .variant(ButtonVariant::Outline)
                            .label("Cancel"),
                    )
                    .child(
                        alert_dialog_action("bookmark.delete.confirm")
                            .variant(ButtonVariant::Destructive)
                            .label("Delete"),
                    ),
            );
        let view = cx.entity().downgrade();
        alert_dialog("bookmark.delete", &self.delete, popup, window, cx).on_ok(move |_, _, cx| {
            view.update(cx, |this, cx| this.confirm_delete(cx)).ok();
            true
        })
    }
}

impl Render for BookmarksBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let Some(model) = self.model.clone() else {
            return div().id("bookmarks");
        };
        let full = model.bookmarks.len() >= matty_sync::MAX_BOOKMARKS;
        let chips: Vec<_> = model
            .bookmarks
            .iter()
            .map(|bookmark| self.chip(bookmark, cx).into_any_element())
            .collect();
        let empty = model.bookmarks.is_empty();
        div()
            .id("bookmarks")
            .flex_shrink_0()
            .h(scaled(&theme, 36.))
            .px(scaled(&theme, metrics::GUTTER) - theme.space(2.))
            .flex()
            .items_center()
            .gap(theme.space(1.))
            .border_b(style::hairline())
            .border_color(theme.colors.border)
            .child(
                div()
                    .id("bookmarks.list")
                    .flex()
                    .items_center()
                    .gap(theme.space(1.))
                    .min_w_0()
                    .overflow_x_scroll()
                    .children(chips),
            )
            .when(model.can_edit && !full, |bar| {
                let add = Button::new("bookmarks.add")
                    .variant(ButtonVariant::Ghost)
                    .size(if empty {
                        ButtonSize::Sm
                    } else {
                        ButtonSize::IconSm
                    })
                    .aria_label("Add a bookmark")
                    .text_color(theme.colors.muted_foreground)
                    .type_style(style::UI, &theme)
                    .on_click(cx.listener(|this, _, window, cx| this.add(window, cx)))
                    .child(
                        lucide(LucideIcon::Plus)
                            .size(scaled(&theme, metrics::ICON_SMALL))
                            .text_color(theme.colors.muted_foreground),
                    )
                    .when(empty, |button| {
                        button.gap(theme.space(1.5)).child("Add a bookmark")
                    });
                bar.child(style::tipped(
                    "bookmarks.add.tip",
                    "Add a bookmark",
                    div().debug_selector(|| "bookmarks-add".into()).child(add),
                ))
            })
            .child(div().flex_1())
            .when(!empty, |bar| {
                bar.child(style::tipped(
                    "bookmarks.more.tip",
                    "Bookmark actions",
                    Menu::new(&self.more, "Bookmark actions").trigger(
                        lucide(LucideIcon::Ellipsis)
                            .size(scaled(&theme, metrics::ICON_SMALL))
                            .text_color(theme.colors.muted_foreground),
                    ),
                ))
            })
            .child(
                Menu::new(&self.menu, "Bookmark menu")
                    .context_area(div())
                    .disabled(true),
            )
            .child(self.editor_dialog(window, cx))
            .child(self.delete_dialog(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Host {
        bar: Entity<BookmarksBar>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.bar.clone())
        }
    }

    fn link(id: &str) -> Bookmark {
        Bookmark {
            id: id.into(),
            name: id.to_uppercase().into(),
            target: Target::Link(format!("https://example.com/{id}").into()),
            emoji: None,
            icon_url: None,
        }
    }

    /// Adding a link goes out as an add, and moving one shows at once and
    /// goes out as a move; clicking one opens it.
    #[gpui_kit::test]
    fn adding_and_reordering_a_bookmark(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let window = cx.add_window({
            let events = events.clone();
            move |window, cx| {
                let bar = cx.new(|cx| BookmarksBar::new(window, cx));
                cx.subscribe(&bar, move |_, _, event: &BarEvent, _| {
                    events.borrow_mut().push(event.clone());
                })
                .detach();
                bar.update(cx, |bar, cx| {
                    bar.set_model(
                        Some(BookmarksModel {
                            channel_id: "c".into(),
                            bookmarks: vec![link("a"), link("b")],
                            can_edit: true,
                        }),
                        cx,
                    )
                });
                Host { bar }
            }
        });
        let mut cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let bar = window.read_with(&cx, |host, _| host.bar.clone()).unwrap();
        let click = |cx: &mut gpui_kit::VisualTestContext, selector: &'static str| {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let bounds = cx.debug_bounds(selector).expect(selector);
            cx.simulate_click(bounds.center(), Default::default());
        };
        assert!(bar.read_with(&cx, |bar, _| bar.shows("c") && !bar.shows("d")));

        click(&mut cx, "bookmark-b");
        assert_eq!(
            events.borrow().last(),
            Some(&BarEvent::OpenLink("https://example.com/b".into()))
        );

        // Add a bookmark: a bad link is refused, a good one goes out.
        click(&mut cx, "bookmarks-add");
        let editor = bar.read_with(&cx, |bar, _| bar.editor.clone());
        assert!(editor.read_with(&cx, |editor, _| editor.handle.is_open()));
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor
                    .link
                    .update(cx, |input, cx| input.set_value("not a link", window, cx));
            })
        });
        click(&mut cx, "bookmark-edit-save");
        assert!(editor.read_with(&cx, |editor, _| editor.tried && editor.handle.is_open()));
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.link.update(cx, |input, cx| {
                    input.set_value("example.com/docs", window, cx)
                });
                editor
                    .emoji
                    .update(cx, |input, cx| input.set_value(":book:", window, cx));
            })
        });
        click(&mut cx, "bookmark-edit-save");
        assert_eq!(
            events.borrow().last(),
            Some(&BarEvent::Ask(BookmarkAsk::Add {
                channel_id: "c".into(),
                name: "example.com".into(),
                url: "https://example.com/docs".into(),
                emoji: "book".into(),
            }))
        );
        assert!(!editor.read_with(&cx, |editor, _| editor.handle.is_open()));

        // Move b left: the bar shows it first at once.
        bar.update(&mut cx, |bar, cx| bar.shift(&"b".into(), -1, cx));
        let order: Vec<String> = bar.read_with(&cx, |bar, _| {
            bar.model()
                .unwrap()
                .bookmarks
                .iter()
                .map(|b| b.id.to_string())
                .collect()
        });
        assert_eq!(order, ["b", "a"]);
        assert_eq!(
            events.borrow().last(),
            Some(&BarEvent::Ask(BookmarkAsk::Move {
                channel_id: "c".into(),
                bookmark_id: "b".into(),
                index: 0,
            }))
        );
        // Already first: nothing moves.
        let before = events.borrow().len();
        bar.update(&mut cx, |bar, cx| bar.shift(&"b".into(), -1, cx));
        assert_eq!(events.borrow().len(), before);
    }

    #[test]
    fn links_are_made_whole_or_refused() {
        assert_eq!(
            normalize_link("example.com/docs").as_deref(),
            Some("https://example.com/docs")
        );
        assert_eq!(
            normalize_link(" http://a.example ").as_deref(),
            Some("http://a.example")
        );
        assert_eq!(normalize_link("not a link"), None);
        assert_eq!(normalize_link("ftp://example.com"), None);
        assert_eq!(normalize_link(""), None);
    }

    #[test]
    fn the_model_names_and_orders_bookmarks() {
        let list: Vec<ChannelBookmark> = serde_json::from_value(serde_json::json!([
            {"id": "a", "channel_id": "c", "display_name": "Docs", "type": "link",
             "link_url": "https://example.com", "emoji": "book"},
            {"id": "b", "channel_id": "c", "display_name": "", "type": "file", "file_id": "f",
             "file": {"id": "f", "name": "plan.png", "extension": "png"}}
        ]))
        .unwrap();
        let model = model("c", &list, true);
        assert_eq!(model.bookmarks[0].name.as_ref(), "Docs");
        assert_eq!(model.bookmarks[0].emoji.as_deref(), Some("book"));
        assert_eq!(model.bookmarks[1].name.as_ref(), "Untitled");
        assert_eq!(
            model.bookmarks[1].target,
            Target::File {
                file_id: "f".into(),
                extension: "png".into()
            }
        );
    }
}
