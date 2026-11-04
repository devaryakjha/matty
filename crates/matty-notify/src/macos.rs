//! The macOS backend: UNUserNotificationCenter (#139). It needs the app to
//! run from a bundle with an identifier; a bare binary, as from
//! `cargo run`, has none and the center would crash, so [`Center::new`]
//! returns `None` and the caller falls back to logging.

use std::collections::HashMap;
use std::sync::Mutex;

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_foundation::{NSArray, NSBundle, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
    UNNotificationSound, UNUserNotificationCenter,
};

use crate::{Notification, Notifier};

/// Posts through the user notification center, one thread per channel.
pub struct Center {
    /// What is on screen per channel, to take it away when the channel is
    /// read.
    shown: Mutex<HashMap<String, Vec<String>>>,
}

impl Center {
    /// The center, once permission is asked for; `None` outside a bundle.
    pub fn new() -> Option<Self> {
        let bundle = NSBundle::mainBundle();
        if bundle.bundleIdentifier().is_none() {
            tracing::warn!(
                "notifications are off: Matty is not running from an app bundle, \
                 so macOS will not show them"
            );
            return None;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let answered = RcBlock::new(|granted: Bool, error: *mut NSError| {
            if !error.is_null() {
                tracing::warn!("the notification permission request failed");
            } else if !granted.as_bool() {
                tracing::info!("notifications are turned off for Matty in System Settings");
            }
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert
                | UNAuthorizationOptions::Sound
                | UNAuthorizationOptions::Badge,
            &answered,
        );
        Some(Self {
            shown: Mutex::new(HashMap::new()),
        })
    }
}

impl Notifier for Center {
    fn show(&self, notification: &Notification) {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notification.title));
        content.setBody(&NSString::from_str(&notification.body));
        content.setThreadIdentifier(&NSString::from_str(&notification.channel_id));
        if notification.sound {
            content.setSound(Some(&UNNotificationSound::defaultSound()));
        }
        let id = format!("{}:{}", notification.channel_id, notification.post_id);
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&id),
            &content,
            None,
        );
        let added = RcBlock::new(|error: *mut NSError| {
            if !error.is_null() {
                tracing::warn!("a notification could not be shown");
            }
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .addNotificationRequest_withCompletionHandler(&request, Some(&added));
        self.shown
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(notification.channel_id.clone())
            .or_default()
            .push(id);
    }

    fn withdraw(&self, channel_id: &str) {
        let Some(ids) = self
            .shown
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(channel_id)
        else {
            return;
        };
        let ids: Vec<_> = ids.iter().map(|id| NSString::from_str(id)).collect();
        let ids = NSArray::from_retained_slice(&ids);
        let center = UNUserNotificationCenter::currentNotificationCenter();
        center.removeDeliveredNotificationsWithIdentifiers(&ids);
        center.removePendingNotificationRequestsWithIdentifiers(&ids);
    }
}
