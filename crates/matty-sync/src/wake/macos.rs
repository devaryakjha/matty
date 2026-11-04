//! The macOS wake signal: an observer for `NSWorkspaceDidWakeNotification`
//! on the shared workspace's notification center. AppKit posts it on the
//! main thread, which the app's run loop drives; the block only sends a
//! message to the engine.

#![allow(unsafe_code)]

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};
use objc2_foundation::{NSNotification, NSNotificationCenter};
use tokio::sync::mpsc::WeakUnboundedSender;

use crate::engine::Message;

/// The registered observer, removed again on drop.
pub(super) struct Observer {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl Observer {
    pub(super) fn new(messages: WeakUnboundedSender<Message>) -> Self {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let block = RcBlock::new(move |_: NonNull<NSNotification>| {
            super::reconnect(&messages, "the computer woke from sleep");
        });
        // SAFETY: the name is AppKit's own constant, there is no object to
        // filter on, and a nil queue runs the block on the posting thread.
        // The block may run on any thread: it holds only a
        // `WeakUnboundedSender`, which is `Send + Sync`.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidWakeNotification),
                None,
                None,
                &block,
            )
        };
        Self { center, token }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        // SAFETY: the token is the observer `addObserverForName` returned
        // for this center.
        unsafe { self.center.removeObserver(self.token.as_ref()) };
    }
}
