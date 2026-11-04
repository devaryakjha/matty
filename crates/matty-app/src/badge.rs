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
/// channel's other unread posts do not. Followed threads with unread
/// replies count as unread.
pub fn dock_badge(sidebar: &SidebarModel, teams: &[Team], current: Option<&SharedString>) -> Badge {
    if sidebar.loading {
        return Badge::Clear;
    }
    let rows = sidebar
        .categories
        .iter()
        .flat_map(|category| category.rows.iter());
    let mut mentions = 0;
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

    #[test]
    fn nothing_shows_while_loading_or_signed_out() {
        assert_eq!(
            dock_badge(&SidebarModel::loading(), &[], None),
            Badge::Clear
        );
    }
}
