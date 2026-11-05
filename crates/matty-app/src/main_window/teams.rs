//! Switching teams (#89): the rail, Cmd/Ctrl+Alt+Up/Down and 1…9, and any
//! channel of another team (a notification, a link, history, a draft)
//! show that team's sidebar, Threads view, unreads and mentions. Each
//! team reopens where the user last was in it.

use gpui_kit::{Context, SharedString, Window};

use super::MainWindow;
use crate::sidebar::{DRAFTS_ID, THREADS_ID};

impl MainWindow {
    /// The team on screen: sync's pick with an account, the rail's in the
    /// preview.
    pub(super) fn current_team(&self, cx: &gpui_kit::App) -> Option<SharedString> {
        match &self.live {
            Some(live) => live.team().map(SharedString::from),
            None => self.rail.read(cx).current().cloned(),
        }
    }

    /// The rail, a shortcut or the menu picked `team`: shows it where the
    /// user last was in it, or at its town square the first time.
    pub(super) fn switch_team(
        &mut self,
        team: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.show_team(team.clone(), window, cx) {
            let place = self.team_places.get(&team).cloned();
            self.arrive_in_team(place, window, cx);
        }
    }

    /// The `n`th team on the rail, from Cmd/Ctrl+Alt+1…9.
    fn switch_team_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let team = self
            .rail
            .read(cx)
            .teams()
            .get(index)
            .map(|team| team.id.clone());
        if let Some(team) = team {
            self.switch_team(team, window, cx);
        }
    }

    /// Cmd/Ctrl+Alt+Up/Down: the team above or below on the rail, wrapping
    /// around as Mattermost does.
    fn step_team(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let teams: Vec<SharedString> = self
            .rail
            .read(cx)
            .teams()
            .iter()
            .map(|team| team.id.clone())
            .collect();
        if teams.len() < 2 {
            return;
        }
        let current = self.current_team(cx);
        let at = teams
            .iter()
            .position(|team| Some(team) == current.as_ref())
            .unwrap_or(0);
        let next = if down {
            (at + 1) % teams.len()
        } else {
            (at + teams.len() - 1) % teams.len()
        };
        self.switch_team(teams[next].clone(), window, cx);
    }

    /// Shows the team holding `channel_id` when it is not the one on
    /// screen, before the channel opens: a notification, a link, history
    /// or a draft may name any team's channel.
    pub(super) fn show_team_of(
        &mut self,
        channel_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let team: Option<SharedString> = match &self.live {
            Some(live) => live.team_of(channel_id).map(SharedString::from),
            None if crate::preview::enabled() => crate::preview::team_of(channel_id),
            None => None,
        };
        if let Some(team) = team
            && Some(&team) != self.current_team(cx).as_ref()
        {
            self.show_team(team, window, cx);
        }
    }

    /// Where the user is in the team on screen: a channel, Threads or
    /// Drafts, by its sidebar id.
    fn place(&self) -> Option<SharedString> {
        if self.threads_open {
            Some(THREADS_ID.into())
        } else if self.drafts_open {
            Some(DRAFTS_ID.into())
        } else {
            self.selected_channel.clone()
        }
    }

    /// Puts `team` on screen without opening anything in it: its sidebar,
    /// name, Threads list and badges. The right pane closes, as what it
    /// held belongs to the last team. `false` when `team` is already shown
    /// or not one of the user's.
    pub(super) fn show_team(
        &mut self,
        team: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let current = self.current_team(cx);
        if current.as_ref() == Some(&team) {
            return false;
        }
        let place = self.place();
        if let Some(live) = self.live.as_mut() {
            let Some(mut update) = live.switch_team(&team) else {
                return false;
            };
            if let Some((teams, current)) = update.teams.take() {
                self.rail
                    .update(cx, |rail, cx| rail.set_teams(teams, current, cx));
            }
            self.apply_update(update, window, cx);
        } else if crate::preview::enabled()
            && self.rail.read(cx).teams().iter().any(|t| t.id == team)
        {
            let name = self
                .rail
                .read(cx)
                .teams()
                .iter()
                .find(|t| t.id == team)
                .map(|t| t.name.clone())
                .unwrap_or_default();
            self.rail
                .update(cx, |rail, cx| rail.set_current(Some(team.clone()), cx));
            self.browse
                .update(cx, |browse, cx| browse.set_team_name(name.clone(), cx));
            let model = self.sidebar.update(cx, |sidebar, cx| {
                let mut model = crate::preview::team_sidebar(&team);
                model.selected = None;
                sidebar.set_team_name(name, cx);
                sidebar.set_model(model, cx);
                sidebar.model().clone()
            });
            self.switcher
                .update(cx, |switcher, cx| switcher.set_model(&model, cx));
        } else {
            return false;
        }
        if let (Some(old), Some(place)) = (current, place) {
            self.team_places.insert(old, place);
        }
        if self.right_pane_open {
            self.leave_thread();
            self.right_pane_open = false;
        }
        tracing::debug!(team = %team, "switched team");
        cx.notify();
        true
    }

    /// Opens `place` in the team just shown when it is still there, else
    /// its town square, else its first channel; with none, the channel
    /// view empties.
    fn arrive_in_team(
        &mut self,
        place: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows: Vec<crate::sidebar::ChannelRow> = self
            .sidebar
            .read(cx)
            .model()
            .categories
            .iter()
            .flat_map(|category| category.rows.iter().cloned())
            .collect();
        match arrival(place, &rows) {
            Arrival::Threads => self.show_threads(cx),
            Arrival::Drafts => self.show_drafts(cx),
            Arrival::Channel(id) => self.open_channel(id, window, cx),
            Arrival::Nothing => {
                self.threads_open = false;
                self.drafts_open = false;
                self.selected_channel = None;
                self.channel
                    .update(cx, |view, cx| view.set_model(None, window, cx));
            }
        }
    }

    /// The first sidebar from a server came with nothing open, as after
    /// signing in: the team opens where the user last was in it this
    /// session, else at its town square or first channel, so the window
    /// does not sit empty while connected.
    pub(super) fn open_first_channel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let somewhere = self.selected_channel.is_some() || self.threads_open || self.drafts_open;
        if somewhere || self.live.is_none() {
            return;
        }
        let place = self
            .rail
            .read(cx)
            .current()
            .and_then(|team| self.team_places.get(team).cloned());
        self.arrive_in_team(place, window, cx);
    }

    /// Sync's teams changed: the rail follows. When the team on screen is
    /// gone (the user left it or was removed), the window moves on to the
    /// team sync shows instead.
    pub(super) fn show_rail_teams(
        &mut self,
        teams: Vec<crate::rail::Team>,
        current: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.rail.read(cx).current().cloned();
        let gone = before
            .as_ref()
            .is_some_and(|before| !teams.iter().any(|team| &team.id == before));
        self.rail
            .update(cx, |rail, cx| rail.set_teams(teams, current.clone(), cx));
        let somewhere = self.selected_channel.is_some() || self.threads_open || self.drafts_open;
        if gone && somewhere && current.is_some() && current != before {
            if self.right_pane_open {
                self.leave_thread();
                self.right_pane_open = false;
            }
            let place = current.and_then(|team| self.team_places.get(&team).cloned());
            self.arrive_in_team(place, window, cx);
        }
    }

    /// Cmd/Ctrl+Alt+Up/Down and 1…9, on the window's root.
    pub(super) fn on_team_actions(root: gpui_kit::Div, cx: &mut Context<Self>) -> gpui_kit::Div {
        use crate::actions::*;
        use gpui_kit::InteractiveElement as _;
        macro_rules! switch {
            ($root:expr, $($action:ident => $index:literal),*) => {
                $root$(.on_action(cx.listener(|this, _: &$action, window, cx| {
                    this.switch_team_at($index, window, cx)
                })))*
            };
        }
        let root = root
            .on_action(
                cx.listener(|this, _: &PreviousTeam, window, cx| this.step_team(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NextTeam, window, cx| this.step_team(true, window, cx)),
            );
        switch!(
            root,
            SwitchTeam1 => 0,
            SwitchTeam2 => 1,
            SwitchTeam3 => 2,
            SwitchTeam4 => 3,
            SwitchTeam5 => 4,
            SwitchTeam6 => 5,
            SwitchTeam7 => 6,
            SwitchTeam8 => 7,
            SwitchTeam9 => 8
        )
    }
}

