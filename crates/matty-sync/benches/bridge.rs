//! `cargo bench -p matty-sync --bench bridge`: the in-tree half of spike S2
//! (#34, #58). Feeds 1,000 `posted` events a second for five seconds into an
//! open channel, through the store, to a receiver that only polls. Fails if
//! the p99 from send to a snapshot holding the post passes one 60 Hz frame,
//! or a receive call's p99 passes 1 ms.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use matty_api::models::Post;
use matty_api::ws::{Posted, WsEvent};
use matty_store::Store;
use matty_sync::{ChangeSet, Command};

const RATE: u64 = 1000;
const SECONDS: u64 = 5;
const FRAME: Duration = Duration::from_micros(16_667);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("bench.sqlite"))?;
    let sync = matty_sync::spawn(store)?;
    let changes = sync.subscribe();
    sync.send(Command::OpenChannel {
        channel_id: "town".into(),
    });

    let total = RATE * SECONDS;
    let origin = Instant::now();
    let sent: Arc<Vec<AtomicU64>> = Arc::new((0..total).map(|_| AtomicU64::new(0)).collect());
    let feeder = {
        let sync = sync.clone();
        let sent = Arc::clone(&sent);
        std::thread::spawn(move || {
            for i in 0..total {
                let due = origin + Duration::from_micros(i * 1_000_000 / RATE);
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
                sent[i as usize].store(origin.elapsed().as_nanos() as u64, Ordering::Relaxed);
                sync.send(Command::Event(Box::new(WsEvent::Posted(Box::new(
                    Posted {
                        post: Post {
                            id: format!("p{i:06}"),
                            channel_id: "town".into(),
                            user_id: "u".into(),
                            create_at: i as i64,
                            message: format!("message {i}"),
                            ..Post::default()
                        },
                        ..Posted::default()
                    },
                )))));
            }
        })
    };

    let mut latencies = Vec::new();
    let mut waits = Vec::new();
    let mut newest: i64 = -1;
    let deadline = origin + Duration::from_secs(SECONDS + 10);
    while newest < total as i64 - 1 && Instant::now() < deadline {
        let start = Instant::now();
        let received = changes.try_recv();
        waits.push(start.elapsed());
        match received {
            Ok(ChangeSet::Posts { posts, .. }) => {
                let now = origin.elapsed().as_nanos() as u64;
                for post in posts.iter().filter(|p| p.create_at > newest) {
                    let at = sent[post.create_at as usize].load(Ordering::Relaxed);
                    latencies.push(Duration::from_nanos(now.saturating_sub(at)));
                }
                if let Some(last) = posts.last() {
                    newest = newest.max(last.create_at);
                }
            }
            Ok(_) => {}
            // A UI would wait for the next frame; poll a little faster.
            Err(_) => std::thread::sleep(Duration::from_micros(500)),
        }
    }
    feeder.join().map_err(|_| "feeder panicked")?;
    if newest != total as i64 - 1 {
        return Err(format!("only saw posts up to {newest}").into());
    }
    latencies.sort();
    waits.sort();
    let p = |v: &[Duration], q: usize| v[(v.len() * q / 100).min(v.len() - 1)];
    let (p50, p99, max) = (
        p(&latencies, 50),
        p(&latencies, 99),
        latencies[latencies.len() - 1],
    );
    let wait99 = p(&waits, 99);
    eprintln!(
        "{RATE} events/s for {SECONDS} s: send to snapshot p50 {p50:?}, p99 {p99:?}, max {max:?}; \
         receive p99 {wait99:?}, max {:?}",
        waits[waits.len() - 1]
    );
    if p99 > FRAME {
        return Err(format!("p99 {p99:?} is over one frame").into());
    }
    if wait99 > Duration::from_millis(1) {
        return Err(format!("receive p99 {wait99:?} is over 1 ms").into());
    }
    Ok(())
}
