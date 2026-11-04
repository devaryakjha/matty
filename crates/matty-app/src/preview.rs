//! Sample data for looking at the design before sync exists.
//!
//! Run Matty with `MATTY_PREVIEW=1` and the views render the fixtures below
//! instead of their loading and empty states. Nothing here talks to a server
//! or is reachable without the variable, and the screenshots in
//! `docs/screenshots/design-*` are taken this way (DESIGN.md §6).

use crate::login::{LoginModel, LoginStep, SignInMethods, SsoProvider};
use crate::sidebar::{Category, CategoryKind, ChannelKind, ChannelRow, Presence, SidebarModel};

static WEB_VALUE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Sets the preview value where there is no environment (the web build reads
/// it from the page's `?preview=` query).
pub fn set(value: String) {
    let _ = WEB_VALUE.set(value);
}

fn value() -> Option<String> {
    WEB_VALUE
        .get()
        .cloned()
        .or_else(|| std::env::var("MATTY_PREVIEW").ok())
}

/// Whether the preview fixtures are on for this process.
/// Who is typing a reply in any preview thread.
pub fn thread_typing() -> Vec<SharedString> {
    vec!["Sam Okafor".into()]
}

pub fn enabled() -> bool {
    value().is_some_and(|value| !value.is_empty() && value != "0")
}

/// A sidebar with every row state DESIGN.md §4 describes.
/// `MATTY_PREVIEW=login` shows the sign-in card at its methods step over the
/// fixtures, and `MATTY_PREVIEW=login-error` the same with a failed sign-in.
/// `login-ldap` offers AD/LDAP beside passwords, `login-mfa` and
/// `login-mfa-error` show the code step, `login-server` the empty server
/// step, and `login-version` the server step
/// refusing a server below the version floor. `sso-waiting` shows the card
/// waiting for the browser after GitLab, and `sso-expired` the same after
/// the three minutes ran out.
pub fn login() -> Option<LoginModel> {
    let value = value()?;
    let methods = SignInMethods {
        password: true,
        ldap: value == "login-ldap",
        sso: vec![SsoProvider::GitLab, SsoProvider::Google],
    };
    let server_name: gpui_kit::SharedString = "chat.example.com".into();
    let wrong_code = "That code is not right. Enter the current code from your authenticator app.";
    let (step, error) = match value.as_str() {
        "login" | "login-ldap" => (
            LoginStep::Methods {
                server_name,
                methods,
            },
            None,
        ),
        "login-error" => (
            LoginStep::Methods {
                server_name,
                methods,
            },
            Some("That password is not right. Check it and try again.".into()),
        ),
        "login-mfa" => (
            LoginStep::Mfa {
                server_name,
                methods,
            },
            None,
        ),
        "login-mfa-error" => (
            LoginStep::Mfa {
                server_name,
                methods,
            },
            Some(wrong_code.into()),
        ),
        "sso-waiting" | "sso-expired" => (
            LoginStep::Methods {
                server_name,
                methods,
            }
            .to_browser(SsoProvider::GitLab),
            (value == "sso-expired").then(|| {
                "Sign-in timed out: the browser did not finish within three minutes.".into()
            }),
        ),
        "login-server" => (LoginStep::Server, None),
        "login-version" => {
            let old = matty_api::UnsupportedVersion {
                found: matty_api::Version::new(9, 3, 2),
                minimum: matty_api::capabilities::MINIMUM_VERSION,
            };
            (LoginStep::Server, Some(old.to_string().into()))
        }
        _ => return None,
    };
    Some(LoginModel {
        step,
        busy: false,
        error,
    })
}

/// `MATTY_PREVIEW=offline` and `reconnecting` show the connection banner.
pub fn connection() -> crate::main_window::ConnectionState {
    use crate::main_window::ConnectionState;
    match value().as_deref() {
        Some("offline") => ConnectionState::Offline,
        Some("reconnecting") => ConnectionState::Reconnecting { attempt: 3 },
        _ => ConnectionState::Online,
    }
}

/// Messages waiting to send in the offline fixtures: Arya's last one.
pub fn queued() -> usize {
    usize::from(connection() != crate::main_window::ConnectionState::Online)
}

/// The teams in the rail: name and whether it is the current one.
pub fn teams() -> Vec<Team> {
    vec![
        Team::new("matty", "Matty"),
        Team::new("design-guild", "Design Guild").mentions(3),
        Team::new("open-source", "Open Source").unread(),
    ]
}

pub fn sidebar() -> SidebarModel {
    let public = |id: &str, name: &str| ChannelRow::new(id, name, ChannelKind::Public);
    let dm = |id: &str, name: &str, presence| {
        ChannelRow::new(id, name, ChannelKind::Direct { presence }).person(format!("u-{id}"))
    };
    SidebarModel {
        loading: false,
        selected: Some("general".into()),
        unread_threads: 2,
        drafts: 2,
        drafted: ["design".into()].into_iter().collect(),
        categories: preview_kinds(vec![
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
        ]),
    }
}

/// Gives the preview's categories the kinds their ids name.
fn preview_kinds(mut categories: Vec<Category>) -> Vec<Category> {
    for category in &mut categories {
        category.kind = match category.id.as_ref() {
            "favorites" => CategoryKind::Favorites,
            "direct" => CategoryKind::Direct,
            _ => CategoryKind::Channels,
        };
    }
    categories
}

use crate::account::Account;
use crate::attachments::{self, AttachmentStatus, PostFile, Upload, UploadSource};
use crate::channel::{
    ChannelModel, DEFAULT_MAX_POST_SIZE, Post, PostReaction, PostStatus, ReplySummary,
};
use crate::directory::DirectoryChannel;
use crate::drafts::{Draft, DraftsModel};
use crate::profile::UserProfile;
use crate::rail::Team;
use crate::right_pane::{ListedPost, ThreadModel};
use crate::threads::{ThreadRow, ThreadsModel};
use gpui_kit::SharedString;

/// Posts for the channel with `id`, with a day divider, a continuation, a
/// mention, reactions, an edit mark and a "New messages" line.
/// The name of a fixture file, for the preview's download toast.
pub fn file_name(file_id: &str) -> Option<String> {
    sidebar()
        .categories
        .iter()
        .flat_map(|category| category.rows.iter())
        .filter_map(|row| channel(&row.id))
        .flat_map(|channel| channel.posts)
        .flat_map(|post| post.files)
        .find(|file| file.id.as_ref() == file_id)
        .map(|file| file.name.to_string())
}

/// `MATTY_PREVIEW_POSTS=<n>` fills `general` with `n` posts, the fixtures
/// over and over, for scrolling a long channel (`scripts/perf-run`, #174).
pub fn long_posts() -> Option<usize> {
    std::env::var("MATTY_PREVIEW_POSTS").ok()?.parse().ok()
}

/// `posts` repeated to `count`, each copy with its own ids.
fn repeat(posts: &[Post], count: usize) -> Vec<Post> {
    posts
        .iter()
        .cycle()
        .take(count)
        .enumerate()
        .map(|(index, post)| Post {
            id: format!("{}-{index}", post.id).into(),
            replies: None,
            ..post.clone()
        })
        .collect()
}

pub fn channel(id: &str) -> Option<ChannelModel> {
    let row = sidebar()
        .categories
        .into_iter()
        .flat_map(|category| category.rows)
        .find(|row| row.id.as_ref() == id)?;
    let post = |id: &str, author: &str, time: &str, day: &str, body: &str| Post {
        kind: crate::channel::PostKind::Regular,
        id: id.into(),
        author: author.into(),
        author_id: person(author),
        time: hm(time),
        day: day.into(),
        date: full_date(day),
        body: body.into(),
        reactions: vec![],
        mentions_me: false,
        edited: false,
        continues: false,
        mine: false,
        saved: false,
        pinned: false,
        status: PostStatus::Sent,
        replies: None,
        root_id: "".into(),
        reply_to: None,
        files: vec![],
        embeds: vec![],
    };
    let reaction = |emoji: &str, count: usize, mine: bool, users: &[&str]| PostReaction {
        emoji: emoji.into(),
        count,
        mine,
        users: users.iter().map(|user| user.to_string().into()).collect(),
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
            reactions: vec![
                reaction(
                    "👍",
                    4,
                    true,
                    &["Sam Okafor", "Lena Fischer", "Jonas Weber"],
                ),
                reaction("🚀", 2, false, &["Sam Okafor", "Priya Nair"]),
            ],
            replies: Some(ReplySummary {
                count: 1,
                last_time: hm("10:02"),
                last_day: "Today".into(),
                participants: vec!["Lena Fischer".into()],
                participant_ids: vec![person("Lena Fischer")],
                following: false,
                unread: true,
            }),
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
            embeds: vec![crate::embeds::Embed::Attachment(
                crate::embeds::AttachmentCard {
                    color: "good".into(),
                    pretext: "".into(),
                    author: "GitHub Actions".into(),
                    author_link: "".into(),
                    title: "Build 1482 passed".into(),
                    title_link: "https://github.com/devaryakjha/matty/actions".into(),
                    text: "All checks are green on `release-2026-11`.".into(),
                    fields: vec![
                        crate::embeds::AttachmentFieldCard {
                            title: "Branch".into(),
                            value: "release-2026-11".into(),
                            short: true,
                        },
                        crate::embeds::AttachmentFieldCard {
                            title: "Duration".into(),
                            value: "6 min 12 s".into(),
                            short: true,
                        },
                    ],
                    footer: "matty / ci.yml".into(),
                    image: None,
                },
            )],
            ..post("p3b", "Build Bot", "09:20", "Yesterday", "")
        },
        Post {
            embeds: vec![crate::embeds::Embed::Link(crate::embeds::LinkCard {
                url: "https://github.com/devaryakjha/matty/releases".into(),
                site_name: "GitHub".into(),
                title: "Releases · devaryakjha/matty".into(),
                description: "A native desktop client for Mattermost, built with GPUI.".into(),
                image: Some(crate::embeds::EmbedImage {
                    url: "https://opengraph.githubassets.com/matty/releases".into(),
                    width: 600,
                    height: 315,
                    loaded: Some(attachments::png(include_bytes!(
                        "../assets/preview/og-releases.png"
                    ))),
                }),
            })],
            ..post(
                "p3c",
                "Sam Okafor",
                "09:22",
                "Yesterday",
                "Release notes draft: https://github.com/devaryakjha/matty/releases",
            )
        },
        Post {
            edited: true,
            pinned: true,
            ..post(
                "p4",
                "Lena Fischer",
                "16:40",
                "Yesterday",
                "Reviewed the sidebar PR. Two small comments on **unread** weights, otherwise good to merge.",
            )
        },
        Post {
            embeds: vec![crate::embeds::Embed::Image(crate::embeds::EmbedImage {
                url: "https://matty.example.com/shots/sidebar-unread.png".into(),
                width: 640,
                height: 400,
                loaded: Some(attachments::png(include_bytes!(
                    "../assets/preview/mockup.png"
                ))),
            })],
            continues: true,
            ..post(
                "p4b",
                "Lena Fischer",
                "16:41",
                "Yesterday",
                "For reference, how it looks here: https://matty.example.com/shots/sidebar-unread.png",
            )
        },
        Post {
            replies: Some(ReplySummary {
                count: 3,
                last_time: hm("08:13"),
                last_day: "Today".into(),
                participants: vec!["Arya".into(), "Jonas Weber".into()],
                participant_ids: vec![person("Arya"), person("Jonas Weber")],
                following: true,
                unread: false,
            }),
            ..post(
                "p5",
                "Jonas Weber",
                "08:02",
                "Today",
                "Does anyone have the link to the design doc? I only find the plan.",
            )
        },
        Post {
            mine: true,
            saved: true,
            ..post(
                "p5b",
                "Arya",
                "08:03",
                "Today",
                "It is in the repo under docs, one sec.",
            )
        },
        Post {
            mentions_me: true,
            reactions: vec![reaction("🙏", 1, false, &["Priya Nair"])],
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
    let mut posts = posts;
    posts.push(Post {
        files: vec![
            PostFile {
                id: "f-mockup".into(),
                name: "sidebar-mockup.png".into(),
                size: 48_312,
                preview: Some(UploadSource::Image(attachments::png(include_bytes!(
                    "../assets/preview/mockup.png"
                )))),
                width: 640,
                height: 400,
            },
            PostFile {
                id: "f-mockup-dark".into(),
                name: "sidebar-mockup-dark.png".into(),
                size: 51_904,
                preview: Some(UploadSource::Image(attachments::png(include_bytes!(
                    "../assets/preview/mockup-dark.png"
                )))),
                width: 640,
                height: 400,
            },
            PostFile {
                id: "f-notes".into(),
                name: "review-notes.pdf".into(),
                size: 2_516_582,
                preview: None,
                width: 0,
                height: 0,
            },
        ],
        ..post(
            "p8b",
            "Lena Fischer",
            "08:33",
            "Today",
            "Sidebar mockups for the review, light and dark, and my notes.",
        )
    });
    if queued() > 0 {
        posts.push(Post {
            mine: true,
            status: PostStatus::Pending,
            ..post(
                "p8c",
                "Arya",
                "08:38",
                "Today",
                "Pushing the title bar fix now.",
            )
        });
    }
    posts.push(Post {
        mine: true,
        status: PostStatus::Failed,
        ..post(
            "p9",
            "Arya",
            "08:40",
            "Today",
            "Also: who owns the Windows build this week?",
        )
    });
    // Two channels show the states PLAN §4.5 asks for: one still loading,
    // one with nothing posted yet.
    // #town-square runs sync's own transcript over server posts, so it
    // shows what system and plugin posts become (#104).
    let (posts, loading, first_unread) = match id {
        "town-square" => (activity(&row), false, None),
        "gpui" => (vec![], true, None),
        "release-2026-11" => (vec![], false, None),
        "general" if long_posts().is_some() => {
            (repeat(&posts, long_posts().unwrap_or(0)), false, None)
        }
        _ => (posts, false, Some(7)),
    };
    Some(ChannelModel {
        // `MATTY_PREVIEW=offline` shows the empty channel as not saved here.
        offline: connection() == crate::main_window::ConnectionState::Offline,
        id: row.id.clone(),
        name: row.name.clone(),
        kind: row.kind.clone(),
        header: match id {
            "general" => Some("Announcements and anything that does not fit elsewhere".into()),
            "town-square" => Some("Company-wide chat. Be kind, stay on topic.".into()),
            _ => None,
        },
        members: 42,
        pinned: 3,
        favorite: id == "design",
        muted: id == "random",
        purpose: (id == "release-2026-11").then(|| {
            "Everything about shipping the November release: checklist, blockers, go/no-go.".into()
        }),
        loading,
        posts,
        first_unread,
        // A short limit on one channel shows the counter without a novel.
        max_post_size: if id == "help-desk" {
            200
        } else {
            DEFAULT_MAX_POST_SIZE
        },
        can_manage: true,
        profiles: profiles(),
        notifications: Default::default(),
        typing: match id {
            "general" => vec!["Priya Nair".into()],
            "design" => vec!["Sam Okafor".into(), "Lena Fischer".into()],
            _ => vec![],
        },
    })
}

/// #town-square's posts as the server sends them, through
/// `transcript::channel_model`: people joining around a few messages, a
/// header change, and a poll from a plugin Matty does not draw (#104).
fn activity(row: &ChannelRow) -> Vec<Post> {
    use matty_api::models::{Post as ApiPost, User};
    use std::collections::HashMap;
    let people = [
        ("priya", "Priya", "Nair"),
        ("sam", "Sam", "Okafor"),
        ("lena", "Lena", "Fischer"),
        ("jonas", "Jonas", "Weber"),
        ("mei", "Mei", "Tanaka"),
        ("tomas", "Tomás", "Silva"),
        ("ola", "Ola", "Berg"),
    ];
    let users: HashMap<String, User> = people
        .iter()
        .map(|(id, first, last)| {
            let user = User {
                id: (*id).into(),
                username: (*id).into(),
                first_name: (*first).into(),
                last_name: (*last).into(),
                ..User::default()
            };
            ((*id).to_string(), user)
        })
        .collect();
    // 2026-11-04, minutes after 08:00 UTC; "now" is noon that day.
    let at = |minute: i64| 1_793_779_200_000 + minute * 60_000;
    let post = |id: &str, user: &str, minute: i64, kind: &str, message: &str| ApiPost {
        id: id.into(),
        user_id: user.into(),
        create_at: at(minute),
        kind: kind.into(),
        message: message.into(),
        ..ApiPost::default()
    };
    let user = |id: &str| User {
        id: id.into(),
        ..User::default()
    };
    let reply = |post: ApiPost, root: &str| ApiPost {
        root_id: root.into(),
        ..post
    };
    let mut header = post("t6", "priya", 12, "system_header_change", "");
    header.props = [
        ("old_header", "Company-wide chat"),
        ("new_header", "Company-wide chat. Be kind, stay on topic."),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), serde_json::Value::from(value)))
    .collect();
    let posts = [
        ApiPost {
            reply_count: 1,
            last_reply_at: at(32),
            participants: vec![user("tomas")],
            ..post(
                "t1",
                "priya",
                0,
                "",
                "Welcome to town-square! Say hi when you get a minute.",
            )
        },
        post("t2", "sam", 1, "system_join_channel", ""),
        post("t2b", "lena", 2, "system_join_channel", ""),
        post("t2c", "jonas", 3, "system_join_channel", ""),
        post("t2d", "mei", 4, "system_join_channel", ""),
        post("t2e", "tomas", 5, "system_join_channel", ""),
        post("t2f", "ola", 6, "system_leave_channel", ""),
        post("t3", "sam", 10, "", "Hi all, Sam from the desktop team."),
        post("t4", "sam", 11, "", "Glad to be here."),
        header,
        // Replies (#123): with collapsed reply threads on they leave the
        // channel and their roots get the reply footer; with
        // `MATTY_PREVIEW=crt-off` they show inline.
        ApiPost {
            reply_count: 2,
            last_reply_at: at(21),
            participants: vec![user("jonas"), user("lena")],
            ..post("t7", "sam", 13, "", "Is lunch at 12 today?")
        },
        post(
            "t8",
            "lena",
            15,
            "custom_matterpoll",
            "Lunch: pizza or ramen?",
        ),
        reply(post("t9", "jonas", 20, "", "Ramen, obviously."), "t7"),
        reply(
            post("t10", "lena", 21, "", "Ramen it is. I'll book for six."),
            "t7",
        ),
        post("t11", "mei", 30, "", "Has anyone seen the new desk plan?"),
        reply(
            post(
                "t12",
                "tomas",
                32,
                "",
                "Hi Priya! Tomas from support, glad to be here.",
            ),
            "t1",
        ),
    ];
    let keys = matty_notify::MentionKeys::username("arya");
    let saved = std::collections::HashSet::new();
    let zone = jiff::tz::TimeZone::UTC;
    let viewer = crate::transcript::Viewer {
        user_id: "arya",
        keys: &keys,
        saved: &saved,
        zone: &zone,
        now: jiff::Timestamp::from_second(1_793_793_600).unwrap_or(jiff::Timestamp::UNIX_EPOCH),
        link_previews: true,
        collapsed_threads: value().as_deref() != Some("crt-off"),
        names: crate::settings::NameFormat::default(),
    };
    crate::transcript::channel_model(row, Some(&posts), &users, &viewer, None, &[]).posts
}

