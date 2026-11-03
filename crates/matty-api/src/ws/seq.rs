//! Event sequence numbers: each event delivered at most once and in order,
//! with a gap reported for every `seq` that never arrived.

use std::ops::Range;

/// What to do with an event, by its `seq`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeqVerdict {
    /// Deliver it. `gap` names the `seq`s skipped to reach it, if any.
    Deliver {
        /// Missing `seq`s just before this event.
        gap: Option<Range<i64>>,
    },
    /// Already delivered, or older than one that was: drop it.
    Stale,
}

/// Tracks the next `seq` a connection should deliver.
///
/// An event below the next `seq` is [`SeqVerdict::Stale`]: a duplicate, a
/// replay, or one that arrived after a later one (whose gap was already
/// reported). An event above it is delivered after a gap. The sync engine
/// decides whether a gap needs a resync.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeqTracker {
    next: i64,
}

impl Default for SeqTracker {
    /// A new connection: `hello` is 0, events count from 1.
    fn default() -> Self {
        Self::starting_at(1)
    }
}

impl SeqTracker {
    /// Expects `next` first, for a resumed connection.
    pub fn starting_at(next: i64) -> Self {
        Self { next }
    }

    /// The `seq` expected next.
    pub fn next(&self) -> i64 {
        self.next
    }

    /// Judges the event `seq` and, if it is delivered, moves past it.
    pub fn accept(&mut self, seq: i64) -> SeqVerdict {
        if seq < self.next {
            return SeqVerdict::Stale;
        }
        let gap = (seq > self.next).then_some(self.next..seq);
        self.next = seq + 1;
        SeqVerdict::Deliver { gap }
    }
}