/// Where arriving in a team lands.
#[derive(Debug, PartialEq, Eq)]
enum Arrival {
    Threads,
    Drafts,
    Channel(SharedString),
    Nothing,
}

/// Where arriving in a team with `rows` lands: `place`, where the user
/// last was in it, while it is still there; else its default channel.
fn arrival(place: Option<SharedString>, rows: &[crate::sidebar::ChannelRow]) -> Arrival {
    match place {
        Some(place) if place.as_ref() == THREADS_ID => Arrival::Threads,
        Some(place) if place.as_ref() == DRAFTS_ID => Arrival::Drafts,
        Some(place) if rows.iter().any(|row| row.id == place) => Arrival::Channel(place),
        _ => default_channel(rows).map_or(Arrival::Nothing, Arrival::Channel),
    }
}

/// The channel a team opens on the first time: its town square, as in
/// Mattermost, else its first channel, else its first conversation.
fn default_channel(rows: &[crate::sidebar::ChannelRow]) -> Option<SharedString> {
    use crate::sidebar::ChannelKind;
    let town_square = |row: &&crate::sidebar::ChannelRow| {
        row.handle.as_deref() == Some("town-square")
            || row.name.as_ref() == "town-square"
            || row.name.as_ref() == "Town Square"
    };
    let channel = |row: &&crate::sidebar::ChannelRow| {
        !matches!(
            row.kind,
            ChannelKind::Direct { .. } | ChannelKind::Group { .. }
        )
    };
    rows.iter()
        .find(town_square)
        .or_else(|| rows.iter().find(channel))
        .or_else(|| rows.first())
        .map(|row| row.id.clone())
}

