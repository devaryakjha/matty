//! The runtime bridge and change-set bus (#34).
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use std::time::{Duration, Instant};

use matty_api::models::Post;
use matty_api::ws::{Posted, WsEvent};
use matty_store::Store;
use matty_sync::{ChangeSet, Command, SyncHandle};

fn posted(id: &str, channel: &str, at: i64) -> Command {
    Command::Event(Box::new(WsEvent::Posted(Box::new(Posted {
        post: Post {
            id: id.into(),
            channel_id: channel.into(),
            user_id: "u".into(),
            create_at: at,
            message: format!("post {id}"),
            ..Post::default()
        },
        ..Posted::default()
    }))))
}

fn engine() -> SyncHandle {
    matty_sync::spawn(Store::open_in_memory().unwrap()).unwrap()
}

/// The next change set, waiting at most a second.
fn next(changes: &async_channel::Receiver<ChangeSet>) -> ChangeSet {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match changes.try_recv() {
            Ok(change) => return change,
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("no change set: {error:?}"),
        }
    }
}

#[test]
fn the_handle_is_send_sync_and_clone() {
    fn check<T: Send + Sync + Clone + 'static>() {}
    check::<SyncHandle>();
}

#[test]
fn an_open_channel_gets_its_posts_and_their_changes() {
    let sync = engine();
    let changes = sync.subscribe();
    sync.send(posted("a", "town", 1));
    sync.send(Command::OpenChannel {
        channel_id: "town".into(),
    });
    let ChangeSet::Posts { channel_id, posts } = next(&changes) else {
        panic!("expected posts");
    };
    assert_eq!(channel_id, "town");
    assert_eq!(posts.len(), 1);

    // A post in a closed channel is stored but not published.
    sync.send(posted("x", "elsewhere", 2));
    sync.send(posted("b", "town", 3));
    let ChangeSet::Posts { channel_id, posts } = next(&changes) else {
        panic!("expected posts");
    };
    assert_eq!(channel_id, "town");
    let ids: Vec<&str> = posts.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["a", "b"]);

    sync.send(Command::Event(Box::new(WsEvent::Typing {
        user_id: "u".into(),
        channel_id: "town".into(),
        parent_id: String::new(),
    })));
    assert!(matches!(next(&changes), ChangeSet::Typing { .. }));

    sync.send(Command::CloseChannel {
        channel_id: "town".into(),
    });
    sync.send(posted("c", "town", 4));
    std::thread::sleep(Duration::from_millis(50));
    assert!(changes.try_recv().is_err());
}

#[test]
fn every_subscriber_hears_every_change() {
    let sync = engine();
    let first = sync.subscribe();
    let second = sync.clone().subscribe();
    sync.send(Command::OpenChannel {
        channel_id: "town".into(),
    });
    assert!(matches!(next(&first), ChangeSet::Posts { .. }));
    assert!(matches!(next(&second), ChangeSet::Posts { .. }));
}

#[test]
fn dropping_the_handle_stops_the_engine_within_a_second() {
    let sync = engine();
    let changes = sync.subscribe();
    let clone = sync.clone();
    drop(sync);
    // One clone keeps it alive.
    assert!(clone.send(Command::OpenChannel {
        channel_id: "town".into(),
    }));
    assert!(matches!(next(&changes), ChangeSet::Posts { .. }));

    let start = Instant::now();
    drop(clone);
    let (done, stopped) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // Returns an error once the engine has dropped its senders.
        let closed = changes.recv_blocking().is_err();
        done.send(closed).unwrap();
    });
    let closed = stopped.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(closed);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn the_receiver_never_waits_on_the_sync_thread_under_load() {
    let sync = engine();
    let changes = sync.subscribe();
    sync.send(Command::OpenChannel {
        channel_id: "town".into(),
    });
    let sender = sync.clone();
    let feeder = std::thread::spawn(move || {
        // 1,000 events a second for one second.
        let start = Instant::now();
        for i in 0..1000 {
            sender.send(posted(&format!("p{i:04}"), "town", i));
            let due = start + Duration::from_millis(i as u64 + 1);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    });

    let mut waits = Vec::new();
    let mut last_seen = String::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while last_seen != "p0999" && Instant::now() < deadline {
        let start = Instant::now();
        let received = changes.try_recv();
        waits.push(start.elapsed());
        match received {
            Ok(ChangeSet::Posts { posts, .. }) => {
                if let Some(post) = posts.last() {
                    last_seen = post.id.clone();
                }
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_micros(200)),
        }
    }
    feeder.join().unwrap();
    assert_eq!(last_seen, "p0999");
    // A preempted test thread can make any one call slow, so judge the p99.
    waits.sort();
    let p99 = waits[waits.len() * 99 / 100];
    assert!(p99 < Duration::from_millis(1), "p99 receive took {p99:?}");
}
