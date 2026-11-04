//! Every saved server stays connected (#206, PLAN §3 M2). Each runs its own
//! sync engine, followed by a [`ServerWatch`] for the rail's badges, the
//! dock's total and its notifications; the shown server's [`Live`] runs
//! over the same engine, so a switch shows the other server at once, from
//! what its watch kept, with no new connection.

use gpui_kit::{Context, Task, Window};
use matty_sync::{ChangeSet, Command};

use super::MainWindow;
use crate::live::Live;
use crate::settings::SavedAccount;
use crate::watch::{Heard, ServerWatch};

/// The saved servers' engines.
#[derive(Default)]
pub(super) struct Watches {
    running: Vec<Running>,
    /// Saved servers with no session to connect with, left alone until
    /// they sign in again.
    idle: std::collections::HashSet<String>,
    /// The latest notifications from servers not shown, by post id, with
    /// the server each came from, so a click switches to it.
    notified: std::collections::VecDeque<(String, String)>,
}

/// Notifications remembered for a click; older ones open on the shown
/// server, as before #206.
const NOTIFIED: usize = 200;

struct Running {
    watch: ServerWatch,
    _task: Task<()>,
}

impl Watches {
    fn get(&self, data_key: &str) -> Option<&ServerWatch> {
        self.running
            .iter()
            .map(|running| &running.watch)
            .find(|watch| watch.data_key() == data_key)
    }

    fn get_mut(&mut self, data_key: &str) -> Option<&mut ServerWatch> {
        self.running
            .iter_mut()
            .map(|running| &mut running.watch)
            .find(|watch| watch.data_key() == data_key)
    }

    /// Stops following the server; its engine stops once no window shows
    /// it either, closing its database.
    pub(super) fn stop(&mut self, data_key: &str) {
        self.running
            .retain(|running| running.watch.data_key() != data_key);
    }

    /// The server's mentions and whether it has unreads, while it runs.
    pub(super) fn unreads(&self, data_key: &str) -> Option<(usize, bool)> {
        self.get(data_key).map(ServerWatch::unreads)
    }
}

impl MainWindow {
    /// A [`Live`] for `account` over its running engine, or a new engine.
    /// With `true`, the engine had already started and the window has been
    /// brought up to date with it; otherwise the caller connects it.
    pub(super) fn watched_live(
        &mut self,
        account: &SavedAccount,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = account.data_key();
        self.watches.idle.remove(&key);
        if self.watches.get(&key).is_none() {
            let path = crate::live::database_path(account);
            match matty_sync::spawn_at(path, crate::live::media_dir(account)) {
                Ok(sync) => self.follow(sync, account, cx),
                Err(error) => {
                    tracing::error!(%error, "could not start sync");
                    return false;
                }
            }
        }
        let Some(watch) = self.watches.get(&key) else {
            return false;
        };
        let started = watch.started();
        let replay = if started { watch.replay() } else { Vec::new() };
        let sync = watch.sync();
        self.run_live(account, Ok(Live::start_with(sync, account)), window, cx);
        if started {
            for change in &replay {
                self.apply(change, window, cx);
            }
            if let Some(live) = &self.live {
                live.send(Command::ShowCache);
            }
        }
        started
    }

    /// Starts an engine for each saved server that has none and a session
    /// to connect with, and stops those of servers no longer saved.
    pub(super) fn watch_servers(&mut self, cx: &mut Context<Self>) {
        if crate::preview::enabled() || crate::demo() {
            return;
        }
        let servers = self.settings.read(cx).get().servers.clone();
        let saved = |key: &str| servers.iter().any(|server| server.data_key() == key);
        self.watches
            .running
            .retain(|running| saved(running.watch.data_key()));
        self.watches.idle.retain(|key| saved(key));
        for account in &servers {
            let key = account.data_key();
            if self.watches.get(&key).is_some() || self.watches.idle.contains(&key) {
                continue;
            }
            if !self.watch_server(account, cx) {
                self.watches.idle.insert(key);
            }
        }
    }

    /// Connects `account` in the background with its saved session;
    /// `matty --offline` shows its cache alone. `false` when it has no
    /// session.
    fn watch_server(&mut self, account: &SavedAccount, cx: &mut Context<Self>) -> bool {
        let client = if crate::offline() {
            None
        } else {
            let Some(token) = crate::secrets::load(account) else {
                return false;
            };
            let Ok(client) = matty_api::Client::new(&account.server) else {
                return false;
            };
            Some(client.with_token(token))
        };
        let path = crate::live::database_path(account);
        let sync = match matty_sync::spawn_at(path, crate::live::media_dir(account)) {
            Ok(sync) => sync,
            Err(error) => {
                tracing::warn!(%error, "could not start a saved server's sync");
                return false;
            }
        };
        self.follow(sync, account, cx);
        let command = match client {
            Some(client) => Command::Connect { client },
            None => Command::ShowCache,
        };
        if let Some(watch) = self.watches.get(&account.data_key()) {
            watch.send(command);
        }
        true
    }

