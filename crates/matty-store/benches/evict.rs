//! `cargo bench -p matty-store --bench evict`: trims 100,000 cached posts in
//! 50 channels down to 200 each and fails past 2 s (#31).

use std::time::{Duration, Instant};

use matty_api::models::Post;
use matty_store::evict::{self, Keep};
use matty_store::media::MediaCache;
use matty_store::{Store, posts};

const POSTS: usize = 100_000;
const CHANNELS: usize = 50;
const BUDGET: Duration = Duration::from_secs(2);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut store = Store::open(&dir.path().join("bench.sqlite"))?;
    let batch: Vec<Post> = (0..POSTS)
        .map(|i| Post {
            id: format!("p{i:06}"),
            channel_id: format!("c{:02}", i % CHANNELS),
            user_id: "u".into(),
            create_at: i as i64,
            message: format!("message number {i}"),
            ..Post::default()
        })
        .collect();
    let tx = store.conn_mut().transaction()?;
    posts::insert_posts(&tx, &batch)?;
    tx.commit()?;

    let media = MediaCache::new(dir.path().join("media"));
    let start = Instant::now();
    let evicted = evict::evict_to(store.conn(), &media, 0, &Keep::default())?;
    let elapsed = start.elapsed();
    eprintln!(
        "evict_to, {POSTS} posts: {elapsed:?}, dropped {}",
        evicted.posts
    );
    if evicted.posts != POSTS - CHANNELS * evict::KEEP_POSTS_PER_CHANNEL {
        return Err(format!("dropped {} posts", evicted.posts).into());
    }
    if elapsed > BUDGET {
        return Err(format!("{elapsed:?} is over the {BUDGET:?} budget").into());
    }
    Ok(())
}
