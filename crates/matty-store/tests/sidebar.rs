//! Sidebar upserts and snapshots (#28).
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use std::collections::HashMap;

use matty_api::models::{
    Channel, ChannelMember, Emoji, Preference, SidebarCategories, SidebarCategory, Team, User,
};
use matty_store::Store;
use matty_store::sidebar::{self, UnreadMode, Unreads};
use rusqlite::Connection;

const ME: &str = "me";

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

fn team(id: &str, name: &str) -> Team {
    Team {
        id: id.into(),
        name: id.into(),
        display_name: name.into(),
        kind: "O".into(),
        ..Team::default()
    }
}

fn channel(id: &str, team: &str, kind: &str) -> Channel {
    Channel {
        id: id.into(),
        team_id: team.into(),
        kind: kind.into(),
        name: id.into(),
        display_name: format!("#{id}"),
        ..Channel::default()
    }
}

fn member(channel: &str) -> ChannelMember {
    ChannelMember {
        channel_id: channel.into(),
        user_id: ME.into(),
        ..ChannelMember::default()
    }
}

fn user(id: &str, username: &str) -> User {
    User {
        id: id.into(),
        username: username.into(),
        ..User::default()
    }
}

fn category(id: &str, kind: &str, channels: &[&str]) -> SidebarCategory {
    SidebarCategory {
        id: id.into(),
        team_id: "t1".into(),
        kind: kind.into(),
        display_name: id.to_uppercase(),
        channel_ids: channels.iter().map(|c| c.to_string()).collect(),
        ..SidebarCategory::default()
    }
}

#[test]
fn upserts_are_idempotent_and_keep_the_latest_values() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let mut town = channel("town", "t1", "O");
    let mut me_in_town = member("town");
    let preference = Preference {
        user_id: ME.into(),
        category: "display_settings".into(),
        name: "use_military_time".into(),
        value: "false".into(),
    };
    let emoji = Emoji {
        id: "e1".into(),
        name: "party".into(),
        ..Emoji::default()
    };
    let categories = SidebarCategories {
        categories: vec![category("fav", "favorites", &["town"])],
        order: vec!["fav".into()],
    };
    for _ in 0..2 {
        sidebar::upsert_teams(conn, &[team("t1", "One")]).unwrap();
        sidebar::upsert_channels(conn, std::slice::from_ref(&town)).unwrap();
        sidebar::upsert_channel_members(conn, std::slice::from_ref(&me_in_town)).unwrap();
        sidebar::replace_categories(conn, "t1", &categories).unwrap();
        sidebar::upsert_users(conn, &[user("u1", "alice")]).unwrap();
        sidebar::upsert_preferences(conn, std::slice::from_ref(&preference)).unwrap();
        sidebar::upsert_emoji(conn, std::slice::from_ref(&emoji)).unwrap();
    }
    for (table, rows) in [
        ("teams", 1),
        ("channels", 1),
        ("channel_members", 1),
        ("categories", 1),
        ("category_channels", 1),
        ("users", 1),
        ("preferences", 1),
        ("emoji", 1),
    ] {
        assert_eq!(count(conn, table), rows, "{table}");
    }

    // A second write with new values updates in place.
    town.display_name = "Town Square".into();
    me_in_town.mention_count = 4;
    sidebar::upsert_channels(conn, &[town]).unwrap();
    sidebar::upsert_channel_members(conn, &[me_in_town]).unwrap();
    sidebar::upsert_preferences(
        conn,
        &[Preference {
            value: "true".into(),
            ..preference.clone()
        }],
    )
    .unwrap();
    assert_eq!(count(conn, "channels"), 1);
    let snapshot = sidebar::sidebar_snapshot(conn, "t1", ME, UnreadMode::AllPosts).unwrap();
    assert_eq!(snapshot[0].channels[0].display_name, "Town Square");
    assert_eq!(snapshot[0].channels[0].unreads.mentions, 4);
    assert_eq!(
        sidebar::preference(conn, "display_settings", "use_military_time").unwrap(),
        Some("true".into())
    );
    sidebar::delete_preferences(conn, &[preference]).unwrap();
    assert_eq!(count(conn, "preferences"), 0);
}