fn listed(channel_id: &str, post_id: &str) -> Option<ListedPost> {
    let channel = channel(channel_id)?;
    let post = channel
        .posts
        .iter()
        .find(|post| post.id.as_ref() == post_id)?;
    Some(ListedPost {
        channel_id: channel.id.clone(),
        channel_name: channel.name.clone(),
        post: post.clone(),
    })
}

/// Message search over the fixtures: every word must appear in the body,
/// `from:` matches the author and `in:` the channel, case-insensitively.
/// Newest first, as the server returns them.
pub fn search(query: &str) -> Vec<ListedPost> {
    let mut words = Vec::new();
    let mut from = None;
    let mut within = None;
    for token in query.split_whitespace() {
        let token = token.to_lowercase();
        if let Some(name) = token.strip_prefix("from:") {
            from = Some(name.trim_start_matches('@').to_string());
        } else if let Some(name) = token.strip_prefix("in:") {
            within = Some(name.trim_start_matches(['#', '~']).to_string());
        } else {
            words.push(
                token
                    .trim_matches(['"', '\u{201c}', '\u{201d}'])
                    .to_string(),
            );
        }
    }
    let ids: Vec<gpui_kit::SharedString> = sidebar()
        .categories
        .iter()
        .flat_map(|category| category.rows.iter().map(|row| row.id.clone()))
        .collect();
    let mut results = Vec::new();
    for id in ids {
        let Some(channel) = channel(&id) else {
            continue;
        };
        if within
            .as_ref()
            .is_some_and(|name| !channel.name.to_lowercase().contains(name))
        {
            continue;
        }
        for post in channel.posts.iter().rev() {
            let author = post.author.to_lowercase();
            if from.as_ref().is_some_and(|name| !author.contains(name)) {
                continue;
            }
            let body = post.body.to_lowercase();
            // The fixtures reuse posts across channels; list each once.
            let seen = results
                .iter()
                .any(|found: &ListedPost| found.post.id == post.id);
            if !seen && words.iter().all(|word| body.contains(word)) {
                results.push(ListedPost {
                    channel_id: channel.id.clone(),
                    channel_name: channel.name.clone(),
                    post: post.clone(),
                });
            }
        }
    }
    results
}

