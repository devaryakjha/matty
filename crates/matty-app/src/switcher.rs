//! The quick switcher (Cmd/Ctrl+K): a command palette over the sidebar's
//! channels and people, DESIGN.md §4, with the user's other teams and
//! their channels (#90) and, on request, other servers' channels (#206).
//! Typing filters by name and keyword; Enter opens the highlighted row.

use gpui_icons::LucideIcon;
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _,
    SharedString, Styled as _, Subscription, Window, div,
};

use crate::sidebar::{ChannelKind, Presence, SidebarModel};
use crate::style::{self, TypeStyleExt as _};
use crate::t;
use crate::ui::{
    avatar::{Avatar, AvatarSize},
    badge::Badge,
    command::{CommandEvent, CommandItem, CommandState, command_dialog_popup},
    dialog::{DialogHandle, dialog},
    theme::UiTheme,
};

mod places;

use places::{ALL_SERVERS, NO_SERVER_MATCH, SERVER_PREFIX, TEAM_PREFIX};
pub use places::{Elsewhere, ServerChannels, TeamChannels};

/// What the switcher asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitcherEvent {
    /// The user picked a channel by id, of any of their teams on the shown
    /// server.
    Open(SharedString),
    /// The user picked one of their other teams (#90).
    Team(SharedString),
    /// The user picked a channel of another saved server, by the server's
    /// key on the rail (#206).
    OpenOnServer {
        server: SharedString,
        channel: SharedString,
    },
    /// The user chose "Search messages for …" with this query.
    Search(SharedString),
    /// The user picked where to forward `post` (#213).
    Forward {
        post: SharedString,
        channel: SharedString,
    },
    /// The user picked a channel to add `user` to (#212).
    AddTo {
        user: crate::profile::UserProfile,
        channel: SharedString,
    },
    /// The user pasted a link and chose "Open link" (#85).
    OpenLink(SharedString),
}

/// The value of the search row; the query follows it.
const SEARCH_PREFIX: &str = "~search:";
/// The value of the open-link row; the link follows it.
const LINK_PREFIX: &str = "~link:";

/// Keeps the rows of public and private channels: people join channels,
/// not conversations (#212).
fn only_channels(model: &SidebarModel, items: &mut Vec<CommandItem>) {
    let channels: std::collections::HashSet<&SharedString> = model
        .categories
        .iter()
        .flat_map(|category| category.rows.iter())
        .filter(|row| matches!(row.kind, ChannelKind::Public | ChannelKind::Private))
        .map(|row| &row.id)
        .collect();
    items.retain(|item| channels.contains(&item.value));
}

/// A channel the switcher picks for something other than opening it.
#[derive(Clone, Debug)]
enum Pick {
    /// Where to forward this post (#213).
    Forward(SharedString),
    /// A channel to add this person to (#212): the team's channels only.
    AddTo(crate::profile::UserProfile),
}

pub struct Switcher {
    handle: DialogHandle,
    command: Entity<CommandState>,
    /// The shown team's sidebar.
    model: SidebarModel,
    /// Other teams, recent channels and other servers, from the window.
    elsewhere: Elsewhere,
    /// "Search all servers" was chosen: other servers' channels list too.
    all_servers: bool,
    query: SharedString,
    /// What the switcher picks a channel for, instead of opening one.
    forwarding: Option<Pick>,
    _subscription: Subscription,
}

impl EventEmitter<SwitcherEvent> for Switcher {}