#[test]
fn unread_math_uses_root_fields_under_crt_and_all_fields_otherwise() {
    let channel = Channel {
        total_msg_count: 120,
        total_msg_count_root: 30,
        ..Channel::default()
    };
    let member = ChannelMember {
        msg_count: 100,
        mention_count: 5,
        msg_count_root: 28,
        mention_count_root: 1,
        urgent_mention_count: 1,
        ..ChannelMember::default()
    };
    assert_eq!(
        sidebar::unreads(&channel, &member, UnreadMode::CollapsedThreads),
        Unreads {
            messages: 2,
            mentions: 1,
            urgent_mentions: 1
        }
    );
    assert_eq!(
        sidebar::unreads(&channel, &member, UnreadMode::AllPosts),
        Unreads {
            messages: 20,
            mentions: 5,
            urgent_mentions: 1
        }
    );

    // Only replies arrived: unread with CRT off, read with it on.
    let replies_only = ChannelMember {
        msg_count_root: 30,
        mention_count_root: 0,
        ..member.clone()
    };
    assert!(!sidebar::unreads(&channel, &replies_only, UnreadMode::CollapsedThreads).is_unread());
    assert!(sidebar::unreads(&channel, &replies_only, UnreadMode::AllPosts).is_unread());

    // A member count ahead of the channel's (events out of order) is not negative.
    let ahead = ChannelMember {
        msg_count: 130,
        ..member
    };
    assert_eq!(
        sidebar::unreads(&channel, &ahead, UnreadMode::AllPosts).messages,
        0
    );
}

/// A team with favorites, channels and DMs, and a second team.
fn seeded() -> Store {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    sidebar::upsert_teams(conn, &[team("t1", "Beta"), team("t2", "Alpha")]).unwrap();
    sidebar::upsert_users(
        conn,
        &[user(ME, "me"), user("u1", "alice"), user("u2", "bob")],
    )
    .unwrap();
    let mut town = channel("town", "t1", "O");
    town.total_msg_count = 10;
    town.total_msg_count_root = 6;
    let mut dev = channel("dev", "t1", "P");
    dev.total_msg_count = 4;
    dev.total_msg_count_root = 4;
    let mut noisy = channel("noisy", "t1", "O");
    noisy.total_msg_count = 50;
    noisy.total_msg_count_root = 50;
    let mut archived = channel("old", "t1", "O");
    archived.delete_at = 1;
    let not_joined = channel("other", "t1", "O");
    let mut dm = channel(&format!("{ME}__u1"), "", "D");
    dm.display_name = String::new();
    dm.total_msg_count = 3;
    dm.total_msg_count_root = 3;
    let self_dm = channel(&format!("{ME}__{ME}"), "", "D");
    let reversed_dm = channel(&format!("u2__{ME}"), "", "D");
    let mut elsewhere = channel("ops", "t2", "O");
    elsewhere.total_msg_count = 7;
    elsewhere.total_msg_count_root = 7;
    sidebar::upsert_channels(
        conn,
        &[
            town,
            dev,
            noisy,
            archived,
            not_joined,
            dm,
            self_dm,
            reversed_dm,
            elsewhere,
        ],
    )
    .unwrap();
    let mut me_town = member("town");
    me_town.msg_count = 7; // 3 unread, all replies but one
    me_town.msg_count_root = 5;
    me_town.mention_count = 2;
    me_town.mention_count_root = 1;
    let mut me_dev = member("dev");
    me_dev.msg_count = 4;
    me_dev.msg_count_root = 4;
    let mut me_noisy = member("noisy");
    me_noisy.msg_count = 40;
    me_noisy.msg_count_root = 40;
    me_noisy.notify_props = HashMap::from([("mark_unread".into(), "mention".into())]);
    let me_ops = member("ops");
    sidebar::upsert_channel_members(
        conn,
        &[
            me_town,
            me_dev,
            me_noisy,
            member("old"),
            member(&format!("{ME}__u1")),
            member(&format!("{ME}__{ME}")),
            member(&format!("u2__{ME}")),
            me_ops,
        ],
    )
    .unwrap();
    sidebar::replace_categories(
        conn,
        "t1",
        &SidebarCategories {
            categories: vec![
                category("chan", "channels", &["town", "noisy", "old", "other"]),
                category("fav", "favorites", &["dev"]),
                category(
                    "dms",
                    "direct_messages",
                    &[
                        &format!("u2__{ME}"),
                        &format!("{ME}__u1"),
                        &format!("{ME}__{ME}"),
                    ],
                ),
            ],
            order: vec!["fav".into(), "chan".into(), "dms".into()],
        },
    )
    .unwrap();
    store
}

