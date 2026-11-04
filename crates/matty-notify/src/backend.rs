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

/// This platform's backend.
pub fn platform() -> Box<dyn Notifier> {
    #[cfg(target_os = "macos")]
    if let Some(center) = crate::macos::Center::new() {
        return Box::new(center);
    }
    Box::new(Logged)
}
