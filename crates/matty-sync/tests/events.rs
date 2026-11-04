//! Applying WebSocket events to the store (#36). Each test starts from the
//! same small cache, sends events through the engine, and checks the store
//! and the change sets.
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use matty_api::models::{
    Channel, ChannelMember, Draft, Post, Preference, Reaction, SidebarCategories, SidebarCategory,
    Team, User, UserThread,
};
use matty_api::ws::{PostUnread, Posted, ThreadReadChanged, WsEvent};
use matty_store::sidebar::{self, UnreadMode};
use matty_store::{Store, posts};
use matty_sync::{ChangeSet, Command};

const ME: &str = "me";

fn post(id: &str, channel: &str, root: &str, user: &str, at: i64) -> Post {
    Post {
        id: id.into(),
        channel_id: channel.into(),
        root_id: root.into(),
        user_id: user.into(),
        create_at: at,
        update_at: at,
        message: format!("post {id}"),
        ..Post::default()
    }
}

/// Team t1 with channel c1 (2 posts, read) and a DM with u2, both in one
/// category; root post r1 in c1.
fn cache(dir: &Path) -> PathBuf {
    let path = dir.join("account.sqlite");
    let store = Store::open(&path).unwrap();
    let conn = store.conn();
    sidebar::upsert_teams(
        conn,
        &[Team {
            id: "t1".into(),
            name: "one".into(),
            display_name: "One".into(),
            ..Team::default()
        }],
    )
    .unwrap();
    sidebar::upsert_users(
        conn,
        &[User {
            id: "u2".into(),
            username: "ada".into(),
            ..User::default()
        }],
    )
    .unwrap();
    let channel = |id: &str, team: &str, kind: &str, name: &str| Channel {
        id: id.into(),
        team_id: team.into(),
        kind: kind.into(),
        name: name.into(),
        display_name: id.into(),
        total_msg_count: 2,
        total_msg_count_root: 2,
        ..Channel::default()
    };
    sidebar::upsert_channels(
        conn,
        &[
            channel("c1", "t1", "O", "c1"),
            channel("dm", "", "D", "me__u2"),
        ],
    )
    .unwrap();
    let member = |channel: &str| ChannelMember {
        channel_id: channel.into(),
        user_id: ME.into(),
        msg_count: 2,
        msg_count_root: 2,
        ..ChannelMember::default()
    };
    sidebar::upsert_channel_members(conn, &[member("c1"), member("dm")]).unwrap();
    sidebar::replace_categories(
        conn,
        "t1",
        &SidebarCategories {
            categories: vec![SidebarCategory {
                id: "cat".into(),
                team_id: "t1".into(),
                kind: "channels".into(),
                display_name: "Channels".into(),
                channel_ids: vec!["c1".into(), "dm".into()],
                ..SidebarCategory::default()
            }],
            order: vec!["cat".into()],
        },
    )
    .unwrap();
    posts::insert_posts(conn, &[post("r1", "c1", "", "u2", 10)]).unwrap();
    posts::set_cursor(conn, "me", ME).unwrap();
    path
}

/// Sends `commands` to an engine over the cache, lets it finish, and
/// returns every change set it published and the store it left.
async fn run(commands: Vec<Command>) -> (Vec<ChangeSet>, Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = cache(dir.path());
    let sync = matty_sync::spawn(Store::open(&path).unwrap()).unwrap();
    let changes = sync.subscribe();
    for command in commands {
        sync.send(command);
    }
    // The engine drains its queue before it stops and closes the stream.
    drop(sync);
    let mut seen = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Ok(change) = changes.recv().await {
            seen.push(change);
        }
    })
    .await
    .unwrap();
    (seen, Store::open(&path).unwrap(), dir)
}

fn event(event: WsEvent) -> Command {
    Command::Event(Box::new(event))
}

fn posted(post: Post, mentions: &[&str]) -> Command {
    event(WsEvent::Posted(Box::new(Posted {
        post,
        mentions: mentions.iter().map(|m| (*m).to_owned()).collect(),
        ..Posted::default()
    })))
}

