//! Where notifications go: one backend per platform (PLAN §6.7). The macOS
//! backend posts through UNUserNotificationCenter (#139); elsewhere, and on
//! macOS outside an app bundle, notifications are only logged.

use crate::Notification;

/// Shows and removes the app's notifications.
pub trait Notifier {
    /// Shows `notification`.
    fn show(&self, notification: &Notification);
    /// Removes the notifications for `channel_id`, as the user is reading it.
    fn withdraw(&self, channel_id: &str);
    /// Shows `badge` on the app icon: the dock on macOS (#141).
    fn set_badge(&self, badge: Badge);
}

/// What the app icon's badge shows (PLAN §4.6): the mention count, a dot
/// when something is unread without a mention, or nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Badge {
    /// Nothing is unread.
    #[default]
    Clear,
    /// Something is unread, with no mention.
    Unread,
    /// Mentions of the user, at least one.
    Mentions(usize),
}

impl Badge {
    /// The badge for `mentions` mentions, with `unread` saying whether
    /// anything else is unread.
    pub fn new(mentions: usize, unread: bool) -> Self {
        match (mentions, unread) {
            (0, false) => Self::Clear,
            (0, true) => Self::Unread,
            (count, _) => Self::Mentions(count),
        }
    }

    /// The text on the icon: the count, a dot, or `None` for no badge.
    pub fn label(self) -> Option<String> {
        match self {
            Self::Clear => None,
            Self::Unread => Some("\u{2022}".to_owned()),
            Self::Mentions(count) => Some(count.to_string()),
        }
    }
}

/// Logs what it would show, without the message: the fallback where the
/// platform has no backend yet.
#[derive(Clone, Copy, Debug, Default)]
pub struct Logged;

impl Notifier for Logged {
    fn show(&self, notification: &Notification) {
        tracing::info!(
            channel = %notification.channel_id,
            post = %notification.post_id,
            mention = notification.mention,
            sound = ?notification.sound,
            "notification"
        );
    }

    fn withdraw(&self, channel_id: &str) {
        tracing::debug!(channel = %channel_id, "notifications withdrawn");
    }

    fn set_badge(&self, badge: Badge) {
        tracing::debug!(?badge, "badge");
    }
}

/// The user clicked a notification: the channel and post it was about,
/// and the thread the post replies in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clicked {
    /// The channel the notification was about.
    pub channel_id: String,
    /// The post it showed.
    pub post_id: String,
    /// The root of the thread the post replies in; empty for a post that
    /// is not a reply. With collapsed threads on, a reply is only in its
    /// thread, so the click opens it there (#141).
    pub root_id: String,
}

impl Clicked {
    /// Reads a notification's identifier, `channel:post` or
    /// `channel:post:root` as [`Clicked::identifier`] makes it.
    pub fn from_identifier(id: &str) -> Option<Self> {
        let mut parts = id.splitn(3, ':');
        let channel_id = parts.next().filter(|channel| !channel.is_empty())?;
        let post_id = parts.next()?;
        let root_id = parts.next().unwrap_or_default();
        Some(Self {
            channel_id: channel_id.to_owned(),
            post_id: post_id.to_owned(),
            root_id: root_id.to_owned(),
        })
    }

    /// The identifier a backend gives a notification about `post_id`,
    /// replying under `root_id` when that is not empty. Server ids never
    /// hold a colon.
    pub fn identifier(channel_id: &str, post_id: &str, root_id: &str) -> String {
        if root_id.is_empty() {
            format!("{channel_id}:{post_id}")
        } else {
            format!("{channel_id}:{post_id}:{root_id}")
        }
    }

    /// The identifier for `notification`.
    pub fn identifier_of(notification: &Notification) -> String {
        Self::identifier(
            &notification.channel_id,
            &notification.post_id,
            &notification.root_id,
        )
    }
}

/// This platform's backend. Clicks on its notifications arrive on
/// `clicked`.
pub fn platform(clicked: async_channel::Sender<Clicked>) -> Box<dyn Notifier> {
    #[cfg(target_os = "macos")]
    if let Some(center) = crate::macos::Center::new(clicked.clone()) {
        return Box::new(center);
    }
    drop(clicked);
    #[cfg(target_os = "macos")]
    return Box::new(crate::macos::DockOnly);
    #[cfg(not(target_os = "macos"))]
    Box::new(Logged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_badge_shows_mentions_then_a_dot_then_nothing() {
        assert_eq!(Badge::new(0, false), Badge::Clear);
        assert_eq!(Badge::new(0, true), Badge::Unread);
        assert_eq!(Badge::new(3, false), Badge::Mentions(3));
        assert_eq!(Badge::new(3, true), Badge::Mentions(3));
        assert_eq!(Badge::Clear.label(), None);
        assert_eq!(Badge::Unread.label().as_deref(), Some("\u{2022}"));
        assert_eq!(Badge::Mentions(7).label().as_deref(), Some("7"));
        assert_eq!(Badge::Mentions(1234).label().as_deref(), Some("1234"));
    }

    #[test]
    fn a_notification_identifier_names_its_channel_and_post() {
        let id = Clicked::identifier("c1", "p1", "");
        assert_eq!(id, "c1:p1");
        assert_eq!(
            Clicked::from_identifier(&id),
            Some(Clicked {
                channel_id: "c1".into(),
                post_id: "p1".into(),
                root_id: String::new(),
            })
        );
        assert_eq!(Clicked::from_identifier("no-colon"), None);
        assert_eq!(Clicked::from_identifier(":p1"), None);
    }

    /// #141: a thread reply's notification carries its root, so the click
    /// can open the thread.
    #[test]
    fn a_reply_identifier_round_trips_its_thread() {
        let clicked = Clicked {
            channel_id: "c1".into(),
            post_id: "reply".into(),
            root_id: "root".into(),
        };
        let id = Clicked::identifier(&clicked.channel_id, &clicked.post_id, &clicked.root_id);
        assert_eq!(id, "c1:reply:root");
        assert_eq!(Clicked::from_identifier(&id), Some(clicked));
        let notification = Notification {
            channel_id: "c1".into(),
            post_id: "reply".into(),
            root_id: "root".into(),
            title: "town".into(),
            body: "Ada: on it".into(),
            mention: false,
            sound: None,
        };
        assert_eq!(Clicked::identifier_of(&notification), id);
    }
}
