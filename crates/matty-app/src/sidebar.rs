//! The channel sidebar: the view model sync fills and the view that draws it
//! (DESIGN.md §4). Categories collapse, unread rows are stronger, mentions
//! are `primary` pills, muted rows fade, and direct messages show presence.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, px,
};

use crate::style::{self, Palette, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    avatar::{Avatar, AvatarSize},
    badge::{Badge, BadgeVariant},
    button::{Button, ButtonSize, ButtonVariant, style_button},
    collapsible::collapsible_trigger,
    menu::{Menu, MenuItem, MenuState},
    sidebar::{Sidebar, SidebarItem, SidebarItemSize, sidebar_menu_action, sidebar_menu_skeleton},
    sortable::{Sortable, SortableGroup, SortableMove, SortableRow, move_group},
    theme::{UiTheme, transition_value},
    toggle::{Toggle, ToggleSize},
};

/// A user's presence, as Mattermost reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Online,
    Away,
    Dnd,
    Offline,
}

/// What a sidebar row points at, which decides its icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    Public,
    Private,
    Direct { presence: Presence },
    Group { members: usize },
}

/// One row in a category.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelRow {
    pub id: SharedString,
    pub name: SharedString,
    pub kind: ChannelKind,
    pub unread: bool,
    pub mentions: usize,
    pub muted: bool,
    /// A direct message's other person, whose picture the row shows.
    pub person: Option<SharedString>,
    /// A direct message's other person's username, so the quick switcher
    /// finds the row by it whatever the name format shows (#156); for a
    /// channel, its URL name, which `~` mentions use.
    pub handle: Option<SharedString>,
}

impl ChannelRow {
    pub fn new(
        id: impl Into<SharedString>,
        name: impl Into<SharedString>,
        kind: ChannelKind,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind,
            unread: false,
            mentions: 0,
            muted: false,
            person: None,
            handle: None,
        }
    }

    /// The direct message's other person, by user id.
    pub fn person(mut self, user_id: impl Into<SharedString>) -> Self {
        self.person = Some(user_id.into());
        self
    }

    /// The direct message's other person's username.
    pub fn handle(mut self, username: impl Into<SharedString>) -> Self {
        self.handle = Some(username.into());
        self
    }

    pub fn unread(mut self) -> Self {
        self.unread = true;
        self
    }

    pub fn mentions(mut self, count: usize) -> Self {
        self.mentions = count;
        self.unread |= count > 0;
        self
    }

    pub fn muted(mut self) -> Self {
        self.muted = true;
        self
    }
}

/// A server-side category (Favorites, Channels, Direct messages, custom).
/// Mattermost's category types: which channels each may hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CategoryKind {
    Favorites,
    /// Public and private channels only.
    #[default]
    Channels,
    /// Direct and group messages only.
    Direct,
    /// Made by the user; holds anything.
    Custom,
}

impl CategoryKind {
    /// The kind for the server's `type`.
    pub fn from_server(kind: &str) -> Self {
        match kind {
            "favorites" => Self::Favorites,
            "direct_messages" => Self::Direct,
            "custom" => Self::Custom,
            _ => Self::Channels,
        }
    }

    /// Whether a row of `kind` may move into a category of this kind.
    pub fn takes(self, kind: &ChannelKind) -> bool {
        let direct = matches!(kind, ChannelKind::Direct { .. } | ChannelKind::Group { .. });
        match self {
            Self::Favorites | Self::Custom => true,
            Self::Channels => !direct,
            Self::Direct => direct,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Category {
    pub id: SharedString,
    pub name: SharedString,
    pub collapsed: bool,
    /// Which of Mattermost's kinds it is; only custom ones can be renamed
    /// and deleted (#208).
    pub kind: CategoryKind,
    pub rows: Vec<ChannelRow>,
}

impl Category {
    pub fn new(
        id: impl Into<SharedString>,
        name: impl Into<SharedString>,
        rows: Vec<ChannelRow>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            collapsed: false,
            kind: CategoryKind::Channels,
            rows,
        }
    }
}

/// Everything the sidebar draws. Sync replaces it whole; the view keeps no
/// other state than which categories the user collapsed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SidebarModel {
    /// True before the first snapshot; the view shows loading rows.
    pub loading: bool,
    pub selected: Option<SharedString>,
    /// Followed threads with replies the user has not read.
    pub unread_threads: usize,
    /// Mentions of the user in those threads' unread replies (#124).
    pub thread_mentions: usize,
    /// Unsent drafts, channels' and threads', for the Drafts row's count.
    pub drafts: usize,
    /// Channels whose composer holds a draft; their rows show a pencil.
    pub drafted: std::collections::HashSet<SharedString>,
    pub categories: Vec<Category>,
}

/// Which way Alt+Up/Down walks the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Previous,
    Next,
}

impl SidebarModel {
    /// The model before anything has loaded.
    pub fn loading() -> Self {
        Self {
            loading: true,
            ..Self::default()
        }
    }

    /// Every row in sidebar order, collapsed categories included, as
    /// Mattermost's Alt+Up/Down does.
    fn rows(&self) -> impl Iterator<Item = &ChannelRow> {
        self.categories
            .iter()
            .flat_map(|category| category.rows.iter())
    }

    /// The row one `step` away from the selection, wrapping at the ends;
    /// with `unread_only`, the nearest unread row in that direction, or
    /// `None` when nothing is unread. Muted channels never count as unread.
    pub fn neighbor(&self, step: Step, unread_only: bool) -> Option<SharedString> {
        let rows: Vec<&ChannelRow> = self
            .rows()
            .filter(|row| !unread_only || (row.unread && !row.muted))
            .collect();
        if rows.is_empty() {
            return None;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|id| self.rows().position(|row| &row.id == id));
        let order: Vec<SharedString> = self.rows().map(|row| row.id.clone()).collect();
        let candidates: Vec<usize> = rows
            .iter()
            .filter_map(|row| order.iter().position(|id| id == &row.id))
            .collect();
        let next = match (current, step) {
            (None, Step::Next) => candidates[0],
            (None, Step::Previous) => candidates[candidates.len() - 1],
            (Some(at), Step::Next) => candidates
                .iter()
                .copied()
                .find(|&index| index > at)
                .unwrap_or(candidates[0]),
            (Some(at), Step::Previous) => candidates
                .iter()
                .rev()
                .copied()
                .find(|&index| index < at)
                .unwrap_or(candidates[candidates.len() - 1]),
        };
        Some(order[next].clone())
    }

