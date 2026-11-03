//! `cargo bench -p matty-store --bench posts`: inserts 10,000 posts with a
//! reaction each in one transaction, into a database on disk, and fails if
//! the median of five runs passes 500 ms (#29).

use std::time::{Duration, Instant};

use matty_api::models::{Post, PostMetadata, Reaction};
use matty_store::{Store, posts};

const POSTS: usize = 10_000;
const BUDGET: Duration = Duration::from_millis(500);

fn batch(run: usize) -> Vec<Post> {
    (0..POSTS)
        .map(|i| {
            let id = format!("r{run}p{i:05}");
            Post {
                id: id.clone(),
                channel_id: format!("c{}", i % 20),
                user_id: format!("u{}", i % 50),
                create_at: i as i64,
                message: format!("message number {i} with a few words in it"),
                metadata: PostMetadata {
                    reactions: vec![Reaction {
                        user_id: "u1".into(),
                        post_id: id,
                        emoji_name: "+1".into(),
                        create_at: i as i64,
                    }],
                    ..PostMetadata::default()
                },
                ..Post::default()
            }
        })
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut store = Store::open(&dir.path().join("bench.sqlite"))?;
    let mut times = Vec::new();
    for run in 0..5 {
        let posts = batch(run);
        let start = Instant::now();
        let tx = store.conn_mut().transaction()?;
        posts::insert_posts(&tx, &posts)?;
        tx.commit()?;
        times.push(start.elapsed());
    }
    times.sort();
    let median = times[times.len() / 2];
    eprintln!("insert_posts, {POSTS} posts in one transaction: median {median:?}");
    if median > BUDGET {
        return Err(format!("median {median:?} is over the {BUDGET:?} budget").into());
    }
    Ok(())
}
