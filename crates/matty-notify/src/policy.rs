//! Whether a new post becomes a desktop notification (PLAN §4.6), as one
//! pure function over the post, the user's and the channel's preferences
//! and what the app is showing. The platform backends only show what
//! [`decide`] returns.

use std::collections::HashMap;

use crate::mention::MentionKeys;
use crate::sound::Sound;

/// How much a user or channel notifies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Every message.
    All,
    /// Mentions and direct messages.
    Mention,
    /// Nothing.
    None,
}

impl Level {
    /// `all`, `mention` or `none`; `default` and anything else is `None`
    /// here, meaning "no override".
    fn parse(value: Option<&String>) -> Option<Self> {
        match value.map(String::as_str) {
            Some("all") => Some(Self::All),
            Some("mention") => Some(Self::Mention),
            Some("none") => Some(Self::None),
            _ => None,
        }
    }
}

/// The user's own notification preferences, from their `notify_props`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserPrefs {
    /// `desktop`: Mattermost's default is mentions.
    pub desktop: Level,
    /// `desktop_threads`: replies in followed threads, with collapsed
    /// reply threads on. The default is all.
    pub desktop_threads: Level,
    /// `desktop_sound`: on unless turned off.
    pub desktop_sound: bool,
    /// `desktop_notification_sound`: which sound plays, Bing by default.
    pub sound: Sound,
    /// What counts as a mention.
    pub keys: MentionKeys,
}

impl UserPrefs {
    /// Reads the user's `notify_props`, with Mattermost's defaults.
    pub fn from_props(username: &str, first_name: &str, props: &HashMap<String, String>) -> Self {
        Self {
            desktop: Level::parse(props.get("desktop")).unwrap_or(Level::Mention),
            desktop_threads: Level::parse(props.get("desktop_threads")).unwrap_or(Level::All),
            desktop_sound: props
                .get("desktop_sound")
                .is_none_or(|value| value != "false"),
            sound: Sound::from_prop(props.get("desktop_notification_sound").map(String::as_str)),
            keys: MentionKeys::from_props(username, first_name, props),
        }
    }
}

/// One channel's overrides, from the user's channel member `notify_props`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelPrefs {
    /// `desktop`, unless it is `default`.
    pub desktop: Option<Level>,
    /// `desktop_threads`, unless it is `default`.
    pub desktop_threads: Option<Level>,
    /// `mark_unread` is `mention`.
    pub muted: bool,
    /// `ignore_channel_mentions` is `on`.
    pub ignore_channel_mentions: bool,
}

impl ChannelPrefs {
    /// Reads the channel member's `notify_props`.
    pub fn from_props(props: &HashMap<String, String>) -> Self {
        let is = |key: &str, value: &str| props.get(key).is_some_and(|v| v == value);
        Self {
            desktop: Level::parse(props.get("desktop")),
            desktop_threads: Level::parse(props.get("desktop_threads")),
            muted: is("mark_unread", "mention"),
            ignore_channel_mentions: is("ignore_channel_mentions", "on"),
        }
    }
}

/// The kind of channel a post arrived in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    /// An open channel.
    Public,
    /// A private channel.
    Private,
    /// A direct message.
    Direct,
    /// A group message.
    Group,
}

/// A post that just arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// The post.
    pub post_id: String,
    /// Its channel.
    pub channel_id: String,
    /// The thread's root, or empty for a post in the channel.
    pub root_id: String,
    /// Who wrote it.
    pub author_id: String,
    /// How the author's name shows, for the title and body.
    pub author_name: String,
    /// How the channel's name shows, for the title.
    pub channel_name: String,
    /// The channel's kind.
    pub channel_kind: ChannelKind,
    /// The post's text.
    pub message: String,
    /// A system message such as a join or a header change.
    pub system: bool,
    /// The server listed the user in the event's `mentions`.
    pub server_mentioned: bool,
    /// The user follows the post's thread.
    pub followed_thread: bool,
}

/// What the app is doing when the post arrives.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppState {
    /// The signed-in user's id.
    pub me: String,
    /// The user's status is Do not disturb or out of office.
    pub dnd: bool,
    /// A Matty window has focus.
    pub focused: bool,
    /// The channel the focused window shows.
    pub visible_channel: Option<String>,
    /// The thread open in the focused window's right pane.
    pub visible_thread: Option<String>,
    /// Collapsed reply threads are on: replies stay out of the channel.
    pub collapsed_threads: bool,
    /// The "show message preview" preference.
    pub show_preview: bool,
    /// Sounds for every message, not only mentions and direct messages.
    pub sound_for_all: bool,
}