    /// What the Unreads filter shows (PLAN §4.4): rows with unread posts
    /// (muted ones only for a mention) and the open channel, in their
    /// categories; categories left empty drop out.
    pub fn unreads(&self) -> SidebarModel {
        let mut model = self.clone();
        for category in &mut model.categories {
            category.rows.retain(|row| {
                (row.unread && !row.muted)
                    || row.mentions > 0
                    || self.selected.as_ref() == Some(&row.id)
            });
        }
        model
            .categories
            .retain(|category| !category.rows.is_empty());
        model
    }

    /// No row needs reading; the open channel does not count.
    pub fn caught_up(&self) -> bool {
        self.rows().all(|row| {
            self.selected.as_ref() == Some(&row.id)
                || (!row.unread || row.muted) && row.mentions == 0
        })
    }

    fn row_mut(&mut self, id: &str) -> Option<&mut ChannelRow> {
        self.categories
            .iter_mut()
            .flat_map(|category| category.rows.iter_mut())
            .find(|row| row.id.as_ref() == id)
    }

    /// Whether `id` sits in the Favorites category.
    pub fn is_favorite(&self, id: &str) -> bool {
        self.categories.iter().any(|category| {
            category.id == FAVORITES_ID && category.rows.iter().any(|row| row.id.as_ref() == id)
        })
    }

    /// Takes `id` out of whatever category holds it.
    pub fn remove(&mut self, id: &str) -> Option<ChannelRow> {
        for category in &mut self.categories {
            if let Some(index) = category.rows.iter().position(|row| row.id.as_ref() == id) {
                return Some(category.rows.remove(index));
            }
        }
        None
    }

    /// Moves `id` into Favorites, or back to the category its kind lives
    /// in (`channels` or `direct`), appending at the end as Mattermost does.
    /// Missing categories are created so the row never disappears.
    pub fn set_favorite(&mut self, id: &str, favorite: bool) {
        let Some(row) = self.remove(id) else { return };
        let (target_id, target_name) = if favorite {
            (FAVORITES_ID, "Favorites")
        } else if matches!(
            row.kind,
            ChannelKind::Direct { .. } | ChannelKind::Group { .. }
        ) {
            (DIRECT_ID, "Direct messages")
        } else {
            (CHANNELS_ID, "Channels")
        };
        match self.categories.iter_mut().find(|c| c.id == target_id) {
            Some(category) => category.rows.push(row),
            None => {
                let category = Category::new(target_id, target_name, vec![row]);
                if favorite {
                    self.categories.insert(0, category);
                } else {
                    self.categories.push(category);
                }
            }
        }
    }

    /// The channels with unread messages or mentions, in sidebar order,
    /// for Mark all read to view on the server.
    pub fn unread_ids(&self) -> Vec<SharedString> {
        self.rows()
            .filter(|row| row.unread || row.mentions > 0)
            .map(|row| row.id.clone())
            .collect()
    }

    /// Clears every unread mark and mention count.
    pub fn mark_all_read(&mut self) {
        for row in self
            .categories
            .iter_mut()
            .flat_map(|category| category.rows.iter_mut())
        {
            row.unread = false;
            row.mentions = 0;
        }
        self.unread_threads = 0;
        self.thread_mentions = 0;
    }

    /// The Threads entry's count: its mentions as a channel row shows
    /// them, otherwise how many threads are unread (#124).
    pub fn threads_count(&self) -> EntryCount {
        if self.thread_mentions > 0 {
            EntryCount::Mentions(self.thread_mentions)
        } else if self.unread_threads > 0 {
            EntryCount::Count(self.unread_threads)
        } else {
            EntryCount::None
        }
    }
}

/// The badge on a fixed sidebar entry (DESIGN.md §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryCount {
    /// Nothing to read or finish.
    None,
    /// Things to read or finish, as a `secondary` badge.
    Count(usize),
    /// Mentions, as the `primary` pill a channel row's mentions use.
    Mentions(usize),
}

/// A stable element id from a prefix and a server id.
fn element_id(prefix: &str, id: &SharedString) -> gpui_kit::ElementId {
    gpui_kit::ElementId::Name(format!("{prefix}.{id}").into())
}

/// What the sidebar asks the window to do.
/// The `selected` id that marks the Threads row, which is not a channel.
pub const THREADS_ID: &str = "~threads";
/// The hover group of a channel row: its options button shows and its
/// count hides while the pointer is over the row.
const ROW_GROUP: &str = "sidebar-row";
/// The Favorites category, as Mattermost names it.
pub const FAVORITES_ID: &str = "favorites";
/// The default category for channels.
pub const CHANNELS_ID: &str = "channels";
/// The default category for direct and group messages.
pub const DIRECT_ID: &str = "direct";
/// The `selected` id that marks the Drafts row.
pub const DRAFTS_ID: &str = "~drafts";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarEvent {
    /// The user opened the Threads entry above the categories.
    OpenThreads,
    /// The user opened the Drafts entry above the categories.
    OpenDrafts,
    /// The user activated a channel row.
    Select(SharedString),
    /// "New message" from the sidebar's plus menu.
    NewMessage,
    /// "Browse channels" from the sidebar's plus menu.
    BrowseChannels,
    /// "Create channel" from the sidebar's plus menu.
    CreateChannel,
    /// "Mark as read" from a row's menu; the row is already clear.
    MarkRead(SharedString),
    /// Favorite or unfavorite a channel from its row menu; the row has
    /// already moved between Favorites and its home category.
    SetFavorite { id: SharedString, favorite: bool },
    /// Mute or unmute a channel from its row menu; the row already shows it.
    SetMuted { id: SharedString, muted: bool },
    /// "Open in new window" from a row's menu (#222).
    PopOut(SharedString),
    /// "Open in split view" from a row's menu (#222).
    OpenInSplit(SharedString),
    /// "Leave channel" from a channel row's menu.
    Leave(SharedString),
    /// "Close conversation" from a direct or group message row's menu.
    Close(SharedString),
    /// "Create new category" from the plus menu, or "New category…" in a
    /// row's Move to menu, which moves that channel into it (#208).
    NewCategory { channel_id: Option<SharedString> },
    /// "Move to" a category from a row's menu.
    MoveToCategory {
        channel_id: SharedString,
        category_id: SharedString,
    },
    /// "Rename category" from a custom category's menu.
    RenameCategory {
        id: SharedString,
        name: SharedString,
    },
    /// "Delete category" from a custom category's menu.
    DeleteCategory {
        id: SharedString,
        name: SharedString,
    },
    /// "Move category up" or "down" from a category's menu.
    MoveCategory { id: SharedString, up: bool },
    /// "Move up" or "Move down" in a row's Move to menu: within its
    /// category, which then sorts by hand.
    MoveChannel {
        category_id: SharedString,
        channel_id: SharedString,
        up: bool,
    },
    /// A category dragged, or moved with Alt+Shift+Up/Down, to `index`;
    /// the sidebar already shows it there.
    PlaceCategory { id: SharedString, index: usize },
    /// A row dragged, or moved with Alt+Shift+Up/Down, to `index` among
    /// all of `category_id`'s rows; the sidebar already shows it there.
    PlaceChannel {
        channel_id: SharedString,
        category_id: SharedString,
        index: usize,
    },
}