impl Switcher {
    pub fn new(model: &SidebarModel, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let handle = DialogHandle::new(false);
        let command = cx.new(|cx| {
            let mut state = CommandState::new(Vec::new(), window, cx);
            // The switcher matches and ranks the rows itself.
            state.set_filter(false, cx);
            state.set_placeholder(t!("switcher-placeholder"), window, cx);
            state
        });
        let close = handle.clone();
        let subscription =
            cx.subscribe_in(
                &command,
                window,
                move |this, _, event, window, cx| match event {
                    CommandEvent::Confirm(value) if value.as_ref() == ALL_SERVERS => {
                        // Stays open, now listing the other servers' channels.
                        this.all_servers = true;
                        this.refresh(cx);
                    }
                    CommandEvent::Confirm(value) if value.as_ref() == NO_SERVER_MATCH => {}
                    CommandEvent::Confirm(value) => {
                        close.close(window, cx);
                        if let Some(event) = this.event_for(value) {
                            cx.emit(event);
                        }
                        this.reset(window, cx);
                    }
                    CommandEvent::Cancel => {
                        close.close(window, cx);
                        this.forwarding = None;
                        this.reset(window, cx);
                    }
                    CommandEvent::Query(query) => {
                        this.query = query.clone();
                        this.refresh(cx);
                    }
                },
            );
        let mut switcher = Self {
            handle,
            command,
            model: model.clone(),
            elsewhere: Elsewhere::default(),
            all_servers: false,
            query: SharedString::default(),
            forwarding: None,
            _subscription: subscription,
        };
        switcher.refresh(cx);
        switcher
    }

    /// What picking the row `value` asks of the window.
    fn event_for(&mut self, value: &SharedString) -> Option<SwitcherEvent> {
        match self.forwarding.take() {
            Some(Pick::Forward(post)) => {
                return Some(SwitcherEvent::Forward {
                    post,
                    channel: value.clone(),
                });
            }
            Some(Pick::AddTo(user)) => {
                return Some(SwitcherEvent::AddTo {
                    user,
                    channel: value.clone(),
                });
            }
            None => {}
        }
        Some(if let Some(query) = value.strip_prefix(SEARCH_PREFIX) {
            SwitcherEvent::Search(query.into())
        } else if let Some(url) = value.strip_prefix(LINK_PREFIX) {
            SwitcherEvent::OpenLink(url.into())
        } else if let Some(team) = value.strip_prefix(TEAM_PREFIX) {
            SwitcherEvent::Team(team.into())
        } else if let Some(rest) = value.strip_prefix(SERVER_PREFIX) {
            let (index, channel) = rest.split_once(':')?;
            let server = self.elsewhere.servers.get(index.parse::<usize>().ok()?)?;
            SwitcherEvent::OpenOnServer {
                server: server.key.clone(),
                channel: channel.into(),
            }
        } else {
            SwitcherEvent::Open(value.clone())
        })
    }

