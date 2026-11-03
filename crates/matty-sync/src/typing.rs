//! How often the user's typing is announced.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The shortest gap between two typing notices for one place.
pub(crate) const TYPING_INTERVAL: Duration = Duration::from_secs(5);

/// Lets a typing notice through at most once per [`TYPING_INTERVAL`] for
/// each channel and thread. Takes the time as an argument, so tests can use
/// any clock.
#[derive(Debug, Default)]
pub(crate) struct TypingThrottle {
    last: HashMap<(String, String), Instant>,
}

impl TypingThrottle {
    /// Whether to send a notice for typing in `channel_id` (and the thread
    /// `parent_id`, or empty) at `now`; records it when so.
    pub(crate) fn allow(&mut self, channel_id: &str, parent_id: &str, now: Instant) -> bool {
        // Forget places quiet for a while, so the map stays small.
        self.last
            .retain(|_, sent| now.saturating_duration_since(*sent) < TYPING_INTERVAL);
        let key = (channel_id.to_owned(), parent_id.to_owned());
        if self.last.contains_key(&key) {
            return false;
        }
        self.last.insert(key, now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_notice_per_interval_for_each_place() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut throttle = TypingThrottle::default();
        assert!(throttle.allow("c1", "", at(0)));
        assert!(!throttle.allow("c1", "", at(1)));
        assert!(!throttle.allow("c1", "", at(4_999)));
        // Another channel, or a thread in the same one, has its own clock.
        assert!(throttle.allow("c2", "", at(10)));
        assert!(throttle.allow("c1", "r1", at(20)));
        assert!(throttle.allow("c1", "", at(5_000)));
        assert!(!throttle.allow("c1", "", at(9_999)));
        assert!(throttle.allow("c1", "", at(10_000)));
    }

    #[test]
    fn quiet_places_are_forgotten() {
        let start = Instant::now();
        let mut throttle = TypingThrottle::default();
        for n in 0..100 {
            assert!(throttle.allow(&format!("c{n}"), "", start));
        }
        assert!(throttle.allow("c0", "", start + TYPING_INTERVAL));
        assert_eq!(throttle.last.len(), 1);
    }
}