pub struct SidebarView {
    model: SidebarModel,
    team_name: SharedString,
    /// The plus menu: new message, browse, create.
    menu: Entity<MenuState>,
    /// The per-row options menu, refilled for the row it opens on.
    row_menu: Entity<MenuState>,
    /// The Unreads filter is on: only rows that need reading show.
    unreads_only: bool,
}

impl EventEmitter<SidebarEvent> for SidebarView {}

impl SidebarView {
    pub fn new(
        team_name: impl Into<SharedString>,
        model: SidebarModel,
        cx: &mut Context<Self>,
    ) -> Self {
        let view = cx.weak_entity();
        let item = |id: &'static str, label: &'static str, event: SidebarEvent| {
            let view = view.clone();
            MenuItem::new(id, label).on_click(move |_, _, cx| {
                view.update(cx, |_, cx| cx.emit(event.clone())).ok();
            })
        };
        let menu = cx.new(|cx| {
            MenuState::new(
                [
                    item(
                        "sidebar.new-message",
                        "New message",
                        SidebarEvent::NewMessage,
                    ),
                    MenuItem::separator(),
                    item(
                        "sidebar.browse-channels",
                        "Browse channels",
                        SidebarEvent::BrowseChannels,
                    ),
                    item(
                        "sidebar.create-channel",
                        "Create channel",
                        SidebarEvent::CreateChannel,
                    ),
                    MenuItem::separator(),
                    item(
                        "sidebar.create-category",
                        "Create new category",
                        SidebarEvent::NewCategory { channel_id: None },
                    ),
                ],
                cx,
            )
        });
        let row_menu = cx.new(|cx| MenuState::new([], cx));
        Self {
            model,
            team_name: team_name.into(),
            menu,
            row_menu,
            unreads_only: false,
        }
    }

    /// Turns the Unreads filter on or off (`Cmd+Shift+U` or the header
    /// toggle).
    pub fn toggle_unreads(&mut self, cx: &mut Context<Self>) {
        self.set_unreads_only(!self.unreads_only, cx);
    }

    fn set_unreads_only(&mut self, on: bool, cx: &mut Context<Self>) {
        self.unreads_only = on;
        cx.notify();
    }