    /// Lists the rows for the query: the channels ranked, a pasted link's
    /// "Open link" first, and "Search messages for …" last.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.query.trim().to_string();
        // Forwarding picks among the shown team's conversations alone.
        let none = Elsewhere::default();
        let elsewhere = if self.forwarding.is_some() {
            &none
        } else {
            &self.elsewhere
        };
        let (mut items, trailing) = places::items(&self.model, elsewhere, &query, self.all_servers);
        if matches!(self.forwarding, Some(Pick::AddTo(_))) {
            only_channels(&self.model, &mut items);
        }
        if self.forwarding.is_none() && crate::links::looks_like_link(&query) {
            items.insert(
                0,
                CommandItem::new(format!("{LINK_PREFIX}{query}"), t!("switcher-open-link"))
                    .group(t!("switcher-group-link"))
                    .icon(LucideIcon::Link)
                    .keywords([query.clone()]),
            );
        }
        if !query.is_empty() && self.forwarding.is_none() {
            items.push(
                CommandItem::new(
                    format!("{SEARCH_PREFIX}{query}"),
                    t!("switcher-search-messages", query = query.to_string()),
                )
                .group(t!("switcher-group-messages"))
                .icon(LucideIcon::Search)
                .keywords([query]),
            );
        }
        self.command
            .update(cx, |state, cx| state.set_items(items, cx));
        Self::install_slots(&self.command, &self.model, trailing, cx);
    }

    /// Rebuilds the rows from a new sidebar snapshot.
    pub fn set_model(&mut self, model: &SidebarModel, cx: &mut Context<Self>) {
        self.model = model.clone();
        self.refresh(cx);
    }

    /// Takes what the window gathered of other teams, recent channels and
    /// other servers, as the switcher opens.
    pub fn set_elsewhere(&mut self, elsewhere: Elsewhere, cx: &mut Context<Self>) {
        self.elsewhere = elsewhere;
        self.refresh(cx);
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
        self.forwarding = Some(Pick::Forward(post));
        self.refresh(cx);
        self.command.update(cx, |state, cx| {
            state.set_placeholder(t!("switcher-forward-placeholder"), window, cx)
        });
        self.open(window, cx);
    }

    /// Opens the switcher to pick a channel of the shown team to add
    /// `user` to (#212).
    pub fn open_add_to(
        &mut self,
        user: crate::profile::UserProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = t!(
            "switcher-add-to-placeholder",
            name = user.display_name.to_string()
        );
        self.forwarding = Some(Pick::AddTo(user));
        self.refresh(cx);
        self.command.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx)
        });
        self.open(window, cx);
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.all_servers = false;
        self.command.update(cx, |state, cx| {
            state.set_query("", window, cx);
            state.set_placeholder(t!("switcher-placeholder"), window, cx);
        });
    }

    /// Direct messages lead with the person's avatar; mention counts sit
    /// at the trailing edge, after the team a row from another names as its
    /// description.
    fn install_slots(
        command: &Entity<CommandState>,
        model: &SidebarModel,
        trailing: Vec<(SharedString, usize)>,
        cx: &mut App,
    ) {
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
                                .aria_label(name.clone())
                                .status(style::avatar_status(*presence, &style::Palette::read(cx)))
                                .status_size(style::scaled(theme, 6.))
                                .ring_color(theme.colors.popover)
                                .w(slot)
                                .h(slot)
                                .text_size(theme.text(9.)),
                            )
                            .into_any_element(),
                    )
                },
                cx,
            );
            state.set_trailing(
                move |item, _, cx| {
                    let theme = UiTheme::read(cx);
                    let (_, mentions) = trailing.iter().find(|(value, _)| *value == item.value)?;
                    Some(
                        Badge::count(*mentions, 99)
                            .h(theme.space(4.5))
                            .min_w(theme.space(4.5))
                            .px(theme.space(1.5))
                            .text_size(theme.text(11.))
                            .line_height(theme.text(11.))
                            .into_any_element(),
                    )
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
        let (items, _) = places::items(&model, &Elsewhere::default(), "", false);
        assert_eq!(items[0].label.as_ref(), "Ada Lovelace");
        assert_eq!(
            items[0].keywords,
            vec![SharedString::from("unread"), "ada".into()]
        );
        let (items, _) = places::items(&model, &Elsewhere::default(), "ada", false);
        assert_eq!(items[0].value.as_ref(), "d1");
    }

    #[test]
    fn adding_someone_to_a_channel_lists_channels_only() {
        let rows = vec![
            ChannelRow::new("c1", "design", ChannelKind::Public),
            ChannelRow::new("c2", "secret", ChannelKind::Private),
            ChannelRow::new(
                "d1",
                "Ada Lovelace",
                ChannelKind::Direct {
                    presence: Presence::Online,
                },
            ),
        ];
        let model = SidebarModel {
            categories: vec![Category::new("all", "Channels", rows)],
            ..SidebarModel::default()
        };
        let (mut items, _) = places::items(&model, &Elsewhere::default(), "", false);
        only_channels(&model, &mut items);
        let values: Vec<_> = items.iter().map(|item| item.value.to_string()).collect();
        assert_eq!(values.len(), 2, "{values:?}");
        assert!(values.contains(&"c1".to_string()) && values.contains(&"c2".to_string()));
    }
}