/// A notification to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    /// The channel, or the author for a direct message.
    pub title: String,
    /// The message, or only who wrote it without previews.
    pub body: String,
    /// Where clicking it goes.
    pub channel_id: String,
    /// The post to highlight.
    pub post_id: String,
    /// The thread, or empty.
    pub root_id: String,
    /// It mentions the user or is a direct message.
    pub mention: bool,
    /// The sound to play with it, if any.
    pub sound: Option<Sound>,
}

/// Whether `event` notifies, and how.
pub fn decide(
    event: &Event,
    user: &UserPrefs,
    channel: &ChannelPrefs,
    app: &AppState,
) -> Option<Notification> {
    if event.author_id == app.me || event.system || channel.muted || app.dnd {
        return None;
    }
    let reply = !event.root_id.is_empty();
    if app.focused {
        let in_channel = app.visible_channel.as_deref() == Some(event.channel_id.as_str())
            && (!reply || !app.collapsed_threads);
        let in_thread = reply && app.visible_thread.as_deref() == Some(event.root_id.as_str());
        if in_channel || in_thread {
            return None;
        }
    }
    let direct = matches!(event.channel_kind, ChannelKind::Direct | ChannelKind::Group);
    let mention = direct
        || event.server_mentioned
        || user.keys.mentioned(&event.message)
        || (!channel.ignore_channel_mentions && user.keys.channel_mentioned(&event.message));
    let level = channel.desktop.unwrap_or(user.desktop);
    if level == Level::None {
        return None;
    }
    let notifies = if reply && app.collapsed_threads {
        // Replies stay in their thread: they notify on a mention, or in a
        // followed thread when thread notifications are all.
        let threads = channel.desktop_threads.unwrap_or(user.desktop_threads);
        mention || (event.followed_thread && threads == Level::All)
    } else {
        mention || level == Level::All
    };
    if !notifies {
        return None;
    }
    let title = if event.channel_kind == ChannelKind::Direct {
        event.author_name.clone()
    } else {
        event.channel_name.clone()
    };
    let body = match (app.show_preview, event.channel_kind) {
        (true, ChannelKind::Direct) => event.message.clone(),
        (true, _) => format!("{}: {}", event.author_name, event.message),
        (false, _) => format!("New message from {}", event.author_name),
    };
    Some(Notification {
        title,
        body,
        channel_id: event.channel_id.clone(),
        post_id: event.post_id.clone(),
        root_id: event.root_id.clone(),
        mention,
        sound: (user.desktop_sound && (mention || app.sound_for_all)).then_some(user.sound),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(message: &str) -> Event {
        Event {
            post_id: "p".into(),
            channel_id: "town".into(),
            root_id: String::new(),
            author_id: "sam".into(),
            author_name: "Sam Okafor".into(),
            channel_name: "Town Square".into(),
            channel_kind: ChannelKind::Public,
            message: message.into(),
            system: false,
            server_mentioned: false,
            followed_thread: false,
        }
    }

    fn user(desktop: Level) -> UserPrefs {
        UserPrefs {
            desktop,
            desktop_threads: Level::All,
            desktop_sound: true,
            sound: Sound::Bing,
            keys: MentionKeys {
                username: "arya".into(),
                channel_wide: true,
                keywords: vec!["deploy".into()],
                ..MentionKeys::default()
            },
        }
    }

    fn app() -> AppState {
        AppState {
            me: "arya-id".into(),
            focused: false,
            show_preview: true,
            ..AppState::default()
        }
    }

    /// One row of the table: a description, the setup, and whether it
    /// notifies.
    struct Case {
        name: &'static str,
        event: Event,
        user: UserPrefs,
        channel: ChannelPrefs,
        app: AppState,
        notifies: bool,
    }

    fn case(name: &'static str, notifies: bool) -> Case {
        Case {
            name,
            event: event("hello"),
            user: user(Level::Mention),
            channel: ChannelPrefs::default(),
            app: app(),
            notifies,
        }
    }

    #[test]
    fn the_rules() {
        let cases = [
            // Levels.
            case("plain post, level mention", false),
            Case {
                user: user(Level::All),
                ..case("plain post, level all", true)
            },
            Case {
                event: event("hi @arya"),
                ..case("@mention, level mention", true)
            },
            Case {
                event: event("hi @arya"),
                user: user(Level::None),
                ..case("@mention, level none", false)
            },
            // Per-channel overrides.
            Case {
                channel: ChannelPrefs {
                    desktop: Some(Level::All),
                    ..Default::default()
                },
                ..case("channel override all beats user mention", true)
            },
            Case {
                event: event("hi @arya"),
                user: user(Level::All),
                channel: ChannelPrefs {
                    desktop: Some(Level::None),
                    ..Default::default()
                },
                ..case("channel override none beats a mention", false)
            },
            // Mention keys.
            Case {
                event: event("the Deploy broke"),
                ..case("keyword", true)
            },
            Case {
                event: event("redeploy"),
                ..case("keyword inside a word", false)
            },
            Case {
                event: Event {
                    server_mentioned: true,
                    ..event("hi")
                },
                ..case("the server says mentioned", true)
            },
            Case {
                event: event("@here standup"),
                ..case("@here", true)
            },
            Case {
                event: event("@channel standup"),
                channel: ChannelPrefs {
                    ignore_channel_mentions: true,
                    ..Default::default()
                },
                ..case("@channel in a channel that ignores it", false)
            },
            // Direct and group messages.
            Case {
                event: Event {
                    channel_kind: ChannelKind::Direct,
                    ..event("hi")
                },
                ..case("direct message, level mention", true)
            },
            Case {
                event: Event {
                    channel_kind: ChannelKind::Group,
                    ..event("hi")
                },
                ..case("group message, level mention", true)
            },
            Case {
                event: Event {
                    channel_kind: ChannelKind::Direct,
                    ..event("hi")
                },
                user: user(Level::None),
                ..case("direct message, level none", false)
            },
            // Who and where.
            Case {
                event: Event {
                    author_id: "arya-id".into(),
                    ..event("@arya")
                },
                ..case("own post", false)
            },
            Case {
                event: Event {
                    system: true,
                    ..event("@arya joined")
                },
                ..case("system message", false)
            },
            Case {
                event: event("hi @arya"),
                channel: ChannelPrefs {
                    muted: true,
                    ..Default::default()
                },
                ..case("muted channel, even a mention", false)
            },
            Case {
                event: event("hi @arya"),
                app: AppState { dnd: true, ..app() },
                ..case("do not disturb", false)
            },
            Case {
                event: event("hi @arya"),
                app: AppState {
                    focused: true,
                    visible_channel: Some("town".into()),
                    ..app()
                },
                ..case("visible channel in a focused window", false)
            },
            Case {
                event: event("hi @arya"),
                app: AppState {
                    visible_channel: Some("town".into()),
                    ..app()
                },
                ..case("visible channel, window not focused", true)
            },
            Case {
                event: event("hi @arya"),
                app: AppState {
                    focused: true,
                    visible_channel: Some("other".into()),
                    ..app()
                },
                ..case("another channel visible", true)
            },
            // Threads with collapsed reply threads on.
            Case {
                event: Event {
                    root_id: "root".into(),
                    followed_thread: true,
                    ..event("hi")
                },
                app: AppState {
                    collapsed_threads: true,
                    ..app()
                },
                ..case("reply in a followed thread", true)
            },
            Case {
                event: Event {
                    root_id: "root".into(),
                    followed_thread: true,
                    ..event("hi")
                },
                user: UserPrefs {
                    desktop_threads: Level::Mention,
                    ..user(Level::Mention)
                },
                app: AppState {
                    collapsed_threads: true,
                    ..app()
                },
                ..case("followed thread, thread level mention", false)
            },
            Case {
                event: Event {
                    root_id: "root".into(),
                    ..event("hi")
                },
                user: user(Level::All),
                app: AppState {
                    collapsed_threads: true,
                    ..app()
                },
                ..case("reply in a thread not followed, level all", false)
            },
            Case {
                event: Event {
                    root_id: "root".into(),
                    ..event("hi @arya")
                },
                app: AppState {
                    collapsed_threads: true,
                    ..app()
                },
                ..case("mention in a thread not followed", true)
            },
            Case {
                event: Event {
                    root_id: "root".into(),
                    followed_thread: true,
                    ..event("hi")
                },
                app: AppState {
                    collapsed_threads: true,
                    focused: true,
                    visible_channel: Some("town".into()),
                    ..app()
                },
                ..case("reply while its channel is visible", true)
            },
            Case {
                event: Event {
                    root_id: "root".into(),
                    followed_thread: true,
                    ..event("hi")
                },
                app: AppState {
                    collapsed_threads: true,
                    focused: true,
                    visible_thread: Some("root".into()),
                    ..app()
                },
                ..case("reply while its thread is open", false)
            },
            Case {
                event: Event {
                    root_id: "root".into(),
                    ..event("hi")
                },
                app: AppState {
                    focused: true,
                    visible_channel: Some("town".into()),
                    ..app()
                },
                user: user(Level::All),
                ..case(
                    "reply in the visible channel without collapsed threads",
                    false,
                )
            },
        ];
        for case in cases {
            let got = decide(&case.event, &case.user, &case.channel, &case.app);
            assert_eq!(got.is_some(), case.notifies, "{}", case.name);
        }
    }

    #[test]
    fn text_follows_the_preview_preference() {
        let shown = decide(
            &event("hi @arya"),
            &user(Level::Mention),
            &ChannelPrefs::default(),
            &app(),
        )
        .unwrap();
        assert_eq!(shown.title, "Town Square");
        assert_eq!(shown.body, "Sam Okafor: hi @arya");
        let hidden = decide(
            &event("hi @arya"),
            &user(Level::Mention),
            &ChannelPrefs::default(),
            &AppState {
                show_preview: false,
                ..app()
            },
        )
        .unwrap();
        assert_eq!(hidden.body, "New message from Sam Okafor");
        let direct = decide(
            &Event {
                channel_kind: ChannelKind::Direct,
                ..event("lunch?")
            },
            &user(Level::Mention),
            &ChannelPrefs::default(),
            &app(),
        )
        .unwrap();
        assert_eq!(
            (direct.title.as_str(), direct.body.as_str()),
            ("Sam Okafor", "lunch?")
        );
    }

    #[test]
    fn sounds_are_for_mentions_unless_turned_on_for_all() {
        let all = user(Level::All);
        let plain = decide(&event("hi"), &all, &ChannelPrefs::default(), &app()).unwrap();
        assert_eq!(plain.sound, None);
        let loud = AppState {
            sound_for_all: true,
            ..app()
        };
        assert_eq!(
            decide(&event("hi"), &all, &ChannelPrefs::default(), &loud)
                .unwrap()
                .sound,
            Some(Sound::Bing)
        );
        let mention = decide(&event("@arya"), &all, &ChannelPrefs::default(), &app()).unwrap();
        assert!(mention.sound.is_some() && mention.mention);
        let quiet = UserPrefs {
            desktop_sound: false,
            ..all
        };
        assert_eq!(
            decide(&event("@arya"), &quiet, &ChannelPrefs::default(), &app())
                .unwrap()
                .sound,
            None
        );
    }

    #[test]
    fn the_chosen_sound_plays_unless_the_channel_is_on_screen() {
        let ripple = UserPrefs {
            sound: Sound::Ripple,
            ..user(Level::Mention)
        };
        let mention = event("@arya look");
        let elsewhere = AppState {
            focused: true,
            visible_channel: Some("other".into()),
            ..app()
        };
        let shown = decide(&mention, &ripple, &ChannelPrefs::default(), &elsewhere).unwrap();
        assert_eq!(shown.sound, Some(Sound::Ripple));
        // The focused window shows the channel: no banner, so no sound.
        let watching = AppState {
            visible_channel: Some("town".into()),
            ..elsewhere.clone()
        };
        assert_eq!(
            decide(&mention, &ripple, &ChannelPrefs::default(), &watching),
            None
        );
        // The same channel in a window without focus still sounds.
        let away = AppState {
            focused: false,
            ..watching
        };
        assert_eq!(
            decide(&mention, &ripple, &ChannelPrefs::default(), &away).and_then(|n| n.sound),
            Some(Sound::Ripple)
        );
    }

    #[test]
    fn preferences_read_from_notify_props() {
        let props: HashMap<String, String> = [
            ("desktop", "all"),
            ("desktop_threads", "mention"),
            ("desktop_sound", "false"),
            ("desktop_notification_sound", "Upstairs"),
            ("mention_keys", "arya,@arya,deploy"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        let prefs = UserPrefs::from_props("arya", "Arya", &props);
        assert_eq!(prefs.desktop, Level::All);
        assert_eq!(prefs.desktop_threads, Level::Mention);
        assert!(!prefs.desktop_sound);
        assert_eq!(prefs.sound, Sound::Upstairs);
        assert_eq!(prefs.keys.keywords, vec!["deploy"]);
        let defaults = UserPrefs::from_props("arya", "", &HashMap::new());
        assert_eq!(defaults.desktop, Level::Mention);
        assert!(defaults.desktop_sound);
        assert_eq!(defaults.sound, Sound::Bing);

        let member: HashMap<String, String> = [
            ("desktop", "default"),
            ("mark_unread", "mention"),
            ("ignore_channel_mentions", "on"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        let channel = ChannelPrefs::from_props(&member);
        assert_eq!(channel.desktop, None);
        assert!(channel.muted && channel.ignore_channel_mentions);
    }
}
