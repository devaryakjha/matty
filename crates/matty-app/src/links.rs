//! Where a link in a message goes (#85, #103): `~channel`, `@user` and
//! `#hashtag` (rewritten to `matty://` links by `matty-markdown`), this
//! server's permalinks and channel URLs, and everything else in the
//! browser. [`route`] decides; the window acts on the answer.

use gpui_kit::{
    App, ElementId, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div,
};

use crate::ui::{
    alert_dialog::{alert_dialog, alert_dialog_action, alert_dialog_cancel},
    button::ButtonVariant,
    dialog::{DialogHandle, dialog_description, dialog_popup, dialog_title},
    markdown::{CustomEmoji as CustomEmojiImages, EntityLinks, MarkdownView, markdown},
    theme::UiTheme,
};

/// Where a clicked link goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// A channel by URL name: in `team` (a team name) from a channel URL,
    /// or in the team on screen from `~name`.
    Channel { team: Option<String>, name: String },
    /// A person by username.
    User(String),
    /// A hashtag, without the `#`: a message search for it.
    Hashtag(String),
    /// A post on this server: its channel, at the post.
    Permalink { team: String, post_id: String },
    /// Nothing to open: `@here`, an emoji, or a `matty://` link Matty
    /// does not know.
    Nothing,
    /// Anywhere else: the system browser.
    External(String),
}

/// Where `url` goes. `server` is the signed-in server's address, and
/// `teams` the names of the user's teams: a channel URL in another team
/// opens in the browser, since Matty cannot show it.
pub fn route(url: &str, server: Option<&str>, teams: &[&str]) -> LinkTarget {
    if let Some(entity) = url.strip_prefix("matty://") {
        return entity_target(entity);
    }
    server
        .and_then(|server| server_target(url, server, teams))
        .unwrap_or_else(|| LinkTarget::External(url.to_owned()))
}

/// `matty://user/ana` and the rest of `matty-markdown`'s entity links.
fn entity_target(entity: &str) -> LinkTarget {
    let (kind, value) = entity.split_once('/').unwrap_or((entity, ""));
    if value.is_empty() {
        return LinkTarget::Nothing;
    }
    match kind {
        "user" => LinkTarget::User(value.to_owned()),
        "channel" => LinkTarget::Channel {
            team: None,
            name: value.to_owned(),
        },
        "hashtag" => LinkTarget::Hashtag(value.to_owned()),
        _ => LinkTarget::Nothing,
    }
}

/// A link into this server's web app: `{server}/{team}/pl/{post_id}`,
/// `{server}/{team}/channels/{name}` or `{server}/{team}/messages/@{user}`.
fn server_target(url: &str, server: &str, teams: &[&str]) -> Option<LinkTarget> {
    let url = url::Url::parse(url).ok()?;
    let server = url::Url::parse(server).ok()?;
    if url.origin() != server.origin() {
        return None;
    }
    let base: Vec<&str> = server.path_segments()?.filter(|s| !s.is_empty()).collect();
    let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    let rest = segments.strip_prefix(base.as_slice())?;
    let [team, kind, value] = rest else {
        return None;
    };
    let id_like = |value: &str| {
        !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    };
    let known = teams.iter().any(|known| known.eq_ignore_ascii_case(team));
    match *kind {
        // A post id names one post on the server, whichever team the link
        // went through.
        "pl" if id_like(value) => Some(LinkTarget::Permalink {
            team: (*team).to_owned(),
            post_id: (*value).to_owned(),
        }),
        "channels" if known && id_like(value) => Some(LinkTarget::Channel {
            team: Some((*team).to_owned()),
            name: (*value).to_owned(),
        }),
        "messages" if known => {
            let user = value.strip_prefix('@')?;
            id_like(user).then(|| LinkTarget::User(user.to_owned()))
        }
        _ => None,
    }
}