/// The channel's pinned posts, newest first.
pub fn pinned(channel: &ChannelModel) -> Vec<ListedPost> {
    channel
        .posts
        .iter()
        .rev()
        .filter(|post| post.pinned)
        .map(|post| ListedPost {
            channel_id: channel.id.clone(),
            channel_name: channel.name.clone(),
            post: post.clone(),
        })
        .collect()
}

/// The user's saved posts across channels, newest first.
pub fn saved() -> Vec<ListedPost> {
    [listed("general", "p5b"), listed("design", "p4")]
        .into_iter()
        .flatten()
        .collect()
}

/// Posts that mention the user, newest first.
pub fn mentions() -> Vec<ListedPost> {
    let mut lena = listed("design", "p4");
    if let Some(item) = lena.as_mut() {
        item.post.id = "p4m".into();
        item.post.time = hm("09:48");
        item.post.day = "Today".into();
        item.post.edited = false;
        item.post.pinned = false;
        item.post.mentions_me = true;
        item.post.body =
            "@arya the icon set is in; can you check the dark variants before I close #72?".into();
    }
    [lena, listed("general", "p6")]
        .into_iter()
        .flatten()
        .collect()
}

/// "08:02" as minutes since midnight, so fixtures read like a clock.
/// The fixtures' tooltip date for a day label, as if today were
/// Wednesday, November 4, 2026.
fn full_date(day: &str) -> SharedString {
    match day {
        "Today" => "Wednesday, November 4, 2026".into(),
        "Yesterday" => "Tuesday, November 3, 2026".into(),
        other => format!("{other}, 2026").into(),
    }
}

