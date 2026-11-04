//! Turns a channel's posts snapshot from sync into the channel view's
//! model: author names, local times and day labels, grouping, reactions.

use std::collections::HashMap;

use gpui_kit::SharedString;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use matty_api::models::{Post as ApiPost, User};
use matty_notify::MentionKeys;

use crate::attachments::PostFile;
use crate::channel::{ChannelModel, DEFAULT_MAX_POST_SIZE, Post, PostReaction, PostStatus};
use crate::sidebar::ChannelRow;

/// Posts by one person within this many milliseconds share a header.
const GROUP_MS: i64 = 5 * 60 * 1000;

/// Who is looking, and where.
pub struct Viewer<'a> {
    pub user_id: &'a str,
    /// What counts as mentioning the viewer, for the mention highlight.
    pub keys: &'a MentionKeys,
    pub zone: &'a TimeZone,
    pub now: Timestamp,
}

/// The name Mattermost shows by default: nickname, then full name, then
/// username.
pub fn display_name(user: &User) -> String {
    let full = format!("{} {}", user.first_name, user.last_name);
    [user.nickname.trim(), full.trim()]
        .into_iter()
        .find(|name| !name.is_empty())
        .unwrap_or(&user.username)
        .to_string()
}

/// The channel view's model for `row`, from sync's posts (oldest first).
pub fn channel_model(
    row: &ChannelRow,
    posts: Option<&[ApiPost]>,
    users: &HashMap<String, User>,
    viewer: &Viewer<'_>,
    max_post_size: Option<usize>,
) -> ChannelModel {
    let today = viewer.now.to_zoned(viewer.zone.clone()).date();
    let mut rows: Vec<Post> = Vec::new();
    let mut previous: Option<(&str, i64, SharedString)> = None;
    let mention = viewer.keys;
    for post in posts.unwrap_or_default() {
        if post.delete_at != 0 || !post.kind.is_empty() && post.kind.starts_with("system_") {
            continue;
        }
        let stamp = when(post.create_at, viewer.zone, today);
        let day = stamp.1.clone();
        let continues = previous.as_ref().is_some_and(|(user, at, last_day)| {
            *user == post.user_id && post.create_at - at < GROUP_MS && *last_day == day
        });
        previous = Some((&post.user_id, post.create_at, day.clone()));
        rows.push(display_post(post, users, viewer, mention, stamp, continues));
    }
    ChannelModel {
        id: row.id.clone(),
        name: row.name.clone(),
        kind: row.kind.clone(),
        header: None,
        members: 0,
        pinned: rows.iter().filter(|post| post.pinned).count(),
        favorite: false,
        muted: row.muted,
        purpose: None,
        loading: posts.is_none(),
        posts: rows,
        first_unread: None,
        max_post_size: max_post_size.unwrap_or(DEFAULT_MAX_POST_SIZE),
        profiles: Vec::new(),
        notifications: Default::default(),
        typing: Vec::new(),
    }
}

/// One post as the views draw it.
fn display_post(
    post: &ApiPost,
    users: &HashMap<String, User>,
    viewer: &Viewer<'_>,
    mention: &MentionKeys,
    (time, day, date): (u16, SharedString, SharedString),
    continues: bool,
) -> Post {
    let author = users
        .get(&post.user_id)
        .map(display_name)
        .unwrap_or_else(|| "Someone".into());
    Post {
        id: post.id.clone().into(),
        author: author.into(),
        time,
        day,
        date,
        body: post.message.clone().into(),
        reactions: reactions(post, viewer.user_id, users),
        mentions_me: mention.mentioned(&post.message),
        edited: post.edit_at != 0,
        continues,
        mine: post.user_id == viewer.user_id,
        saved: false,
        pinned: post.is_pinned,
        // Sync's optimistic copy of a send keeps the client's id until
        // the server's post replaces it.
        status: if !post.pending_post_id.is_empty() && post.id == post.pending_post_id {
            PostStatus::Pending
        } else {
            PostStatus::Sent
        },
        replies: None,
        files: post
            .metadata
            .files
            .iter()
            .filter(|file| file.delete_at == 0)
            .map(|file| PostFile {
                id: file.id.clone().into(),
                name: file.name.clone().into(),
                size: u64::try_from(file.size).unwrap_or(0),
                preview: None,
                width: u32::try_from(file.width).unwrap_or(0),
                height: u32::try_from(file.height).unwrap_or(0),
            })
            .collect(),
        embeds: crate::embeds::from_post(post),
    }
}