/// A message body: Markdown with `~channel`, `@user` and `#hashtag` as
/// gpuicn entity chips, and every click, chip or link, handed to
/// `on_link` with its URL.
pub fn message_body(
    id: impl Into<ElementId>,
    body: &str,
    on_link: impl Fn(&SharedString, &mut Window, &mut App) + Clone + Send + Sync + 'static,
    cx: &App,
) -> MarkdownView {
    let custom = cx.try_global::<crate::emoji::CustomEmoji>().cloned();
    let is_custom = |name: &str| custom.as_ref().is_some_and(|known| known.contains(name));
    let source = matty_markdown::link_entities_in_source(body, &is_custom);
    let chip = on_link.clone();
    markdown(id, source, cx)
        // Custom emoji as images sized to the text, from sync's media cache
        // (#153); until one lands, and for names the server does not know,
        // the text stays `:name:`.
        .plugin(CustomEmojiImages::new("emoji", |name, _, cx| {
            crate::emoji::custom_source(name, cx)
        }))
        .plugin(
            EntityLinks::new("matty").on_click(move |target, window, cx| {
                chip(&format!("matty:{target}").into(), window, cx)
            }),
        )
        .on_link_click(move |url, _, window, cx| on_link(url, window, cx))
}

/// What to do with the channel a link named, once found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opening {
    /// The user is in it: open it.
    Open,
    /// A public channel the user is not in: offer to join.
    Join,
    /// A channel of a team other than the one on screen, which Matty
    /// cannot switch to yet.
    OtherTeam,
    /// Private, archived, or otherwise not the user's to open.
    Closed,
}

/// Whether the user can open `channel` from a link: `member` says it is
/// in the shown sidebar, `team` is the team on screen.
pub fn opening(channel: &matty_api::models::Channel, member: bool, team: Option<&str>) -> Opening {
    if member {
        return Opening::Open;
    }
    if !channel.team_id.is_empty() && Some(channel.team_id.as_str()) != team {
        return Opening::OtherTeam;
    }
    if channel.kind == "O" && channel.delete_at == 0 {
        Opening::Join
    } else {
        Opening::Closed
    }
}

/// A public channel a link named that the user is not in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinOffer {
    pub channel_id: SharedString,
    pub name: SharedString,
    /// The permalink's post, to jump to once joined.
    pub post: Option<SharedString>,
}

/// "You're not in #name": gpuicn's alert dialog, kept mounted while closed. `on_join`
/// runs when the user chooses Join.
pub fn join_dialog(
    handle: &DialogHandle,
    offer: Option<&JoinOffer>,
    on_join: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let theme = UiTheme::read(cx).clone();
    let name = offer.map_or_else(
        || "this channel".to_owned(),
        |offer| format!("#{}", offer.name),
    );
    let what = if offer.is_some_and(|offer| offer.post.is_some()) {
        "The link points to a message there. Join the channel to read it."
    } else {
        "Join the channel to read it and get its new messages."
    };
    let popup = dialog_popup("link.join.popup", "Join channel", cx)
        .child(dialog_title("link.join.title", cx).child(format!("You're not in {name}")))
        .child(dialog_description("link.join.description", cx).child(what))
        .child(
            div()
                .flex()
                .justify_end()
                .gap(theme.space(2.))
                .pt(theme.space(2.))
                .child(
                    alert_dialog_cancel("link.join.cancel")
                        .variant(ButtonVariant::Outline)
                        .label("Cancel"),
                )
                .child(alert_dialog_action("link.join.confirm").label("Join channel")),
        );
    alert_dialog("link.join", handle, popup, window, cx).on_ok(move |_, window, cx| {
        on_join(window, cx);
        true
    })
}