fn hm(time: &str) -> u16 {
    let (hour, minute) = time.split_once(':').expect("HH:MM");
    hour.parse::<u16>().expect("hour") * 60 + minute.parse::<u16>().expect("minute")
}

/// The signed-in user, for the account menu.
/// Posts that arrive in #general twelve seconds after it opens, to show
/// the live path and the "N new messages" pill.
pub fn incoming(channel_id: &str) -> Vec<Post> {
    if channel_id != "general" {
        return vec![];
    }
    let post = |id: &str, author: &str, time: &str, body: &str| Post {
        kind: crate::channel::PostKind::Regular,
        id: id.into(),
        author: author.into(),
        author_id: person(author),
        time: hm(time),
        day: "Today".into(),
        date: full_date("Today"),
        body: body.into(),
        reactions: vec![],
        mentions_me: false,
        edited: false,
        continues: false,
        mine: false,
        saved: false,
        pinned: false,
        status: PostStatus::Sent,
        replies: None,
        root_id: "".into(),
        reply_to: None,
        files: vec![],
        embeds: vec![],
    };
    vec![
        post("p10", "Jonas Weber", "08:41", "Found it, thanks!"),
        post(
            "p11",
            "Priya Nair",
            "08:42",
            "Merging the sidebar PR now; the title bar fix can follow in its own PR.",
        ),
    ]
}

