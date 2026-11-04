//! Links the OS hands Matty: a `mattermost://` URL opened while Matty runs
//! or at launch, or the first command-line argument (PLAN §6.6, §6.7).
//!
//! Besides the SSO hand-off, a `matty://` or `mattermost://` link names a
//! page of a server, such as `mattermost://chat.example.com/team/pl/{post}`
//! or `matty://chat.example.com/team/channels/town-square`, and the window
//! opens it through [`crate::links::route`] like a link in a message (#84).
//! Anything else is logged and ignored, never an error the person sees.

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
    /// A page of the server at `host`: `path` is its path there, such as
    /// `/team/pl/{post_id}` or `/team/channels/{name}`.
    Page {
        /// The server's host, with its port when the link gave one.
        host: String,
        /// The page's path, starting with `/`.
        path: String,
    },
    /// One of `matty-markdown`'s entity links, such as
    /// `matty://channel/town-square`, opened from outside Matty.
    Entity(String),
}

/// Never prints the tokens: they sign someone in for about three minutes.
impl std::fmt::Debug for DeepLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeepLink::SsoCallback { .. } => f.write_str("SsoCallback { .. }"),
            DeepLink::Page { host, path } => f
                .debug_struct("Page")
                .field("host", host)
                .field("path", path)
                .finish(),
            DeepLink::Entity(link) => f.debug_tuple("Entity").field(link).finish(),
        }
    }
}

/// The schemes the server's `/login/desktop` page hands off to: the dev one
/// for `dev-` client tokens.
const SSO_SCHEMES: [&str; 2] = ["mattermost", "mattermost-dev"];

/// The schemes that open a page: Matty's own and the Mattermost desktop
/// app's, each with its dev twin, which the dev bundle registers.
const PAGE_SCHEMES: [&str; 4] = ["matty", "matty-dev", "mattermost", "mattermost-dev"];

/// The kinds of `matty://{kind}/{value}` entity link, which `matty://`
/// shares with server pages.
const ENTITY_KINDS: [&str; 3] = ["channel", "user", "hashtag"];

