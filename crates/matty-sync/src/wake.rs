//! Reconnects the moment the computer wakes from sleep (#165).
//!
//! The socket notices a sleep on its own on every platform, because the
//! wall clock runs ahead of the monotonic clock, which stops while the
//! computer sleeps; but only at its next check, up to five seconds after
//! waking. On macOS, AppKit says so at once (`NSWorkspaceDidWakeNotification`),
//! so the socket redials without waiting for that check. Elsewhere the
//! socket's own check is the fallback.
//!
//! A wake that also brings the network back may ask twice, here and from
//! the network watch; the socket folds requests that arrive while it dials
//! into that one dial.

use tokio::sync::mpsc::WeakUnboundedSender;

use crate::Command;
use crate::engine::Message;

#[cfg(target_os = "macos")]
mod macos;

/// Watches for wake from sleep while it lives. The watch only holds a weak
/// sender, so it never keeps the engine running.
pub(crate) struct WakeWatch {
    #[cfg(target_os = "macos")]
    _platform: macos::Observer,
}

impl WakeWatch {
    /// Starts watching, on platforms that say when they wake.
    pub(crate) fn start(messages: WeakUnboundedSender<Message>) -> Self {
        #[cfg(target_os = "macos")]
        {
            Self {
                _platform: macos::Observer::new(messages),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = messages;
            Self {}
        }
    }
}

/// Asks the engine to reconnect now, skipping the socket's backoff wait.
/// False once the engine is gone.
pub(crate) fn reconnect(messages: &WeakUnboundedSender<Message>, why: &str) -> bool {
    tracing::info!("{why}; reconnecting");
    messages
        .upgrade()
        .is_some_and(|m| m.send(Message::Command(Command::Reconnect)).is_ok())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use tokio::sync::mpsc;

    #[test]
    fn a_wake_asks_the_engine_to_reconnect_until_it_is_gone() {
        let (messages, mut inbox) = mpsc::unbounded_channel();
        let weak = messages.downgrade();
        let _watch = WakeWatch::start(weak.clone());

        assert!(reconnect(&weak, "the computer woke from sleep"));
        assert!(matches!(
            inbox.try_recv().unwrap(),
            Message::Command(Command::Reconnect)
        ));

        // The watch holds no strong sender: dropping the engine's ends it.
        drop(messages);
        assert!(!reconnect(&weak, "the computer woke from sleep"));
    }
}
