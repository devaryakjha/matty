//! What the quick switcher lists and in what order (#90, #206): the shown
//! team's channels and people, the user's other teams' channels and the
//! teams themselves, and, after "Search all servers", the other saved
//! servers' channels.
//!
//! Ranked as Mattermost's switcher ranks: with no query, unread channels,
//! then the ones opened lately, then the rest by sidebar; with a query,
//! better matches first and, among equal matches, unread then recent, the
//! shown team's before other teams'.

use gpui_icons::LucideIcon;
use gpui_kit::SharedString;

use crate::sidebar::{ChannelKind, ChannelRow, SidebarModel};
use crate::t;
use crate::ui::command::CommandItem;

/// The value of a team's row; its id follows.
pub(super) const TEAM_PREFIX: &str = "~team:";
/// The value of another server's channel: the server's place in
/// [`Elsewhere::servers`], a colon, then the channel id.
pub(super) const SERVER_PREFIX: &str = "~server:";
/// The value of the "Search all servers" row.
pub(super) const ALL_SERVERS: &str = "~all-servers";
/// The value of the row saying no other server's channel matches.
pub(super) const NO_SERVER_MATCH: &str = "~no-server-match";

/// At most this many channels opened lately lead the list.
const RECENT: usize = 8;

/// One of the user's other teams on the shown server.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TeamChannels {
    pub id: SharedString,
    pub name: SharedString,
    /// Its channels, in sidebar order; direct messages belong to every
    /// team and come with the shown one.
    pub rows: Vec<ChannelRow>,
}

/// Another saved server and the channels its cache holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServerChannels {
    /// The server's key on the rail.
    pub key: SharedString,
    pub name: SharedString,
    /// Each channel with the name of its team, in team then sidebar order.
    pub channels: Vec<(SharedString, ChannelRow)>,
}

/// What the switcher lists beyond the shown team's sidebar, gathered by
/// the window as the switcher opens.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Elsewhere {
    /// The user's other teams on this server, in rail order.
    pub teams: Vec<TeamChannels>,
    /// Channels opened lately, the latest first.
    pub recent: Vec<SharedString>,
    /// The other saved servers, in rail order.
    pub servers: Vec<ServerChannels>,
}

/// What a row shows at its trailing edge.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Trailing {
    /// The team (and server) a row from elsewhere belongs to.
    pub note: Option<SharedString>,
    pub mentions: usize,
}

/// A channel the switcher may list, with where it comes from.
struct Place<'a> {
    row: &'a ChannelRow,
    value: SharedString,
    /// The sidebar category, or the team or server it is in.
    home: SharedString,
    /// The team's name for rows of other teams; with the server's for rows
    /// of other servers.
    note: Option<SharedString>,
    /// 0 for the shown team, then each other team, then other servers.
    team_order: usize,
}

impl Place<'_> {
    fn unread(&self) -> bool {
        self.row.mentions > 0 || (self.row.unread && !self.row.muted)
    }
}