/// What the composer tray shows after Attach: a thumbnail, a file still
/// uploading and one that failed.
/// The team's public channels for Browse: the sidebar's plus five more.
pub fn directory() -> Vec<DirectoryChannel> {
    let joined: Vec<SharedString> = sidebar()
        .categories
        .into_iter()
        .flat_map(|category| category.rows)
        .filter(|row| row.kind == ChannelKind::Public)
        .map(|row| row.id)
        .collect();
    let entry = |id: &str, name: &str, purpose: Option<&str>, members: usize| DirectoryChannel {
        id: id.into(),
        name: name.into(),
        purpose: purpose.map(Into::into),
        members,
        joined: joined.iter().any(|j| j.as_ref() == id),
    };
    vec![
        entry(
            "announcements",
            "announcements",
            Some("Company-wide news; read-only for most people."),
            120,
        ),
        entry(
            "design",
            "design",
            Some("Mockups, reviews and the design system."),
            18,
        ),
        entry(
            "dev-rust",
            "dev-rust",
            Some("Rust questions, crate picks and clippy fights."),
            38,
        ),
        entry(
            "general",
            "general",
            Some("Announcements and anything that does not fit elsewhere"),
            42,
        ),
        entry("gpui", "gpui", None, 11),
        entry(
            "help-desk",
            "help-desk",
            Some("Ask IT about laptops, accounts and access."),
            42,
        ),
        entry(
            "hiring",
            "hiring",
            Some("Open roles and interview loops."),
            12,
        ),
        entry(
            "off-topic",
            "off-topic",
            Some("Pets, food, weekend plans."),
            77,
        ),
        entry("random", "random", None, 40),
        entry("town-square", "town-square", None, 42),
        entry(
            "ux-research",
            "ux-research",
            Some("Interview notes and study plans."),
            9,
        ),
    ]
}

