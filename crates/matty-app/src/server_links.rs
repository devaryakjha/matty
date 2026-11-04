//! Which saved server a link belongs to (#206). A `matty://` or
//! `mattermost://` page link, or a server's own `https://` link in a
//! message, may name a server other than the one shown: the window shows
//! that server first, then opens the link there as it would its own. A page
//! link to a server Matty has not saved offers to add it.

use crate::deeplink::page_url;

/// Where a `matty://` or `mattermost://` page link opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageServer {
    /// On the server shown: `url` is the page's address there.
    Shown(String),
    /// On the `index`th saved server, once it is shown: `url` is the page's
    /// address there.
    Saved {
        /// The server's place in the saved servers.
        index: usize,
        /// The page's address on that server.
        url: String,
    },
    /// On a server Matty has not saved: `address` is what the Add server
    /// card starts with, the link's host and port.
    Unsaved {
        /// The host, with its port when the link gave one.
        address: String,
    },
}

/// Where the page at `path` on `host` opens. `shown` is the address of the
/// server shown, if any, and `saved` those of the saved servers, in order.
/// When more than one server lives on `host`, as under subpaths, the one
/// whose subpath the link's path starts with wins; then the shown one.
pub fn page_server(host: &str, path: &str, shown: Option<&str>, saved: &[&str]) -> PageServer {
    let mut best: Option<(usize, PageServer)> = None;
    let candidates = shown.map(|server| (None, server)).into_iter().chain(
        saved
            .iter()
            .enumerate()
            .map(|(i, server)| (Some(i), *server)),
    );
    for (index, server) in candidates {
        let Some(url) = page_url(host, path, server) else {
            continue;
        };
        let fit = subpath_fit(path, server);
        if best.as_ref().is_some_and(|(best, _)| *best >= fit) {
            continue;
        }
        let place = match index {
            None => PageServer::Shown(url),
            Some(index) => PageServer::Saved { index, url },
        };
        best = Some((fit, place));
    }
    match best {
        // The shown server is saved too: it stays the one shown.
        Some((_, PageServer::Saved { index, url }))
            if shown.is_some_and(|shown| same_server(shown, saved[index])) =>
        {
            PageServer::Shown(url)
        }
        Some((_, place)) => place,
        None => PageServer::Unsaved {
            address: host.to_owned(),
        },
    }
}

/// How much of `path` the server's subpath covers: its length when the
/// path starts with it, else nothing.
fn subpath_fit(path: &str, server: &str) -> usize {
    let Ok(url) = url::Url::parse(server) else {
        return 0;
    };
    let base = url.path().trim_end_matches('/');
    if !base.is_empty() && (path == base || path.starts_with(&format!("{base}/"))) {
        base.len()
    } else {
        0
    }
}

/// Whether two saved addresses name the same server.
fn same_server(a: &str, b: &str) -> bool {
    let normal = |server: &str| server.trim_end_matches('/').to_ascii_lowercase();
    normal(a) == normal(b)
}