/// The rows for `query`, ranked, with the trailing note and count of
/// each by value.
pub(super) fn items(
    model: &SidebarModel,
    elsewhere: &Elsewhere,
    query: &str,
    all_servers: bool,
) -> (Vec<CommandItem>, Vec<(SharedString, Trailing)>) {
    let typed = query.trim();
    let query = typed.to_lowercase();
    let mut places: Vec<Place> = Vec::new();
    for category in &model.categories {
        for row in &category.rows {
            places.push(Place {
                row,
                value: row.id.clone(),
                home: category.name.clone(),
                note: None,
                team_order: 0,
            });
        }
    }
    for (index, team) in elsewhere.teams.iter().enumerate() {
        for row in team.rows.iter().filter(|row| is_channel(row)) {
            places.push(Place {
                row,
                value: row.id.clone(),
                home: team.name.clone(),
                note: Some(team.name.clone()),
                team_order: index + 1,
            });
        }
    }
    if all_servers {
        let after = elsewhere.teams.len() + 1;
        for (index, server) in elsewhere.servers.iter().enumerate() {
            for (team, row) in &server.channels {
                places.push(Place {
                    row,
                    value: format!("{SERVER_PREFIX}{index}:{}", row.id).into(),
                    home: server.name.clone(),
                    note: Some(team.clone()),
                    team_order: after + index,
                });
            }
        }
    }
    let recent: Vec<&SharedString> = elsewhere
        .recent
        .iter()
        .filter(|id| model.selected.as_ref() != Some(*id))
        .take(RECENT)
        .collect();
    // Only this server's channels count as opened lately.
    let opened = |place: &Place| -> Option<usize> {
        if place.value.starts_with(SERVER_PREFIX) {
            return None;
        }
        recent.iter().position(|id| **id == place.row.id)
    };

    let mut ranked: Vec<(Rank, usize)> = Vec::new();
    for (index, place) in places.iter().enumerate() {
        let Some(matched) = match_rank(&query, place) else {
            continue;
        };
        let elsewhere_server = place.value.starts_with(SERVER_PREFIX);
        let opened = opened(place);
        // Rows of other servers keep to their own groups below this
        // server's, whatever they match.
        let first = if elsewhere_server {
            Bucket::Server
        } else if place.unread() {
            Bucket::Unread
        } else if opened.is_some() {
            Bucket::Recent
        } else {
            Bucket::Rest
        };
        ranked.push((
            Rank {
                server: elsewhere_server,
                matched: if query.is_empty() { 0 } else { matched },
                first,
                team_order: place.team_order,
                opened: opened.unwrap_or(usize::MAX),
                index,
            },
            index,
        ));
    }
    ranked.sort_by_key(|(rank, _)| rank.key(query.is_empty()));

    let mut items = Vec::new();
    let mut trailing = Vec::new();
    for (rank, index) in ranked {
        let place = &places[index];
        let group: SharedString = match rank.first {
            Bucket::Server => place.home.clone(),
            _ if !query.is_empty() => t!("switcher-group-matches"),
            Bucket::Unread => t!("switcher-group-unread"),
            Bucket::Recent => t!("switcher-group-recent"),
            Bucket::Rest => place.home.clone(),
        };
        items.push(item(place, group));
        if place.note.is_some() || place.row.mentions > 0 {
            trailing.push((
                place.value.clone(),
                Trailing {
                    note: place.note.clone(),
                    mentions: place.row.mentions,
                },
            ));
        }
    }
    if all_servers && !elsewhere.servers.is_empty() && !items.iter().any(is_server) {
        items.push(
            CommandItem::new(NO_SERVER_MATCH, t!("switcher-no-server-match"))
                .group(t!("switcher-group-servers"))
                .icon(LucideIcon::Server)
                .disabled(true),
        );
    }
    for team in &elsewhere.teams {
        let item = CommandItem::new(format!("{TEAM_PREFIX}{}", team.id), team.name.clone())
            .group(t!("switcher-group-teams"))
            .icon(LucideIcon::Building2)
            .keywords([t!("switcher-keyword-team")]);
        if query.is_empty() || matches(&query, &item.label, &item.keywords).is_some() {
            items.push(item);
        }
    }
    if !all_servers && !query.is_empty() && !elsewhere.servers.is_empty() {
        items.push(
            CommandItem::new(
                ALL_SERVERS,
                t!("switcher-search-servers", query = typed.to_string()),
            )
            .group(t!("switcher-group-servers"))
            .icon(LucideIcon::Server),
        );
    }
    (items, trailing)
}

fn is_server(item: &CommandItem) -> bool {
    item.value.starts_with(SERVER_PREFIX)
}