/// An empty channel for one just joined from Browse.
pub fn joined_channel(channel: &DirectoryChannel) -> ChannelModel {
    let mut model = empty_channel(
        channel.id.clone(),
        channel.name.clone(),
        ChannelKind::Public,
        channel.purpose.clone(),
    );
    model.members = channel.members;
    model
}

/// A channel with nothing in it yet: just created, or a new direct message.
pub fn empty_channel(
    id: SharedString,
    name: SharedString,
    kind: ChannelKind,
    purpose: Option<SharedString>,
) -> ChannelModel {
    let members = match kind {
        ChannelKind::Direct { .. } => 2,
        ChannelKind::Group { members } => members,
        _ => 1,
    };
    ChannelModel {
        offline: false,
        id,
        name,
        kind,
        header: None,
        members,
        pinned: 0,
        favorite: false,
        muted: false,
        purpose,
        loading: false,
        posts: vec![],
        first_unread: None,
        max_post_size: DEFAULT_MAX_POST_SIZE,
        can_manage: true,
        profiles: profiles(),
        notifications: Default::default(),
        typing: vec![],
    }
}

pub fn uploads() -> Vec<Upload> {
    let mut mockup = Upload::from_image(
        "u-mockup",
        attachments::png(include_bytes!("../assets/preview/mockup.png")),
    )
    .ready(48_312);
    mockup.name = "sidebar-mockup.png".into();
    vec![
        mockup,
        Upload {
            id: "u-notes".into(),
            name: "release-notes.pdf".into(),
            size: 2_516_582,
            source: UploadSource::Path("release-notes.pdf".into()),
            status: AttachmentStatus::Uploading(0.6),
        },
        Upload {
            id: "u-log".into(),
            name: "matty-2026-11-03.log".into(),
            size: 918_000,
            source: UploadSource::Path("matty-2026-11-03.log".into()),
            status: AttachmentStatus::Failed,
        },
    ]
}

/// A screen recording over the default 100 MB limit, which the tray
/// refuses when the fixtures go in.
pub fn oversize_upload() -> Upload {
    Upload {
        id: "u-recording".into(),
        name: "screen-recording.mov".into(),
        size: 251_658_240,
        source: UploadSource::Path("screen-recording.mov".into()),
        status: AttachmentStatus::Uploading(0.),
    }
}