/// The saved server, other than the one shown (`shown`, its index), whose
/// post, channel or direct message page `url` is: a link in a message that
/// the shown server does not own. `None` for anything else, which opens in
/// the browser as before.
pub fn message_link_server(url: &str, shown: Option<usize>, saved: &[&str]) -> Option<usize> {
    saved
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != shown)
        .find(|(_, server)| crate::links::opens_on(url, server))
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deeplink::DeepLink;

    const SHOWN: &str = "https://chat.example.com";
    const SAVED: [&str; 3] = [
        "https://chat.example.com",
        "https://acme.example.org",
        "http://intranet.example.net:8065/mm",
    ];

    fn open(link: &str) -> PageServer {
        match DeepLink::parse(link) {
            Some(DeepLink::Page { host, path }) => page_server(&host, &path, Some(SHOWN), &SAVED),
            other => panic!("{link} is not a page link: {other:?}"),
        }
    }

    #[test]
    fn links_to_the_shown_server_open_there() {
        assert_eq!(
            open("mattermost://chat.example.com/team/pl/abc"),
            PageServer::Shown("https://chat.example.com/team/pl/abc".into())
        );
        assert_eq!(
            open("matty://CHAT.example.com/team/channels/town-square"),
            PageServer::Shown("https://chat.example.com/team/channels/town-square".into())
        );
    }

    #[test]
    fn links_to_another_saved_server_switch_to_it() {
        assert_eq!(
            open("mattermost://acme.example.org/sales/pl/xyz"),
            PageServer::Saved {
                index: 1,
                url: "https://acme.example.org/sales/pl/xyz".into(),
            }
        );
        // A server under a subpath, with or without it in the link.
        assert_eq!(
            open("matty://intranet.example.net:8065/mm/eng/channels/ops"),
            PageServer::Saved {
                index: 2,
                url: "http://intranet.example.net:8065/mm/eng/channels/ops".into(),
            }
        );
        assert_eq!(
            open("mattermost://intranet.example.net:8065/eng/pl/abc"),
            PageServer::Saved {
                index: 2,
                url: "http://intranet.example.net:8065/mm/eng/pl/abc".into(),
            }
        );
    }

    #[test]
    fn links_to_unsaved_servers_offer_to_add_them() {
        assert_eq!(
            open("mattermost://new.example.com/team/pl/abc"),
            PageServer::Unsaved {
                address: "new.example.com".into()
            }
        );
        // Another port is another server.
        assert_eq!(
            open("matty://acme.example.org:8443/team/pl/abc"),
            PageServer::Unsaved {
                address: "acme.example.org:8443".into()
            }
        );
        // Nothing shown and nothing saved: still an offer, not a guess.
        assert_eq!(
            page_server("chat.example.com", "/t/pl/a", None, &[]),
            PageServer::Unsaved {
                address: "chat.example.com".into()
            }
        );
    }

    #[test]
    fn the_subpath_picks_between_servers_on_one_host() {
        let saved = ["https://example.com/a", "https://example.com/b"];
        assert_eq!(
            page_server("example.com", "/b/team/pl/p", Some(saved[0]), &saved),
            PageServer::Saved {
                index: 1,
                url: "https://example.com/b/team/pl/p".into(),
            }
        );
        // No subpath in the link: the shown server keeps it.
        assert_eq!(
            page_server("example.com", "/team/pl/p", Some(saved[0]), &saved),
            PageServer::Shown("https://example.com/a/team/pl/p".into())
        );
    }

    #[test]
    fn a_saved_server_without_a_shown_one_switches() {
        assert_eq!(
            page_server("acme.example.org", "/t/pl/p", None, &SAVED),
            PageServer::Saved {
                index: 1,
                url: "https://acme.example.org/t/pl/p".into(),
            }
        );
    }

    #[test]
    fn message_links_find_their_saved_server() {
        let shown = Some(0);
        assert_eq!(
            message_link_server("https://acme.example.org/sales/pl/xyz", shown, &SAVED),
            Some(1)
        );
        assert_eq!(
            message_link_server(
                "https://acme.example.org/sales/channels/leads",
                shown,
                &SAVED
            ),
            Some(1)
        );
        assert_eq!(
            message_link_server(
                "http://intranet.example.net:8065/mm/eng/messages/@ana",
                shown,
                &SAVED
            ),
            Some(2)
        );
        // The shown server's own links are `links::route`'s.
        assert_eq!(
            message_link_server("https://chat.example.com/team/pl/abc", shown, &SAVED),
            None
        );
        // Other pages of a saved server, and other sites, open in the browser.
        for url in [
            "https://acme.example.org/sales/threads/abc",
            "https://acme.example.org/admin_console",
            "http://acme.example.org/sales/pl/xyz",
            "https://intranet.example.net:8065/mm/eng/pl/abc",
            "https://example.com/team/pl/abc",
        ] {
            assert_eq!(message_link_server(url, shown, &SAVED), None, "{url}");
        }
    }
}