    /// The rows as drawn: the whole model, or what the Unreads filter
    /// leaves, so Alt+Up/Down walks what the user sees.
    pub fn shown(&self) -> std::borrow::Cow<'_, SidebarModel> {
        if self.unreads_only {
            std::borrow::Cow::Owned(self.model.unreads())
        } else {
            std::borrow::Cow::Borrowed(&self.model)
        }
    }

    /// Clears one row's unread mark, as "Mark as read" does.
    pub fn mark_read(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(row) = self.model.row_mut(id) {
            row.unread = false;
            row.mentions = 0;
            cx.notify();
        }
    }

    pub fn set_muted(&mut self, id: &str, muted: bool, cx: &mut Context<Self>) {
        if let Some(row) = self.model.row_mut(id) {
            row.muted = muted;
            cx.notify();
        }
    }

    pub fn set_favorite(&mut self, id: &str, favorite: bool, cx: &mut Context<Self>) {
        self.model.set_favorite(id, favorite);
        cx.notify();
    }

    /// The row menu's items for `row`: mark read while unread, favorite,
    /// mute, move, open in a new window, then leave or close.
    fn row_items(&self, row: &ChannelRow, cx: &Context<Self>) -> Vec<MenuItem> {
        let view = cx.weak_entity();
        let emit = |id: &'static str, label: &str, event: SidebarEvent| {
            let view = view.clone();
            MenuItem::new(id, label.to_string()).on_click(move |_, _, cx| {
                view.update(cx, |_, cx| cx.emit(event.clone())).ok();
            })
        };
        let direct = matches!(
            row.kind,
            ChannelKind::Direct { .. } | ChannelKind::Group { .. }
        );
        let favorite = self.model.is_favorite(&row.id);
        let mut items = Vec::new();
        if row.unread {
            items.push(emit(
                "sidebar.row.read",
                "Mark as read",
                SidebarEvent::MarkRead(row.id.clone()),
            ));
        }
        items.push(emit(
            "sidebar.row.favorite",
            if favorite {
                "Remove from favorites"
            } else {
                "Add to favorites"
            },
            SidebarEvent::SetFavorite {
                id: row.id.clone(),
                favorite: !favorite,
            },
        ));
        items.push(emit(
            "sidebar.row.mute",
            match (row.muted, direct) {
                (true, true) => "Unmute conversation",
                (true, false) => "Unmute channel",
                (false, true) => "Mute conversation",
                (false, false) => "Mute channel",
            },
            SidebarEvent::SetMuted {
                id: row.id.clone(),
                muted: !row.muted,
            },
        ));
        items.push(self.move_to(row, cx));
        items.push(emit(
            "sidebar.row.split",
            "Open in split view",
            SidebarEvent::OpenInSplit(row.id.clone()),
        ));
        items.push(emit(
            "sidebar.row.pop-out",
            "Open in new window",
            SidebarEvent::PopOut(row.id.clone()),
        ));
        items.push(MenuItem::separator());
        items.push(if direct {
            emit(
                "sidebar.row.close",
                "Close conversation",
                SidebarEvent::Close(row.id.clone()),
            )
        } else {
            emit(
                "sidebar.row.leave",
                "Leave channel",
                SidebarEvent::Leave(row.id.clone()),
            )
        });
        items
    }

    /// "Move to": every other category, then a new one.
    fn move_to(&self, row: &ChannelRow, cx: &Context<Self>) -> MenuItem {
        let view = cx.weak_entity();
        let emit = |id: String, label: SharedString, event: SidebarEvent| {
            let view = view.clone();
            MenuItem::new(SharedString::from(id), label).on_click(move |_, _, cx| {
                view.update(cx, |_, cx| cx.emit(event.clone())).ok();
            })
        };
        let mut items: Vec<MenuItem> = Vec::new();
        if let Some(category) = self
            .model
            .categories
            .iter()
            .find(|category| category.rows.iter().any(|other| other.id == row.id))
        {
            let at = category
                .rows
                .iter()
                .position(|other| other.id == row.id)
                .unwrap_or_default();
            for (up, label, end) in [
                (true, "Move up", at == 0),
                (false, "Move down", at + 1 >= category.rows.len()),
            ] {
                items.push(
                    emit(
                        format!("sidebar.row.move.{}", if up { "up" } else { "down" }),
                        label.into(),
                        SidebarEvent::MoveChannel {
                            category_id: category.id.clone(),
                            channel_id: row.id.clone(),
                            up,
                        },
                    )
                    .disabled(end),
                );
            }
            items.push(MenuItem::separator());
        }
        let before = items.len();
        items.extend(
            self.model
                .categories
                .iter()
                .filter(|category| {
                    category.kind.takes(&row.kind)
                        && !category.rows.iter().any(|other| other.id == row.id)
                })
                .map(|category| {
                    emit(
                        format!("sidebar.row.move.{}", category.id),
                        category.name.clone(),
                        SidebarEvent::MoveToCategory {
                            channel_id: row.id.clone(),
                            category_id: category.id.clone(),
                        },
                    )
                }),
        );
        if items.len() > before {
            items.push(MenuItem::separator());
        }
        items.push(emit(
            "sidebar.row.move.new".into(),
            "New category\u{2026}".into(),
            SidebarEvent::NewCategory {
                channel_id: Some(row.id.clone()),
            },
        ));
        MenuItem::submenu("sidebar.row.move", "Move to", items)
    }

    /// A category's menu: move it, and for a custom one rename or delete it.
    fn open_category_menu(
        &mut self,
        id: &SharedString,
        at: gpui_kit::Point<gpui_kit::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let categories = &self.model.categories;
        let Some(index) = categories.iter().position(|category| &category.id == id) else {
            return;
        };
        let category = &categories[index];
        let custom = category.kind == CategoryKind::Custom;
        let view = cx.weak_entity();
        let emit = |id: &'static str, label: &'static str, event: SidebarEvent| {
            let view = view.clone();
            MenuItem::new(id, label).on_click(move |_, _, cx| {
                view.update(cx, |_, cx| cx.emit(event.clone())).ok();
            })
        };
        let (id, name) = (category.id.clone(), category.name.clone());
        let mut items = Vec::new();
        if custom {
            items.push(emit(
                "sidebar.category.rename",
                "Rename category",
                SidebarEvent::RenameCategory {
                    id: id.clone(),
                    name: name.clone(),
                },
            ));
        }
        items.push(emit(
            "sidebar.category.new",
            "Create new category",
            SidebarEvent::NewCategory { channel_id: None },
        ));
        items.push(MenuItem::separator());
        items.push(
            emit(
                "sidebar.category.up",
                "Move category up",
                SidebarEvent::MoveCategory {
                    id: id.clone(),
                    up: true,
                },
            )
            .disabled(index == 0),
        );
        items.push(
            emit(
                "sidebar.category.down",
                "Move category down",
                SidebarEvent::MoveCategory {
                    id: id.clone(),
                    up: false,
                },
            )
            .disabled(index + 1 >= categories.len()),
        );
        if custom {
            items.push(MenuItem::separator());
            items.push(emit(
                "sidebar.category.delete",
                "Delete category",
                SidebarEvent::DeleteCategory { id, name },
            ));
        }
        self.row_menu.update(cx, |menu, cx| {
            menu.set_items(items, cx);
            menu.open_at(Some(at), window, cx);
        });
    }

    fn open_row_menu(
        &mut self,
        id: &SharedString,
        at: gpui_kit::Point<gpui_kit::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self
            .model
            .categories
            .iter()
            .flat_map(|category| category.rows.iter())
            .find(|row| &row.id == id)
            .cloned()
        else {
            return;
        };
        let items = self.row_items(&row, cx);
        self.row_menu.update(cx, |menu, cx| {
            menu.set_items(items, cx);
            menu.open_at(Some(at), window, cx);
        });
    }

    pub fn model(&self) -> &SidebarModel {
        &self.model
    }

    /// The team name in the header.
    pub fn set_team_name(&mut self, name: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.team_name = name.into();
        cx.notify();
    }

    /// Replaces the model, keeping the user's collapsed categories.
    pub fn set_model(&mut self, mut model: SidebarModel, cx: &mut Context<Self>) {
        for category in &mut model.categories {
            if let Some(old) = self.model.categories.iter().find(|c| c.id == category.id) {
                category.collapsed = old.collapsed;
            }
        }
        self.model = model;
        cx.notify();
    }

    /// Selects `id` and clears its unread mark, as viewing a channel does;
    /// sync confirms the view with the server.
    pub fn select(&mut self, id: Option<SharedString>, cx: &mut Context<Self>) {
        if let Some(row) = id.as_ref().and_then(|id| {
            self.model
                .categories
                .iter_mut()
                .flat_map(|category| category.rows.iter_mut())
                .find(|row| &row.id == id)
        }) {
            row.unread = false;
            row.mentions = 0;
        }
        self.model.selected = id;
        cx.notify();
    }

    /// Marks `id` as the one shown, leaving its unread mark: split view's
    /// focus moved to a pane that already shows it (#222).
    pub fn highlight(&mut self, id: Option<SharedString>, cx: &mut Context<Self>) {
        if self.model.selected != id {
            self.model.selected = id;
            cx.notify();
        }
    }

    /// Marks one row unread, as U on a selected post does.
    pub fn mark_unread(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(row) = self.model.row_mut(id) {
            row.unread = true;
            cx.notify();
        }
    }

    /// Whether `id` shows unread messages or mentions.
    /// Whether the sidebar on screen lists the channel.
    pub fn has_row(&self, id: &str) -> bool {
        self.model.rows().any(|row| row.id.as_ref() == id)
    }

    pub fn is_unread(&self, id: &str) -> bool {
        self.model
            .rows()
            .any(|row| row.id.as_ref() == id && (row.unread || row.mentions > 0))
    }

    /// The channels Mark all read clears.
    pub fn unread_ids(&self) -> Vec<SharedString> {
        self.model.unread_ids()
    }

    pub fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        self.model.mark_all_read();
        cx.notify();
    }

    fn toggle_category(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        if let Some(category) = self.model.categories.iter_mut().find(|c| &c.id == id) {
            category.collapsed = !category.collapsed;
            cx.notify();
        }
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let palette = Palette::read(cx);
        let team = Button::new("sidebar.team")
            .variant(ButtonVariant::Ghost)
            .size(ButtonSize::Sm)
            .aria_label("Team menu")
            .px(theme.space(2.))
            .type_style(style::TITLE, &theme)
            .text_color(theme.colors.sidebar_foreground)
            .child(self.team_name.clone())
            .child(
                lucide(LucideIcon::ChevronDown)
                    .size(scaled(&theme, metrics::ICON_SMALL))
                    .text_color(palette.sidebar_muted),
            );
        let loading = self.model.loading;
        let button_theme = theme.clone();
        let compose = Menu::new(&self.menu, "New")
            .trigger(
                lucide(LucideIcon::SquarePen)
                    .size(scaled(&theme, metrics::ICON))
                    .text_color(palette.sidebar_muted),
            )
            .trigger_style_with(move |button| {
                style_button(
                    button.aria_label("New…").disabled(loading),
                    false,
                    ButtonVariant::Ghost,
                    ButtonSize::IconSm,
                    &button_theme,
                )
            });
        let on = self.unreads_only;
        let filter = Toggle::new("sidebar.unreads")
            .size(ToggleSize::Sm)
            .pressed(on)
            .disabled(loading)
            .aria_label("Unreads filter")
            .on_change({
                let view = cx.weak_entity();
                move |on, _, _, cx| {
                    view.update(cx, |this, cx| this.set_unreads_only(on, cx))
                        .ok();
                }
            })
            .child(
                lucide(LucideIcon::ListFilter)
                    .size(scaled(&theme, metrics::ICON))
                    .text_color(if on {
                        theme.colors.sidebar_foreground
                    } else {
                        palette.sidebar_muted
                    }),
            );
        let filter_tip = if on {
            "Show all channels"
        } else {
            "Show unreads only"
        };
        div()
            .h(scaled(&theme, metrics::HEADER_HEIGHT) - theme.space(4.))
            .flex()
            .items_center()
            .justify_between()
            .child(team)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space(0.5))
                    .child(style::tipped("sidebar.unreads.tip", filter_tip, filter))
                    .child(style::tipped("sidebar.compose.tip", "New…", compose)),
            )
    }

    /// The pencil a row with a draft shows in its icon's slot.
    fn draft_icon(theme: &UiTheme, palette: &Palette) -> impl IntoElement + use<> {
        div()
            .size(scaled(theme, metrics::ROW_SLOT))
            .flex()
            .items_center()
            .justify_center()
            .child(
                lucide(LucideIcon::Pencil)
                    .size(scaled(theme, metrics::ICON))
                    .text_color(palette.sidebar_muted),
            )
    }

    /// The icon slot: `#` and lock for channels, an avatar with a presence
    /// dot for people, a member count for group messages.
    fn row_icon(
        row: &ChannelRow,
        theme: &UiTheme,
        palette: &Palette,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let icon_size = scaled(theme, metrics::ICON);
        let slot = scaled(theme, metrics::ROW_SLOT);
        let color = palette.sidebar_muted;
        let boxed = |inner: gpui_kit::AnyElement| {
            div()
                .size(slot)
                .flex()
                .items_center()
                .justify_center()
                .child(inner)
                .into_any_element()
        };
        match &row.kind {
            ChannelKind::Public => boxed(
                lucide(LucideIcon::Hash)
                    .size(icon_size)
                    .text_color(color)
                    .into_any_element(),
            ),
            ChannelKind::Private => boxed(
                lucide(LucideIcon::Lock)
                    .size(icon_size)
                    .text_color(color)
                    .into_any_element(),
            ),
            ChannelKind::Group { members } => boxed(
                div()
                    .size(icon_size)
                    .rounded(theme.radius.sm)
                    .bg(theme.colors.sidebar_accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(theme.text(10.))
                    .line_height(theme.text(10.))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(color)
                    .child(members.to_string())
                    .into_any_element(),
            ),
            ChannelKind::Direct { presence } => {
                let initial: String = row.name.chars().take(1).collect();
                div()
                    .relative()
                    .size(slot)
                    .child(
                        crate::avatars::with_picture(
                            Avatar::new(element_id("sidebar.avatar", &row.id))
                                .size(AvatarSize::Sm)
                                .fallback(initial)
                                .aria_label(crate::style::presence_name(&row.name, *presence)),
                            row.person.as_deref().unwrap_or_default(),
                            cx,
                        )
                        .w(slot)
                        .h(slot)
                        .text_size(theme.text(9.)),
                    )
                    .child(crate::style::presence_dot(
                        *presence,
                        scaled(theme, 8.),
                        theme.colors.sidebar,
                        theme,
                        palette,
                    ))
                    .into_any_element()
            }
        }
    }

    fn row(&self, row: &ChannelRow, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let palette = Palette::read(cx);
        let selected = self.model.selected.as_ref() == Some(&row.id);
        let strong = row.unread && !row.muted;
        let id = row.id.clone();
        let mut item = SidebarItem::new(element_id("sidebar.channel", &row.id), row.name.clone())
            .size(SidebarItemSize::Small)
            .selected(selected)
            .icon(if self.model.drafted.contains(&row.id) && !selected {
                // An unsent draft takes the icon's place, as in Mattermost
                // (#114).
                Self::draft_icon(&theme, &palette).into_any_element()
            } else {
                Self::row_icon(row, &theme, &palette, cx).into_any_element()
            })
            .type_style(if strong { style::UI_STRONG } else { style::UI }, &theme)
            .on_activate(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                    this.model.selected = Some(id.clone());
                    cx.emit(SidebarEvent::Select(id.clone()));
                    // A click lands in the composer, as in Mattermost, so the
                    // row keeps focus (and Alt+Shift+Up/Down moves it) only
                    // when the list was reached from the keyboard.
                    if !event.is_keyboard() {
                        window.dispatch_action(Box::new(crate::actions::FocusComposer), cx);
                    }
                    cx.notify();
                }),
            );
        if !strong && !selected {
            item = item.text_color(palette.sidebar_muted);
        }
        if row.muted && !selected {
            item = item.opacity(0.6);
        }
        // The count or dot hides while the row is hovered, where the options
        // button takes its place, as in Mattermost.
        if row.mentions > 0 {
            item = item.trailing(
                div()
                    .group_hover(ROW_GROUP, |style| style.invisible())
                    .child(
                        Badge::count(row.mentions, 99)
                            .bg(palette.mention)
                            .text_color(palette.mention_foreground)
                            .h(theme.space(4.5))
                            .min_w(theme.space(4.5))
                            .px(theme.space(1.5))
                            .text_size(theme.text(11.))
                            .line_height(theme.text(11.)),
                    ),
            );
        } else if row.unread && !row.muted {
            item = item.trailing(
                div()
                    .size(theme.space(1.5))
                    .rounded_full()
                    .bg(theme.colors.sidebar_foreground)
                    .mr(theme.space(1.))
                    .group_hover(ROW_GROUP, |style| style.invisible()),
            );
        }
        let menu_id = row.id.clone();
        let context_id = row.id.clone();
        let options = sidebar_menu_action(
            element_id("sidebar.channel.options", &row.id),
            format!("Options for {}", row.name),
            cx,
        )
        .child(
            lucide(LucideIcon::Ellipsis)
                .size(scaled(&theme, metrics::ICON_SMALL))
                .text_color(palette.sidebar_muted),
        )
        .on_click(
            cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                // The button floats over the row, whose own click would
                // otherwise fire too and open the channel.
                cx.stop_propagation();
                this.open_row_menu(&menu_id, event.position(), window, cx)
            }),
        );
        div()
            .id(element_id("sidebar.channel.row", &row.id))
            .group(ROW_GROUP)
            .relative()
            .w_full()
            .on_mouse_down(
                gpui_kit::MouseButton::Right,
                cx.listener(move |this, event: &gpui_kit::MouseDownEvent, window, cx| {
                    this.open_row_menu(&context_id, event.position, window, cx)
                }),
            )
            .child(item)
            .child(
                div()
                    .absolute()
                    .right(theme.space(1.))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .invisible()
                    .group_hover(ROW_GROUP, |style| style.visible())
                    .child(options),
            )
    }

    /// DESIGN.md §4: the Threads and Drafts entries sit above the categories,
    /// drawn like channel rows with a count badge where there is something
    /// to read or finish.
    fn fixed_entries(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let palette = Palette::read(cx);
        let entry = |id: &'static str,
                     label: &'static str,
                     glyph: LucideIcon,
                     count: EntryCount,
                     event: SidebarEvent,
                     cx: &mut Context<Self>| {
            let strong = count != EntryCount::None;
            let selected = self.model.selected.as_deref() == Some(id);
            let mut item = SidebarItem::new(id, label)
                .size(SidebarItemSize::Small)
                .selected(selected)
                .icon(
                    div()
                        .size(scaled(&theme, metrics::ROW_SLOT))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            lucide(glyph)
                                .size(scaled(&theme, metrics::ICON))
                                .text_color(palette.sidebar_muted),
                        ),
                )
                .type_style(if strong { style::UI_STRONG } else { style::UI }, &theme)
                .on_activate(cx.listener(move |_, _, _, cx| cx.emit(event.clone())));
            let badge = match count {
                EntryCount::None => None,
                EntryCount::Count(count) => {
                    Some(Badge::new(count.to_string()).variant(BadgeVariant::Secondary))
                }
                EntryCount::Mentions(count) => Some(
                    Badge::count(count, 99)
                        .bg(palette.mention)
                        .text_color(palette.mention_foreground),
                ),
            };
            if let Some(badge) = badge {
                item = item.trailing(
                    div().debug_selector(move || format!("{id}.count")).child(
                        badge
                            .h(theme.space(4.5))
                            .min_w(theme.space(4.5))
                            .px(theme.space(1.5))
                            .text_size(theme.text(11.))
                            .line_height(theme.text(11.)),
                    ),
                );
            } else {
                item = item.text_color(palette.sidebar_muted);
            }
            item
        };
        div()
            .flex()
            .flex_col()
            .gap(px(1.))
            .pb(theme.space(3.))
            .child(entry(
                THREADS_ID,
                "Threads",
                LucideIcon::MessageSquareText,
                self.model.threads_count(),
                SidebarEvent::OpenThreads,
                cx,
            ))
            .child(entry(
                DRAFTS_ID,
                "Drafts",
                LucideIcon::FilePenLine,
                match self.model.drafts {
                    0 => EntryCount::None,
                    count => EntryCount::Count(count),
                },
                SidebarEvent::OpenDrafts,
                cx,
            ))
    }

    /// A category's header: the collapsible trigger, which also opens the
    /// category's menu and is the handle for dragging the category.
    fn category_header(
        &self,
        category: &Category,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let palette = Palette::read(cx);
        let open = !category.collapsed;
        let id = category.id.clone();
        let menu_id = category.id.clone();
        let turn = transition_value(
            element_id("sidebar.category.chevron", &category.id),
            if open { 1. } else { 0. },
            theme.motion.fast,
            window,
            cx,
        );
        collapsible_trigger(element_id("sidebar.category", &category.id), open, cx)
            .aria_label(category.name.clone())
            .w_full()
            .h(theme.space(6.))
            .px(theme.space(2.))
            .py(px(0.))
            .flex()
            .items_center()
            .justify_start()
            .gap(theme.space(1.))
            .type_style(style::LABEL, &theme)
            .text_color(palette.sidebar_muted)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_category(&id, cx)))
            .on_mouse_down(
                gpui_kit::MouseButton::Right,
                cx.listener(move |this, event: &gpui_kit::MouseDownEvent, window, cx| {
                    this.open_category_menu(&menu_id, event.position, window, cx)
                }),
            )
            .child(
                // The chevron turns from right to down as the category opens
                // (DESIGN.md §1 motion), snapping under reduced motion.
                lucide(LucideIcon::ChevronRight)
                    .size(scaled(&theme, metrics::ICON_TINY))
                    .text_color(palette.sidebar_muted)
                    .with_transformation(gpui_kit::Transformation::rotate(gpui_kit::radians(
                        turn * std::f32::consts::FRAC_PI_2,
                    ))),
            )
            .child(category.name.to_uppercase())
    }

    /// The rows a category shows: all of them when open, and only unread
    /// rows and the open channel when collapsed, as Mattermost does.
    fn shown_rows<'a>(&self, category: &'a Category) -> Vec<&'a ChannelRow> {
        category
            .rows
            .iter()
            .filter(|row| {
                !category.collapsed || row.unread || self.model.selected.as_ref() == Some(&row.id)
            })
            .collect()
    }

    /// The categories as one list whose rows and headers drag, or move
    /// with Alt+Shift+Up and Down (#121).
    fn sortable(&self, cx: &mut Context<Self>) -> Sortable {
        let view = cx.entity();
        let groups = self.model.categories.iter().map(|category| {
            let header = {
                let (view, category) = (view.clone(), category.clone());
                move |window: &mut Window, cx: &mut App| {
                    view.update(cx, |this, cx| {
                        this.category_header(&category, window, cx)
                            .into_any_element()
                    })
                }
            };
            SortableGroup::new(
                element_id("sidebar.group", &category.id),
                category.name.clone(),
            )
            .header(header)
            .rows(self.shown_rows(category).into_iter().map(|row| {
                let (view, row) = (view.clone(), row.clone());
                SortableRow::new(
                    element_id("sidebar.drag", &row.id),
                    row.name.clone(),
                    move |_: &mut Window, cx: &mut App| {
                        // The 1px under each row keeps the static list's spacing.
                        let row = view.update(cx, |this, cx| this.row(&row, cx));
                        div().pb(px(1.)).child(row)
                    },
                )
            }))
        });
        let (rows, groups_moved) = (view.downgrade(), view.downgrade());
        let theme = UiTheme::read(cx);
        Sortable::new("sidebar.categories", "Categories")
            .gap(theme.space(3.))
            .reorder_groups(true)
            .groups(groups.collect::<Vec<_>>())
            .on_reorder(move |movement, _, cx| {
                rows.update(cx, |this, cx| this.place_channel(movement, cx))
                    .ok();
            })
            .on_reorder_group(move |from, to, _, cx| {
                groups_moved
                    .update(cx, |this, cx| this.place_category(from, to, cx))
                    .ok();
            })
    }

    /// Applies a dragged or Alt+Shift-moved row to the model and asks the
    /// window to save it. Rows a category may not hold stay put.
    fn place_channel(&mut self, movement: &SortableMove, cx: &mut Context<Self>) {
        let categories = &self.model.categories;
        let (Some(from), Some(to)) = (
            categories.get(movement.from.group),
            categories.get(movement.to.group),
        ) else {
            return;
        };
        let Some(row) = self
            .shown_rows(from)
            .get(movement.from.index)
            .map(|row| (*row).clone())
        else {
            return;
        };
        if from.id != to.id && !to.kind.takes(&row.kind) {
            return;
        }
        let full: Vec<SharedString> = to
            .rows
            .iter()
            .filter(|other| other.id != row.id)
            .map(|other| other.id.clone())
            .collect();
        let shown: Vec<SharedString> = self
            .shown_rows(to)
            .into_iter()
            .filter(|other| other.id != row.id)
            .map(|other| other.id.clone())
            .collect();
        let index = placed_index(&full, &shown, movement.to.index);
        let category_id = to.id.clone();
        self.model.remove(&row.id);
        let channel_id = row.id.clone();
        if let Some(category) = self.model.categories.get_mut(movement.to.group) {
            let index = index.min(category.rows.len());
            category.rows.insert(index, row);
        }
        cx.emit(SidebarEvent::PlaceChannel {
            channel_id,
            category_id,
            index,
        });
        cx.notify();
    }

    /// Applies a dragged or Alt+Shift-moved category header.
    fn place_category(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from >= self.model.categories.len() {
            return;
        }
        let id = self.model.categories[from].id.clone();
        move_group(&mut self.model.categories, from, to);
        cx.emit(SidebarEvent::PlaceCategory { id, index: to });
        cx.notify();
    }
}

