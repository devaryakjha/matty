//! Reconnects as soon as the network changes (#165): a new address on a
//! network interface, as when Wi-Fi comes back or the Mac joins another
//! network, asks the socket to reconnect now instead of waiting out its
//! backoff. Waking from sleep is caught by the socket itself.

use std::net::IpAddr;
use std::time::Duration;

use futures_util::{Stream, StreamExt as _};
use if_watch::IfEvent;
use tokio::sync::mpsc::WeakUnboundedSender;

use crate::Command;
use crate::engine::Message;

/// Addresses listed while the watcher starts are the network as it is,
/// not a change.
const SETTLE: Duration = Duration::from_secs(2);
/// A network coming up adds several addresses in a burst; one reconnect
/// follows once it has been quiet this long, so a flapping network does
/// not stack reconnects.
const QUIET: Duration = Duration::from_millis(750);

/// Watches the machine's network interfaces until the engine is gone. The
/// sender is weak so the watch never keeps the engine running.
pub(crate) async fn watch(messages: WeakUnboundedSender<Message>) {
    match if_watch::tokio::IfWatcher::new() {
        Ok(watcher) => reconnect_on_change(watcher, messages).await,
        Err(error) => tracing::debug!(%error, "cannot watch network changes"),
    }
}

/// Sends [`Command::Reconnect`] after each burst of new addresses.
pub(crate) async fn reconnect_on_change<S>(events: S, messages: WeakUnboundedSender<Message>)
where
    S: Stream<Item = std::io::Result<IfEvent>> + Unpin,
{
    let mut events = events.fuse();
    let settled = tokio::time::Instant::now() + SETTLE;
    let mut pending = false;
    loop {
        let event = if pending {
            match tokio::time::timeout(QUIET, events.next()).await {
                Ok(event) => event,
                Err(_) => {
                    pending = false;
                    tracing::info!("the network changed; reconnecting");
                    let sent = messages
                        .upgrade()
                        .is_some_and(|m| m.send(Message::Command(Command::Reconnect)).is_ok());
                    if !sent {
                        return;
                    }
                    continue;
                }
            }
        } else {
            events.next().await
        };
        match event {
            None => return,
            Some(Err(error)) => tracing::debug!(%error, "network watch error"),
            Some(Ok(IfEvent::Up(net))) => {
                if tokio::time::Instant::now() >= settled && reaches_out(net.addr()) {
                    pending = true;
                }
            }
            // Losing an address alone leaves nothing to reconnect over.
            Some(Ok(IfEvent::Down(_))) => {}
        }
    }
}

/// Whether an address can reach a server: not loopback or link-local.
fn reaches_out(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified(),
        IpAddr::V6(v6) => !v6.is_loopback() && !v6.is_unspecified() && !v6.is_unicast_link_local(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use tokio::sync::mpsc;

    fn up(addr: &str) -> std::io::Result<IfEvent> {
        Ok(IfEvent::Up(addr.parse().unwrap()))
    }

    fn reconnects(messages: &mut mpsc::UnboundedReceiver<Message>) -> usize {
        std::iter::from_fn(|| messages.try_recv().ok())
            .filter(|m| matches!(m, Message::Command(Command::Reconnect)))
            .count()
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_of_new_addresses_reconnects_once_and_startup_does_not() {
        let (events, rx) = mpsc::unbounded_channel();
        let (messages, mut out) = mpsc::unbounded_channel();
        let watcher = tokio::spawn(reconnect_on_change(
            tokio_stream_from(rx),
            messages.downgrade(),
        ));
        // The addresses the machine already has.
        events.send(up("192.168.1.5/24")).unwrap();
        events.send(up("127.0.0.1/8")).unwrap();
        tokio::time::sleep(SETTLE * 2).await;
        assert_eq!(reconnects(&mut out), 0, "startup is not a change");

        // Wi-Fi comes back: several addresses at once, one reconnect.
        events.send(up("10.0.0.7/24")).unwrap();
        events.send(up("fe80::1/64")).unwrap();
        events.send(up("2001:db8::7/64")).unwrap();
        tokio::time::sleep(QUIET / 2).await;
        assert_eq!(reconnects(&mut out), 0, "waits for the burst to end");
        tokio::time::sleep(QUIET * 2).await;
        assert_eq!(reconnects(&mut out), 1);

        // Only loopback and link-local: nothing to reconnect over.
        events.send(up("fe80::2/64")).unwrap();
        events
            .send(Ok(IfEvent::Down("10.0.0.7/24".parse().unwrap())))
            .unwrap();
        tokio::time::sleep(QUIET * 2).await;
        assert_eq!(reconnects(&mut out), 0);

        drop(events);
        watcher.await.unwrap();
        drop(messages);
    }

    fn tokio_stream_from<T>(mut rx: mpsc::UnboundedReceiver<T>) -> impl Stream<Item = T> + Unpin {
        Box::pin(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
    }
}
