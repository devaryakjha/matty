//! `cargo bench -p matty-store --bench sidebar`: times `sidebar_snapshot` for
//! a team with 50 channels and fails if the median passes 5 ms (#28).

use std::time::{Duration, Instant};

use matty_api::models::{Channel, ChannelMember, SidebarCategories, SidebarCategory, Team, User};
use matty_store::Store;
use matty_store::sidebar::{self, UnreadMode};

const ME: &str = "me";
const CHANNELS: usize = 50;
const BUDGET: Duration = Duration::from_millis(5);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = Store::open_in_memory()?;
    let conn = store.conn();
    sidebar::upsert_teams(
        conn,
        &[Team {
            id: "t".into(),
            name: "t".into(),
            display_name: "Team".into(),
            ..Team::default()
        }],
    )?;
    let mut channels = Vec::new();
    let mut members = Vec::new();
    let mut users = Vec::new();
    for i in 0..CHANNELS {
        let dm = i % 5 == 0;
        let id = format!("c{i:02}");
        users.push(User {
            id: format!("u{i:02}"),
            username: format!("user{i:02}"),
            ..User::default()
        });
        channels.push(Channel {
            id: id.clone(),
            team_id: if dm { String::new() } else { "t".into() },
            kind: if dm { "D".into() } else { "O".into() },
            name: if dm {
                format!("{ME}__u{i:02}")
            } else {
                id.clone()
            },
            display_name: format!("Channel {i}"),
            total_msg_count: 100,
            total_msg_count_root: 60,
            last_post_at: i as i64,
            ..Channel::default()
        });
        members.push(ChannelMember {
            channel_id: id,
            user_id: ME.into(),
            msg_count: 90,
            msg_count_root: 55,
            mention_count: (i % 3) as i64,
            ..ChannelMember::default()
        });
    }
    sidebar::upsert_users(conn, &users)?;
    sidebar::upsert_channels(conn, &channels)?;
    sidebar::upsert_channel_members(conn, &members)?;
    let category = |id: &str, kind: &str, range: std::ops::Range<usize>| SidebarCategory {
        id: id.into(),
        team_id: "t".into(),
        kind: kind.into(),
        display_name: id.into(),
        channel_ids: channels[range].iter().map(|c| c.id.clone()).collect(),
        ..SidebarCategory::default()
    };
    sidebar::replace_categories(
        conn,
        "t",
        &SidebarCategories {
            categories: vec![
                category("favorites", "favorites", 0..5),
                category("channels", "channels", 5..40),
                category("direct_messages", "direct_messages", 40..50),
            ],
            order: vec![
                "favorites".into(),
                "channels".into(),
                "direct_messages".into(),
            ],
        },
    )?;

    // Warm the statement cache, then time.
    let sections = sidebar::sidebar_snapshot(conn, "t", ME, UnreadMode::CollapsedThreads)?;
    let shown: usize = sections.iter().map(|s| s.channels.len()).sum();
    assert_eq!(shown, CHANNELS);
    let mut times: Vec<Duration> = (0..500)
        .map(|_| {
            let start = Instant::now();
            let sections = sidebar::sidebar_snapshot(conn, "t", ME, UnreadMode::CollapsedThreads);
            let elapsed = start.elapsed();
            std::hint::black_box(sections).map(|_| elapsed)
        })
        .collect::<Result<_, _>>()?;
    times.sort();
    let median = times[times.len() / 2];
    let p99 = times[times.len() * 99 / 100];
    eprintln!("sidebar_snapshot, {CHANNELS} channels: median {median:?}, p99 {p99:?}");
    if median > BUDGET {
        return Err(format!("median {median:?} is over the {BUDGET:?} budget").into());
    }
    Ok(())
}