#[cfg(test)]
mod tests {
    use super::{Arrival, arrival, default_channel};
    use crate::sidebar::{ChannelKind, ChannelRow, Presence};

    /// After sign-in the team opens where the user last was in it, while
    /// that channel is still there, else at its default channel; a team
    /// with nothing leaves the view empty.
    #[test]
    fn arriving_opens_the_last_place_or_the_default_channel() {
        let design = ChannelRow::new("c1", "Design", ChannelKind::Public);
        let mut town = ChannelRow::new("c2", "Town Square", ChannelKind::Public);
        town.handle = Some("town-square".into());
        let rows = vec![design, town];
        assert_eq!(arrival(None, &rows), Arrival::Channel("c2".into()));
        assert_eq!(
            arrival(Some("c1".into()), &rows),
            Arrival::Channel("c1".into())
        );
        assert_eq!(
            arrival(Some("gone".into()), &rows),
            Arrival::Channel("c2".into())
        );
        assert_eq!(
            arrival(Some(super::THREADS_ID.into()), &rows),
            Arrival::Threads
        );
        assert_eq!(arrival(None, &[]), Arrival::Nothing);
    }

    #[test]
    fn a_team_opens_first_at_its_town_square() {
        let dm = ChannelRow::new(
            "dm",
            "Ana",
            ChannelKind::Direct {
                presence: Presence::Online,
            },
        );
        let design = ChannelRow::new("c1", "Design", ChannelKind::Public);
        let mut town = ChannelRow::new("c2", "Town Square", ChannelKind::Public);
        town.handle = Some("town-square".into());
        let rows = vec![dm.clone(), design.clone(), town];
        assert_eq!(default_channel(&rows).as_deref(), Some("c2"));
        let rows = vec![dm.clone(), design];
        assert_eq!(default_channel(&rows).as_deref(), Some("c1"));
        assert_eq!(default_channel(&[dm]).as_deref(), Some("dm"));
        assert_eq!(default_channel(&[]), None);
    }
}