/// Where a row dropped at `index` among a category's `shown` rows goes in
/// all of its rows, `full`; both leave the moved row out. Past the last
/// shown row, as on a collapsed category, it goes to the end.
fn placed_index(full: &[SharedString], shown: &[SharedString], index: usize) -> usize {
    shown
        .get(index)
        .and_then(|id| full.iter().position(|other| other == id))
        .unwrap_or(full.len())
}

impl Render for SidebarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let palette = Palette::read(cx);
        let mut sidebar = Sidebar::new("sidebar", "Channels")
            .roving_focus(true)
            .header(self.header(cx));
        if self.model.loading {
            for (label, rows) in [("Favorites", 2), ("Channels", 4), ("Direct messages", 3)] {
                sidebar = sidebar.child(
                    div()
                        .flex()
                        .flex_col()
                        .pb(theme.space(3.))
                        .child(
                            div()
                                .h(theme.space(6.))
                                .px(theme.space(2.))
                                .flex()
                                .items_center()
                                .type_style(style::LABEL, &theme)
                                .text_color(palette.sidebar_muted)
                                .child(label.to_uppercase()),
                        )
                        .children((0..rows).map(|row| sidebar_menu_skeleton(row % 2 == 0, cx))),
                );
            }
            return sidebar;
        }
        sidebar = sidebar.child(self.fixed_entries(cx));
        if self.unreads_only {
            // The filtered list leaves rows out, so it does not reorder.
            for category in &self.shown().categories {
                let header = self.category_header(category, window, cx);
                let rows: Vec<_> = self
                    .shown_rows(category)
                    .into_iter()
                    .map(|row| self.row(row, cx).into_any_element())
                    .collect();
                sidebar = sidebar.child(
                    div()
                        .flex()
                        .flex_col()
                        .pb(theme.space(3.))
                        .child(header)
                        .child(div().flex().flex_col().gap(px(1.)).children(rows)),
                );
            }
        } else {
            sidebar = sidebar.child(
                div()
                    .key_context(crate::actions::SIDEBAR_SORTABLE)
                    .child(self.sortable(cx)),
            );
        }
        // Under the filter with nothing to read, only the open channel is
        // left; say so rather than look broken.
        if self.unreads_only && self.model.caught_up() {
            sidebar = sidebar.child(div().pt(theme.space(4.)).child(style::empty_state(
                LucideIcon::CircleCheck,
                "You're all caught up",
                "Nothing unread. Turn the filter off to see every channel.",
                cx,
            )));
        }
        sidebar.child(
            Menu::new(&self.row_menu, "Channel options")
                .context_area(div())
                .disabled(true),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_dropped_among_shown_rows_land_between_the_same_rows_in_the_full_list() {
        let ids = |ids: &[&str]| {
            ids.iter()
                .map(|id| SharedString::from(*id))
                .collect::<Vec<_>>()
        };
        let full = ids(&["a", "b", "c", "d"]);
        // An open category shows every row: the index is unchanged.
        assert_eq!(placed_index(&full, &full, 0), 0);
        assert_eq!(placed_index(&full, &full, 2), 2);
        assert_eq!(placed_index(&full, &full, 4), 4);
        // A collapsed one shows only b and d: before d is before d.
        let shown = ids(&["b", "d"]);
        assert_eq!(placed_index(&full, &shown, 0), 1);
        assert_eq!(placed_index(&full, &shown, 1), 3);
        // Past the last shown row, or into an empty group, is the end.
        assert_eq!(placed_index(&full, &shown, 2), 4);
        assert_eq!(placed_index(&full, &[], 0), 4);
    }

    #[test]
    fn favorite_moves_rows_between_categories() {
        let mut model = SidebarModel {
            loading: false,
            selected: None,
            unread_threads: 0,
            thread_mentions: 0,
            drafts: 0,
            drafted: Default::default(),
            categories: vec![
                Category::new(FAVORITES_ID, "Favorites", vec![]),
                Category::new(
                    CHANNELS_ID,
                    "Channels",
                    vec![
                        ChannelRow::new("a", "a", ChannelKind::Public),
                        ChannelRow::new("b", "b", ChannelKind::Public),
                    ],
                ),
                Category::new(
                    DIRECT_ID,
                    "Direct messages",
                    vec![ChannelRow::new(
                        "d",
                        "Dana",
                        ChannelKind::Direct {
                            presence: Presence::Online,
                        },
                    )],
                ),
            ],
        };
        assert!(!model.is_favorite("a"));
        model.set_favorite("a", true);
        assert!(model.is_favorite("a"));
        assert_eq!(model.categories[1].rows.len(), 1);
        model.set_favorite("d", true);
        assert_eq!(model.categories[0].rows.len(), 2);
        model.set_favorite("a", false);
        model.set_favorite("d", false);
        assert_eq!(model.categories[0].rows.len(), 0);
        // Back home, appended after the rows that stayed.
        assert_eq!(model.categories[1].rows[1].id, "a");
        assert_eq!(model.categories[2].rows[0].id, "d");
        assert_eq!(model.remove("b").map(|row| row.id), Some("b".into()));
        assert!(model.remove("b").is_none());
    }

    fn ids(model: &SidebarModel) -> Vec<Vec<&str>> {
        model
            .categories
            .iter()
            .map(|category| category.rows.iter().map(|row| row.id.as_ref()).collect())
            .collect()
    }

    #[test]
    fn the_unreads_filter_keeps_unread_rows_and_the_open_channel() {
        // Muted c is left out; the open channel stays though it is read.
        assert_eq!(ids(&model("a").unreads()), [vec!["a", "b"], vec!["e"]]);
        assert_eq!(ids(&model("d").unreads()), [vec!["b"], vec!["d", "e"]]);
        // Alt+Down walks the filtered rows: from b straight to e.
        assert_eq!(
            model("b").unreads().neighbor(Step::Next, false),
            Some("e".into())
        );
    }

    #[test]
    fn the_unreads_filter_is_empty_when_everything_is_read() {
        let mut all_read = model("a");
        assert!(!all_read.caught_up());
        // Shift+Escape views every unread row on the server, muted ones too.
        assert_eq!(all_read.unread_ids(), ["b", "c", "e"]);
        all_read.mark_all_read();
        assert!(all_read.unread_ids().is_empty());
        // Only the open channel is left, and that is all caught up.
        assert_eq!(ids(&all_read.unreads()), [vec!["a"]]);
        assert!(all_read.caught_up());
        all_read.selected = None;
        assert!(all_read.unreads().categories.is_empty());
    }

    fn model(selected: &str) -> SidebarModel {
        SidebarModel {
            loading: false,
            selected: Some(selected.into()),
            unread_threads: 0,
            thread_mentions: 0,
            drafts: 0,
            drafted: Default::default(),
            categories: vec![
                Category::new(
                    "fav",
                    "Favorites",
                    vec![
                        ChannelRow::new("a", "a", ChannelKind::Public),
                        ChannelRow::new("b", "b", ChannelKind::Public).unread(),
                    ],
                ),
                Category::new(
                    "ch",
                    "Channels",
                    vec![
                        ChannelRow::new("c", "c", ChannelKind::Public)
                            .unread()
                            .muted(),
                        ChannelRow::new("d", "d", ChannelKind::Public),
                        ChannelRow::new("e", "e", ChannelKind::Public).mentions(1),
                    ],
                ),
            ],
        }
    }

    #[test]
    fn neighbor_walks_across_categories_and_wraps() {
        let m = model("b");
        assert_eq!(m.neighbor(Step::Next, false).as_deref(), Some("c"));
        assert_eq!(m.neighbor(Step::Previous, false).as_deref(), Some("a"));
        assert_eq!(model("e").neighbor(Step::Next, false).as_deref(), Some("a"));
        assert_eq!(
            model("a").neighbor(Step::Previous, false).as_deref(),
            Some("e")
        );
    }

    #[test]
    fn unread_neighbor_skips_read_and_muted_rows() {
        let m = model("a");
        assert_eq!(m.neighbor(Step::Next, true).as_deref(), Some("b"));
        assert_eq!(model("b").neighbor(Step::Next, true).as_deref(), Some("e"));
        assert_eq!(
            model("b").neighbor(Step::Previous, true).as_deref(),
            Some("e")
        );
        let mut read = model("a");
        read.mark_all_read();
        assert_eq!(read.neighbor(Step::Next, true), None);
        assert_eq!(read.neighbor(Step::Next, false).as_deref(), Some("b"));
    }

    /// #124: the Threads entry shows its mentions as channel rows do,
    /// otherwise how many threads are unread.
    #[test]
    fn the_threads_entry_counts_mentions_first() {
        let mut model = model("a");
        assert_eq!(model.threads_count(), EntryCount::None);
        model.unread_threads = 2;
        assert_eq!(model.threads_count(), EntryCount::Count(2));
        model.thread_mentions = 3;
        assert_eq!(model.threads_count(), EntryCount::Mentions(3));
        model.mark_all_read();
        assert_eq!(model.threads_count(), EntryCount::None);
    }

    #[test]
    fn mentions_imply_unread_and_set_model_keeps_collapse() {
        let row = ChannelRow::new("a", "a", ChannelKind::Public).mentions(3);
        assert!(row.unread);
        let mut old = SidebarModel::default();
        old.categories.push(Category {
            collapsed: true,
            ..Category::new("c", "C", vec![])
        });
        let mut next = SidebarModel::default();
        next.categories.push(Category::new("c", "C", vec![row]));
        // The same merge the view does.
        for category in &mut next.categories {
            if let Some(prev) = old.categories.iter().find(|c| c.id == category.id) {
                category.collapsed = prev.collapsed;
            }
        }
        assert!(next.categories[0].collapsed);
    }

    #[test]
    fn categories_take_only_the_rows_mattermost_allows() {
        let channel = ChannelKind::Public;
        let direct = ChannelKind::Direct {
            presence: Presence::Online,
        };
        let group = ChannelKind::Group { members: 3 };
        assert!(CategoryKind::Channels.takes(&channel));
        assert!(!CategoryKind::Channels.takes(&direct));
        assert!(CategoryKind::Direct.takes(&group));
        assert!(!CategoryKind::Direct.takes(&ChannelKind::Private));
        for any in [CategoryKind::Favorites, CategoryKind::Custom] {
            assert!(any.takes(&channel) && any.takes(&direct));
        }
        assert_eq!(CategoryKind::from_server("custom"), CategoryKind::Custom);
        assert_eq!(
            CategoryKind::from_server("direct_messages"),
            CategoryKind::Direct
        );
    }
}
