//! Turns a channel's posts snapshot from sync into the channel view's
//! model: author names, local times and day labels, grouping, reactions,
//! and the wording of system messages (#104).

use std::collections::HashMap;

use gpui_kit::SharedString;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use matty_api::models::{Post as ApiPost, User};
use matty_notify::MentionKeys;

use crate::attachments::PostFile;
use crate::channel::{
    ChannelModel, DEFAULT_MAX_POST_SIZE, Post, PostKind, PostReaction, PostStatus,
};
use crate::settings::NameFormat;
use crate::sidebar::ChannelRow;

/// Posts by one person within this many milliseconds share a header.
const GROUP_MS: i64 = 5 * 60 * 1000;

/// Who is looking, and where.
pub struct Viewer<'a> {
    pub user_id: &'a str,
    /// What counts as mentioning the viewer, for the mention highlight.
    pub keys: &'a MentionKeys,
    /// The ids of the posts the viewer saved.
    pub saved: &'a std::collections::HashSet<String>,
    pub zone: &'a TimeZone,
    pub now: Timestamp,
    /// Whether link previews show: the server allows them and the user
    /// has not turned them off (#187). Message attachments always show.
    pub link_previews: bool,
    /// Collapsed reply threads (#123): on, a root with replies gets the
    /// reply footer and replies stay in their thread; off, replies show in
    /// the channel under a "Commented on" line.
    pub collapsed_threads: bool,
    /// How people's names read (#156).
    pub names: NameFormat,
}

/// The name Matty shows when the user has not chosen a format: nickname,
/// then full name, then username.
pub fn display_name(user: &User) -> String {
    display_name_as(user, NameFormat::default())
}

/// `user`'s name in `format`, as the web app's "Teammate name display":
/// a name that is not set falls back to the full name, then the username.
pub fn display_name_as(user: &User, format: NameFormat) -> String {
    let full = format!("{} {}", user.first_name.trim(), user.last_name.trim());
    let nickname = match format {
        NameFormat::NicknameFullName => user.nickname.trim(),
        NameFormat::Username | NameFormat::FullName => "",
    };
    let full = match format {
        NameFormat::Username => "",
        NameFormat::NicknameFullName | NameFormat::FullName => full.trim(),
    };
    [nickname, full]
        .into_iter()
        .find(|name| !name.is_empty())
        .unwrap_or(&user.username)
        .to_string()
}

/// The channel view's model for `row`, from sync's posts (oldest first).
/// `roots` are cached roots of replies in `posts` that `posts` leaves out,
/// for the line above a reply when collapsed reply threads are off.
pub fn channel_model(
    row: &ChannelRow,
    posts: Option<&[ApiPost]>,
    users: &HashMap<String, User>,
    viewer: &Viewer<'_>,
    max_post_size: Option<usize>,
    roots: &[ApiPost],
) -> ChannelModel {
    let today = viewer.now.to_zoned(viewer.zone.clone()).date();
    let mut rows: Vec<Post> = Vec::new();
    let mut previous: Option<(&str, i64, SharedString)> = None;
    // Joins and leaves since the last other row, combined into one row.
    let mut activity: Vec<Activity> = Vec::new();
    let mention = viewer.keys;
    let known: HashMap<&str, &ApiPost> = posts
        .unwrap_or_default()
        .iter()
        .chain(roots)
        .map(|post| (post.id.as_str(), post))
        .collect();
    // The thread of the last message row, so a run of one thread's posts
    // names the root only once.
    let mut thread: Option<&str> = None;
    for post in posts.unwrap_or_default() {
        if post.delete_at != 0 {
            continue;
        }
        // Collapsed threads keep replies in their thread; sync leaves
        // them out, and any that slip through stay out too.
        if viewer.collapsed_threads && post.is_reply() {
            continue;
        }
        let stamp = when(post.create_at, viewer.zone, today);
        let day = stamp.1.clone();
        if let Some(entry) = Activity::of(post, users, viewer) {
            let last = rows.last_mut().filter(|last| {
                last.kind == (PostKind::System { activity: true })
                    && last.day == day
                    && !activity.is_empty()
            });
            match last {
                Some(last) => {
                    activity.push(entry);
                    last.body = activity_text(&activity).into();
                }
                None => {
                    activity = vec![entry];
                    rows.push(system_post(post, activity_text(&activity), true, stamp));
                }
            }
            previous = None;
            thread = None;
            continue;
        }
        activity.clear();
        if is_system(&post.kind) {
            // A system row breaks the run of one author's posts.
            previous = None;
            thread = None;
            if let Some(text) = system_text(post, users, viewer) {
                rows.push(system_post(post, text, false, stamp));
            }
            continue;
        }
        let root_id = if post.is_reply() {
            post.root_id.as_str()
        } else {
            post.id.as_str()
        };
        let reply_to = (post.is_reply() && thread != Some(root_id))
            .then(|| {
                known
                    .get(root_id)
                    .map(|root| reply_context(root, users, viewer))
            })
            .flatten();
        thread = Some(root_id);
        // A reply under its own "Commented on" line starts a new run.
        let continues = reply_to.is_none()
            && previous.as_ref().is_some_and(|(user, at, last_day)| {
                *user == post.user_id && post.create_at - at < GROUP_MS && *last_day == day
            });
        previous = Some((&post.user_id, post.create_at, day.clone()));
        let mut shown = display_post(post, users, viewer, mention, stamp, continues);
        shown.reply_to = reply_to;
        if viewer.collapsed_threads && post.reply_count > 0 {
            shown.replies = Some(reply_summary(post, users, viewer, today));
        }
        rows.push(shown);
    }
    ChannelModel {
        offline: false,
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
        can_manage: false,
        profiles: Vec::new(),
        notifications: Default::default(),
        typing: Vec::new(),
    }
}

