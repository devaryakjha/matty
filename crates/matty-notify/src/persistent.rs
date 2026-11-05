//! Persistent notifications (#220): an urgent post that mentions the user
//! and asks for them notifies again every few minutes, up to the server's
//! count, until the user reads or acknowledges it. Plain bookkeeping; the
//! app ticks it and shows what is due.

use crate::Notification;

/// One post still repeating.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    notification: Notification,
    /// When it shows next (ms since the epoch).
    next_at: i64,
    /// Repeats still to come.
    left: u64,
    interval_ms: i64,
}

/// The posts whose notifications repeat.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Persistent {
    entries: Vec<Entry>,
}

impl Persistent {
    /// Starts repeating `notification`, just shown at `now` (ms): again
    /// every `interval_minutes`, `max_count` more times at most. Starting
    /// a post already repeating changes nothing.
    pub fn start(
        &mut self,
        notification: Notification,
        now: i64,
        interval_minutes: u64,
        max_count: u64,
    ) {
        if max_count == 0
            || self
                .entries
                .iter()
                .any(|entry| entry.notification.post_id == notification.post_id)
        {
            return;
        }
        let interval_ms = i64::try_from(interval_minutes.max(1))
            .unwrap_or(i64::MAX / 60_000)
            .saturating_mul(60_000);
        self.entries.push(Entry {
            notification,
            next_at: now.saturating_add(interval_ms),
            left: max_count,
            interval_ms,
        });
    }

    /// Stops a post's repeats: the user acknowledged it, or it is gone.
    pub fn stop(&mut self, post_id: &str) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|entry| entry.notification.post_id != post_id);
        before != self.entries.len()
    }

    /// Stops the repeats of every post in a channel the user read.
    pub fn stop_channel(&mut self, channel_id: &str) {
        self.entries
            .retain(|entry| entry.notification.channel_id != channel_id);
    }

    /// The notifications due at `now`, each counted; a post drops out
    /// after its last repeat.
    pub fn due(&mut self, now: i64) -> Vec<Notification> {
        let mut due = Vec::new();
        for entry in &mut self.entries {
            if entry.next_at <= now && entry.left > 0 {
                due.push(entry.notification.clone());
                entry.left -= 1;
                entry.next_at = now.saturating_add(entry.interval_ms);
            }
        }
        self.entries.retain(|entry| entry.left > 0);
        due
    }

    /// Whether nothing repeats.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(post: &str, channel: &str) -> Notification {
        Notification {
            title: "Town Square".into(),
            body: "Ada: @me deploy is down".into(),
            channel_id: channel.into(),
            post_id: post.into(),
            root_id: String::new(),
            mention: true,
            sound: None,
        }
    }

    const MINUTE: i64 = 60_000;

    #[test]
    fn repeats_at_the_interval_up_to_the_count() {
        let mut repeats = Persistent::default();
        repeats.start(notification("p1", "c1"), 0, 5, 2);
        assert!(repeats.due(4 * MINUTE).is_empty());
        assert_eq!(repeats.due(5 * MINUTE).len(), 1);
        assert!(repeats.due(6 * MINUTE).is_empty());
        assert_eq!(repeats.due(10 * MINUTE).len(), 1);
        assert!(repeats.is_empty(), "the count is spent");
        assert!(repeats.due(60 * MINUTE).is_empty());
    }

    #[test]
    fn acknowledging_or_reading_stops_it() {
        let mut repeats = Persistent::default();
        repeats.start(notification("p1", "c1"), 0, 5, 6);
        repeats.start(notification("p1", "c1"), 0, 5, 6);
        repeats.start(notification("p2", "c2"), 0, 5, 6);
        repeats.start(notification("p3", "c2"), 0, 5, 6);
        assert_eq!(repeats.due(5 * MINUTE).len(), 3, "a post repeats once");
        assert!(repeats.stop("p1"));
        assert!(!repeats.stop("p1"));
        repeats.stop_channel("c2");
        assert!(repeats.is_empty());
    }

    #[test]
    fn no_count_means_no_repeats() {
        let mut repeats = Persistent::default();
        repeats.start(notification("p1", "c1"), 0, 5, 0);
        assert!(repeats.is_empty());
    }
}
