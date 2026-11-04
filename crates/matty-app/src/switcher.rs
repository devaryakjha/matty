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
    /// The user picked where to forward `post` (#213).
    Forward {
        post: SharedString,
        channel: SharedString,
    },
    /// The user pasted a link and chose "Open link" (#85).
    OpenLink(SharedString),
}

/// The value of the search row; the query follows it.
const SEARCH_PREFIX: &str = "~search:";
/// The value of the open-link row; the link follows it.
const LINK_PREFIX: &str = "~link:";

pub struct Switcher {
    handle: DialogHandle,
    command: Entity<CommandState>,
    /// The channel rows; the search row is added behind them per query.
    items: Vec<CommandItem>,
    /// The post being forwarded, while the switcher picks its destination.
    forwarding: Option<SharedString>,
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
                        if let Some(post) = this.forwarding.take() {
                            cx.emit(SwitcherEvent::Forward {
                                post,
                                channel: value.clone(),
                            });
                        } else if let Some(query) = value.strip_prefix(SEARCH_PREFIX) {
                            cx.emit(SwitcherEvent::Search(query.into()));
                        } else if let Some(url) = value.strip_prefix(LINK_PREFIX) {
                            cx.emit(SwitcherEvent::OpenLink(url.into()));
                        } else {
                            cx.emit(SwitcherEvent::Open(value.clone()));
                        }
                        this.reset(window, cx);
                    }
                    CommandEvent::Cancel => {
                        close.close(window, cx);
                        this.forwarding = None;
                        this.reset(window, cx);
                    }
                    CommandEvent::Query(query) => this.offer_search(query.clone(), cx),
                },
            );
        Self {
            handle,
            command,
            items: Self::items(model),
            forwarding: None,
            _subscription: subscription,
        }
    }

    /// Any typed query also offers a message search, last in the list; a
    /// pasted link offers to open it, first.
    fn offer_search(&mut self, query: SharedString, cx: &mut Context<Self>) {
        let query = query.trim().to_string();
        let mut items = self.items.clone();
        if self.forwarding.is_none() && crate::links::looks_like_link(&query) {
            items.insert(
                0,
                CommandItem::new(format!("{LINK_PREFIX}{query}"), "Open link")
                    .group("Link")
                    .icon(LucideIcon::Link)
                    .keywords([query.clone()]),
            );
        }
        if !query.is_empty() && self.forwarding.is_none() {
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

    /// Opens the switcher to pick where to forward `post`.
    pub fn open_forward(
        &mut self,
        post: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.forwarding = Some(post);
        self.command.update(cx, |state, cx| {
            state.set_placeholder("Forward to a channel or person\u{2026}", window, cx)
        });
        self.open(window, cx);
    }

    fn reset(&self, window: &mut Window, cx: &mut App) {
        self.command.update(cx, |state, cx| {
            state.set_query("", window, cx);
            state.set_placeholder("Jump to a channel or person\u{2026}", window, cx);
        });
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
                    // `keywords` replaces, so the words go in one call: "unread"
                    // and, for a direct message, the person's username, which
                    // finds the row whatever the name format shows (#156).
                    let keywords: Vec<SharedString> = row
                        .unread
                        .then(|| SharedString::from("unread"))
                        .into_iter()
                        .chain(row.handle.clone())
                        .collect();
                    if !keywords.is_empty() {
                        item = item.keywords(keywords);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar::{Category, ChannelRow, Presence};

    #[test]
    fn a_dm_shows_its_formatted_name_and_is_found_by_username() {
        let dm = ChannelRow::new(
            "d1",
            "Ada Lovelace",
            ChannelKind::Direct {
                presence: Presence::Online,
            },
        )
        .person("u1")
        .handle("ada")
        .unread();
        let model = SidebarModel {
            categories: vec![Category::new("dms", "Direct messages", vec![dm])],
            ..SidebarModel::default()
        };
        let items = Switcher::items(&model);
        assert_eq!(items[0].label.as_ref(), "Ada Lovelace");
        assert_eq!(
            items[0].keywords,
            vec![SharedString::from("unread"), "ada".into()]
        );
    }
}