fn open(channel: &str) -> Command {
    Command::OpenChannel {
        channel_id: channel.into(),
    }
}

fn unreads(store: &Store, channel: &str, mode: UnreadMode) -> (i64, i64) {
    let sections = sidebar::sidebar_snapshot(store.conn(), "t1", ME, mode).unwrap();
    let row = sections[0]
        .channels
        .iter()
        .find(|c| c.id == channel)
        .unwrap();
    (row.unreads.messages, row.unreads.mentions)
}

fn last_posts(seen: &[ChangeSet], channel: &str) -> Vec<String> {
    seen.iter()
        .rev()
        .find_map(|c| match c {
            ChangeSet::Posts { channel_id, posts } if channel_id == channel => {
                Some(posts.iter().map(|p| p.id.clone()).collect())
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn has_sidebar(seen: &[ChangeSet]) -> bool {
    seen.iter()
        .any(|c| matches!(c, ChangeSet::Sidebar { team_id, .. } if team_id == "t1"))
}

#[tokio::test]
async fn posted_stores_counts_and_publishes() {
    let (seen, store, _dir) = run(vec![
        open("c1"),
        posted(post("p2", "c1", "", "u2", 20), &[ME]),
        posted(post("reply", "c1", "r1", "u2", 30), &[]),
    ])
    .await;
    assert_eq!(last_posts(&seen, "c1"), ["r1", "p2", "reply"]);
    assert!(has_sidebar(&seen));
    assert!(seen.iter().any(|c| matches!(c, ChangeSet::Unreads { .. })));
    // Two new posts, one a root; one mention, in a root post.
    assert_eq!(unreads(&store, "c1", UnreadMode::AllPosts), (2, 1));
    assert_eq!(unreads(&store, "c1", UnreadMode::CollapsedThreads), (1, 1));
    assert_eq!(
        posts::post(store.conn(), "r1")
            .unwrap()
            .unwrap()
            .reply_count,
        1
    );
}

#[tokio::test]
async fn my_own_post_counts_as_read() {
    let (_, store, _dir) = run(vec![posted(post("mine", "c1", "", ME, 20), &[])]).await;
    assert_eq!(unreads(&store, "c1", UnreadMode::AllPosts), (0, 0));
    let channel = sidebar::channel(store.conn(), "c1").unwrap().unwrap();
    assert_eq!((channel.total_msg_count, channel.last_post_at), (3, 20));
}

#[tokio::test]
async fn posts_from_others_arrive_for_notifications_once() {
    let p2 = post("p2", "c1", "", "u2", 20);
    let (seen, _store, _dir) = run(vec![
        posted(p2.clone(), &[ME]),
        posted(p2, &[ME]),
        posted(post("mine", "c1", "", ME, 30), &[]),
    ])
    .await;
    let arrived: Vec<_> = seen
        .iter()
        .filter_map(|c| match c {
            ChangeSet::Arrived(arrived) => Some(arrived),
            _ => None,
        })
        .collect();
    // The duplicate and the user's own post do not arrive.
    assert_eq!(arrived.len(), 1);
    assert_eq!(arrived[0].post.id, "p2");
    assert!(arrived[0].mentioned);
    assert!(!arrived[0].followed);
}

#[tokio::test]
async fn a_duplicate_posted_is_a_no_op() {
    let p2 = post("p2", "c1", "", "u2", 20);
    let (_, store, _dir) = run(vec![posted(p2.clone(), &[ME]), posted(p2, &[ME])]).await;
    assert_eq!(unreads(&store, "c1", UnreadMode::AllPosts), (1, 1));
}

#[tokio::test]
async fn an_edit_before_its_post_counts_once_and_keeps_the_edit() {
    let original = post("p2", "c1", "", "u2", 20);
    let edited = Post {
        message: "edited".into(),
        edit_at: 25,
        update_at: 25,
        ..original.clone()
    };
    let (_, store, _dir) = run(vec![
        event(WsEvent::PostEdited(Box::new(edited))),
        posted(original, &[]),
    ])
    .await;
    let stored = posts::post(store.conn(), "p2").unwrap().unwrap();
    assert_eq!(stored.message, "edited");
    assert_eq!(unreads(&store, "c1", UnreadMode::AllPosts), (1, 0));
}

#[tokio::test]
async fn edits_and_deletes_change_the_window() {
    let edited = Post {
        message: "changed".into(),
        edit_at: 11,
        ..post("r1", "c1", "", "u2", 10)
    };
    let (seen, store, _dir) = run(vec![
        open("c1"),
        posted(post("reply", "c1", "r1", "u2", 30), &[]),
        event(WsEvent::PostEdited(Box::new(edited))),
        event(WsEvent::PostDeleted(Box::new(Post {
            delete_at: 40,
            ..post("reply", "c1", "r1", "u2", 30)
        }))),
        // Unknown: ignored.
        event(WsEvent::PostDeleted(Box::new(post(
            "ghost", "c1", "", "u2", 1,
        )))),
    ])
    .await;
    assert_eq!(last_posts(&seen, "c1"), ["r1"]);
    let root = posts::post(store.conn(), "r1").unwrap().unwrap();
    assert_eq!((root.message.as_str(), root.reply_count), ("changed", 0));
    assert!(posts::post(store.conn(), "ghost").unwrap().is_none());
}

#[tokio::test]
async fn reactions_follow_their_post() {
    let thumbs = Reaction {
        user_id: "u2".into(),
        post_id: "r1".into(),
        emoji_name: "+1".into(),
        create_at: 5,
    };
    let (seen, store, _dir) = run(vec![
        open("c1"),
        event(WsEvent::ReactionAdded(thumbs.clone())),
        event(WsEvent::ReactionAdded(Reaction {
            post_id: "unknown".into(),
            ..thumbs.clone()
        })),
    ])
    .await;
    assert!(seen.iter().any(
        |c| matches!(c, ChangeSet::Posts { posts, .. } if posts[0].metadata.reactions.len() == 1)
    ));
    assert_eq!(
        posts::reactions(store.conn(), "r1").unwrap(),
        std::slice::from_ref(&thumbs)
    );
    let (_, store, _dir) = run(vec![event(WsEvent::ReactionRemoved(thumbs))]).await;
    assert!(posts::reactions(store.conn(), "r1").unwrap().is_empty());
}

#[tokio::test]
async fn viewed_and_unread_events_move_the_read_position() {
    let (_, store, _dir) = run(vec![
        posted(post("p2", "c1", "", "u2", 20), &[ME]),
        posted(post("p3", "dm", "", "u2", 21), &[ME]),
        event(WsEvent::ChannelViewed {
            channel_id: "c1".into(),
        }),
        event(WsEvent::MultipleChannelsViewed {
            channel_times: HashMap::from([("dm".to_owned(), 30)]),
        }),
    ])
    .await;
    assert_eq!(unreads(&store, "c1", UnreadMode::AllPosts), (0, 0));
    assert_eq!(unreads(&store, "dm", UnreadMode::AllPosts), (0, 0));

    let (_, store, _dir) = run(vec![event(WsEvent::PostUnread {
        channel_id: "c1".into(),
        unread: PostUnread {
            post_id: "r1".into(),
            last_viewed_at: 9,
            msg_count: 1,
            msg_count_root: 1,
            mention_count: 1,
            mention_count_root: 1,
            ..PostUnread::default()
        },
    })])
    .await;
    assert_eq!(unreads(&store, "c1", UnreadMode::AllPosts), (1, 1));
}

#[tokio::test]
async fn channel_changes() {
    let renamed = Channel {
        display_name: "Renamed".into(),
        ..sidebar::channel(
            Store::open(&cache(tempfile::tempdir().unwrap().path()))
                .unwrap()
                .conn(),
            "c1",
        )
        .unwrap()
        .unwrap()
    };
    let (seen, store, _dir) = run(vec![
        event(WsEvent::ChannelUpdated(Box::new(renamed))),
        event(WsEvent::ChannelMemberUpdated(Box::new(ChannelMember {
            channel_id: "c1".into(),
            user_id: ME.into(),
            msg_count: 2,
            msg_count_root: 2,
            notify_props: HashMap::from([("mark_unread".to_owned(), "mention".to_owned())]),
            ..ChannelMember::default()
        }))),
    ])
    .await;
    assert!(has_sidebar(&seen));
    let sections = sidebar::sidebar_snapshot(store.conn(), "t1", ME, UnreadMode::AllPosts).unwrap();
    let c1 = sections[0].channels.iter().find(|c| c.id == "c1").unwrap();
    assert_eq!(c1.display_name, "Renamed");
    assert!(c1.muted);

    let (_, store, _dir) = run(vec![event(WsEvent::ChannelDeleted {
        channel_id: "c1".into(),
        delete_at: 50,
    })])
    .await;
    let sections = sidebar::sidebar_snapshot(store.conn(), "t1", ME, UnreadMode::AllPosts).unwrap();
    assert!(sections[0].channels.iter().all(|c| c.id != "c1"));
    assert!(
        posts::post(store.conn(), "r1").unwrap().is_some(),
        "posts stay"
    );

    let (_, store, _dir) = run(vec![
        // Someone else leaving changes nothing; the user leaving removes it.
        event(WsEvent::UserRemoved {
            user_id: "u2".into(),
            channel_id: "dm".into(),
            remover_id: "u2".into(),
        }),
        event(WsEvent::UserRemoved {
            user_id: ME.into(),
            channel_id: "c1".into(),
            remover_id: ME.into(),
        }),
    ])
    .await;
    assert!(sidebar::channel(store.conn(), "c1").unwrap().is_none());
    assert!(sidebar::channel(store.conn(), "dm").unwrap().is_some());
}

#[tokio::test]
async fn users_statuses_and_preferences() {
    let (seen, store, _dir) = run(vec![
        event(WsEvent::UserUpdated(Box::new(User {
            id: "u2".into(),
            username: "ada.l".into(),
            last_picture_update: 42,
            ..User::default()
        }))),
        event(WsEvent::StatusChange {
            user_id: "u2".into(),
            status: "away".into(),
        }),
        event(WsEvent::PreferencesChanged(vec![Preference {
            user_id: ME.into(),
            category: "favorite_channel".into(),
            name: "c1".into(),
            value: "true".into(),
        }])),
    ])
    .await;
    let sections = sidebar::sidebar_snapshot(store.conn(), "t1", ME, UnreadMode::AllPosts).unwrap();
    let dm = sections[0].channels.iter().find(|c| c.id == "dm").unwrap();
    assert_eq!(dm.display_name, "ada.l");
    assert!(
        seen.iter()
            .any(|c| matches!(c, ChangeSet::Presence(s) if s[0].status == "away"))
    );
    assert!(seen.iter().any(|c| matches!(c, ChangeSet::Preferences)));
    // The app hears the new picture time, to fetch the new avatar (#153).
    assert!(seen.iter().any(|c| matches!(
        c,
        ChangeSet::Users(users) if users[0].id == "u2" && users[0].last_picture_update == 42
    )));
    assert_eq!(
        sidebar::preference(store.conn(), "favorite_channel", "c1")
            .unwrap()
            .as_deref(),
        Some("true")
    );
}

#[tokio::test]
async fn teams_and_categories() {
    let (seen, store, _dir) = run(vec![
        event(WsEvent::SidebarCategoryUpdated(vec![SidebarCategory {
            id: "cat".into(),
            team_id: "t1".into(),
            kind: "channels".into(),
            display_name: "Renamed".into(),
            channel_ids: vec!["dm".into(), "c1".into()],
            ..SidebarCategory::default()
        }])),
        event(WsEvent::UpdateTeam(Box::new(Team {
            id: "t1".into(),
            name: "one".into(),
            display_name: "First".into(),
            ..Team::default()
        }))),
    ])
    .await;
    let sections = sidebar::sidebar_snapshot(store.conn(), "t1", ME, UnreadMode::AllPosts).unwrap();
    assert_eq!(sections[0].display_name, "Renamed");
    assert_eq!(sections[0].channels[0].id, "dm");
    let rail = seen
        .iter()
        .find_map(|c| match c {
            ChangeSet::Unreads { teams } => Some(teams.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(rail[0].display_name, "First");

    let (_, store, _dir) = run(vec![event(WsEvent::LeaveTeam {
        team_id: "t1".into(),
        user_id: ME.into(),
    })])
    .await;
    assert!(
        sidebar::teams_with_unreads(store.conn(), ME, UnreadMode::AllPosts)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn threads_and_drafts() {
    let thread = UserThread {
        id: "r1".into(),
        reply_count: 1,
        last_reply_at: 30,
        unread_replies: 1,
        post: post("r1", "c1", "", "u2", 10),
        ..UserThread::default()
    };
    let draft = Draft {
        channel_id: "c1".into(),
        message: "half".into(),
        update_at: 5,
        ..Draft::default()
    };
    let (seen, store, _dir) = run(vec![
        Command::OpenThreadList {
            team_id: "t1".into(),
        },
        event(WsEvent::ThreadUpdated {
            thread: Box::new(thread),
            previous_unread_replies: 0,
            previous_unread_mentions: 0,
        }),
        event(WsEvent::ThreadReadChanged(ThreadReadChanged {
            thread_id: "r1".into(),
            timestamp: 40,
            ..ThreadReadChanged::default()
        })),
        event(WsEvent::DraftCreated(draft.clone())),
    ])
    .await;
    let list = seen
        .iter()
        .rev()
        .find_map(|c| match c {
            ChangeSet::ThreadList { threads, .. } => Some(threads.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!((list[0].id.as_str(), list[0].unread_replies), ("r1", 0));
    assert!(
        seen.iter()
            .any(|c| matches!(c, ChangeSet::Draft { draft: Some(d), .. } if d.message == "half"))
    );
    assert_eq!(
        posts::draft(store.conn(), "c1", "")
            .unwrap()
            .unwrap()
            .message,
        "half"
    );

    let (seen, store, _dir) = run(vec![event(WsEvent::DraftDeleted(draft))]).await;
    assert!(
        seen.iter()
            .any(|c| matches!(c, ChangeSet::Draft { draft: None, .. }))
    );
    assert!(posts::draft(store.conn(), "c1", "").unwrap().is_none());
}

#[tokio::test]
async fn open_threads_get_their_replies() {
    let (seen, _, _dir) = run(vec![
        Command::OpenThread {
            root_id: "r1".into(),
        },
        posted(post("reply", "c1", "r1", "u2", 30), &[]),
    ])
    .await;
    let thread = seen
        .iter()
        .rev()
        .find_map(|c| match c {
            ChangeSet::Thread { posts, .. } => Some(posts.clone()),
            _ => None,
        })
        .unwrap();
    let ids: Vec<&str> = thread.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["r1", "reply"]);
}

#[tokio::test]
async fn ephemeral_and_typing_are_not_stored() {
    let (seen, store, _dir) = run(vec![
        event(WsEvent::EphemeralMessage(Box::new(post(
            "eph", "c1", "", "system", 50,
        )))),
        event(WsEvent::Typing {
            user_id: "u2".into(),
            channel_id: "c1".into(),
            parent_id: String::new(),
        }),
    ])
    .await;
    assert!(
        seen.iter()
            .any(|c| matches!(c, ChangeSet::Ephemeral(p) if p.id == "eph"))
    );
    assert!(seen.iter().any(|c| matches!(c, ChangeSet::Typing { .. })));
    assert!(posts::post(store.conn(), "eph").unwrap().is_none());
}

/// #211: an integration changes or removes its ephemeral post with the
/// usual post events; those go to the view and stay out of the store too.
#[tokio::test]
async fn ephemeral_edits_and_deletes_are_not_stored() {
    let ephemeral = |at: i64, deleted: i64| {
        let mut post = post("eph", "c1", "", "bot", 50);
        post.kind = "system_ephemeral".into();
        post.update_at = at;
        post.delete_at = deleted;
        Box::new(post)
    };
    let (seen, store, _dir) = run(vec![
        event(WsEvent::EphemeralMessage(ephemeral(50, 0))),
        event(WsEvent::PostEdited(ephemeral(60, 0))),
        event(WsEvent::PostDeleted(ephemeral(70, 70))),
    ])
    .await;
    let shown: Vec<(i64, i64)> = seen
        .iter()
        .filter_map(|c| match c {
            ChangeSet::Ephemeral(p) => Some((p.update_at, p.delete_at)),
            _ => None,
        })
        .collect();
    assert_eq!(shown, [(50, 0), (60, 0), (70, 70)]);
    assert!(posts::post(store.conn(), "eph").unwrap().is_none());
}

/// #211: an integration's dialog goes to the view as it came.
#[tokio::test]
async fn an_open_dialog_reaches_the_view() {
    let open = matty_api::models::OpenDialogRequest {
        trigger_id: "t1".into(),
        url: "https://hook.example.com".into(),
        dialog: matty_api::models::Dialog {
            callback_id: "deploy".into(),
            title: "Approve".into(),
            ..Default::default()
        },
    };
    let (seen, _store, _dir) = run(vec![event(WsEvent::OpenDialog(Box::new(open.clone())))]).await;
    assert!(
        seen.iter()
            .any(|c| matches!(c, ChangeSet::OpenDialog(d) if **d == open))
    );
}

mod fetches {
    use super::*;
    use matty_api::Client;
    use matty_sync::Connection;
    use serde_json::{Value, json};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fixture(name: &str) -> Value {
        let path = format!(
            "{}/../matty-api/tests/fixtures/v10.5/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    fn get(route: &str, body: Value) -> Mock {
        Mock::given(method("GET"))
            .and(path(format!("/api/v4/{route}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
    }

    /// A server whose account has no teams yet, and channel `new` in t9.
    async fn server() -> MockServer {
        let server = MockServer::start().await;
        let mocks = [
            get("users/me", json!({"id": ME, "username": "me"})),
            get("config/client", fixture("config_client")),
            get("license/client", fixture("license_client")),
            get("users/me/preferences", json!([])),
            get("users/me/teams", json!([])),
            get("emoji", json!([])),
            get(
                "users/me/teams/t9/channels/categories",
                json!({"categories": [{
                    "id": "t9-channels", "team_id": "t9", "type": "channels",
                    "display_name": "Channels", "channel_ids": ["new"]
                }], "order": ["t9-channels"]}),
            ),
        ];
        for mock in mocks {
            mock.mount(&server).await;
        }
        Mock::given(method("GET"))
            .and(path("/api/v4/channels/new"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "new", "team_id": "t9", "type": "O", "name": "new",
                "display_name": "New", "total_msg_count": 1, "total_msg_count_root": 1
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v4/channels/new/members/me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "channel_id": "new", "user_id": ME, "msg_count": 0, "msg_count_root": 0,
                "mention_count": 0, "notify_props": {"mark_unread": "all"}
            })))
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_post_in_an_unknown_channel_fetches_it() {
        let server = server().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("account.sqlite");
        let sync = matty_sync::spawn(Store::open(&path).unwrap()).unwrap();
        let changes = sync.subscribe();
        sync.send(Command::Connect {
            client: Client::new(server.uri()).unwrap().with_token("token"),
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !matches!(
                changes.recv().await.unwrap(),
                ChangeSet::Connection(Connection::Loaded)
            ) {}
        })
        .await
        .unwrap();

        sync.send(posted(post("p1", "new", "", "u2", 5), &[]));
        // The channel arrives, then its categories, then the sidebar.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let ChangeSet::Sidebar { team_id, sections } = changes.recv().await.unwrap()
                    && team_id == "t9"
                    && sections.first().is_some_and(|s| !s.channels.is_empty())
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        drop(sync);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let store = Store::open(&path).unwrap();
        let channel = sidebar::channel(store.conn(), "new").unwrap().unwrap();
        assert_eq!(channel.display_name, "New");
        assert!(posts::post(store.conn(), "p1").unwrap().is_some());
        server.verify().await;
    }
}