/// The preview user's notification preferences, with two keywords.
/// A link preview for a draft's link: GitHub links have one, others none.
pub fn link_card(url: &str) -> Option<crate::embeds::LinkCard> {
    url.starts_with("https://github.com/")
        .then(|| crate::embeds::LinkCard {
            url: url.to_owned().into(),
            site_name: "GitHub".into(),
            title: "devaryakjha/matty: A native Mattermost client".into(),
            description: "Fast, keyboard-first and offline-friendly, built with GPUI.".into(),
            image: None,
        })
}

pub fn notify_props() -> std::collections::HashMap<String, String> {
    [
        ("desktop", "mention"),
        ("mention_keys", "arya,@arya,release,design review"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect()
}

pub fn account() -> Account {
    Account {
        user_id: "u-arya".into(),
        display_name: "Arya".into(),
        username: "arya".into(),
        presence: Presence::Online,
        status_text: None,
        dnd_end: None,
    }
}

/// A fixture person's user id from their display name: `u-` and the first
/// name, as in [`profiles`]. Bots and strangers get none.
fn person(name: &str) -> SharedString {
    let first = name.split(' ').next().unwrap_or_default().to_lowercase();
    if profiles()
        .iter()
        .any(|profile| profile.username.as_ref() == first)
    {
        format!("u-{first}").into()
    } else {
        SharedString::default()
    }
}

/// Profile pictures for the fixture people. Jonas has none, so his
/// avatar shows the initials as it does when a picture fails to load.
pub fn avatars() -> crate::avatars::Avatars {
    let avatars = crate::avatars::Avatars::default();
    let pictures: [(&str, &[u8]); 4] = [
        (
            "u-priya",
            include_bytes!("../assets/preview/avatar-priya.png"),
        ),
        ("u-sam", include_bytes!("../assets/preview/avatar-sam.png")),
        (
            "u-lena",
            include_bytes!("../assets/preview/avatar-lena.png"),
        ),
        (
            "u-arya",
            include_bytes!("../assets/preview/avatar-arya.png"),
        ),
    ];
    for (user_id, bytes) in pictures {
        if let Some(picture) = crate::avatars::picture(bytes, "image/png") {
            avatars.insert(user_id, picture);
        }
    }
    avatars
}

/// Everyone who posts in the fixtures, for the profile card.
pub fn profiles() -> Vec<UserProfile> {
    let profile = |username: &str,
                   display_name: &str,
                   position: Option<&str>,
                   presence,
                   status_text: Option<&str>,
                   time_zone: Option<&str>| UserProfile {
        user_id: format!("u-{username}").into(),
        username: username.into(),
        display_name: display_name.into(),
        position: position.map(Into::into),
        presence,
        status_text: status_text.map(Into::into),
        time_zone: time_zone.map(Into::into),
    };
    vec![
        profile(
            "priya",
            "Priya Nair",
            Some("Product designer"),
            Presence::Online,
            Some("In a review until 3"),
            Some("Asia/Kolkata"),
        ),
        profile(
            "sam",
            "Sam Okafor",
            Some("Release manager"),
            Presence::Away,
            None,
            Some("Africa/Lagos"),
        ),
        profile(
            "lena",
            "Lena Fischer",
            Some("Engineer, Windows"),
            Presence::Dnd,
            Some("Heads down"),
            Some("Europe/Berlin"),
        ),
        profile(
            "jonas",
            "Jonas Weber",
            Some("Support"),
            Presence::Offline,
            None,
            None,
        ),
        profile(
            "arya",
            "Arya",
            Some("Maintainer"),
            Presence::Online,
            None,
            None,
        ),
    ]
}

/// The user's unsent messages, for the Drafts view.
pub fn drafts() -> DraftsModel {
    DraftsModel {
        drafts: vec![
            Draft {
                channel_id: "design".into(),
                channel_name: "design".into(),
                thread_root: None,
                body: "Proposal for the empty states: one icon in a tinted circle, a `title` line and **one** sentence, never a paragraph. Screens in the PR.".into(),
                updated: "Today 10:12".into(),
            },
            Draft {
                channel_id: "general".into(),
                channel_name: "general".into(),
                thread_root: Some("p5".into()),
                body: "Also added it to the README so nobody has to ask again.".into(),
                updated: "Yesterday 17:03".into(),
            },
        ],
    }
}

/// Puts the fixture drafts in the composers' book, so their channels show
/// the sidebar's pencil and their composers open with the text.
pub fn seed_drafts(cx: &mut gpui_kit::App) {
    use crate::composer_draft::{DraftBook, DraftKey, Kept};
    for draft in drafts().drafts {
        let key = match draft.thread_root {
            Some(root) => DraftKey::thread(draft.channel_id, root),
            None => DraftKey::channel(draft.channel_id),
        };
        let kept = Kept {
            text: draft.body,
            ..Kept::default()
        };
        DraftBook::put(key, kept, cx);
    }
}

/// The threads the user follows, for the Threads view.
pub fn threads() -> ThreadsModel {
    let row = |channel_id: &str, post_id: &str, summary: ReplySummary| {
        thread(channel_id, post_id).map(|thread| ThreadRow {
            channel_id: channel_id.into(),
            thread,
            summary,
        })
    };
    let summary = |count, last: &str, participants: &[&str], unread| ReplySummary {
        count,
        last_time: if last.contains(':') { hm(last) } else { 0 },
        last_day: if last.contains(':') {
            "Today".into()
        } else {
            last.into()
        },
        participants: participants.iter().map(|name| (*name).into()).collect(),
        participant_ids: participants.iter().map(|name| person(name)).collect(),
        following: true,
        unread,
    };
    ThreadsModel {
        threads: [
            row(
                "general",
                "p2",
                summary(1, "10:02", &["Lena Fischer"], true),
            ),
            row(
                "design",
                "p6",
                summary(2, "09:48", &["Priya Nair", "Sam Okafor"], true),
            ),
            row(
                "general",
                "p5",
                summary(12, "08:13", &["Arya", "Jonas Weber"], false),
            ),
            row(
                "help-desk",
                "p8",
                summary(3, "Yesterday", &["Arya", "Sam Okafor"], false),
            ),
        ]
        .into_iter()
        .flatten()
        .collect(),
        loading: false,
        // The list's last row shows the next page on its way (#122).
        more: true,
        failed: false,
    }
}

/// The thread under `post_id` in `channel_id`: the post as root plus three
/// replies, the last two from the same person.
pub fn thread(channel_id: &str, post_id: &str) -> Option<ThreadModel> {
    let channel = channel(channel_id)?;
    let root = channel
        .posts
        .iter()
        .find(|post| post.id.as_ref() == post_id)?
        .clone();
    let reply = |id: &str, author: &str, time: &str, body: &str, continues: bool| Post {
        kind: crate::channel::PostKind::Regular,
        id: format!("{}-{id}", root.id).into(),
        author: author.into(),
        author_id: person(author),
        time: hm(time),
        day: root.day.clone(),
        date: root.date.clone(),
        body: body.into(),
        reactions: vec![],
        mentions_me: false,
        edited: false,
        continues,
        mine: author == "Arya",
        saved: false,
        pinned: false,
        status: PostStatus::Sent,
        replies: None,
        root_id: "".into(),
        reply_to: None,
        files: vec![],
        embeds: vec![],
    };
    Some(ThreadModel {
        channel_id: channel.id.clone(),
        channel_name: channel.name.clone(),
        replies: vec![
            reply(
                "r1",
                "Jonas Weber",
                "08:09",
                "Reproduced on Windows 11 at 480px. The switcher pill keeps its minimum width and pushes the controls.",
                false,
            ),
            reply(
                "r2",
                "Arya",
                "08:12",
                "Thanks. The pill should give way first; I'll cap it at the remaining width and let it shrink to the icon.",
                false,
            ),
            // Offline, Arya's last reply waits to send under the one
            // before it (#195).
            Post {
                status: if queued() > 0 {
                    PostStatus::Pending
                } else {
                    PostStatus::Sent
                },
                ..reply("r3", "Arya", "08:13", "Tracking it in #79.", true)
            },
        ],
        root: Post {
            continues: false,
            ..root
        },
        following: true,
        // #5 is a long thread whose older replies are still on their way
        // (#122).
        reply_count: if post_id == "p5" { 12 } else { 3 },
        older: post_id == "p5",
        failed_older: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every fixture builds; a bad literal would otherwise only show up
    /// when someone runs the preview.
    #[test]
    fn fixtures_build() {
        let sidebar = sidebar();
        let ids: Vec<_> = sidebar
            .categories
            .iter()
            .flat_map(|category| category.rows.iter().map(|row| row.id.clone()))
            .collect();
        assert!(!ids.is_empty());
        assert_eq!(uploads().len(), 3);
        let directory = directory();
        assert_eq!(directory.iter().filter(|c| !c.joined).count(), 5);
        for entry in directory.iter().filter(|c| c.joined) {
            assert!(channel(&entry.id).is_some(), "{} has a fixture", entry.id);
        }
        joined_channel(&directory[0]);
        assert_eq!(incoming("general").len(), 2);
        assert!(incoming("design").is_empty());
        for id in &ids {
            if let Some(channel) = channel(id) {
                pinned(&channel);
                for post in &channel.posts {
                    thread(id, &post.id);
                }
            }
        }
        assert!(!saved().is_empty());
        assert!(!mentions().is_empty());
        assert!(!profiles().is_empty());
        assert!(!drafts().drafts.is_empty());
        assert!(!threads().threads.is_empty());
    }

    #[test]
    fn search_filters_by_words_author_and_channel() {
        assert!(!search("title bar").is_empty());
        assert!(
            search("title bar from:priya")
                .iter()
                .all(|r| r.post.author == "Priya Nair")
        );
        assert!(
            search("in:general")
                .iter()
                .all(|r| r.channel_id == "general")
        );
        assert!(search("no such words anywhere").is_empty());
    }
}
