//! Which users' statuses to ask the server for, and when (#135).
//!
//! Authors show up a few at a time as posts, threads and search results
//! load. Asking for each batch at once would flood `users/status/ids`, so
//! new ids wait in a window and go together once it closes: at most one
//! request per [`STATUS_WINDOW`]. After that, `status_change` events keep
//! them current.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// How long new ids wait for others before their statuses are asked for.
pub(crate) const STATUS_WINDOW: Duration = Duration::from_millis(300);

/// Collects ids whose status is not known yet and lets them out at most
/// once per [`STATUS_WINDOW`]. Takes the time as an argument, so tests can
/// use any clock.
#[derive(Debug, Default)]
pub(crate) struct StatusBatcher {
    /// Every id asked for, or waiting to be, so none is asked twice.
    known: BTreeSet<String>,
    pending: BTreeSet<String>,
    /// When the open window closes; `None` while nothing waits. A window
    /// opens only after the last one closed, so requests are at least a
    /// window apart.
    due: Option<Instant>,
}

impl StatusBatcher {
    /// Adds `ids` at `now`. Returns when to call [`Self::take`] if this
    /// opened a window; `None` when one is already open or nothing was new.
    pub(crate) fn add<'a>(
        &mut self,
        ids: impl IntoIterator<Item = &'a str>,
        now: Instant,
    ) -> Option<Instant> {
        let mut added = false;
        for id in ids {
            if !id.is_empty() && self.known.insert(id.to_owned()) {
                self.pending.insert(id.to_owned());
                added = true;
            }
        }
        if !added || self.due.is_some() {
            return None;
        }
        let due = now + STATUS_WINDOW;
        self.due = Some(due);
        Some(due)
    }

    /// The ids to ask for at `now`, when the window has closed.
    pub(crate) fn take(&mut self, now: Instant) -> Option<Vec<String>> {
        let due = self.due?;
        if now < due {
            return None;
        }
        self.due = None;
        let ids: Vec<String> = std::mem::take(&mut self.pending).into_iter().collect();
        (!ids.is_empty()).then_some(ids)
    }

    /// Every id asked for so far, for a refresh after a reconnect.
    pub(crate) fn known(&self) -> impl Iterator<Item = &str> {
        self.known.iter().map(String::as_str)
    }

    /// Forgets everything, for a new session.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The statuses in a `get_statuses_by_ids` reply, which maps user id to
/// status.
pub(crate) fn statuses_reply(data: Option<serde_json::Value>) -> Vec<matty_api::models::Status> {
    let Some(serde_json::Value::Object(map)) = data else {
        return Vec::new();
    };
    map.into_iter()
        .filter_map(|(user_id, status)| {
            Some(matty_api::models::Status {
                user_id,
                status: status.as_str()?.to_owned(),
                ..Default::default()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_wait_for_the_window_and_go_together() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut batcher = StatusBatcher::default();
        assert_eq!(batcher.add(["u1", "u2"], at(0)), Some(at(300)));
        // A window is open: the new id joins it.
        assert_eq!(batcher.add(["u3", "u1"], at(100)), None);
        assert_eq!(batcher.take(at(299)), None);
        assert_eq!(
            batcher.take(at(300)),
            Some(vec!["u1".into(), "u2".into(), "u3".into()])
        );
        assert_eq!(batcher.take(at(301)), None);
    }

    #[test]
    fn at_most_one_request_per_window() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut batcher = StatusBatcher::default();
        let mut sent = Vec::new();
        // An id every 10 ms for two seconds, flushing whenever due.
        let mut due = None;
        for tick in 0..200u64 {
            let now = at(tick * 10);
            if due.is_some_and(|due| now >= due) && batcher.take(now).is_some() {
                sent.push(now);
                due = None;
            }
            if let Some(next) = batcher.add([format!("u{tick}").as_str()], now) {
                due = Some(next);
            }
        }
        assert!(sent.len() >= 5, "{sent:?}");
        for pair in sent.windows(2) {
            assert!(pair[1] - pair[0] >= STATUS_WINDOW, "{sent:?}");
        }
    }

    #[test]
    fn known_ids_are_not_asked_again() {
        let start = Instant::now();
        let mut batcher = StatusBatcher::default();
        batcher.add(["u1"], start);
        batcher.take(start + STATUS_WINDOW);
        assert_eq!(batcher.add(["u1", ""], start + STATUS_WINDOW * 2), None);
        assert_eq!(batcher.known().collect::<Vec<_>>(), ["u1"]);
        batcher.clear();
        assert!(batcher.add(["u1"], start).is_some());
    }

    #[test]
    fn a_socket_reply_maps_ids_to_statuses() {
        let reply = serde_json::json!({ "u1": "online", "u2": "dnd", "u3": 7 });
        let mut statuses = statuses_reply(Some(reply));
        statuses.sort_by(|a, b| a.user_id.cmp(&b.user_id));
        let pairs: Vec<_> = statuses
            .iter()
            .map(|s| (s.user_id.as_str(), s.status.as_str()))
            .collect();
        assert_eq!(pairs, [("u1", "online"), ("u2", "dnd")]);
        assert!(statuses_reply(None).is_empty());
    }
}