/// What a reply answers: its root's author and words.
fn reply_context(
    root: &ApiPost,
    users: &HashMap<String, User>,
    viewer: &Viewer<'_>,
) -> crate::channel::ReplyContext {
    let author = users
        .get(&root.user_id)
        .map(|user| display_name_as(user, viewer.names))
        .unwrap_or_else(|| "Someone".into());
    let words = crate::channel::plain_text(&root.message);
    let excerpt = words.split_whitespace().collect::<Vec<_>>().join(" ");
    crate::channel::ReplyContext {
        author: author.into(),
        excerpt: excerpt.into(),
    }
}

/// The reply footer under a root while collapsed reply threads are on
/// (#123), from what the server and the cache know of its thread.
fn reply_summary(
    root: &ApiPost,
    users: &HashMap<String, User>,
    viewer: &Viewer<'_>,
    today: jiff::civil::Date,
) -> crate::channel::ReplySummary {
    let (last_time, last_day, _) = when(root.last_reply_at.max(root.create_at), viewer.zone, today);
    // The server lists who replied oldest first; the footer shows the
    // newest three.
    let people: Vec<&User> = root
        .participants
        .iter()
        .rev()
        .filter_map(|user| users.get(&user.id))
        .take(3)
        .collect();
    crate::channel::ReplySummary {
        count: usize::try_from(root.reply_count).unwrap_or(0),
        last_time,
        last_day,
        participants: people
            .iter()
            .map(|user| display_name_as(user, viewer.names).into())
            .collect(),
        participant_ids: people.iter().map(|user| user.id.clone().into()).collect(),
        following: root.is_following.unwrap_or(false),
        unread: false,
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
        .map(|user| display_name_as(user, viewer.names))
        .unwrap_or_else(|| "Someone".into());
    let kind = if is_regular(&post.kind) {
        PostKind::Regular
    } else if is_system(&post.kind) {
        PostKind::System { activity: false }
    } else {
        PostKind::Unsupported(post.kind.clone().into())
    };
    let body = match kind {
        PostKind::System { .. } => system_text(post, users, viewer)
            .unwrap_or_else(|| post.message.clone())
            .into(),
        _ => post.message.clone().into(),
    };
    Post {
        id: post.id.clone().into(),
        author: author.into(),
        author_id: post.user_id.clone().into(),
        kind,
        time,
        day,
        date,
        body,
        reactions: reactions(post, viewer.user_id, users, viewer.names),
        mentions_me: mention.mentioned(&post.message),
        edited: post.edit_at != 0,
        continues,
        mine: post.user_id == viewer.user_id,
        saved: viewer.saved.contains(&post.id),
        pinned: post.is_pinned,
        // Sync's optimistic copy of a send keeps the client's id until
        // the server's post replaces it.
        status: if !post.pending_post_id.is_empty() && post.id == post.pending_post_id {
            PostStatus::Pending
        } else {
            PostStatus::Sent
        },
        replies: None,
        root_id: post.root_id.clone().into(),
        reply_to: None,
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
        embeds: crate::embeds::from_post(post)
            .into_iter()
            .filter(|embed| viewer.link_previews || !matches!(embed, crate::embeds::Embed::Link(_)))
            .collect(),
    }
}

/// Post types drawn as ordinary messages: a normal post, a webhook's
/// attachments, and `/me`.
fn is_regular(kind: &str) -> bool {
    matches!(kind, "" | "slack_attachment" | "me")
}

fn is_system(kind: &str) -> bool {
    kind.starts_with("system_")
}

/// A system row: no author header, no reactions or actions, never the
/// viewer's own (so Up never edits it).
fn system_post(
    post: &ApiPost,
    text: String,
    activity: bool,
    (time, day, date): (u16, SharedString, SharedString),
) -> Post {
    Post {
        id: post.id.clone().into(),
        author: SharedString::default(),
        author_id: SharedString::default(),
        kind: PostKind::System { activity },
        time,
        day,
        date,
        body: text.into(),
        reactions: Vec::new(),
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
        files: Vec::new(),
        embeds: Vec::new(),
    }
}

/// A string prop, or empty.
fn prop<'a>(post: &'a ApiPost, key: &str) -> &'a str {
    post.props
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
}

