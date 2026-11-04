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
            sound = notification.sound,
            "notification"
        );
    }

    fn withdraw(&self, channel_id: &str) {
        tracing::debug!(channel = %channel_id, "notifications withdrawn");
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
    Box::new(Logged)
}

#[cfg(test)]
mod tests {
    use super::*;

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
