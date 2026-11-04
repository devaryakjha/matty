//! Links the OS hands Matty: a `mattermost://` URL opened while Matty runs
//! or at launch, or the first command-line argument (PLAN §6.6, §6.7).
//!
//! Only the SSO hand-off is understood today; permalinks and `matty://`
//! links become more variants of [`DeepLink`]. Anything else is logged and
//! ignored, never an error the person sees.

use gpui_kit::{App, Global};

/// A link Matty knows what to do with.
#[derive(Clone, PartialEq, Eq)]
pub enum DeepLink {
    /// The end of the browser SSO flow: the server sent the browser to
    /// `{server}/login/desktop`, which handed off to
    /// `mattermost://{host}/login/desktop?client_token=…&server_token=…`.
    SsoCallback {
        /// The token Matty put in the login URL, to match against the
        /// outstanding one.
        client_token: String,
        /// The short-lived token for `users/login/desktop_token`.
        server_token: String,
    },
}

/// Never prints the tokens: they sign someone in for about three minutes.
impl std::fmt::Debug for DeepLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeepLink::SsoCallback { .. } => f.write_str("SsoCallback { .. }"),
        }
    }
}

/// The schemes the server's `/login/desktop` page hands off to: the dev one
/// for `dev-` client tokens.
const SSO_SCHEMES: [&str; 2] = ["mattermost", "mattermost-dev"];

impl DeepLink {
    /// Reads `link`, or `None` when it is not one Matty handles.
    pub fn parse(link: &str) -> Option<Self> {
        let url = url::Url::parse(link.trim()).ok()?;
        if !SSO_SCHEMES.contains(&url.scheme()) {
            return None;
        }
        // The server may live under a subpath: `/team/login/desktop`.
        if !url.path().trim_end_matches('/').ends_with("/login/desktop") {
            return None;
        }
        let mut client_token = None;
        let mut server_token = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "client_token" => client_token = Some(value.into_owned()),
                "server_token" => server_token = Some(value.into_owned()),
                _ => {}
            }
        }
        let client_token = client_token.filter(|token| !token.is_empty())?;
        let server_token = server_token.filter(|token| !token.is_empty())?;
        Some(DeepLink::SsoCallback {
            client_token,
            server_token,
        })
    }
}

/// `link` with its query dropped, for the log: the query carries tokens.
pub fn redacted(link: &str) -> String {
    let link = link.trim();
    match link.split_once('?') {
        Some((head, _)) => format!("{head}?…"),
        None => link.to_string(),
    }
}

/// A new SSO `client_token`: 64 hex characters, `dev-` first in debug
/// builds so the server hands back to `mattermost-dev://` and dev builds
/// never take a release build's callback.
pub fn new_client_token() -> String {
    let mut bytes = [0u8; 32];
    if let Err(error) = getrandom::fill(&mut bytes) {
        // Not secret beyond the round trip, but it must not repeat.
        tracing::warn!(%error, "no system randomness for the SSO token; using the clock");
        let nanos = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = (nanos >> ((i % 16) * 8)) as u8 ^ (i as u8).wrapping_mul(97);
        }
    }
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut token = if cfg!(debug_assertions) {
        format!("dev-{hex}")
    } else {
        hex
    };
    token.truncate(64);
    token
}

/// Where the browser starts `provider`'s sign-in on `server`.
pub fn sso_login_url(
    server: &str,
    provider: crate::login::SsoProvider,
    client_token: &str,
) -> String {
    use crate::login::SsoProvider;
    let server = server.trim_end_matches('/');
    let path = match provider {
        SsoProvider::GitLab => "oauth/gitlab/login",
        SsoProvider::Google => "oauth/google/login",
        SsoProvider::Office365 => "oauth/office365/login",
        SsoProvider::OpenId => "oauth/openid/login",
        SsoProvider::Saml => "login/sso/saml",
    };
    format!("{server}/{path}?desktop_token={client_token}")
}

/// The links that arrived before or while the main window runs; the window
/// takes the receiver and handles them in order.
pub(crate) struct Incoming(pub async_channel::Receiver<String>);

impl Global for Incoming {}

impl Incoming {
    pub(crate) fn receiver(cx: &App) -> Option<async_channel::Receiver<String>> {
        cx.try_global::<Incoming>()
            .map(|incoming| incoming.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::login::SsoProvider;

    fn callback(client: &str, server: &str) -> Option<DeepLink> {
        Some(DeepLink::SsoCallback {
            client_token: client.into(),
            server_token: server.into(),
        })
    }

    #[test]
    fn reads_the_sso_hand_off() {
        assert_eq!(
            DeepLink::parse(
                "mattermost://chat.example.com/login/desktop?client_token=abc&server_token=xyz"
            ),
            callback("abc", "xyz")
        );
        assert_eq!(
            DeepLink::parse(
                "mattermost-dev://chat.example.com:8065/login/desktop?server_token=xyz&client_token=dev-abc&isDesktopDev=true"
            ),
            callback("dev-abc", "xyz")
        );
        assert_eq!(
            DeepLink::parse(
                "mattermost://example.com/mm/login/desktop/?client_token=a%2Bb&server_token=z"
            ),
            callback("a+b", "z")
        );
    }

    #[test]
    fn ignores_what_it_does_not_know() {
        for link in [
            "",
            "not a url",
            "https://chat.example.com/login/desktop?client_token=a&server_token=b",
            "slack://chat.example.com/login/desktop?client_token=a&server_token=b",
            "mattermost://chat.example.com/team/pl/abc123",
            "mattermost://chat.example.com/login/desktop?client_token=a",
            "mattermost://chat.example.com/login/desktop?client_token=&server_token=b",
            "mattermost://chat.example.com/login/desktopx?client_token=a&server_token=b",
            "mattermost://",
        ] {
            assert_eq!(DeepLink::parse(link), None, "{link}");
        }
    }

    #[test]
    fn redacts_the_query_and_debug() {
        assert_eq!(
            redacted("mattermost://h/login/desktop?client_token=a&server_token=b"),
            "mattermost://h/login/desktop?…"
        );
        let link = callback("secret", "secret").unwrap_or_else(|| unreachable!());
        assert!(!format!("{link:?}").contains("secret"));
    }

    #[test]
    fn client_tokens_are_64_characters_and_differ() {
        let first = new_client_token();
        let second = new_client_token();
        assert_eq!(first.len(), 64);
        assert_ne!(first, second);
        assert_eq!(first.starts_with("dev-"), cfg!(debug_assertions));
        let hex = first.strip_prefix("dev-").unwrap_or(&first);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn login_urls_follow_the_server() {
        assert_eq!(
            sso_login_url("https://chat.example.com/", SsoProvider::GitLab, "t"),
            "https://chat.example.com/oauth/gitlab/login?desktop_token=t"
        );
        assert_eq!(
            sso_login_url("https://example.com/mm", SsoProvider::OpenId, "t"),
            "https://example.com/mm/oauth/openid/login?desktop_token=t"
        );
        assert_eq!(
            sso_login_url("https://chat.example.com", SsoProvider::Saml, "t"),
            "https://chat.example.com/login/sso/saml?desktop_token=t"
        );
        assert_eq!(
            sso_login_url("https://chat.example.com", SsoProvider::Office365, "t"),
            "https://chat.example.com/oauth/office365/login?desktop_token=t"
        );
        assert_eq!(
            sso_login_url("https://chat.example.com", SsoProvider::Google, "t"),
            "https://chat.example.com/oauth/google/login?desktop_token=t"
        );
    }
}
