//! Every fixture in `tests/fixtures/ws/` decodes to its typed variant.

#![allow(clippy::unwrap_used)]

use matty_api::ws::{Event, RawEvent, WsEvent};
use serde_json::json;

fn fixture(name: &str) -> RawEvent {
    let path = format!(
        "{}/tests/fixtures/ws/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn decode(name: &str) -> WsEvent {
    let raw = fixture(name);
    assert_eq!(raw.event, name);
    WsEvent::decode(&raw)
}

/// The variant name each event must decode to.
const EXPECTED: &[(&str, &str)] = &[
    ("added_to_team", "AddedToTeam"),
    ("channel_converted", "ChannelConverted"),
    ("channel_created", "ChannelCreated"),
    ("channel_deleted", "ChannelDeleted"),
    ("channel_member_updated", "ChannelMemberUpdated"),
    ("channel_restored", "ChannelRestored"),
    ("channel_scheme_updated", "ChannelSchemeUpdated"),
    ("channel_updated", "ChannelUpdated"),
    ("channel_viewed", "ChannelViewed"),
    ("config_changed", "ConfigChanged"),
    ("delete_team", "DeleteTeam"),
    ("direct_added", "DirectAdded"),
    ("draft_created", "DraftCreated"),
    ("draft_deleted", "DraftDeleted"),
    ("draft_updated", "DraftUpdated"),
    ("emoji_added", "EmojiAdded"),
    ("ephemeral_message", "EphemeralMessage"),
    ("group_added", "GroupAdded"),
    ("hello", "Hello"),
    ("leave_team", "LeaveTeam"),
    ("license_changed", "LicenseChanged"),
    ("multiple_channels_viewed", "MultipleChannelsViewed"),
    ("new_user", "NewUser"),
    ("post_acknowledgement_added", "PostAcknowledgementAdded"),
    ("post_acknowledgement_removed", "PostAcknowledgementRemoved"),
    ("post_deleted", "PostDeleted"),
    ("post_edited", "PostEdited"),
    ("post_unread", "PostUnread"),
    ("posted", "Posted"),
    ("preference_changed", "PreferencesChanged"),
    ("preferences_changed", "PreferencesChanged"),
    ("preferences_deleted", "PreferencesDeleted"),
    ("reaction_added", "ReactionAdded"),
    ("reaction_removed", "ReactionRemoved"),
    ("sidebar_category_created", "SidebarCategoryCreated"),
    ("sidebar_category_deleted", "SidebarCategoryDeleted"),
    (
        "sidebar_category_order_updated",
        "SidebarCategoryOrderUpdated",
    ),
    ("sidebar_category_updated", "SidebarCategoryUpdated"),
    ("status_change", "StatusChange"),
    ("thread_follow_changed", "ThreadFollowChanged"),
    ("thread_read_changed", "ThreadReadChanged"),
    ("thread_updated", "ThreadUpdated"),
    ("typing", "Typing"),
    ("update_team", "UpdateTeam"),
    ("user_added", "UserAdded"),
    ("user_removed", "UserRemoved"),
    ("user_role_updated", "UserRoleUpdated"),
    ("user_updated", "UserUpdated"),
];

#[test]
fn every_fixture_decodes_to_its_variant() {
    let dir = format!("{}/tests/fixtures/ws", env!("CARGO_MANIFEST_DIR"));
    let mut on_disk: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter_map(|name| name.strip_suffix(".json").map(str::to_string))
        .collect();
    on_disk.sort();
    let listed: Vec<&str> = EXPECTED.iter().map(|(name, _)| *name).collect();
    assert_eq!(on_disk, listed, "every fixture is listed, and only those");

    for (name, variant) in EXPECTED {
        let event = decode(name);
        let debug = format!("{event:?}");
        assert!(
            debug.starts_with(&format!("{variant}("))
                || debug.starts_with(&format!("{variant} {{")),
            "{name} decoded to {debug:.200}"
        );
    }
}

#[test]
fn values_come_through() {
    let WsEvent::Posted(posted) = decode("posted") else {
        unreachable!()
    };
    assert_eq!(posted.post.message, "@tester ws fixture root");
    assert_eq!(posted.channel_name, "random");
    assert_eq!(posted.mentions.len(), 1);
    assert!(posted.post.pending_post_id.contains(':'));

    let WsEvent::ReactionAdded(reaction) = decode("reaction_added") else {
        unreachable!()
    };
    assert_eq!(reaction.emoji_name, "tada");

    let raw = fixture("typing");
    let WsEvent::Typing {
        channel_id,
        user_id,
        ..
    } = WsEvent::decode(&raw)
    else {
        unreachable!()
    };
    assert_eq!(channel_id, raw.broadcast.channel_id, "from broadcast");
    assert!(!user_id.is_empty());

    let WsEvent::ThreadUpdated { thread, .. } = decode("thread_updated") else {
        unreachable!()
    };
    assert_eq!(thread.id, thread.post.id);

    let WsEvent::SidebarCategoryUpdated(categories) = decode("sidebar_category_updated") else {
        unreachable!()
    };
    assert!(categories[0].display_name.starts_with("WS "));

    let WsEvent::GroupAdded { teammate_ids, .. } = decode("group_added") else {
        unreachable!()
    };
    assert_eq!(teammate_ids.len(), 2);

    let WsEvent::ConfigChanged(config) = decode("config_changed") else {
        unreachable!()
    };
    assert!(config.contains_key("Version"));

    let WsEvent::PreferencesChanged(preferences) = decode("preference_changed") else {
        unreachable!()
    };
    assert_eq!(preferences[0].name, "use_military_time");

    let WsEvent::MultipleChannelsViewed { channel_times } = decode("multiple_channels_viewed")
    else {
        unreachable!()
    };
    assert!(channel_times.values().all(|t| *t > 0));

    let event = Event::from(fixture("user_removed"));
    let WsEvent::UserRemoved { user_id, .. } = &event.kind else {
        unreachable!()
    };
    assert_eq!(user_id, &event.broadcast.user_id, "the removed user");
    assert!(event.seq > 0);
}

#[test]
fn unknown_events_are_not_errors() {
    let raw: RawEvent = serde_json::from_value(json!({
        "event": "custom_plugin_event",
        "data": {"anything": [1, 2, 3]},
        "broadcast": {"omit_users": null, "user_id": "", "channel_id": "c1", "team_id": ""},
        "seq": 7
    }))
    .unwrap();
    assert_eq!(WsEvent::decode(&raw), WsEvent::Unknown(raw.clone()));
}

#[test]
fn double_encoded_and_plain_posts_both_decode() {
    let post = json!({"id": "p1", "channel_id": "c1", "message": "hi", "create_at": 5});
    for data in [
        json!({"post": post.to_string(), "channel_type": "O"}),
        json!({"post": post, "channel_type": "O"}),
    ] {
        let raw: RawEvent =
            serde_json::from_value(json!({"event": "posted", "data": data, "seq": 1})).unwrap();
        let WsEvent::Posted(posted) = WsEvent::decode(&raw) else {
            panic!("expected Posted");
        };
        assert_eq!(posted.post.id, "p1");
        assert_eq!(posted.post.message, "hi");
        assert_eq!(posted.channel_type, "O");
    }
}

#[test]
fn broken_known_events_are_undecodable() {
    let raw: RawEvent = serde_json::from_value(json!({
        "event": "posted",
        "data": {"post": "{not json"},
        "seq": 3
    }))
    .unwrap();
    let WsEvent::Undecodable { error, .. } = WsEvent::decode(&raw) else {
        panic!("expected Undecodable");
    };
    assert!(error.contains("post"), "{error}");
}
