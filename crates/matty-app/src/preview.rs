//! Sample data for looking at the design before sync exists.
//!
//! Run Matty with `MATTY_PREVIEW=1` and the views render the fixtures below
//! instead of their loading and empty states. Nothing here talks to a server
//! or is reachable without the variable, and the screenshots in
//! `docs/screenshots/design-*` are taken this way (DESIGN.md §6).

use crate::sidebar::{Category, ChannelKind, ChannelRow, Presence, SidebarModel};

/// Whether the preview fixtures are on for this process.
pub fn enabled() -> bool {
    std::env::var_os("MATTY_PREVIEW").is_some_and(|value| !value.is_empty() && value != "0")
}

/// A sidebar with every row state DESIGN.md §4 describes.
pub fn sidebar() -> SidebarModel {
    let public = |id: &str, name: &str| ChannelRow::new(id, name, ChannelKind::Public);
    let dm = |id: &str, name: &str, presence| {
        ChannelRow::new(id, name, ChannelKind::Direct { presence })
    };
    SidebarModel {
        loading: false,
        selected: Some("general".into()),
        categories: vec![
            Category::new(
                "favorites",
                "Favorites",
                vec![
                    public("design", "design").unread().mentions(2),
                    dm("priya", "Priya Nair", Presence::Online).unread(),
                ],
            ),
            Category::new(
                "channels",
                "Channels",
                vec![
                    public("general", "general"),
                    public("town-square", "town-square").unread(),
                    public("random", "random").muted(),
                    ChannelRow::new("release-2026-11", "release-2026-11", ChannelKind::Private),
                    public("gpui", "gpui"),
                    public("help-desk", "help-desk").unread().mentions(1),
                ],
            ),
            Category::new(
                "direct",
                "Direct messages",
                vec![
                    dm("sam", "Sam Okafor", Presence::Away),
                    dm("lena", "Lena Fischer", Presence::Dnd)
                        .unread()
                        .mentions(1),
                    ChannelRow::new("priya-sam", "Priya, Sam", ChannelKind::Group { members: 3 }),
                    dm("jonas", "Jonas Weber", Presence::Offline),
                ],
            ),
        ],
    }
}

use crate::channel::{ChannelModel, Post, PostReaction};

/// Posts for the channel with `id`, with a day divider, a continuation, a
/// mention, reactions, an edit mark and a "New messages" line.
pub fn channel(id: &str) -> Option<ChannelModel> {
    let row = sidebar()
        .categories
        .into_iter()
        .flat_map(|category| category.rows)
        .find(|row| row.id.as_ref() == id)?;
    let post = |id: &str, author: &str, time: &str, day: &str, body: &str| Post {
        id: id.into(),
        author: author.into(),
        time: time.into(),
        day: day.into(),
        body: body.into(),
        reactions: vec![],
        mentions_me: false,
        edited: false,
        continues: false,
    };
    let reaction = |emoji: &str, count: usize, mine: bool| PostReaction {
        emoji: emoji.into(),
        count,
        mine,
    };
    let posts = vec![
        post(
            "p1",
            "Priya Nair",
            "09:12",
            "Yesterday",
            "Morning all. The release branch is cut, so anything that is not a fix waits for next week.",
        ),
        Post {
            reactions: vec![reaction("👍", 4, true), reaction("🚀", 2, false)],
            ..post(
                "p2",
                "Sam Okafor",
                "09:14",
                "Yesterday",
                "Thanks! Notes are in ~release-2026-11, and the checklist is pinned there.",
            )
        },
        Post {
            continues: true,
            ..post(
                "p3",
                "Sam Okafor",
                "09:15",
                "Yesterday",
                "One more thing: the macOS build now needs `MATTY_PREVIEW=1` to show sample data, nothing else changed.",
            )
        },
        Post {
            edited: true,
            ..post(
                "p4",
                "Lena Fischer",
                "16:40",
                "Yesterday",
                "Reviewed the sidebar PR. Two small comments on **unread** weights, otherwise good to merge.",
            )
        },
        post(
            "p5",
            "Jonas Weber",
            "08:02",
            "Today",
            "Does anyone have the link to the design doc? I only find the plan.",
        ),
        Post {
            mentions_me: true,
            reactions: vec![reaction("🙏", 1, false)],
            ..post(
                "p6",
                "Priya Nair",
                "08:05",
                "Today",
                "@arya could you take a look at the title bar on Windows when you get a minute? The controls overlap the switcher at 480px wide.",
            )
        },
        Post {
            continues: true,
            ..post(
                "p7",
                "Priya Nair",
                "08:06",
                "Today",
                "Here is the doc Jonas asked for: [DESIGN.md](https://github.com/devaryakjha/matty/blob/master/docs/DESIGN.md)",
            )
        },
        post(
            "p8",
            "Sam Okafor",
            "08:31",
            "Today",
            "On it. Code blocks render like this now:\n\n```rust\nlet theme = UiTheme::read(cx);\n```",
        ),
    ];
    Some(ChannelModel {
        id: row.id.clone(),
        name: row.name.clone(),
        kind: row.kind.clone(),
        header: (id == "general")
            .then(|| "Announcements and anything that does not fit elsewhere".into()),
        members: 42,
        pinned: 3,
        favorite: id == "design",
        posts,
        first_unread: Some(5),
    })
}