/// Whether the switcher's query is a link worth offering to open.
pub fn looks_like_link(query: &str) -> bool {
    let query = query.trim();
    !query.contains(char::is_whitespace)
        && url::Url::parse(query).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER: &str = "https://chat.example.com";
    const TEAMS: &[&str] = &["matty", "design-guild"];

    fn go(url: &str) -> LinkTarget {
        route(url, Some(SERVER), TEAMS)
    }

    #[test]
    fn entities() {
        assert_eq!(
            go("matty://channel/town-square"),
            LinkTarget::Channel {
                team: None,
                name: "town-square".into()
            }
        );
        assert_eq!(go("matty://user/ana"), LinkTarget::User("ana".into()));
        assert_eq!(
            go("matty://hashtag/release"),
            LinkTarget::Hashtag("release".into())
        );
        assert_eq!(go("matty://special/here"), LinkTarget::Nothing);
        assert_eq!(go("matty://emoji/tada"), LinkTarget::Nothing);
        assert_eq!(go("matty://user/"), LinkTarget::Nothing);
    }

    #[test]
    fn permalinks_on_this_server() {
        let post = "8xk4f1qjtbyzjm7ffwz3ipk3ya";
        assert_eq!(
            go(&format!("{SERVER}/matty/pl/{post}")),
            LinkTarget::Permalink {
                team: "matty".into(),
                post_id: post.into()
            }
        );
        // Any team: the post id alone finds the post.
        assert_eq!(
            go(&format!("{SERVER}/elsewhere/pl/{post}?x=1#y")),
            LinkTarget::Permalink {
                team: "elsewhere".into(),
                post_id: post.into()
            }
        );
    }

    #[test]
    fn channel_and_message_urls() {
        assert_eq!(
            go(&format!("{SERVER}/design-guild/channels/reviews")),
            LinkTarget::Channel {
                team: Some("design-guild".into()),
                name: "reviews".into()
            }
        );
        assert_eq!(
            go(&format!("{SERVER}/matty/messages/@ana")),
            LinkTarget::User("ana".into())
        );
        // A team the user is not in: the browser shows it.
        let other = format!("{SERVER}/other/channels/reviews");
        assert_eq!(go(&other), LinkTarget::External(other.clone()));
    }

    #[test]
    fn a_server_under_a_path() {
        let server = "https://example.com/mattermost/";
        assert_eq!(
            route(
                "https://example.com/mattermost/matty/pl/abc123",
                Some(server),
                TEAMS
            ),
            LinkTarget::Permalink {
                team: "matty".into(),
                post_id: "abc123".into()
            }
        );
        let outside = "https://example.com/matty/pl/abc123";
        assert_eq!(
            route(outside, Some(server), TEAMS),
            LinkTarget::External(outside.into())
        );
    }

    #[test]
    fn everything_else_opens_in_the_browser() {
        for url in [
            "https://github.com/devaryakjha/matty",
            "http://chat.example.com/matty/pl/abc",
            "https://chat.example.com:8443/matty/pl/abc",
            "https://chat.example.com/matty/pl/abc/extra",
            "https://chat.example.com/matty/pl/a%20b",
            "https://chat.example.com/matty",
            "mailto:ana@example.com",
        ] {
            assert_eq!(go(url), LinkTarget::External(url.into()), "{url}");
        }
        // Signed out: no server to compare with.
        let link = "https://chat.example.com/matty/pl/abc";
        assert_eq!(route(link, None, TEAMS), LinkTarget::External(link.into()));
    }

    #[test]
    fn openings() {
        let channel = |kind: &str, team: &str, delete_at: i64| matty_api::models::Channel {
            kind: kind.into(),
            team_id: team.into(),
            delete_at,
            ..Default::default()
        };
        let shown = Some("t1");
        assert_eq!(opening(&channel("O", "t1", 0), true, shown), Opening::Open);
        assert_eq!(opening(&channel("O", "t1", 0), false, shown), Opening::Join);
        assert_eq!(
            opening(&channel("O", "t2", 0), false, shown),
            Opening::OtherTeam
        );
        assert_eq!(
            opening(&channel("P", "t1", 0), false, shown),
            Opening::Closed
        );
        assert_eq!(
            opening(&channel("O", "t1", 5), false, shown),
            Opening::Closed
        );
        assert_eq!(opening(&channel("D", "", 0), false, shown), Opening::Closed);
    }

    #[test]
    fn switcher_links() {
        assert!(looks_like_link("  https://chat.example.com/matty/pl/abc "));
        assert!(!looks_like_link("design"));
        assert!(!looks_like_link("https://a.com b"));
        assert!(!looks_like_link("ftp://a.com"));
    }
}
