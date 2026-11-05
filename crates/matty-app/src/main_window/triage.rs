//! Keyboard triage (#223): Alt+Shift+Up/Down walk the unread channels and
//! carry on into the next team with unreads when the team on screen has
//! none left that way, then into the next server with unreads. While a channel reached that way is open, Escape
//! reads it and moves on to the next unread one. Cmd/Ctrl+Shift+A shows
//! the unread digest.

use gpui_kit::{Context, SharedString, Window};

use super::MainWindow;
use crate::right_pane::RightPaneContent;
use crate::sidebar::Step;

impl MainWindow {
    /// The next unread channel in `step`'s direction: in this team, then in
    /// the nearest team on the rail with unreads, then on the nearest
    /// server with unreads, then wrapping round this team's own.
    pub(super) fn step_unread(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        let ahead = self.sidebar.read(cx).shown().unread_ahead(step);
        if let Some(id) = ahead {
            return self.triage_to(id, window, cx);
        }
        if self.triage_other_team(step, window, cx) {
            return;
        }
        if let Some(index) = self.unread_server(step, cx) {
            self.switch_server(index, window, cx);
            // A server whose session is gone asks to sign in instead.
            if self.settings.read(cx).get().current_server() == Some(index) {
                self.triage_on_load = Some(step);
                return;
            }
        }
        let wrapped = self.sidebar.read(cx).shown().neighbor(step, true);
        if let Some(id) = wrapped {
            self.triage_to(id, window, cx);
        }
    }

    /// After triage switched servers, once the new server's sidebar is
    /// here: its first (or last) unread channel, in the team it opens on
    /// or the nearest other one.
    pub(super) fn continue_triage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(step) = self.triage_on_load.take() else {
            return;
        };
        let mut shown = self.sidebar.read(cx).shown().into_owned();
        shown.selected = None;
        let first = shown.unread_ahead(step);
        match first {
            Some(id) => self.triage_to(id, window, cx),
            None => {
                self.triage_other_team(step, window, cx);
            }
        }
    }

    /// Opens the first (or last) unread channel of the nearest other team
    /// with unreads; `false` when there is none.
    fn triage_other_team(
        &mut self,
        step: Step,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(team) = self.unread_team(step, cx) else {
            return false;
        };
        if !self.show_team(team, window, cx) {
            return false;
        }
        // The new team's sidebar is here from the cache: start at its
        // first (or last) unread channel.
        let mut shown = self.sidebar.read(cx).shown().into_owned();
        shown.selected = None;
        let first = shown.unread_ahead(step);
        match first {
            Some(id) => {
                self.triage_to(id, window, cx);
                true
            }
            None => false,
        }
    }

    /// The nearest other saved server on the rail, in `step`'s direction,
    /// with an unread channel or a mention.
    fn unread_server(&self, step: Step, cx: &gpui_kit::App) -> Option<usize> {
        let rail = self.rail.read(cx);
        let servers = rail.servers();
        let current = servers.get(rail.current_server()?)?.key.clone();
        let key = next_unread_team(
            servers
                .iter()
                .map(|server| (&server.key, server.unread || server.mentions > 0)),
            Some(&current),
            step,
        )?;
        servers.iter().position(|server| server.key == key)
    }

    /// Cmd/Ctrl+Shift+A and the title bar's inbox (#223): the first
    /// unread messages of every unread channel in the team, in the right
    /// pane, channel by channel in sidebar order.
    pub(super) fn show_unreads(&mut self, cx: &mut Context<Self>) {
        let ids = self.sidebar.read(cx).model().digest_ids();
        self.show_listing(RightPaneContent::Unreads(Vec::new()), cx);
        if let Some(live) = self.live.as_mut() {
            live.show_unreads(ids.iter().map(ToString::to_string).collect());
            self.right_pane
                .update(cx, |pane, cx| pane.set_loading(true, cx));
        }
    }

    fn triage_to(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.open_channel(id.clone(), window, cx);
        self.triaging = Some(id);
    }

    /// The nearest other team on the rail, in `step`'s direction and
    /// wrapping round, with an unread channel.
    fn unread_team(&self, step: Step, cx: &gpui_kit::App) -> Option<SharedString> {
        let current = self.current_team(cx);
        let rail = self.rail.read(cx);
        next_unread_team(
            rail.teams().iter().map(|team| (&team.id, team.unread)),
            current.as_ref(),
            step,
        )
    }
}

/// From `teams` (or servers) in rail order with their unread flags, the
/// nearest one after (or before) `current` that is unread, wrapping round
/// and never `current` itself.
fn next_unread_team<'a>(
    teams: impl Iterator<Item = (&'a SharedString, bool)>,
    current: Option<&SharedString>,
    step: Step,
) -> Option<SharedString> {
    let teams: Vec<(&SharedString, bool)> = teams.collect();
    let at = teams.iter().position(|(id, _)| Some(*id) == current)?;
    let count = teams.len();
    (1..count)
        .map(|offset| match step {
            Step::Next => (at + offset) % count,
            Step::Previous => (at + count - offset) % count,
        })
        .find(|&index| teams[index].1)
        .map(|index| teams[index].0.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triage_moves_to_the_nearest_team_with_unreads() {
        let ids: Vec<SharedString> = ["a", "b", "c", "d"].map(SharedString::from).into();
        let teams = |unread: [bool; 4]| ids.iter().zip(unread).collect::<Vec<_>>();
        let next = |unread, current: &str, step| {
            next_unread_team(teams(unread).into_iter(), Some(&current.into()), step)
                .map(|id| id.to_string())
        };
        assert_eq!(
            next([false, false, true, true], "a", Step::Next).as_deref(),
            Some("c")
        );
        assert_eq!(
            next([false, false, true, true], "a", Step::Previous).as_deref(),
            Some("d")
        );
        // Round the end of the rail.
        assert_eq!(
            next([true, false, false, false], "c", Step::Next).as_deref(),
            Some("a")
        );
        // The team on screen never counts, even when it is unread.
        assert_eq!(next([true, false, false, false], "a", Step::Next), None);
        assert_eq!(next([false; 4], "b", Step::Next), None);
    }
}