/// Channels, not conversations: these are what other teams and servers
/// add, as direct messages already come with the shown team.
pub(super) fn is_channel(row: &ChannelRow) -> bool {
    matches!(row.kind, ChannelKind::Public | ChannelKind::Private)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Bucket {
    Unread,
    Recent,
    Rest,
    Server,
}

struct Rank {
    server: bool,
    matched: u8,
    first: Bucket,
    team_order: usize,
    opened: usize,
    index: usize,
}

impl Rank {
    fn key(&self, empty: bool) -> (bool, u8, Bucket, usize, usize, usize) {
        if empty {
            // Unread, then lately opened in the order opened, then the
            // sidebar's order team by team.
            let opened = if self.first == Bucket::Recent {
                self.opened
            } else {
                0
            };
            let team = if self.first == Bucket::Recent {
                0
            } else {
                self.team_order
            };
            (self.server, 0, self.first, opened, team, self.index)
        } else {
            (
                self.server,
                self.matched,
                self.first,
                self.team_order,
                self.opened,
                self.index,
            )
        }
    }
}

fn match_rank(query: &str, place: &Place) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    let handle: Vec<SharedString> = place.row.handle.iter().cloned().collect();
    // A team's name finds its rows only by a word or a run of letters, as
    // its letters scattered would find most rows,
    let team = place
        .note
        .as_ref()
        .and_then(|note| matches(query, note, &[]))
        .filter(|rank| *rank < 2)
        // and ranks below a row whose own name matches as well.
        .map(|rank| rank + 1);
    matches(query, &place.row.name, &handle)
        .into_iter()
        .chain(team)
        .min()
}

/// How well `query` (lowercase) finds a row: 0 when the name or a word in
/// it starts with it, 1 when it holds it, 2 when it holds its letters in
/// order, as the palette's own filter allows; `None` when it does not.
fn matches(query: &str, label: &str, keywords: &[SharedString]) -> Option<u8> {
    std::iter::once(label)
        .chain(keywords.iter().map(|keyword| keyword.as_ref()))
        .filter_map(|text| {
            let text = text.to_lowercase();
            let word_start = text
                .split(|c: char| !c.is_alphanumeric())
                .any(|word| word.starts_with(query));
            if text.starts_with(query) || word_start {
                Some(0)
            } else if text.contains(query) {
                Some(1)
            } else {
                let mut chars = text.chars();
                query
                    .chars()
                    .all(|wanted| chars.any(|c| c == wanted))
                    .then_some(2)
            }
        })
        .min()
}

