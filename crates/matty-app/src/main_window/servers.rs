//! Opening, adding, switching and removing saved servers (#206). Every
//! saved server's sync keeps running (see [`super::background`]); the
//! window shows one at a time, and switching shows another over its
//! running sync.

use gpui_kit::{AppContext as _, Context, Entity, Window};

use super::{ConnectionState, MainWindow};
use crate::login::{LoginEvent, LoginModel, LoginView};
use crate::rail::{RailEvent, Server};
use crate::settings::SavedAccount;
use crate::sidebar::SidebarModel;

impl MainWindow {
    /// Opens `account` at launch or after a switch: its cache alone with
    /// `matty --offline`, otherwise its cache and a connection with the
    /// saved session, unless its sync is connected already. `false` when it
    /// has no session to connect with.
    pub(super) fn open_server(
        &mut self,
        account: &SavedAccount,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if crate::offline() {
            self.login = None;
            self.start_live(account, window, cx);
            // Tells the views too, so they refuse what needs the server (#163).
            self.set_connection(ConnectionState::Offline, cx);
            if let Some(live) = &self.live {
                live.send(matty_sync::Command::ShowCache);
            }
            return true;
        }
        let Some(token) = crate::secrets::load(account) else {
            return false;
        };
        let Ok(client) = matty_api::Client::new(&account.server) else {
            return false;
        };
        self.login = None;
        if self.start_live(account, window, cx) {
            return true;
        }
        if let Some(live) = &self.live {
            live.send(matty_sync::Command::Connect {
                client: client.with_token(token),
            });
        }
        true
    }

