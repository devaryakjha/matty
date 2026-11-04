//! The macOS backend: UNUserNotificationCenter (#139). It needs the app to
//! run from a bundle with an identifier; a bare binary, as from
//! `cargo run`, has none and the center would crash, so [`Center::new`]
//! returns `None` and the caller falls back to logging.
//!
//! Clicks come back through a delegate object, which is an Objective-C
//! class of our own: defining one is the module's only `unsafe`.
//!
//! The dock badge (#141) needs no bundle, so it is set either way, and so
//! is the sound (#142): NSSound plays the system sound that stands in for
//! the user's Mattermost sound, since the notification center can only
//! play the default sound or files shipped inside the bundle.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::sync::Mutex;

use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, MainThreadMarker, define_class, msg_send};
use objc2_app_kit::{NSApplication, NSSound};
use objc2_foundation::{NSArray, NSBundle, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

use crate::backend::{Badge, Clicked, Logged};
use crate::{Notification, Notifier};

/// What the delegate holds: where clicks go.
struct DelegateIvars {
    clicked: async_channel::Sender<Clicked>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, the class adds no
    // `Drop`, and its ivars are `Send + Sync`, so any thread may own it.
    #[unsafe(super(NSObject))]
    #[thread_kind = AllocAnyThread]
    #[name = "MattyNotificationDelegate"]
    #[ivars = DelegateIvars]
    struct Delegate;

    // SAFETY: NSObjectProtocol has no requirements beyond NSObject's.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: each method's signature matches the protocol's declaration in
    // objc2-user-notifications, and each completion handler is called once.
    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        /// A click on a notification, or on its default action.
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &DynBlock<dyn Fn()>,
        ) {
            let id = response.notification().request().identifier().to_string();
            if let Some(clicked) = Clicked::from_identifier(&id) {
                let _ = self.ivars().clicked.try_send(clicked);
            }
            completion_handler.call(());
        }

        /// Matty is frontmost: the policy already decided this one should
        /// show, so macOS should not hold it back.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion_handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }
    }
);

impl Delegate {
    fn new(clicked: async_channel::Sender<Clicked>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { clicked });
        // SAFETY: NSObject's `init` on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

/// Posts through the user notification center, one thread per channel.
pub struct Center {
    /// What is on screen per channel, to take it away when the channel is
    /// read.
    shown: Mutex<HashMap<String, Vec<String>>>,
    /// The center holds its delegate weakly; this keeps it alive.
    _delegate: Retained<Delegate>,
}

impl Center {
    /// The center, once permission is asked for; `None` outside a bundle.
    /// Clicks on its notifications arrive on `clicked`.
    pub fn new(clicked: async_channel::Sender<Clicked>) -> Option<Self> {
        let bundle = NSBundle::mainBundle();
        if bundle.bundleIdentifier().is_none() {
            tracing::warn!(
                "notifications are off: Matty is not running from an app bundle, \
                 so macOS will not show them"
            );
            return None;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = Delegate::new(clicked);
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
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
            _delegate: delegate,
        })
    }
}

impl Notifier for Center {
    fn show(&self, notification: &Notification) {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notification.title));
        content.setBody(&NSString::from_str(&notification.body));
        content.setThreadIdentifier(&NSString::from_str(&notification.channel_id));
        if let Some(sound) = notification.sound {
            play(sound);
        }
        let id = Clicked::identifier_of(notification);
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

    fn set_badge(&self, badge: Badge) {
        set_dock_badge(badge);
    }
}

/// Outside a bundle: notifications are logged, the dock badge still shows.
pub struct DockOnly;

impl Notifier for DockOnly {
    fn show(&self, notification: &Notification) {
        Logged.show(notification);
        if let Some(sound) = notification.sound {
            play(sound);
        }
    }

    fn withdraw(&self, channel_id: &str) {
        Logged.withdraw(channel_id);
    }

    fn set_badge(&self, badge: Badge) {
        set_dock_badge(badge);
    }
}

/// Plays the system sound standing in for `sound`, restarting it when it is
/// already playing. A missing system sound is logged and skipped.
pub(crate) fn play(sound: crate::Sound) {
    let Some(player) = NSSound::soundNamed(&NSString::from_str(sound.system_name())) else {
        tracing::warn!(?sound, "the system sound is missing");
        return;
    };
    player.stop();
    player.play();
}

/// Puts `badge` on the dock icon. AppKit allows this only on the main
/// thread, where the app calls it; elsewhere it is skipped with a warning.
fn set_dock_badge(badge: Badge) {
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::warn!("the dock badge can only be set on the main thread");
        return;
    };
    let label = badge.label().map(|label| NSString::from_str(&label));
    NSApplication::sharedApplication(mtm)
        .dockTile()
        .setBadgeLabel(label.as_deref());
}