fn item(place: &Place, group: SharedString) -> CommandItem {
    let row = place.row;
    let icon = match row.kind {
        ChannelKind::Public => LucideIcon::Hash,
        ChannelKind::Private => LucideIcon::Lock,
        ChannelKind::Direct { .. } => LucideIcon::User,
        ChannelKind::Group { .. } => LucideIcon::Users,
    };
    let mut item = CommandItem::new(place.value.clone(), row.name.clone())
        .group(group)
        .icon(icon);
    // `keywords` replaces, so the words go in one call: "unread"; for a
    // direct message, the person's username, which finds the row whatever
    // the name format shows (#156); and the team a row of another is in.
    let keywords: Vec<SharedString> = row
        .unread
        .then(|| t!("switcher-keyword-unread"))
        .into_iter()
        .chain(row.handle.clone())
        .chain(place.note.clone())
        .collect();
    if !keywords.is_empty() {
        item = item.keywords(keywords);
    }
    item
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar::{Category, Presence};

    fn public(id: &str) -> ChannelRow {
        ChannelRow::new(id, id, ChannelKind::Public)
    }

    fn shown() -> SidebarModel {
        SidebarModel {
            selected: Some("general".into()),
            categories: vec![
                Category::new(
                    "channels",
                    "Channels",
                    vec![
                        public("general"),
                        public("random"),
                        public("town-square").unread(),
                        public("quiet").unread().muted(),
                    ],
                ),
                Category::new(
                    "direct",
                    "Direct messages",
                    vec![
                        ChannelRow::new(
                            "dm",
                            "Ada Lovelace",
                            ChannelKind::Direct {
                                presence: Presence::Online,
                            },
                        )
                        .handle("ada"),
                    ],
                ),
            ],
            ..SidebarModel::default()
        }
    }

    fn elsewhere() -> Elsewhere {
        Elsewhere {
            teams: vec![TeamChannels {
                id: "t2".into(),
                name: "Design Guild".into(),
                rows: vec![
                    public("critique").unread().mentions(2),
                    public("general-design"),
                    public("motion"),
                    // A direct message is the shown team's already.
                    ChannelRow::new(
                        "dm",
                        "Ada Lovelace",
                        ChannelKind::Direct {
                            presence: Presence::Online,
                        },
                    ),
                ],
            }],
            recent: vec!["motion".into(), "general".into(), "random".into()],
            servers: vec![ServerChannels {
                key: "acme".into(),
                name: "Acme Corp".into(),
                channels: vec![
                    ("Engineering".into(), public("deploys")),
                    ("Engineering".into(), public("general-acme")),
                ],
            }],
        }
    }

    fn rows(items: &[CommandItem]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|item| {
                (
                    item.group.clone().unwrap_or_default().to_string(),
                    item.value.to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn with_no_query_unread_then_recent_then_each_team_and_the_teams() {
        let (items, trailing) = super::items(&shown(), &elsewhere(), "", false);
        let expected: Vec<(String, String)> = [
            ("Unread", "town-square"),
            ("Unread", "critique"),
            // The open channel is not "recent"; Design Guild's motion is.
            ("Recent", "motion"),
            ("Recent", "random"),
            ("Channels", "general"),
            ("Channels", "quiet"),
            ("Direct messages", "dm"),
            ("Design Guild", "general-design"),
            ("Teams", "~team:t2"),
        ]
        .into_iter()
        .map(|(group, value)| (group.into(), value.into()))
        .collect();
        assert_eq!(rows(&items), expected);
        // Other teams' rows name their team; counts come along.
        assert_eq!(
            trailing,
            vec![
                (
                    "critique".into(),
                    Trailing {
                        note: Some("Design Guild".into()),
                        mentions: 2
                    }
                ),
                (
                    "motion".into(),
                    Trailing {
                        note: Some("Design Guild".into()),
                        mentions: 0
                    }
                ),
                (
                    "general-design".into(),
                    Trailing {
                        note: Some("Design Guild".into()),
                        mentions: 0
                    }
                ),
            ]
        );
    }

    #[test]
    fn a_query_ranks_better_matches_then_the_shown_team_first() {
        let (items, _) = super::items(&shown(), &elsewhere(), "gen", false);
        assert_eq!(
            rows(&items),
            vec![
                ("Channels and people".into(), "general".into()),
                ("Channels and people".into(), "general-design".into()),
                ("Other servers".into(), "~all-servers".into()),
            ]
        );
        assert_eq!(
            items.last().unwrap().label.as_ref(),
            "Search all servers for \u{201c}gen\u{201d}"
        );
        // A team's name finds the team and its channels.
        let (items, _) = super::items(&shown(), &elsewhere(), "design guild", false);
        let values: Vec<_> = items.iter().map(|item| item.value.to_string()).collect();
        assert_eq!(
            values,
            [
                "critique",
                "motion",
                "general-design",
                "~team:t2",
                "~all-servers"
            ]
        );
        // The username finds the direct message.
        let (items, _) = super::items(&shown(), &elsewhere(), "ada", false);
        assert_eq!(items[0].value.as_ref(), "dm");
    }

    #[test]
    fn searching_all_servers_adds_their_matching_channels() {
        let (items, trailing) = super::items(&shown(), &elsewhere(), "gen", true);
        assert_eq!(
            rows(&items),
            vec![
                ("Channels and people".into(), "general".into()),
                ("Channels and people".into(), "general-design".into()),
                ("Acme Corp".into(), "~server:0:general-acme".into()),
            ]
        );
        assert!(trailing.iter().any(|(value, trailing)| value.as_ref()
            == "~server:0:general-acme"
            && trailing.note.as_deref() == Some("Engineering")));
        // Nothing matching says so.
        let (items, _) = super::items(&shown(), &elsewhere(), "random", true);
        assert_eq!(
            rows(&items),
            vec![
                ("Channels and people".into(), "random".into()),
                ("Other servers".into(), NO_SERVER_MATCH.into()),
            ]
        );
        assert!(items[1].disabled);
    }

    #[test]
    fn with_no_other_teams_or_servers_the_list_is_the_sidebar() {
        let (items, trailing) = super::items(&shown(), &Elsewhere::default(), "", false);
        let values: Vec<_> = items.iter().map(|item| item.value.to_string()).collect();
        assert_eq!(values, ["town-square", "general", "random", "quiet", "dm"]);
        assert!(trailing.is_empty());
    }
}
