//! The browser SSO flow and incoming links (#75, #84): the window opens the
//! browser with a fresh client token, waits up to three minutes for the
//! `mattermost://` hand-off, and finishes sign-in with its server token.
//! Other links open their channel or post, as a link in a message would.

use std::time::Duration;

use gpui_kit::{AppContext as _, Context, Entity, SharedString, Task, WeakEntity, Window};

use super::MainWindow;
use crate::deeplink::{self, DeepLink};
use crate::login::{LoginStep, LoginView, SsoProvider};
use crate::url_handler;

/// How long the server's token lives, so how long the card waits.
pub(super) const SSO_TIMEOUT: Duration = Duration::from_secs(3 * 60);

/// How long the server waits before it hands the token to the browser's
/// `mattermost://` link; after that the card offers a token instead.
const HAND_OFF_WAIT: Duration = Duration::from_secs(10);

/// How often, and how many times, the card checks again after asking to
/// open `mattermost://` links, since macOS asks the person first.
const CLAIM_CHECK: Duration = Duration::from_secs(2);
const CLAIM_CHECKS: usize = 30;

/// A browser sign-in Matty is waiting on.
pub(super) struct PendingSso {
    client_token: String,
    server: String,
    /// Shows the timeout on the card; dropping it cancels.
    _timeout: Task<()>,
}

impl MainWindow {
    /// Handles links as the OS opens them, for as long as the window lives.
    pub(super) fn follow_links(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(links) = deeplink::Incoming::receiver(cx) else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(link) = links.recv().await {
                let handled = this.update_in(cx, |this, window, cx| {
                    cx.activate(true);
                    window.activate_window();
                    this.open_link(&link, window, cx);
                });
                if handled.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn open_link(&mut self, link: &str, window: &mut Window, cx: &mut Context<Self>) {
        // A second launch without a link only brings the window forward.
        if link.is_empty() {
            return;
        }
        let Some(parsed) = DeepLink::parse(link) else {
            tracing::info!(link = %deeplink::redacted(link), "ignored a link Matty does not handle");
            return;
        };
        match parsed {
            // On the server shown, another saved one, or one to add (#206).
            DeepLink::Page { host, path } => self.open_page(&host, &path, window, cx),
            DeepLink::Entity(url) => self.go_to_link(&url, false, window, cx),
            DeepLink::SsoCallback {
                client_token,
                server_token,
            } => {
                let expected = self.sso.as_ref().map(|p| p.client_token.as_str());
                let (Some(view), true) = (self.login.clone(), expected == Some(&*client_token))
                else {
                    tracing::info!(
                        link = %deeplink::redacted(link),
                        waiting = expected.is_some(),
                        "ignored an SSO hand-off that does not match the sign-in in progress"
                    );
                    return;
                };
                let Some(pending) = self.sso.take() else {
                    return;
                };
                tracing::info!("finishing SSO sign-in");
                view.update(cx, |view, cx| {
                    let mut model = view.model().clone();
                    model.busy = true;
                    model.error = None;
                    view.set_model(model, window, cx);
                });
                let signing_in =
                    crate::live::sign_in_with_desktop_token(pending.server.clone(), server_token);
                self.finish_sign_in(pending.server, signing_in, &view, window, cx);
            }
        }
    }

    /// Opens the browser at `provider`'s sign-in on `server` and starts the
    /// three-minute wait; a new start replaces the one before.
    pub(super) fn start_sso(
        &mut self,
        server: String,
        provider: SsoProvider,
        view: &Entity<LoginView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let client_token = deeplink::new_client_token();
        cx.open_url(&deeplink::sso_login_url(&server, provider, &client_token));
        tracing::info!(provider = provider.label(), "opened the browser for SSO");
        let view = view.downgrade();
        self.check_scheme_owner(view.clone(), 1, Duration::ZERO, window, cx);
        let token = client_token.clone();
        let offer_view = view.clone();
        let offer_token = token.clone();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(HAND_OFF_WAIT).await;
            let _ = this.update(cx, |this, cx| {
                if this.sso.as_ref().map(|p| &p.client_token) == Some(&offer_token) {
                    let _ = offer_view.update(cx, |view, cx| view.offer_token(cx));
                }
            });
        })
        .detach();
        let timeout = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SSO_TIMEOUT).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.sso.as_ref().map(|p| &p.client_token) != Some(&token) {
                    return;
                }
                this.sso = None;
                tracing::info!("SSO sign-in timed out");
                let _ = view.update(cx, |view, cx| {
                    let mut model = view.model().clone();
                    if matches!(model.step, LoginStep::Browser { .. }) {
                        model.busy = false;
                        model.error = Some(
                            "Sign-in timed out: the browser did not finish within three minutes."
                                .into(),
                        );
                        view.set_model(model, window, cx);
                    }
                });
            });
        });
        self.sso = Some(PendingSso {
            client_token,
            server,
            _timeout: timeout,
        });
    }

    /// Asks the system who opens `mattermost://` links, up to `checks`
    /// times `every` apart, and tells the card about any other app; it
    /// stops once Matty holds them or the sign-in ends (#489).
    fn check_scheme_owner(
        &self,
        view: WeakEntity<LoginView>,
        checks: usize,
        every: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            for _ in 0..checks {
                cx.background_executor().timer(every).await;
                let owner = cx.background_spawn(async { url_handler::owner() }).await;
                let other = match owner {
                    url_handler::Owner::Other(app) => Some(SharedString::from(app)),
                    url_handler::Owner::Matty | url_handler::Owner::Nobody => None,
                };
                let done = other.is_none();
                let waiting = this
                    .update(cx, |this, cx| {
                        let waiting = this.sso.is_some();
                        if waiting {
                            let _ = view.update(cx, |view, cx| view.set_scheme_owner(other, cx));
                        }
                        waiting
                    })
                    .unwrap_or(false);
                if done || !waiting {
                    break;
                }
            }
        })
        .detach();
    }

    /// Asks to make Matty the app that opens `mattermost://` links, then
    /// watches for the change: macOS shows its own confirmation first.
    pub(super) fn claim_links(
        &mut self,
        view: &Entity<LoginView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::info!("asking to open mattermost:// links");
        cx.background_spawn(async { url_handler::claim() }).detach();
        self.check_scheme_owner(view.downgrade(), CLAIM_CHECKS, CLAIM_CHECK, window, cx);
    }

    /// Stops waiting: a later hand-off is ignored.
    pub(super) fn cancel_sso(&mut self) {
        self.sso = None;
    }
}