/// How a system message names someone: "You" for the viewer, then their
/// display name, then the username the post carries, then "Someone".
fn person(id: &str, username: &str, users: &HashMap<String, User>, viewer: &Viewer<'_>) -> String {
    if !id.is_empty() && id == viewer.user_id {
        return "You".into();
    }
    match users.get(id) {
        Some(user) => display_name_as(user, viewer.names),
        None if !username.is_empty() => format!("@{username}"),
        None => "Someone".into(),
    }
}

/// What a join, leave, add or remove did, worded as the web app does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verb {
    JoinChannel,
    JoinChannelAsGuest,
    LeaveChannel,
    AddToChannel,
    AddGuestToChannel,
    RemoveFromChannel,
    JoinTeam,
    LeaveTeam,
    AddToTeam,
    RemoveFromTeam,
}

impl Verb {
    fn of(kind: &str) -> Option<Self> {
        Some(match kind {
            "system_join_channel" => Self::JoinChannel,
            "system_guest_join_channel" => Self::JoinChannelAsGuest,
            "system_leave_channel" => Self::LeaveChannel,
            "system_add_to_channel" => Self::AddToChannel,
            "system_add_guest_to_chan" => Self::AddGuestToChannel,
            "system_remove_from_channel" => Self::RemoveFromChannel,
            "system_join_team" => Self::JoinTeam,
            "system_leave_team" => Self::LeaveTeam,
            "system_add_to_team" => Self::AddToTeam,
            "system_remove_from_team" => Self::RemoveFromTeam,
            _ => return None,
        })
    }

    /// The predicate after the names, without the actor of an add.
    fn words(self, many: bool) -> &'static str {
        match self {
            Self::JoinChannel => "joined the channel",
            Self::JoinChannelAsGuest => "joined the channel as a guest",
            Self::LeaveChannel => "left the channel",
            Self::AddToChannel => "added to the channel by",
            Self::AddGuestToChannel => "added to the channel as a guest by",
            Self::RemoveFromChannel if many => "were removed from the channel",
            Self::RemoveFromChannel => "was removed from the channel",
            Self::JoinTeam => "joined the team",
            Self::LeaveTeam => "left the team",
            Self::AddToTeam => "added to the team by",
            Self::RemoveFromTeam if many => "were removed from the team",
            Self::RemoveFromTeam => "was removed from the team",
        }
    }
}

/// One join, leave, add or remove: who it happened to and, for an add,
/// who did it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Activity {
    verb: Verb,
    who: String,
    by: Option<String>,
}

impl Activity {
    fn of(post: &ApiPost, users: &HashMap<String, User>, viewer: &Viewer<'_>) -> Option<Self> {
        let verb = Verb::of(&post.kind)?;
        let name = |id_key: &str, name_key: &str, fallback_id: &str| {
            let id = Some(prop(post, id_key))
                .filter(|id| !id.is_empty())
                .unwrap_or(fallback_id);
            person(id, prop(post, name_key), users, viewer)
        };
        let actor = || person(&post.user_id, prop(post, "username"), users, viewer);
        Some(match verb {
            Verb::AddToChannel | Verb::AddGuestToChannel | Verb::AddToTeam => Self {
                verb,
                who: name("addedUserId", "addedUsername", ""),
                by: Some(actor()),
            },
            Verb::RemoveFromChannel | Verb::RemoveFromTeam => Self {
                verb,
                who: name("removedUserId", "removedUsername", &post.user_id),
                by: None,
            },
            _ => Self {
                verb,
                who: actor(),
                by: None,
            },
        })
    }
}