impl DeepLink {
    /// Reads `link`, or `None` when it is not one Matty handles.
    pub fn parse(link: &str) -> Option<Self> {
        let url = url::Url::parse(link.trim()).ok()?;
        if !PAGE_SCHEMES.contains(&url.scheme()) {
            return None;
        }
        // The server may live under a subpath: `/team/login/desktop`.
        let sso = url.path().trim_end_matches('/').ends_with("/login/desktop");
        if !sso || !SSO_SCHEMES.contains(&url.scheme()) {
            return Self::page(&url);
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

    /// A server page or, for `matty://`, an entity link.
    fn page(url: &url::Url) -> Option<Self> {
        // Hosts of non-special schemes keep their case.
        let host = url.host_str().filter(|host| !host.is_empty())?;
        let host = host.to_ascii_lowercase();
        let path = url.path();
        if path.trim_matches('/').is_empty() {
            return None;
        }
        let matty = url.scheme().starts_with("matty");
        if matty && ENTITY_KINDS.contains(&host.as_str()) {
            let value = path.trim_matches('/');
            return Some(DeepLink::Entity(format!("matty://{host}/{value}")));
        }
        let host = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host,
        };
        Some(DeepLink::Page {
            host,
            path: path.to_owned(),
        })
    }
}

/// The address of `path` on `server`, for [`crate::links::route`], when
/// `host` is that server's host; `None` for a link to another server. A
/// path without the server's subpath gets it: links from the desktop app
/// leave it out.
pub fn page_url(host: &str, path: &str, server: &str) -> Option<String> {
    let mut url = url::Url::parse(server).ok()?;
    let (name, port) = match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => {
            (name, port.parse::<u16>().ok())
        }
        _ => (host, None),
    };
    let same_host = url
        .host_str()
        .is_some_and(|server| server.eq_ignore_ascii_case(name));
    if !same_host || port.is_some_and(|port| Some(port) != url.port_or_known_default()) {
        return None;
    }
    let base = url.path().trim_end_matches('/').to_owned();
    let under_base = path == base || path.starts_with(&format!("{base}/"));
    let path = if base.is_empty() || under_base {
        path.to_owned()
    } else {
        format!("{base}{path}")
    };
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Some(url.into())
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
            "mattermost://chat.example.com/login/desktop?client_token=a",
            "mattermost://chat.example.com/login/desktop?client_token=&server_token=b",
            "mattermost://",
            "matty://chat.example.com",
            "matty://chat.example.com/",
            "matty:no-host",
            "file:///team/pl/abc",
        ] {
            assert_eq!(DeepLink::parse(link), None, "{link}");
        }
    }

    fn page(host: &str, path: &str) -> Option<DeepLink> {
        Some(DeepLink::Page {
            host: host.into(),
            path: path.into(),
        })
    }

    #[test]
    fn reads_server_pages_and_entities() {
        assert_eq!(
            DeepLink::parse("mattermost://chat.example.com/team/pl/abc123"),
            page("chat.example.com", "/team/pl/abc123")
        );
        assert_eq!(
            DeepLink::parse("matty://Chat.Example.com:8065/team/channels/town-square?x=1"),
            page("chat.example.com:8065", "/team/channels/town-square")
        );
        assert_eq!(
            DeepLink::parse("matty-dev://example.com/mm/team/pl/abc"),
            page("example.com", "/mm/team/pl/abc")
        );
        assert_eq!(
            DeepLink::parse("mattermost://h/login/desktopx?client_token=a&server_token=b"),
            page("h", "/login/desktopx")
        );
        // Only mattermost:// carries the SSO hand-off.
        assert_eq!(
            DeepLink::parse("matty://h/login/desktop?client_token=a&server_token=b"),
            page("h", "/login/desktop")
        );
        assert_eq!(
            DeepLink::parse("matty://channel/town-square"),
            Some(DeepLink::Entity("matty://channel/town-square".into()))
        );
        assert_eq!(
            DeepLink::parse("matty://user/ana/"),
            Some(DeepLink::Entity("matty://user/ana".into()))
        );
        // mattermost:// has no entity links: that is a server named channel.
        assert_eq!(
            DeepLink::parse("mattermost://channel/town-square"),
            page("channel", "/town-square")
        );
    }

    #[test]
    fn pages_land_on_the_signed_in_server_only() {
        let server = "https://chat.example.com";
        assert_eq!(
            page_url("chat.example.com", "/team/pl/abc", server).as_deref(),
            Some("https://chat.example.com/team/pl/abc")
        );
        assert_eq!(
            page_url("CHAT.example.com:443", "/team/pl/abc", server).as_deref(),
            Some("https://chat.example.com/team/pl/abc")
        );
        assert_eq!(page_url("chat.example.com:8065", "/t/pl/a", server), None);
        assert_eq!(page_url("other.example.com", "/t/pl/a", server), None);
        // A server under a subpath, with or without it in the link.
        let sub = "http://example.com:8065/mm/";
        assert_eq!(
            page_url("example.com:8065", "/mm/team/pl/abc", sub).as_deref(),
            Some("http://example.com:8065/mm/team/pl/abc")
        );
        assert_eq!(
            page_url("example.com", "/team/pl/abc", sub).as_deref(),
            Some("http://example.com:8065/mm/team/pl/abc")
        );
        assert_eq!(page_url("example.com", "/t/pl/a", "not a url"), None);
    }

    /// A deep link reaches `links::route` as the same link in a message.
    #[test]
    fn pages_route_like_links_in_messages() {
        use crate::links::{LinkTarget, route};
        let server = "https://chat.example.com/mm";
        let open = |link: &str| match DeepLink::parse(link)? {
            DeepLink::Page { host, path } => {
                let url = page_url(&host, &path, server)?;
                Some(route(&url, Some(server), &["team"]))
            }
            DeepLink::Entity(url) => Some(route(&url, Some(server), &["team"])),
            DeepLink::SsoCallback { .. } => None,
        };
        assert_eq!(
            open("mattermost://chat.example.com/team/pl/abc123"),
            Some(LinkTarget::Permalink {
                team: "team".into(),
                post_id: "abc123".into(),
            })
        );
        assert_eq!(
            open("matty://chat.example.com/mm/team/channels/town-square"),
            Some(LinkTarget::Channel {
                team: Some("team".into()),
                name: "town-square".into(),
            })
        );
        assert_eq!(
            open("matty://user/ana"),
            Some(LinkTarget::User("ana".into()))
        );
        assert_eq!(open("matty://elsewhere.example.com/team/pl/abc"), None);
        assert_eq!(
            open("matty://chat.example.com/team/threads/abc"),
            Some(LinkTarget::External(
                "https://chat.example.com/mm/team/threads/abc".into()
            ))
        );
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
