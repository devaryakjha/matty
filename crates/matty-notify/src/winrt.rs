//! Windows notifications as WinRT toasts (#215, PLAN §6.7).
//!
//! Windows files a toast under the process's AppUserModelID, so the backend
//! sets Matty's before the first one. Toasts from an unpackaged build show
//! once the installer's Start Menu shortcut carries the same id; until then
//! Windows drops them and nothing breaks. Clicking a toast reports its post.
//! Each toast is shown on its own short-lived thread, since showing one can
//! block on the notification platform.
// SetCurrentProcessExplicitAppUserModelID is a plain Win32 call taking a
// string the windows crate keeps alive for the call.
#![allow(unsafe_code)]

use async_channel::Sender;
use tauri_winrt_notification::Toast;

use crate::{Badge, Clicked, Notification, Notifier};

/// Matty's AppUserModelID, the same id as its bundle and desktop entry.
pub const APP_USER_MODEL_ID: &str = "dev.arya.matty";

/// Posts WinRT toasts.
pub struct Toasts {
    clicked: Sender<Clicked>,
}

impl Toasts {
    /// Names the process for Windows and starts posting. Clicks arrive on
    /// `clicked`.
    pub fn new(clicked: Sender<Clicked>) -> Self {
        let id = windows::core::HSTRING::from(APP_USER_MODEL_ID);
        // SAFETY: `id` outlives the call, which copies it.
        if let Err(error) =
            unsafe { windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(&id) }
        {
            tracing::warn!(%error, "no AppUserModelID");
        }
        Self { clicked }
    }
}

impl Notifier for Toasts {
    fn show(&self, notification: &Notification) {
        let clicked = self.clicked.clone();
        let target = Clicked {
            channel_id: notification.channel_id.clone(),
            post_id: notification.post_id.clone(),
            root_id: notification.root_id.clone(),
        };
        let (title, body) = (notification.title.clone(), notification.body.clone());
        let shown = std::thread::Builder::new()
            .name("toast".into())
            .spawn(move || {
                // Matty plays its own sounds.
                let toast = Toast::new(APP_USER_MODEL_ID)
                    .title(&title)
                    .text1(&body)
                    .sound(None)
                    .on_activated(move |_| {
                        let _ = clicked.try_send(target.clone());
                        Ok(())
                    });
                if let Err(error) = toast.show() {
                    tracing::debug!(%error, "toast");
                }
            });
        if let Err(error) = shown {
            tracing::warn!(%error, "no toast thread");
        }
    }

    fn withdraw(&self, channel_id: &str) {
        // Removing a channel's toasts from Action Center needs them tagged
        // by channel, which the toast crate does not expose yet.
        tracing::debug!(channel = %channel_id, "toasts stay in Action Center");
    }

    fn set_badge(&self, badge: Badge) {
        // The taskbar overlay needs the window handle, from the app (#215).
        tracing::debug!(?badge, "badge");
    }
}