#[test]
fn sidebar_snapshot_orders_categories_and_channels_and_names_dms() {
    let store = seeded();
    let sections =
        sidebar::sidebar_snapshot(store.conn(), "t1", ME, UnreadMode::CollapsedThreads).unwrap();
    let layout: Vec<(&str, Vec<&str>)> = sections
        .iter()
        .map(|s| {
            (
                s.kind.as_str(),
                s.channels.iter().map(|c| c.display_name.as_str()).collect(),
            )
        })
        .collect();
    assert_eq!(
        layout,
        vec![
            ("favorites", vec!["#dev"]),
            // Archived and not-joined channels are left out.
            ("channels", vec!["#town", "#noisy"]),
            ("direct_messages", vec!["bob", "alice", "me"]),
        ]
    );
    let dms = &sections[2].channels;
    assert_eq!(dms[0].dm_user_id.as_deref(), Some("u2"));
    assert_eq!(dms[1].dm_user_id.as_deref(), Some("u1"));
    assert_eq!(dms[2].dm_user_id.as_deref(), Some(ME));
    assert!(sections[1].channels[1].muted);
    assert!(!sections[1].channels[0].muted);
}

#[test]
fn a_dm_row_carries_the_partners_names_for_the_name_format() {
    let store = seeded();
    let conn = store.conn();
    let mut alice = user("u1", "alice");
    alice.first_name = "Alice".into();
    alice.last_name = "Liddell".into();
    alice.nickname = "Al".into();
    sidebar::upsert_users(conn, &[alice]).unwrap();
    let sections = sidebar::sidebar_snapshot(conn, "t1", ME, UnreadMode::AllPosts).unwrap();
    let dm = sections[2]
        .channels
        .iter()
        .find(|c| c.dm_user_id.as_deref() == Some("u1"))
        .unwrap();
    // The stored name stays the username; the app formats from the parts.
    assert_eq!(dm.display_name, "alice");
    let person = dm.dm_user.as_ref().unwrap();
    assert_eq!(person.id, "u1");
    assert_eq!(person.username, "alice");
    assert_eq!(person.first_name, "Alice");
    assert_eq!(person.last_name, "Liddell");
    assert_eq!(person.nickname, "Al");
    // Channels other than direct messages carry no person.
    assert!(sections[1].channels.iter().all(|c| c.dm_user.is_none()));
}

#[test]
fn sidebar_snapshot_counts_per_crt_mode() {
    let store = seeded();
    let unread = |mode| {
        let sections = sidebar::sidebar_snapshot(store.conn(), "t1", ME, mode).unwrap();
        sections
            .iter()
            .flat_map(|s| s.channels.iter())
            .map(|c| (c.id.clone(), (c.unreads.messages, c.unreads.mentions)))
            .collect::<HashMap<_, _>>()
    };
    let crt = unread(UnreadMode::CollapsedThreads);
    let all = unread(UnreadMode::AllPosts);
    assert_eq!(crt["town"], (1, 1));
    assert_eq!(all["town"], (3, 2));
    assert_eq!(crt["dev"], (0, 0));
    assert_eq!(crt["noisy"], (10, 0));
    assert_eq!(crt[&format!("{ME}__u1")], (3, 0));
}

#[test]
fn a_muted_category_mutes_its_channels() {
    let store = seeded();
    let mut muted = category("chan", "channels", &["town"]);
    muted.muted = true;
    sidebar::upsert_category(store.conn(), "t1", &muted, 1).unwrap();
    let sections = sidebar::sidebar_snapshot(store.conn(), "t1", ME, UnreadMode::AllPosts).unwrap();
    let chan = sections.iter().find(|s| s.id == "chan").unwrap();
    assert!(chan.muted);
    assert!(chan.channels.iter().all(|c| c.muted));
    assert_eq!(
        chan.channels.len(),
        1,
        "the category's channels were replaced"
    );
}

