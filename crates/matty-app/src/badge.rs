//! The app icon's badge (#141, PLAN §4.6): the user's mention count, a dot
//! when something is unread without a mention, nothing otherwise.

use gpui_kit::SharedString;
use matty_notify::Badge;

use crate::rail::Team;
use crate::sidebar::SidebarModel;

/// The badge for what the sidebar shows of the current team, plus the
/// other teams on the rail (the current one is in the sidebar already).
///
/// Mentions in muted channels count, as the sidebar shows them; a muted
/// channel's other unread posts do not. Mentions in followed threads'
/// unread replies count, as the webapp's badge counts them; other unread
/// replies count as unread.
pub fn dock_badge(sidebar: &SidebarModel, teams: &[Team], current: Option<&SharedString>) -> Badge {
    if sidebar.loading {
        return Badge::Clear;
    }
    let rows = sidebar
        .categories
        .iter()
        .flat_map(|category| category.rows.iter());
    let mut mentions = sidebar.thread_mentions;
    let mut unread = sidebar.unread_threads > 0;
    for row in rows {
        mentions += row.mentions;
        unread |= row.unread && !row.muted;
    }
    for team in teams.iter().filter(|team| Some(&team.id) != current) {
        mentions += team.mentions;
        unread |= team.unread;
    }
    Badge::new(mentions, unread)
}

/// `badge` with the other saved servers' mentions and unreads added
/// (#206); the shown server, `current`, is in `badge` already.
pub fn with_servers(
    badge: Badge,
    servers: &[crate::rail::Server],
    current: Option<usize>,
) -> Badge {
    let (mut mentions, mut unread) = match badge {
        Badge::Clear => (0, false),
        Badge::Unread => (0, true),
        Badge::Mentions(count) => (count, true),
    };
    for (_, server) in servers
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != current)
    {
        mentions += server.mentions;
        unread |= server.unread;
    }
    Badge::new(mentions, unread)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar::{Category, ChannelKind, ChannelRow};

    fn sidebar(rows: Vec<ChannelRow>) -> SidebarModel {
        SidebarModel {
            categories: vec![Category::new("channels", "Channels", rows)],
            ..SidebarModel::default()
        }
    }

    fn row(id: &str) -> ChannelRow {
        ChannelRow::new(id, id, ChannelKind::Public)
    }

    #[test]
    fn other_servers_add_to_the_badge() {
        use crate::rail::Server;
        let servers = [
            Server::new("Acme").mentions(2),
            Server::new("Quiet").unread(true),
        ];
        assert_eq!(
            with_servers(Badge::Mentions(3), &servers, None),
            Badge::Mentions(5)
        );
        // The shown server's counts are the sidebar's.
        assert_eq!(with_servers(Badge::Clear, &servers, Some(0)), Badge::Unread);
        assert_eq!(
            with_servers(Badge::Clear, &[Server::new("Calm")], None),
            Badge::Clear
        );
    }

    #[test]
    fn mentions_add_up_across_channels_and_other_teams() {
        let model = sidebar(vec![row("a").mentions(2), row("b").mentions(1).muted()]);
        let current: SharedString = "t1".into();
        let teams = [
            Team::new("t1", "One").mentions(3),
            Team::new("t2", "Two").mentions(4),
        ];
        assert_eq!(
            dock_badge(&model, &teams, Some(&current)),
            Badge::Mentions(7)
        );
    }

    #[test]
    fn unread_without_mentions_is_a_dot_unless_muted() {
        assert_eq!(
            dock_badge(&sidebar(vec![row("a")]), &[], None),
            Badge::Clear
        );
        assert_eq!(
            dock_badge(&sidebar(vec![row("a").unread()]), &[], None),
            Badge::Unread
        );
        assert_eq!(
            dock_badge(&sidebar(vec![row("a").unread().muted()]), &[], None),
            Badge::Clear
        );
        let threads = SidebarModel {
            unread_threads: 1,
            ..sidebar(vec![])
        };
        assert_eq!(dock_badge(&threads, &[], None), Badge::Unread);
        let other = [Team::new("t2", "Two").unread()];
        assert_eq!(dock_badge(&sidebar(vec![]), &other, None), Badge::Unread);
    }

    /// #141: mentions in followed threads' unread replies count, as the
    /// webapp's badge counts them.
    #[test]
    fn thread_mentions_count_toward_the_badge() {
        let model = SidebarModel {
            unread_threads: 2,
            thread_mentions: 3,
            ..sidebar(vec![row("a").mentions(1)])
        };
        assert_eq!(dock_badge(&model, &[], None), Badge::Mentions(4));
        let quiet = SidebarModel {
            thread_mentions: 0,
            ..model
        };
        assert_eq!(dock_badge(&quiet, &[], None), Badge::Mentions(1));
    }

    #[test]
    fn nothing_shows_while_loading_or_signed_out() {
        assert_eq!(
            dock_badge(&SidebarModel::loading(), &[], None),
            Badge::Clear
        );
    }
}
