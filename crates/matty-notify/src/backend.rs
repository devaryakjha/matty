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

/// The user clicked a notification: the channel and post it was about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clicked {
    /// The channel the notification was about.
    pub channel_id: String,
    /// The post it showed.
    pub post_id: String,
}

impl Clicked {
    /// Reads a notification's identifier, `channel:post` as the backends
    /// set it.
    pub fn from_identifier(id: &str) -> Option<Self> {
        let (channel_id, post_id) = id.split_once(':')?;
        (!channel_id.is_empty()).then(|| Self {
            channel_id: channel_id.to_owned(),
            post_id: post_id.to_owned(),
        })
    }

    /// The identifier a backend gives a notification about `post_id`.
    pub fn identifier(channel_id: &str, post_id: &str) -> String {
        format!("{channel_id}:{post_id}")
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
        let id = Clicked::identifier("c1", "p1");
        assert_eq!(
            Clicked::from_identifier(&id),
            Some(Clicked {
                channel_id: "c1".into(),
                post_id: "p1".into(),
            })
        );
        assert_eq!(Clicked::from_identifier("no-colon"), None);
        assert_eq!(Clicked::from_identifier(":p1"), None);
    }
}