#[test]
fn teams_with_unreads_orders_and_badges_teams() {
    let store = seeded();
    let conn = store.conn();
    let teams = sidebar::teams_with_unreads(conn, ME, UnreadMode::CollapsedThreads).unwrap();
    let summary: Vec<_> = teams
        .iter()
        .map(|t| (t.id.as_str(), t.has_unreads, t.mentions))
        .collect();
    // No order preference yet: by name. t2's ops has 7 unread.
    assert_eq!(summary, vec![("t2", true, 0), ("t1", true, 1)]);

    sidebar::set_team_order(conn, &["t1".into(), "t2".into()]).unwrap();
    let ids: Vec<_> = sidebar::teams_with_unreads(conn, ME, UnreadMode::AllPosts)
        .unwrap()
        .into_iter()
        .map(|t| (t.id, t.mentions))
        .collect();
    assert_eq!(ids, vec![("t1".into(), 2), ("t2".into(), 0)]);

    // Only a muted channel unread: no unread dot.
    let mut town = member("town");
    town.msg_count = 10;
    town.msg_count_root = 6;
    sidebar::upsert_channel_members(conn, &[town]).unwrap();
    let t1 = sidebar::teams_with_unreads(conn, ME, UnreadMode::CollapsedThreads)
        .unwrap()
        .into_iter()
        .find(|t| t.id == "t1")
        .unwrap();
    assert!(!t1.has_unreads, "{t1:?}");
}

#[test]
fn deleting_a_channel_or_team_clears_it_from_the_sidebar() {
    let store = seeded();
    let conn = store.conn();
    sidebar::delete_channel(conn, "town").unwrap();
    let sections = sidebar::sidebar_snapshot(conn, "t1", ME, UnreadMode::AllPosts).unwrap();
    assert!(
        sections
            .iter()
            .all(|s| s.channels.iter().all(|c| c.id != "town"))
    );
    sidebar::delete_team(conn, "t1").unwrap();
    assert!(
        sidebar::sidebar_snapshot(conn, "t1", ME, UnreadMode::AllPosts)
            .unwrap()
            .is_empty()
    );
    let teams = sidebar::teams_with_unreads(conn, ME, UnreadMode::AllPosts).unwrap();
    assert_eq!(teams.len(), 1);
}

#[test]
fn a_closed_conversation_leaves_the_sidebar_until_shown_again() {
    let store = seeded();
    let conn = store.conn();
    let set = |value: &str| {
        sidebar::upsert_preferences(
            conn,
            &[matty_api::models::Preference {
                user_id: ME.into(),
                category: "direct_channel_show".into(),
                name: "u1".into(),
                value: value.into(),
            }],
        )
        .unwrap();
    };
    let dms = || -> Vec<String> {
        sidebar::sidebar_snapshot(conn, "t1", ME, UnreadMode::AllPosts)
            .unwrap()
            .into_iter()
            .find(|s| s.kind == "direct_messages")
            .unwrap()
            .channels
            .into_iter()
            .map(|c| c.display_name)
            .collect()
    };
    set("false");
    assert_eq!(dms(), vec!["bob", "me"]);
    set("true");
    assert_eq!(dms(), vec!["bob", "alice", "me"]);
    assert_eq!(
        sidebar::show_preference("G", "g1", "", ME),
        Some(("group_channel_show", "g1".to_owned()))
    );
    assert_eq!(sidebar::show_preference("O", "c1", "town", ME), None);
}

#[test]
fn a_category_reads_back_as_the_server_sent_it() {
    let store = seeded();
    let chan = sidebar::category(store.conn(), "chan").unwrap().unwrap();
    assert_eq!(
        chan,
        category("chan", "channels", &["town", "noisy", "old", "other"])
    );
    assert_eq!(sidebar::category(store.conn(), "nope").unwrap(), None);
}

#[test]
fn links_find_channels_by_team_and_name_and_users_by_username() {
    let store = seeded();
    let conn = store.conn();
    let town = sidebar::channel_by_name(conn, "t1", "town")
        .unwrap()
        .unwrap();
    assert_eq!(town.id, "town");
    assert_eq!(sidebar::channel_by_name(conn, "t2", "town").unwrap(), None);
    assert_eq!(sidebar::channel_by_name(conn, "t1", "nope").unwrap(), None);
    let alice = sidebar::user_by_username(conn, "Alice").unwrap().unwrap();
    assert_eq!(alice.id, "u1");
    assert_eq!(sidebar::user_by_username(conn, "carol").unwrap(), None);
}