/// A post shown on its own, as in search results: never a continuation.
pub fn listed_post(post: &ApiPost, users: &HashMap<String, User>, viewer: &Viewer<'_>) -> Post {
    let today = viewer.now.to_zoned(viewer.zone.clone()).date();
    let mention = viewer.keys;
    display_post(
        post,
        users,
        viewer,
        mention,
        when(post.create_at, viewer.zone, today),
        false,
    )
}

/// Minutes since local midnight, the day label the post falls under, and
/// the full date for the time's tooltip.
fn when(
    millis: i64,
    zone: &TimeZone,
    today: jiff::civil::Date,
) -> (u16, SharedString, SharedString) {
    let Ok(at) = Timestamp::from_millisecond(millis) else {
        return (0, "Today".into(), SharedString::default());
    };
    let local = at.to_zoned(zone.clone());
    let minutes =
        u16::try_from(i32::from(local.hour()) * 60 + i32::from(local.minute())).unwrap_or(0);
    let date = local.date();
    let day = if date == today {
        "Today".to_string()
    } else if today.yesterday().ok() == Some(date) {
        "Yesterday".to_string()
    } else if date.year() == today.year() {
        date.strftime("%A, %B %-d").to_string()
    } else {
        date.strftime("%B %-d, %Y").to_string()
    };
    let full = date.strftime("%A, %B %-d, %Y").to_string();
    (minutes, day.into(), full.into())
}

/// Reaction chips in the order each emoji was first used.
fn reactions(post: &ApiPost, me: &str, users: &HashMap<String, User>) -> Vec<PostReaction> {
    let mut chips: Vec<(String, PostReaction)> = Vec::new();
    for reaction in &post.metadata.reactions {
        // Others by name for the tooltip; people not loaded yet are left
        // out, as the field allows.
        let name = (reaction.user_id != me)
            .then(|| users.get(&reaction.user_id))
            .flatten()
            .map(|user| SharedString::from(display_name(user)));
        match chips
            .iter_mut()
            .find(|(name, _)| *name == reaction.emoji_name)
        {
            Some((_, chip)) => {
                chip.count += 1;
                chip.mine |= reaction.user_id == me;
                chip.users.extend(name);
            }
            None => chips.push((
                reaction.emoji_name.clone(),
                PostReaction {
                    emoji: emoji(&reaction.emoji_name).into(),
                    count: 1,
                    mine: reaction.user_id == me,
                    users: name.into_iter().collect(),
                },
            )),
        }
    }
    chips.into_iter().map(|(_, chip)| chip).collect()
}

/// The Mattermost name for what a reaction chip or the picker gives: a
/// glyph becomes its shortcode, `:name:` loses its colons.
pub fn emoji_name(emoji: &str) -> String {
    emojis::get(emoji)
        .and_then(|emoji| emoji.shortcode())
        .map(str::to_string)
        .unwrap_or_else(|| emoji.trim_matches(':').to_string())
}

