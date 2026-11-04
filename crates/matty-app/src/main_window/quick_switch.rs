//! The quick switcher across teams and servers (#90, #206): as it opens,
//! the window hands it the user's other teams and their channels, the
//! channels opened lately and the other saved servers' channels; picking
//! one from elsewhere switches team or server first.

use gpui_kit::{Context, SharedString, Window};

use super::MainWindow;
use crate::switcher::{Elsewhere, SwitcherEvent, TeamChannels};

impl MainWindow {
    /// Cmd/Ctrl+K: the switcher, with what it lists beyond the shown team.
    pub(super) fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let elsewhere = self.elsewhere(cx);
        self.switcher.update(cx, |switcher, cx| {
            switcher.set_elsewhere(elsewhere, cx);
            switcher.open(window, cx);
        });
    }

    fn elsewhere(&self, cx: &Context<Self>) -> Elsewhere {
        // Back's history, latest first, each channel once.
        let mut recent: Vec<SharedString> = Vec::new();
        let visited = self.history.get(..=self.history_at).unwrap_or_default();
        for id in visited.iter().rev() {
            if !recent.contains(id) {
                recent.push(id.clone());
            }
        }
        let (teams, servers) = match &self.live {
            Some(live) => (live.other_teams(), self.other_server_channels(cx)),
            None if crate::preview::enabled() => {
                let rail = self.rail.read(cx);
                let current = rail.current();
                let teams = rail
                    .teams()
                    .iter()
                    .filter(|team| Some(&team.id) != current)
                    .map(|team| TeamChannels {
                        id: team.id.clone(),
                        name: team.name.clone(),
                        rows: crate::preview::team_sidebar(&team.id)
                            .categories
                            .into_iter()
                            .flat_map(|category| category.rows)
                            .collect(),
                    })
                    .collect();
                let shown = rail
                    .current_server()
                    .and_then(|index| rail.servers().get(index))
                    .map(|server| server.key.clone());
                let servers = crate::preview::other_servers()
                    .into_iter()
                    .filter(|server| Some(&server.key) != shown.as_ref())
                    .collect();
                (teams, servers)
            }
            None => Default::default(),
        };
        Elsewhere {
            teams,
            recent,
            servers,
        }
    }

    pub(super) fn on_switcher_event(
        &mut self,
        event: &SwitcherEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // Another team's channel shows its team first (`show_channel`).
            SwitcherEvent::Open(id) => self.open_channel(id.clone(), window, cx),
            SwitcherEvent::Team(team) => self.switch_team(team.clone(), window, cx),
            SwitcherEvent::OpenOnServer { server, channel } => {
                self.open_on_server(server, channel.clone(), window, cx)
            }
            SwitcherEvent::Search(query) => self.search(query.clone(), window, cx),
            SwitcherEvent::Forward { post, channel } => {
                self.ask_forward(post.clone(), channel.clone(), window, cx)
            }
            SwitcherEvent::OpenLink(url) => self.open_message_link(url.clone(), window, cx),
        }
    }

    /// Shows the server with rail key `server`, then its `channel`: at
    /// once when its cache already holds the channel, else once its
    /// sidebar does. A server that needs signing in again asks for that
    /// instead.
    fn open_on_server(
        &mut self,
        server: &SharedString,
        channel: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = self
            .rail
            .read(cx)
            .servers()
            .iter()
            .position(|candidate| &candidate.key == server);
        let Some(index) = index else {
            return;
        };
        self.switch_server(index, window, cx);
        if crate::preview::enabled() {
            if self.rail.read(cx).current_server() == Some(index) {
                self.open_channel(channel, window, cx);
            }
            return;
        }
        if !self.is_shown(server, cx) {
            return;
        }
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if live.team_of(&channel).is_some() {
            self.open_channel(channel, window, cx);
        } else {
            live.open_when_ready(channel.to_string());
        }
    }
}
