//! The quick switcher (Cmd/Ctrl+K): a command palette over the sidebar's
//! channels and people, DESIGN.md §4. Typing filters by name and keyword;
//! Enter opens the highlighted row.

use gpui_icons::LucideIcon;
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _,
    SharedString, Styled as _, Subscription, Window, div,
};

use crate::sidebar::{ChannelKind, Presence, SidebarModel};
use crate::style::{self, TypeStyleExt as _};
use crate::ui::{
    avatar::{Avatar, AvatarSize},
    badge::Badge,
    command::{CommandEvent, CommandItem, CommandState, command_dialog_popup},
    dialog::{DialogHandle, dialog},
    theme::UiTheme,
};

/// What the switcher asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitcherEvent {
    /// The user picked a channel by id.
    Open(SharedString),
    /// The user chose "Search messages for …" with this query.
    Search(SharedString),
}

/// The value of the search row; the query follows it.
const SEARCH_PREFIX: &str = "~search:";

pub struct Switcher {
    handle: DialogHandle,
    command: Entity<CommandState>,
    /// The channel rows; the search row is added behind them per query.
    items: Vec<CommandItem>,
    _subscription: Subscription,
}

impl EventEmitter<SwitcherEvent> for Switcher {}

impl Switcher {
    pub fn new(model: &SidebarModel, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let handle = DialogHandle::new(false);
        let command = cx.new(|cx| {
            let mut state = CommandState::new(Self::items(model), window, cx);
            state.set_placeholder("Jump to a channel or person…", window, cx);
            state
        });
        Self::install_slots(&command, model, cx);
        let close = handle.clone();
        let subscription =
            cx.subscribe_in(
                &command,
                window,
                move |this, _, event, window, cx| match event {
                    CommandEvent::Confirm(value) => {
                        close.close(window, cx);
                        match value.strip_prefix(SEARCH_PREFIX) {
                            Some(query) => cx.emit(SwitcherEvent::Search(query.into())),
                            None => cx.emit(SwitcherEvent::Open(value.clone())),
                        }
                        this.reset(window, cx);
                    }
                    CommandEvent::Cancel => {
                        close.close(window, cx);
                        this.reset(window, cx);
                    }
                    CommandEvent::Query(query) => this.offer_search(query.clone(), cx),
                },
            );
        Self {
            handle,
            command,
            items: Self::items(model),
            _subscription: subscription,
        }
    }

    /// Any typed query also offers a message search, last in the list.
    fn offer_search(&mut self, query: SharedString, cx: &mut Context<Self>) {
        let query = query.trim().to_string();
        let mut items = self.items.clone();
        if !query.is_empty() {
            items.push(
                CommandItem::new(
                    format!("{SEARCH_PREFIX}{query}"),
                    format!("Search messages for \u{201c}{query}\u{201d}"),
                )
                .group("Messages")
                .icon(LucideIcon::Search)
                .keywords([query]),
            );
        }
        self.command
            .update(cx, |state, cx| state.set_items(items, cx));
    }

    /// Rebuilds the rows from a new sidebar snapshot.
    pub fn set_model(&mut self, model: &SidebarModel, cx: &mut Context<Self>) {
        self.items = Self::items(model);
        let items = self.items.clone();
        self.command
            .update(cx, |state, cx| state.set_items(items, cx));
        Self::install_slots(&self.command, model, cx);
    }

    pub fn open(&self, window: &mut Window, cx: &mut App) {
        self.handle.open(window, cx);
        // The dialog mounts on the next frame; focus the input once it exists.
        let command = self.command.clone();
        window.on_next_frame(move |window, cx| {
            command.update(cx, |state, cx| state.focus(window, cx));
        });
    }

    fn reset(&self, window: &mut Window, cx: &mut App) {
        self.command
            .update(cx, |state, cx| state.set_query("", window, cx));
    }

    /// One row per sidebar channel, grouped as the sidebar groups them.
    fn items(model: &SidebarModel) -> Vec<CommandItem> {
        model
            .categories
            .iter()
            .flat_map(|category| {
                category.rows.iter().map(move |row| {
                    let icon = match row.kind {
                        ChannelKind::Public => LucideIcon::Hash,
                        ChannelKind::Private => LucideIcon::Lock,
                        ChannelKind::Direct { .. } => LucideIcon::User,
                        ChannelKind::Group { .. } => LucideIcon::Users,
                    };
                    let mut item = CommandItem::new(row.id.clone(), row.name.clone())
                        .group(category.name.clone())
                        .icon(icon);
                    if row.unread {
                        item = item.keywords(["unread"]);
                    }
                    item
                })
            })
            .collect()
    }

    /// Mention counts at the trailing edge, as in the sidebar.
    fn install_slots(command: &Entity<CommandState>, model: &SidebarModel, cx: &mut App) {
        let mentions: Vec<(SharedString, usize)> = model
            .categories
            .iter()
            .flat_map(|category| category.rows.iter())
            .filter(|row| row.mentions > 0)
            .map(|row| (row.id.clone(), row.mentions))
            .collect();
        // Direct messages lead with the person's avatar and presence (#135).
        let people: Vec<(SharedString, SharedString, Presence)> = model
            .categories
            .iter()
            .flat_map(|category| category.rows.iter())
            .filter_map(|row| match row.kind {
                ChannelKind::Direct { presence } => {
                    Some((row.id.clone(), row.name.clone(), presence))
                }
                _ => None,
            })
            .collect();
        command.update(cx, |state, cx| {
            state.set_leading(
                move |item, _, cx| {
                    let (id, name, presence) = people.iter().find(|(id, ..)| *id == item.value)?;
                    let theme = UiTheme::read(cx);
                    let slot = style::scaled(theme, 18.);
                    let initial: String = name.chars().take(1).collect();
                    Some(
                        div()
                            .relative()
                            .size(slot)
                            .flex_shrink_0()
                            .child(
                                Avatar::new(gpui_kit::ElementId::Name(
                                    format!("switcher.avatar.{id}").into(),
                                ))
                                .size(AvatarSize::Sm)
                                .fallback(initial)
                                .aria_label(style::presence_name(name, *presence))
                                .w(slot)
                                .h(slot)
                                .text_size(theme.text(9.)),
                            )
                            .child(style::presence_dot(
                                *presence,
                                style::scaled(theme, 8.),
                                theme.colors.popover,
                                theme,
                                &style::Palette::read(cx),
                            ))
                            .into_any_element(),
                    )
                },
                cx,
            );
            state.set_trailing(
                move |item, _, cx| {
                    let theme = UiTheme::read(cx);
                    mentions
                        .iter()
                        .find(|(id, _)| *id == item.value)
                        .map(|(_, count)| {
                            Badge::count(*count, 99)
                                .h(theme.space(4.5))
                                .min_w(theme.space(4.5))
                                .px(theme.space(1.5))
                                .text_size(theme.text(11.))
                                .line_height(theme.text(11.))
                                .into_any_element()
                        })
                },
                cx,
            );
        });
    }

    /// The dialog element; the window mounts it every frame so focus
    /// returns to the right place when it closes.
    pub fn element(&self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let popup = command_dialog_popup("switcher.popup", &self.command, cx)
            .max_w(style::scaled(&theme, style::metrics::SWITCHER_MAX_WIDTH))
            .type_style(style::UI, &theme);
        dialog("switcher", &self.handle, popup, window, cx)
    }
}