    /// Empties what the window showed of the shown server, before it shows
    /// another. The server's sync runs on for its badges.
    pub(super) fn leave_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(live) = self.live.take() {
            live.release();
        }
        self._live_task = None;
        self._typing_task = None;
        self._view_task = None;
        cx.set_global(crate::avatars::Avatars::default());
        cx.set_global(crate::composer_draft::DraftBook::default());
        self.selected_channel = None;
        self.history.clear();
        self.history_at = 0;
        self.kept_unread = None;
        self.link_asked = None;
        self.link_join_offer = None;
        self.link_jump = None;
        self.right_pane_open = false;
        self.threads_open = false;
        self.drafts_open = false;
        self.queued = 0;
        self.notify_props = None;
        self.collapsed_threads = None;
        self.highlight_keys = false;
        self.set_connection(ConnectionState::Online, cx);
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_model(SidebarModel::loading(), cx)
        });
        self.channel
            .update(cx, |channel, cx| channel.set_model(None, window, cx));
    }

    /// Shows the sign-in card to add a server, over the server shown now,
    /// which stays connected until the new one signs in. With `server`,
    /// the card checks that address at once, as when a saved server's
    /// session is gone.
    pub(super) fn add_server(
        &mut self,
        server: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.login.is_some() {
            return;
        }
        let model = LoginModel {
            adding: true,
            ..LoginModel::default()
        };
        let view = cx.new(|cx| LoginView::new(model, window, cx));
        cx.subscribe_in(&view, window, Self::on_login_event)
            .detach();
        self.login = Some(view.clone());
        if let Some(url) = server {
            Self::on_login_event(self, &view, &LoginEvent::CheckServer(url), window, cx);
        }
        cx.notify();
    }

    /// Cancel on the card that adds a server: back to the one shown.
    pub(super) fn cancel_add_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self
            .login
            .as_ref()
            .is_some_and(|login| login.read(cx).model().adding)
        {
            return;
        }
        self.cancel_sso();
        self.login = None;
        self.login_server = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Cmd/Ctrl+number or a click on the rail: shows the `index`th saved
    /// server. One whose session is gone asks to sign in to it again.
    pub(super) fn switch_server(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.login.is_some() {
            return;
        }
        if crate::preview::enabled() {
            let servers = self.rail.read(cx).servers().len();
            if index < servers {
                self.rail.update(cx, |rail, cx| {
                    rail.set_servers(crate::preview::servers(), Some(index), cx)
                });
            }
            return;
        }
        let settings = self.settings.read(cx).get();
        if settings.current_server() == Some(index) {
            return;
        }
        let Some(account) = settings.servers.get(index).cloned() else {
            return;
        };
        if !crate::offline() && crate::secrets::load(&account).is_none() {
            return self.add_server(Some(account.server), window, cx);
        }
        self.settings.update(cx, |settings, cx| {
            settings.update(cx, |s| {
                s.select_server(index);
            })
        });
        self.open_server(&account, window, cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Removes the `index`th saved server and deletes its data. The shown
    /// one signs out, as the account menu does, and the next saved server
    /// opens; another is signed out on its server in the background.
    pub(super) fn remove_server(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if crate::preview::enabled() {
            return;
        }
        let settings = self.settings.read(cx).get();
        let Some(account) = settings.servers.get(index).cloned() else {
            return;
        };
        if settings.current_server() == Some(index) {
            return self.sign_out(window, cx);
        }
        if let Some(token) = crate::secrets::load(&account)
            && let Ok(client) = matty_api::Client::new(&account.server)
        {
            let client = client.with_token(token);
            drop(matty_sync::io::run(async move {
                if let Err(error) = client.logout().await {
                    tracing::info!(%error, "could not end a removed server's session");
                }
            }));
        }
        crate::secrets::delete(&account);
        self.watches.stop(&account.data_key());
        self.settings.update(cx, |settings, cx| {
            settings.update(cx, |s| {
                s.remove_server(&account.data_key());
            })
        });
        forget_files(
            crate::live::database_path(&account),
            crate::live::media_dir(&account),
            cx,
        );
    }

    /// After signing out of the shown server, opens the next saved one;
    /// `false` when there is none, or none with a session.
    pub(super) fn open_next_server(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let next = self
            .settings
            .update(cx, |settings, cx| settings.launch_server(cx));
        next.is_some_and(|account| self.open_server(&account, window, cx))
    }

    /// The rail follows the saved servers, each of which stays connected.
    pub(super) fn refresh_server_rail(&mut self, cx: &mut Context<Self>) {
        self.watch_servers(cx);
        self.show_servers(cx);
    }

    /// The rail's servers, with the unreads and mentions of those not shown.
    pub(super) fn show_servers(&mut self, cx: &mut Context<Self>) {
        let (servers, current) = if crate::preview::enabled() {
            let servers = crate::preview::servers();
            let current = self.rail.read(cx).current_server().or(Some(0));
            (servers, current)
        } else {
            let settings = self.settings.read(cx).get();
            let current = settings.current_server();
            let servers = settings
                .servers
                .iter()
                .enumerate()
                .map(|(index, server)| {
                    let rail = Server::new(server.label().to_owned());
                    match self.watches.unreads(&server.data_key()) {
                        Some((mentions, unread)) if current != Some(index) => {
                            rail.unread(unread).mentions(mentions)
                        }
                        _ => rail,
                    }
                })
                .collect();
            (servers, current)
        };
        self.rail
            .update(cx, |rail, cx| rail.set_servers(servers, current, cx));
    }

    pub(super) fn on_rail_event(
        &mut self,
        _: &Entity<crate::rail::RailView>,
        event: &RailEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            RailEvent::SelectServer(index) => self.switch_server(*index, window, cx),
            RailEvent::AddServer => self.add_server(None, window, cx),
            RailEvent::RemoveServer(index) => self.remove_server(*index, window, cx),
            // Switching teams and joining one are sync's (PLAN §3 M2); the
            // rail already shows the pick.
            RailEvent::Select(_) | RailEvent::Join => cx.notify(),
        }
    }

    /// Cmd/Ctrl+1…9 and Add server, on the window's root.
    pub(super) fn on_server_actions(root: gpui_kit::Div, cx: &mut Context<Self>) -> gpui_kit::Div {
        use crate::actions::*;
        use gpui_kit::InteractiveElement as _;
        macro_rules! switch {
            ($root:expr, $($action:ident => $index:literal),*) => {
                $root$(.on_action(cx.listener(|this, _: &$action, window, cx| {
                    this.switch_server($index, window, cx)
                })))*
            };
        }
        let root = root.on_action(
            cx.listener(|this, _: &AddServer, window, cx| this.add_server(None, window, cx)),
        );
        switch!(
            root,
            SwitchServer1 => 0,
            SwitchServer2 => 1,
            SwitchServer3 => 2,
            SwitchServer4 => 3,
            SwitchServer5 => 4,
            SwitchServer6 => 5,
            SwitchServer7 => 6,
            SwitchServer8 => 7,
            SwitchServer9 => 8
        )
    }
}

/// Deletes an account's database and media folder in the background. Its
/// sync may still be closing the database, so a failed delete is tried
/// again for a few seconds.
pub(super) fn forget_files(
    database: std::path::PathBuf,
    media: Option<std::path::PathBuf>,
    cx: &mut gpui_kit::App,
) {
    let executor = cx.background_executor().clone();
    cx.background_executor()
        .spawn(async move {
            for _ in 0..20 {
                match matty_sync::forget_account(&database, media.as_deref()) {
                    Ok(()) => return,
                    Err(error) => {
                        tracing::debug!(%error, "the cache is still in use");
                        executor.timer(std::time::Duration::from_millis(250)).await;
                    }
                }
            }
            tracing::warn!(path = %database.display(), "could not delete a signed-out cache");
        })
        .detach();
}