/// "Ada", "Ada and Sam", "Ada, Sam and Lena", "Ada, Sam and 3 others".
fn name_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        [first, second, third] => format!("{first}, {second} and {third}"),
        [first, second, rest @ ..] => format!("{first}, {second} and {} others", rest.len()),
    }
}

/// One sentence per kind of change, in the order they first happened:
/// "Ada, Sam and 3 others joined the channel. Lena left the channel."
fn activity_text(entries: &[Activity]) -> String {
    let mut groups: Vec<(Verb, Option<&str>, Vec<String>)> = Vec::new();
    for entry in entries {
        let by = entry.by.as_deref();
        let group = match groups
            .iter_mut()
            .position(|(verb, actor, _)| *verb == entry.verb && *actor == by)
        {
            Some(index) => &mut groups[index],
            None => {
                groups.push((entry.verb, by, Vec::new()));
                groups.last_mut().expect("just pushed")
            }
        };
        if !group.2.contains(&entry.who) {
            group.2.push(entry.who.clone());
        }
    }
    groups
        .iter()
        .map(|(verb, by, names)| {
            let words = verb.words(names.len() > 1);
            match by {
                Some(by) => format!("{} {words} {by}.", name_list(names)),
                None => format!("{} {words}.", name_list(names)),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The sentence for a system post other than a join or leave, in the web
/// app's wording; the server's own text for types Matty does not word
/// itself. `None` when there is nothing to say.
fn system_text(
    post: &ApiPost,
    users: &HashMap<String, User>,
    viewer: &Viewer<'_>,
) -> Option<String> {
    if let Some(activity) = Activity::of(post, users, viewer) {
        return Some(activity_text(&[activity]));
    }
    let actor = || person(&post.user_id, prop(post, "username"), users, viewer);
    let server_text = || Some(post.message.trim().to_string()).filter(|text| !text.is_empty());
    let change = |what: &str, old: &str, new: &str| {
        let (old, new) = (prop(post, old).trim(), prop(post, new).trim());
        match (old.is_empty(), new.is_empty()) {
            (true, true) => server_text(),
            (false, true) => Some(format!(
                "{} removed the channel {what} (was: {old})",
                actor()
            )),
            (true, false) => Some(format!("{} updated the channel {what} to: {new}", actor())),
            (false, false) => Some(format!(
                "{} updated the channel {what} from: {old} to: {new}",
                actor()
            )),
        }
    };
    match post.kind.as_str() {
        "system_header_change" => change("header", "old_header", "new_header"),
        "system_purpose_change" => change("purpose", "old_purpose", "new_purpose"),
        "system_displayname_change" => change("display name", "old_displayname", "new_displayname"),
        "system_channel_deleted" => Some(format!("{} archived the channel", actor())),
        "system_channel_restored" => Some(format!("{} unarchived the channel", actor())),
        _ => server_text(),
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

/// A followed thread as the Threads view lists it (#124): its root post,
/// and a summary of its replies from the server's thread record.
pub fn thread_row(
    thread: &matty_sync::ThreadSummary,
    root: &ApiPost,
    channel_name: SharedString,
    users: &HashMap<String, User>,
    viewer: &Viewer<'_>,
) -> crate::threads::ThreadRow {
    let today = viewer.now.to_zoned(viewer.zone.clone()).date();
    let (last_time, last_day, _) = when(thread.last_reply_at, viewer.zone, today);
    let people: Vec<&User> = thread
        .participants
        .iter()
        .rev()
        .filter_map(|id| users.get(id))
        .take(3)
        .collect();
    let participants = people
        .iter()
        .map(|user| display_name_as(user, viewer.names).into())
        .collect();
    let participant_ids = people.iter().map(|user| user.id.clone().into()).collect();
    crate::threads::ThreadRow {
        channel_id: root.channel_id.clone().into(),
        thread: crate::right_pane::ThreadModel {
            channel_id: root.channel_id.clone().into(),
            channel_name,
            root: listed_post(root, users, viewer),
            replies: Vec::new(),
            following: true,
            reply_count: usize::try_from(thread.reply_count).unwrap_or(0),
            older: false,
            failed_older: false,
        },
        summary: crate::channel::ReplySummary {
            count: usize::try_from(thread.reply_count).unwrap_or(0),
            last_time,
            last_day,
            participants,
            participant_ids,
            following: true,
            unread: thread.unread_replies > 0,
        },
    }
}

/// Minutes since local midnight, the day label the post falls under, and
/// the full date for the time's tooltip.
pub(crate) fn when(
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
fn reactions(
    post: &ApiPost,
    me: &str,
    users: &HashMap<String, User>,
    names: NameFormat,
) -> Vec<PostReaction> {
    let mut chips: Vec<(String, PostReaction)> = Vec::new();
    for reaction in &post.metadata.reactions {
        // Others by name for the tooltip; people not loaded yet are left
        // out, as the field allows.
        let name = (reaction.user_id != me)
            .then(|| users.get(&reaction.user_id))
            .flatten()
            .map(|user| SharedString::from(display_name_as(user, names)));
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

    static SAVED: std::sync::LazyLock<std::collections::HashSet<String>> =
        std::sync::LazyLock::new(Default::default);

    fn viewer(zone: &TimeZone) -> Viewer<'_> {
        Viewer {
            user_id: "me",
            keys: &KEYS,
            saved: &SAVED,
            zone,
            // 2026-11-04 12:00 UTC.
            now: Timestamp::from_second(1_793_793_600).unwrap(),
            link_previews: true,
            collapsed_threads: false,
            names: NameFormat::default(),
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
        let model = channel_model(&row, Some(&posts), &users, &viewer(&zone), Some(4000), &[]);
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
        let model = channel_model(
            &row,
            Some(&posts),
            &HashMap::new(),
            &viewer(&zone),
            None,
            &[],
        );
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
        let model = channel_model(&row, Some(&[p]), &HashMap::new(), &viewer(&zone), None, &[]);
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
        let model = channel_model(&row, None, &HashMap::new(), &viewer(&zone), None, &[]);
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
        let chips = reactions(&p, "me", &users, NameFormat::default());
        assert_eq!(chips.len(), 2);
        assert_eq!(
            (chips[0].emoji.as_ref(), chips[0].count, chips[0].mine),
            ("👍", 2, true)
        );
        assert_eq!(chips[0].users, ["ada"]);
        assert_eq!(chips[1].emoji.as_ref(), ":partyparrot:");
    }

    fn system(id: &str, kind: &str, user: &str, at: i64, props: &[(&str, &str)]) -> ApiPost {
        ApiPost {
            kind: kind.into(),
            props: props
                .iter()
                .map(|(key, value)| (key.to_string(), serde_json::Value::from(*value)))
                .collect(),
            ..post(id, user, at, "")
        }
    }

    fn person(id: &str, first: &str) -> (String, User) {
        (
            id.to_string(),
            User {
                id: id.into(),
                username: id.into(),
                first_name: first.into(),
                ..User::default()
            },
        )
    }

    fn shown(model: &ChannelModel) -> Vec<(PostKind, &str, bool)> {
        model
            .posts
            .iter()
            .map(|p| (p.kind.clone(), p.body.as_ref(), p.continues))
            .collect()
    }

    const ACTIVITY: PostKind = PostKind::System { activity: true };
    const CHANGE: PostKind = PostKind::System { activity: false };

    #[test]
    fn a_channel_of_only_system_posts_is_not_empty() {
        let zone = TimeZone::UTC;
        let users = HashMap::from([person("ada", "Ada")]);
        let posts = [system("s1", "system_join_channel", "ada", NOON, &[])];
        let row = ChannelRow::new("c1", "town", ChannelKind::Public);
        let model = channel_model(&row, Some(&posts), &users, &viewer(&zone), None, &[]);
        assert_eq!(
            shown(&model),
            [(ACTIVITY, "Ada joined the channel.", false)]
        );
        let post = &model.posts[0];
        assert!(!post.mine && post.reactions.is_empty() && post.author.is_empty());
    }

    #[test]
    fn joins_and_leaves_in_a_row_combine() {
        let zone = TimeZone::UTC;
        let users = HashMap::from([
            person("ada", "Ada"),
            person("sam", "Sam"),
            person("lena", "Lena"),
            person("jonas", "Jonas"),
            person("priya", "Priya"),
        ]);
        let mut posts: Vec<ApiPost> = ["ada", "sam", "lena", "jonas", "priya"]
            .iter()
            .enumerate()
            .map(|(index, user)| {
                let at = NOON + index as i64 * 60_000;
                system(&format!("j{index}"), "system_join_channel", user, at, &[])
            })
            .collect();
        // Someone the client has not loaded yet is named from the post.
        posts.push(system(
            "l1",
            "system_leave_channel",
            "zed",
            NOON + 400_000,
            &[("username", "zed")],
        ));
        posts.push(system(
            "a1",
            "system_add_to_channel",
            "ada",
            NOON + 500_000,
            &[("addedUserId", "me"), ("addedUsername", "arya")],
        ));
        // The same person joining twice is named once.
        posts.push(system(
            "j9",
            "system_join_channel",
            "sam",
            NOON + 600_000,
            &[],
        ));
        let row = ChannelRow::new("c1", "town", ChannelKind::Public);
        let model = channel_model(&row, Some(&posts), &users, &viewer(&zone), None, &[]);
        assert_eq!(
            shown(&model),
            [(
                ACTIVITY,
                "Ada, Sam and 3 others joined the channel. @zed left the channel. \
                 You added to the channel by Ada.",
                false
            )]
        );
        // The row sits where the first change happened.
        assert_eq!(model.posts[0].id, "j0");
        assert_eq!(model.posts[0].time, 12 * 60);
    }

    #[test]
    fn name_lists_shorten_past_three() {
        let names = |n: usize| -> Vec<String> {
            ["A", "B", "C", "D", "E"][..n]
                .iter()
                .map(|name| name.to_string())
                .collect()
        };
        assert_eq!(name_list(&names(1)), "A");
        assert_eq!(name_list(&names(2)), "A and B");
        assert_eq!(name_list(&names(3)), "A, B and C");
        assert_eq!(name_list(&names(5)), "A, B and 3 others");
    }

    #[test]
    fn removals_agree_with_their_count() {
        let zone = TimeZone::UTC;
        let users = HashMap::from([person("ada", "Ada"), person("sam", "Sam")]);
        let remove = |id: &str, who: &str, at: i64| {
            system(
                id,
                "system_remove_from_channel",
                "me",
                at,
                &[("removedUserId", who)],
            )
        };
        let row = ChannelRow::new("c1", "town", ChannelKind::Public);
        let one = [remove("r1", "ada", NOON)];
        let model = channel_model(&row, Some(&one), &users, &viewer(&zone), None, &[]);
        assert_eq!(model.posts[0].body, "Ada was removed from the channel.");
        let two = [remove("r1", "ada", NOON), remove("r2", "sam", NOON + 1)];
        let model = channel_model(&row, Some(&two), &users, &viewer(&zone), None, &[]);
        assert_eq!(
            model.posts[0].body,
            "Ada and Sam were removed from the channel."
        );
    }

    #[test]
    fn other_system_posts_are_worded_and_break_runs() {
        let zone = TimeZone::UTC;
        let users = HashMap::from([person("ada", "Ada"), person("sam", "Sam")]);
        let posts = [
            post("p1", "ada", NOON, "one"),
            system("s1", "system_join_channel", "sam", NOON + 1_000, &[]),
            // A message ends the run of joins; the next join starts a row.
            post("p2", "ada", NOON + 2_000, "two"),
            system(
                "s2",
                "system_header_change",
                "sam",
                NOON + 3_000,
                &[("old_header", "Old"), ("new_header", "New")],
            ),
            post("p3", "ada", NOON + 4_000, "three"),
            system(
                "s3",
                "system_purpose_change",
                "sam",
                NOON + 5_000,
                &[("old_purpose", "Ship it")],
            ),
            system(
                "s4",
                "system_displayname_change",
                "ada",
                NOON + 6_000,
                &[("old_displayname", ""), ("new_displayname", "Town")],
            ),
            system("s5", "system_join_channel", "ada", NOON + 7_000, &[]),
            post("p4", "ada", NOON + 8_000, "four"),
            post("p5", "ada", NOON + 9_000, "five"),
        ];
        let row = ChannelRow::new("c1", "town", ChannelKind::Public);
        let model = channel_model(&row, Some(&posts), &users, &viewer(&zone), None, &[]);
        assert_eq!(
            shown(&model),
            [
                (PostKind::Regular, "one", false),
                (ACTIVITY, "Sam joined the channel.", false),
                (PostKind::Regular, "two", false),
                (
                    CHANGE,
                    "Sam updated the channel header from: Old to: New",
                    false
                ),
                (PostKind::Regular, "three", false),
                (
                    CHANGE,
                    "Sam removed the channel purpose (was: Ship it)",
                    false
                ),
                (
                    CHANGE,
                    "Ada updated the channel display name to: Town",
                    false
                ),
                (ACTIVITY, "Ada joined the channel.", false),
                (PostKind::Regular, "four", false),
                (PostKind::Regular, "five", true),
            ]
        );
    }

    #[test]
    fn a_day_change_splits_combined_activity() {
        let zone = TimeZone::UTC;
        let users = HashMap::from([person("ada", "Ada"), person("sam", "Sam")]);
        let posts = [
            system("s1", "system_join_channel", "ada", NOON - 86_400_000, &[]),
            system("s2", "system_join_channel", "sam", NOON, &[]),
        ];
        let row = ChannelRow::new("c1", "town", ChannelKind::Public);
        let model = channel_model(&row, Some(&posts), &users, &viewer(&zone), None, &[]);
        let days: Vec<_> = model.posts.iter().map(|p| p.day.as_ref()).collect();
        assert_eq!(days, ["Yesterday", "Today"]);
    }

    #[test]
    fn unknown_system_types_use_the_servers_text() {
        let zone = TimeZone::UTC;
        let mut known = system("s1", "system_change_chan_privacy", "ada", NOON, &[]);
        known.message = "This channel has been converted to a Private Channel.".into();
        let silent = system("s2", "system_ephemeral", "ada", NOON + 1, &[]);
        let row = ChannelRow::new("c1", "town", ChannelKind::Public);
        let posts = [known, silent];
        let model = channel_model(
            &row,
            Some(&posts),
            &HashMap::new(),
            &viewer(&zone),
            None,
            &[],
        );
        assert_eq!(
            shown(&model),
            [(
                CHANGE,
                "This channel has been converted to a Private Channel.",
                false
            )]
        );
    }

    #[test]
    fn plugin_posts_are_unsupported_and_others_are_regular() {
        let zone = TimeZone::UTC;
        let mut poll = post("p1", "ada", NOON, "Poll: lunch?");
        poll.kind = "custom_matterpoll".into();
        let mut hook = post("p2", "ada", NOON + 1, "");
        hook.kind = "slack_attachment".into();
        let mut me = post("p3", "ada", NOON + 2, "waves");
        me.kind = "me".into();
        let row = ChannelRow::new("c1", "town", ChannelKind::Public);
        let posts = [poll, hook, me];
        let model = channel_model(
            &row,
            Some(&posts),
            &HashMap::new(),
            &viewer(&zone),
            None,
            &[],
        );
        let kinds: Vec<_> = model.posts.iter().map(|p| p.kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                PostKind::Unsupported("custom_matterpoll".into()),
                PostKind::Regular,
                PostKind::Regular,
            ]
        );
        // An unsupported post still has its author and groups like any other.
        assert_eq!(model.posts[0].author, "Someone");
        assert!(model.posts[1].continues);
    }

    fn people() -> HashMap<String, User> {
        ["ada", "sam", "lena", "jonas"]
            .into_iter()
            .map(|id| {
                let mut name = id.to_string();
                name[..1].make_ascii_uppercase();
                (
                    id.to_string(),
                    User {
                        id: id.into(),
                        username: id.into(),
                        first_name: name,
                        ..User::default()
                    },
                )
            })
            .collect()
    }

    fn reply_to(id: &str, user: &str, at: i64, root: &str) -> ApiPost {
        ApiPost {
            root_id: root.into(),
            ..post(id, user, at, &format!("reply {id}"))
        }
    }

    /// #123, collapsed reply threads off: a reply names its root above it
    /// ("Commented on Ada's message: …") unless the row above is already in
    /// that thread, starts its own run, and opens its root's thread. A root
    /// the posts leave out comes from `roots`.
    #[test]
    fn replies_show_inline_with_what_they_answer() {
        let zone = TimeZone::UTC;
        let row = ChannelRow::new("c1", "town", ChannelKind::Public);
        let old = post(
            "old",
            "ada",
            NOON - 86_400_000,
            "The **old** plan\nwith two lines",
        );
        let posts = [
            post("root", "sam", NOON, "Lunch at 12?"),
            reply_to("r1", "lena", NOON + 1, "root"),
            reply_to("r2", "lena", NOON + 2, "root"),
            post("other", "lena", NOON + 3, "Unrelated"),
            reply_to("r3", "lena", NOON + 4, "root"),
            reply_to("r4", "lena", NOON + 5, "old"),
            reply_to("r5", "jonas", NOON + 6, "gone"),
        ];
        let model = channel_model(
            &row,
            Some(&posts),
            &people(),
            &viewer(&zone),
            None,
            std::slice::from_ref(&old),
        );
        let context: Vec<_> = model
            .posts
            .iter()
            .map(|p| {
                p.reply_to
                    .as_ref()
                    .map(|c| format!("{}: {}", c.author, c.excerpt))
            })
            .collect();
        assert_eq!(
            context,
            [
                None,
                // Right under its root: no need to name it.
                None,
                None,
                None,
                Some("Sam: Lunch at 12?".into()),
                Some("Ada: The old plan with two lines".into()),
                // The root is not cached: nothing to name.
                None,
            ]
        );
        let continues: Vec<_> = model.posts.iter().map(|p| p.continues).collect();
        assert_eq!(continues, [false, false, true, true, false, false, false]);
        assert_eq!(model.posts[5].thread_root(), "old");
        assert_eq!(model.posts[0].thread_root(), "root");
        assert!(model.posts.iter().all(|p| p.replies.is_none()));
    }

    /// #123, collapsed reply threads on: replies stay in their thread and a
    /// root with replies gets the footer from what sync knows of them.
    #[test]
    fn collapsed_threads_give_roots_a_reply_footer() {
        let zone = TimeZone::UTC;
        let row = ChannelRow::new("c1", "town", ChannelKind::Public);
        let user = |id: &str| User {
            id: id.into(),
            ..User::default()
        };
        let root = ApiPost {
            reply_count: 3,
            last_reply_at: NOON + 60_000 * 13,
            participants: vec![user("ada"), user("lena"), user("jonas"), user("sam")],
            is_following: Some(true),
            ..post("root", "sam", NOON, "Lunch at 12?")
        };
        let posts = [
            root,
            reply_to("r1", "lena", NOON + 1, "root"),
            post("plain", "ada", NOON + 2, "No replies here"),
        ];
        let viewer = Viewer {
            collapsed_threads: true,
            ..viewer(&zone)
        };
        let model = channel_model(&row, Some(&posts), &people(), &viewer, None, &[]);
        let ids: Vec<_> = model.posts.iter().map(|p| p.id.as_ref()).collect();
        assert_eq!(ids, ["root", "plain"]);
        let footer = model.posts[0].replies.as_ref().expect("a footer");
        assert_eq!(footer.count, 3);
        assert_eq!(footer.participants, ["Sam", "Jonas", "Lena"]);
        assert_eq!(footer.participant_ids, ["sam", "jonas", "lena"]);
        assert_eq!(
            (footer.last_time, footer.last_day.as_ref()),
            (12 * 60 + 13, "Today")
        );
        assert!(footer.following);
        assert!(model.posts[1].replies.is_none());
    }

    #[test]
    fn reactions_go_back_to_the_server_by_name() {
        assert_eq!(emoji_name("👍"), "+1");
        assert_eq!(emoji_name(":partyparrot:"), "partyparrot");
        assert_eq!(emoji_name("tada"), "tada");
    }

    #[test]
    fn names_follow_the_chosen_format() {
        let user = User {
            username: "ada".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            nickname: "Countess".into(),
            ..User::default()
        };
        assert_eq!(display_name_as(&user, NameFormat::Username), "ada");
        assert_eq!(display_name_as(&user, NameFormat::FullName), "Ada Lovelace");
        assert_eq!(
            display_name_as(&user, NameFormat::NicknameFullName),
            "Countess"
        );
        let bare = User {
            username: "sam".into(),
            ..User::default()
        };
        for format in [
            NameFormat::Username,
            NameFormat::FullName,
            NameFormat::NicknameFullName,
        ] {
            assert_eq!(display_name_as(&bare, format), "sam");
        }
    }
}