/// The glyph for a Mattermost emoji name, or `:name:` for custom emoji.
fn emoji(name: &str) -> String {
    emojis::get_by_shortcode(name)
        .map(|emoji| emoji.as_str().to_string())
        .unwrap_or_else(|| format!(":{name}:"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar::ChannelKind;
    use matty_api::models::Reaction;

    fn post(id: &str, user: &str, at: i64, message: &str) -> ApiPost {
        ApiPost {
            id: id.into(),
            user_id: user.into(),
            create_at: at,
            message: message.into(),
            ..ApiPost::default()
        }
    }

    static KEYS: std::sync::LazyLock<MentionKeys> =
        std::sync::LazyLock::new(|| MentionKeys::username("arya"));

    fn viewer(zone: &TimeZone) -> Viewer<'_> {
        Viewer {
            user_id: "me",
            keys: &KEYS,
            zone,
            // 2026-11-04 12:00 UTC.
            now: Timestamp::from_second(1_793_793_600).unwrap(),
        }
    }

    const NOON: i64 = 1_793_793_600_000;

    #[test]
    fn posts_get_names_times_days_and_groups() {
        let zone = TimeZone::UTC;
        let users = HashMap::from([(
            "ada".to_string(),
            User {
                id: "ada".into(),
                username: "ada".into(),
                first_name: "Ada".into(),
                last_name: "Lovelace".into(),
                ..User::default()
            },
        )]);
        let posts = [
            post("p1", "ada", NOON - 86_400_000, "yesterday"),
            post("p2", "ada", NOON - 60_000, "one"),
            post("p3", "ada", NOON, "two, hi @arya"),
            post("p4", "me", NOON + 60_000, "mine"),
            post("p5", "ghost", NOON + 120_000, "who"),
        ];
        let row = ChannelRow::new("c1", "town", ChannelKind::Public);
        let model = channel_model(&row, Some(&posts), &users, &viewer(&zone), Some(4000));
        let shown: Vec<_> = model
            .posts
            .iter()
            .map(|p| (p.author.as_ref(), p.day.as_ref(), p.continues))
            .collect();
        assert_eq!(
            shown,
            [
                ("Ada Lovelace", "Yesterday", false),
                ("Ada Lovelace", "Today", false),
                ("Ada Lovelace", "Today", true),
                ("Someone", "Today", false),
                ("Someone", "Today", false),
            ]
        );
        assert_eq!(model.posts[2].time, 12 * 60);
        // The time's tooltip names the full date, even under "Today".
        assert_eq!(model.posts[2].date, "Wednesday, November 4, 2026");
        assert_eq!(model.posts[0].date, "Tuesday, November 3, 2026");
        assert!(model.posts[2].mentions_me && !model.posts[1].mentions_me);
        assert!(model.posts[3].mine);
        assert_eq!(model.posts[3].status, PostStatus::Sent);
        assert_eq!(model.max_post_size, 4000);
        assert!(!model.loading);
    }

    #[test]
    fn an_unsent_post_is_pending() {
        let zone = TimeZone::UTC;
        let mut sending = post("me:1", "me", NOON, "on its way");
        sending.pending_post_id = "me:1".into();
        let mut sent = post("p9", "me", NOON, "arrived");
        sent.pending_post_id = "me:0".into();
        let row = ChannelRow::new("c1", "town", ChannelKind::Public);
        let posts = [sent, sending];
        let model = channel_model(&row, Some(&posts), &HashMap::new(), &viewer(&zone), None);
        assert_eq!(model.posts[0].status, PostStatus::Sent);
        assert_eq!(model.posts[1].status, PostStatus::Pending);
    }

    #[test]
    fn a_post_lists_its_files() {
        let zone = TimeZone::UTC;
        let mut p = post("p1", "ada", NOON, "the notes");
        for (id, deleted) in [("f1", 0), ("f2", 5)] {
            p.metadata.files.push(matty_api::models::FileInfo {
                id: id.into(),
                name: "notes.pdf".into(),
                size: 2048,
                width: 640,
                height: 400,
                delete_at: deleted,
                ..Default::default()
            });
        }
        let row = ChannelRow::new("c1", "town", ChannelKind::Public);
        let model = channel_model(&row, Some(&[p]), &HashMap::new(), &viewer(&zone), None);
        assert_eq!(
            model.posts[0].files,
            vec![PostFile {
                id: "f1".into(),
                name: "notes.pdf".into(),
                size: 2048,
                preview: None,
                width: 640,
                height: 400,
            }]
        );
    }

    #[test]
    fn no_snapshot_yet_is_loading() {
        let zone = TimeZone::UTC;
        let row = ChannelRow::new("c1", "town", ChannelKind::Public);
        let model = channel_model(&row, None, &HashMap::new(), &viewer(&zone), None);
        assert!(model.loading && model.posts.is_empty());
    }

    #[test]
    fn reactions_group_by_emoji_and_know_mine() {
        let mut p = post("p1", "ada", NOON, "x");
        for (user, emoji) in [("ada", "+1"), ("me", "+1"), ("ada", "partyparrot")] {
            p.metadata.reactions.push(Reaction {
                user_id: user.into(),
                post_id: "p1".into(),
                emoji_name: emoji.into(),
                ..Reaction::default()
            });
        }
        let users = HashMap::from([(
            "ada".to_string(),
            User {
                id: "ada".into(),
                username: "ada".into(),
                ..User::default()
            },
        )]);
        let chips = reactions(&p, "me", &users);
        assert_eq!(chips.len(), 2);
        assert_eq!(
            (chips[0].emoji.as_ref(), chips[0].count, chips[0].mine),
            ("👍", 2, true)
        );
        assert_eq!(chips[0].users, ["ada"]);
        assert_eq!(chips[1].emoji.as_ref(), ":partyparrot:");
    }

    #[test]
    fn reactions_go_back_to_the_server_by_name() {
        assert_eq!(emoji_name("👍"), "+1");
        assert_eq!(emoji_name(":partyparrot:"), "partyparrot");
        assert_eq!(emoji_name("tada"), "tada");
    }
}