    fn follow(
        &mut self,
        sync: matty_sync::SyncHandle,
        account: &SavedAccount,
        cx: &mut Context<Self>,
    ) {
        let (watch, changes) = ServerWatch::new(sync, account);
        let key = account.data_key();
        let task = cx.spawn(async move |this, cx| {
            while let Ok(change) = changes.recv().await {
                if this
                    .update(cx, |this, cx| this.heard(&key, &change, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self.watches.running.push(Running { watch, _task: task });
    }

    /// Shows a notification for a post on a server that is not shown, as
    /// its preferences say, its title naming the server. No channel of it
    /// is on screen, so only its own Do not disturb holds it back.
    fn notify_from(
        &mut self,
        data_key: &str,
        arrival: &crate::live::Arrival,
        cx: &mut Context<Self>,
    ) {
        let Some(watch) = self.watches.get(data_key) else {
            return;
        };
        let app = matty_notify::AppState {
            dnd: watch.dnd(jiff::Timestamp::now().as_second()),
            show_preview: true,
            ..matty_notify::AppState::default()
        };
        let Some(mut notification) = arrival.decide(app) else {
            return;
        };
        let settings = self.settings.read(cx).get();
        if let Some(server) = settings
            .servers
            .iter()
            .find(|server| server.data_key() == data_key)
        {
            notification.title = server_title(&notification.title, server.label());
        }
        self.notifier.show(&notification);
        let notified = &mut self.watches.notified;
        notified.push_back((notification.post_id, data_key.to_owned()));
        while notified.len() > NOTIFIED {
            notified.pop_front();
        }
    }

    /// Before a clicked notification opens its post: switches to the server
    /// it came from when that is not the one shown. `false` when that
    /// server could not be shown, as when it needs signing in again.
    pub(super) fn show_notified_server(
        &mut self,
        clicked: &matty_notify::Clicked,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(data_key) = self
            .watches
            .notified
            .iter()
            .rev()
            .find(|(post_id, _)| *post_id == clicked.post_id)
            .map(|(_, data_key)| data_key.clone())
        else {
            return true;
        };
        if self.is_shown(&data_key, cx) {
            return true;
        }
        let index = self
            .settings
            .read(cx)
            .get()
            .servers
            .iter()
            .position(|server| server.data_key() == data_key);
        let Some(index) = index else {
            return false;
        };
        self.switch_server(index, window, cx);
        self.is_shown(&data_key, cx) && self.live.is_some()
    }

    /// Whether `data_key` names the server the window shows.
    fn is_shown(&self, data_key: &str, cx: &Context<Self>) -> bool {
        self.settings
            .read(cx)
            .get()
            .account
            .as_ref()
            .is_some_and(|account| account.data_key() == data_key)
    }

    fn heard(&mut self, data_key: &str, change: &ChangeSet, cx: &mut Context<Self>) {
        let Some(watch) = self.watches.get_mut(data_key) else {
            return;
        };
        match watch.apply(change) {
            Heard::Nothing => {}
            Heard::Arrived(arrival) => {
                if !self.is_shown(data_key, cx) {
                    self.notify_from(data_key, &arrival, cx);
                }
            }
            Heard::Unreads => {
                if !self.is_shown(data_key, cx) {
                    self.show_servers(cx);
                }
            }
            Heard::Ended => {
                self.watches.stop(data_key);
                // The shown server's own sync asks to sign in again; another
                // asks when the user switches to it.
                if !self.is_shown(data_key, cx) {
                    let settings = self.settings.read(cx).get();
                    if let Some(account) = settings
                        .servers
                        .iter()
                        .find(|server| server.data_key() == data_key)
                    {
                        crate::secrets::delete(account);
                    }
                    self.watches.idle.insert(data_key.to_owned());
                }
                self.show_servers(cx);
            }
        }
    }
}

/// A notification's title with the server it came from, which another
/// server's notifications would otherwise not say.
fn server_title(title: &str, server: &str) -> String {
    format!("{title} · {server}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_background_notification_names_its_server() {
        assert_eq!(
            super::server_title("town-square", "Acme Corp"),
            "town-square · Acme Corp"
        );
    }
}
