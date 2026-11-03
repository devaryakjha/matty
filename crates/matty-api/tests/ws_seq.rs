//! Property tests for sequence handling: duplicated, reordered and missing
//! `seq`s give each event at most once, in order, and a gap for exactly the
//! `seq`s that never arrived (PLAN §11).

use std::collections::BTreeSet;

use matty_api::ws::{SeqTracker, SeqVerdict};
use proptest::prelude::*;

/// What the server meant to send, and what arrived.
#[derive(Clone, Debug)]
struct Arrival {
    sent: Vec<i64>,
    arrived: Vec<i64>,
}

/// 1..=n, then with some `seq`s dropped, some duplicated, and some moved
/// later than they belong.
fn arrivals() -> impl Strategy<Value = Arrival> {
    (1usize..120).prop_flat_map(|n| {
        let ops = proptest::collection::vec((0u8..10, 0usize..8), n);
        ops.prop_map(move |ops| {
            let sent: Vec<i64> = (1..=n as i64).collect();
            let mut arrived: Vec<i64> = Vec::new();
            let mut late: Vec<(usize, i64)> = Vec::new();
            for (seq, (op, shift)) in sent.iter().zip(ops) {
                match op {
                    0 => {}                                        // dropped
                    1 => arrived.extend([*seq, *seq]),             // duplicated
                    2 => late.push((arrived.len() + shift, *seq)), // delayed
                    _ => arrived.push(*seq),
                }
            }
            // Insert delayed events from the back so positions stay valid.
            late.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
            for (at, seq) in late {
                let at = at.min(arrived.len());
                arrived.insert(at, seq);
            }
            Arrival { sent, arrived }
        })
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn delivers_once_in_order_and_reports_every_skipped_seq(arrival in arrivals()) {
        let mut tracker = SeqTracker::default();
        let mut delivered: Vec<i64> = Vec::new();
        let mut gaps: Vec<std::ops::Range<i64>> = Vec::new();
        let mut seen = BTreeSet::new();
        for &seq in &arrival.arrived {
            let first_time = seen.insert(seq);
            match tracker.accept(seq) {
                SeqVerdict::Deliver { gap } => {
                    prop_assert!(first_time, "seq {seq} delivered twice");
                    if let Some(gap) = gap {
                        prop_assert!(!gap.is_empty());
                        gaps.push(gap);
                    }
                    delivered.push(seq);
                }
                SeqVerdict::Stale => {}
            }
        }

        // At most once, in seq order.
        prop_assert!(delivered.windows(2).all(|w| w[0] < w[1]), "{delivered:?}");
        // Every seq up to the last delivered is delivered or in exactly one gap.
        let last = delivered.last().copied().unwrap_or(0);
        let mut covered: Vec<i64> = delivered.clone();
        for gap in &gaps {
            covered.extend(gap.clone());
        }
        covered.sort_unstable();
        prop_assert_eq!(covered, (1..=last).collect::<Vec<_>>());
        // A gap never covers a seq that was delivered (so never a duplicate).
        for gap in &gaps {
            prop_assert!(delivered.iter().all(|seq| !gap.contains(seq)));
        }
        // Only skipped seqs are gaps: each one arrived late or not at all
        // before the event that jumped past it.
        let arrived: BTreeSet<i64> = arrival.arrived.iter().copied().collect();
        let missing: BTreeSet<i64> = arrival.sent.iter().copied().filter(|s| !arrived.contains(s)).collect();
        for seq in &missing {
            if *seq < last {
                prop_assert!(gaps.iter().any(|g| g.contains(seq)), "missing {seq} not reported");
            }
        }
        prop_assert_eq!(tracker.next(), last + 1);
    }

    #[test]
    fn duplicates_alone_never_make_a_gap(n in 1i64..200, dups in proptest::collection::vec(0usize..200, 0..50)) {
        let mut arrived: Vec<i64> = (1..=n).collect();
        for d in dups {
            let at = d % arrived.len();
            // Repeat an already-sent seq right after itself or later.
            let seq = arrived[at];
            arrived.insert(at + 1, seq);
        }
        let mut tracker = SeqTracker::default();
        let mut delivered = 0;
        for seq in arrived {
            match tracker.accept(seq) {
                SeqVerdict::Deliver { gap } => {
                    prop_assert_eq!(gap, None);
                    delivered += 1;
                }
                SeqVerdict::Stale => {}
            }
        }
        prop_assert_eq!(delivered, n);
    }
}

#[test]
fn a_resumed_tracker_continues_from_its_start() {
    let mut tracker = SeqTracker::starting_at(41);
    assert_eq!(tracker.accept(40), SeqVerdict::Stale);
    assert_eq!(tracker.accept(41), SeqVerdict::Deliver { gap: None });
    assert_eq!(
        tracker.accept(44),
        SeqVerdict::Deliver { gap: Some(42..44) }
    );
    assert_eq!(
        tracker.accept(43),
        SeqVerdict::Stale,
        "late, already reported"
    );
    assert_eq!(tracker.next(), 45);
}
