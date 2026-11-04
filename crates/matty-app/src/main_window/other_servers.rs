//! Links to servers other than the one shown (#206): a page link from the
//! OS or a server's link in a message switches to its saved server first,
//! then opens there as one of its own would, its team shown first. A page
//! link to a server not saved offers the Add server card with its address
//! filled in. [`crate::server_links`] decides which server; this acts.

use gpui_kit::{Context, Window};

use super::MainWindow;
use crate::server_links::{self, PageServer};

impl MainWindow {
    /// Opens the page at `path` on `host`, from a `matty://` or
    /// `mattermost://` link.
    pub(super) fn open_page(
        &mut self,
        host: &str,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shown = match &self.live {
            Some(live) => Some(live.server().to_owned()),
            None => crate::preview::enabled().then(|| super::PREVIEW_SERVER.to_owned()),
        };
        let saved = self.saved_addresses(cx);
        let saved: Vec<&str> = saved.iter().map(String::as_str).collect();
        match server_links::page_server(host, path, shown.as_deref(), &saved) {
            PageServer::Shown(url) => self.go_to_link(&url, false, window, cx),
            PageServer::Saved { index, url } => {
                if self.show_saved_server(index, window, cx) {
                    self.go_to_link(&url, false, window, cx);
                }
            }
            PageServer::Unsaved { address } => self.offer_server(address, window, cx),
        }
    }

    /// A link in a message that the shown server does not own: when it is
    /// another saved server's post, channel or direct message, shows that
    /// server and opens it there. `false` when the link is not one, so it
    /// opens in the browser.
    pub(super) fn open_on_other_server(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let saved = self.saved_addresses(cx);
        let saved: Vec<&str> = saved.iter().map(String::as_str).collect();
        let shown = self.settings.read(cx).get().current_server();
        let Some(index) = server_links::message_link_server(url, shown, &saved) else {
            return false;
        };
        if self.show_saved_server(index, window, cx) {
            self.go_to_link(url, false, window, cx);
        }
        true
    }

    /// The saved servers' addresses, in the rail's order; none in the
    /// preview, whose servers have no address.
    fn saved_addresses(&self, cx: &Context<Self>) -> Vec<String> {
        if crate::preview::enabled() {
            return Vec::new();
        }
        let settings = self.settings.read(cx).get();
        settings
            .servers
            .iter()
            .map(|server| server.server.clone())
            .collect()
    }

    /// Shows the `index`th saved server, as its rail button does. `false`
    /// when it could not be shown, as when it asks to sign in again first.
    fn show_saved_server(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.switch_server(index, window, cx);
        let shown = self.settings.read(cx).get().current_server() == Some(index);
        if !shown || self.live.is_none() {
            tracing::info!(index, "a link's server could not be shown");
            return false;
        }
        true
    }

    /// The Add server card, its address filled in from a link to a server
    /// Matty has not saved; the person checks it and presses Continue. A
    /// sign-in already on screen is left alone.
    fn offer_server(&mut self, address: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.login.is_some() {
            tracing::info!(%address, "ignored a link to an unsaved server during sign-in");
            return;
        }
        tracing::info!(%address, "offering to add a linked server");
        self.add_server(None, window, cx);
        if let Some(login) = &self.login {
            login.update(cx, |login, cx| login.prefill_server(&address, window, cx));
        }
    }
}
