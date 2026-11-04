//! Post, thread, reaction, file, draft and cursor queries (#29).
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

use matty_api::models::{Draft, FileInfo, Post, PostMetadata, Reaction, User, UserThread};
use matty_store::Store;
use matty_store::posts::{self, Window};

fn post(id: &str, channel: &str, at: i64) -> Post {
    Post {
        id: id.into(),
        channel_id: channel.into(),
        user_id: "u1".into(),
        create_at: at,
        message: format!("text of {id}"),
        ..Post::default()
    }
}

fn reply(id: &str, root: &str, at: i64) -> Post {
    Post {
        root_id: root.into(),
        ..post(id, "c", at)
    }
}

fn reaction(post: &str, user: &str, emoji: &str, at: i64) -> Reaction {
    Reaction {
        post_id: post.into(),
        user_id: user.into(),
        emoji_name: emoji.into(),
        create_at: at,
    }
}

fn file(id: &str, post: &str, at: i64) -> FileInfo {
    FileInfo {
        id: id.into(),
        post_id: post.into(),
        channel_id: "c".into(),
        name: format!("{id}.png"),
        extension: "png".into(),
        size: 1234,
        mime_type: "image/png".into(),
        width: 10,
        height: 20,
        has_preview_image: true,
        create_at: at,
        ..FileInfo::default()
    }
}

fn ids(posts: &[Post]) -> Vec<&str> {
    posts.iter().map(|p| p.id.as_str()).collect()
}

#[test]
fn window_is_in_create_at_order_with_reactions_and_files() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let mut with_extras = post("b", "c", 20);
    with_extras.metadata = PostMetadata {
        reactions: vec![
            reaction("b", "u2", "smile", 2),
            reaction("b", "u1", "+1", 1),
        ],
        files: vec![file("f2", "b", 2), file("f1", "b", 1)],
        ..PostMetadata::default()
    };
    // Out of order on purpose, plus another channel.
    posts::insert_posts(
        conn,
        &[
            post("c", "c", 30),
            post("a", "c", 10),
            with_extras,
            post("x", "other", 15),
        ],
    )
    .unwrap();

    let latest = posts::posts_window(conn, "c", Window::Latest, 10, false).unwrap();
    assert_eq!(ids(&latest), ["a", "b", "c"]);
    let b = &latest[1];
    let emoji: Vec<&str> = b
        .metadata
        .reactions
        .iter()
        .map(|r| r.emoji_name.as_str())
        .collect();
    assert_eq!(emoji, ["+1", "smile"]);
    let files: Vec<&str> = b.metadata.files.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(files, ["f1", "f2"]);
    assert_eq!(b.metadata.files[0], file("f1", "b", 1));
    assert!(latest[0].metadata.reactions.is_empty());

    // The newest two, still oldest first.
    let two = posts::posts_window(conn, "c", Window::Latest, 2, false).unwrap();
    assert_eq!(ids(&two), ["b", "c"]);
    let before = posts::posts_window(conn, "c", Window::Before(30), 1, false).unwrap();
    assert_eq!(ids(&before), ["b"]);
    let after = posts::posts_window(conn, "c", Window::After(10), 1, false).unwrap();
    assert_eq!(ids(&after), ["b"]);
    let between = posts::posts_window(conn, "c", Window::Between(20, 30), 10, false).unwrap();
    assert_eq!(ids(&between), ["b", "c"]);
}

#[test]
fn roots_only_and_threads() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    posts::insert_posts(
        conn,
        &[
            post("root", "c", 10),
            reply("r2", "root", 30),
            reply("r1", "root", 20),
            post("other", "c", 25),
        ],
    )
    .unwrap();
    let all = posts::posts_window(conn, "c", Window::Latest, 10, false).unwrap();
    assert_eq!(ids(&all), ["root", "r1", "other", "r2"]);
    let roots = posts::posts_window(conn, "c", Window::Latest, 10, true).unwrap();
    assert_eq!(ids(&roots), ["root", "other"]);

    assert_eq!(
        ids(&posts::thread(conn, "root").unwrap()),
        ["root", "r1", "r2"]
    );
    assert!(posts::thread(conn, "missing").unwrap().is_empty());
}

#[test]
fn replace_pending_keeps_the_position() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let mut optimistic = post("pending-1", "c", 20);
    optimistic.pending_post_id = "pending-1".into();
    posts::insert_posts(conn, &[post("a", "c", 10), optimistic, post("c", "c", 30)]).unwrap();

    let mut server = post("real-1", "c", 21);
    server.metadata.reactions = vec![reaction("real-1", "u2", "+1", 22)];
    posts::replace_pending(conn, "pending-1", &server).unwrap();

    let window = posts::posts_window(conn, "c", Window::Latest, 10, false).unwrap();
    assert_eq!(ids(&window), ["a", "real-1", "c"]);
    assert_eq!(window[1].pending_post_id, "pending-1");
    assert_eq!(window[1].metadata.reactions.len(), 1);
    assert!(posts::post(conn, "pending-1").unwrap().is_none());

    // The WebSocket echo of the same post changes nothing.
    let mut echo = post("real-1", "c", 21);
    echo.pending_post_id = "pending-1".into();
    posts::insert_posts(conn, &[echo]).unwrap();
    let window = posts::posts_window(conn, "c", Window::Latest, 10, false).unwrap();
    assert_eq!(ids(&window), ["a", "real-1", "c"]);
}

#[test]
fn server_post_arriving_first_drops_the_optimistic_row() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let mut optimistic = post("pending-2", "c", 20);
    optimistic.pending_post_id = "pending-2".into();
    posts::insert_posts(conn, &[optimistic]).unwrap();

    // `posted` over the WebSocket, before the REST response.
    let mut posted = post("real-2", "c", 21);
    posted.pending_post_id = "pending-2".into();
    posts::insert_posts(conn, &[posted]).unwrap();
    let window = posts::posts_window(conn, "c", Window::Latest, 10, false).unwrap();
    assert_eq!(ids(&window), ["real-2"]);

    // Then the REST response.
    posts::replace_pending(conn, "pending-2", &post("real-2", "c", 21)).unwrap();
    let window = posts::posts_window(conn, "c", Window::Latest, 10, false).unwrap();
    assert_eq!(ids(&window), ["real-2"]);
}

#[test]
fn a_fetched_post_takes_the_place_of_its_optimistic_row() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    // Two sends of the same text, and one of other text.
    for (id, at, text) in [("p-1", 20, "ok"), ("p-2", 21, "ok"), ("p-3", 22, "other")] {
        let mut optimistic = post(id, "c", at);
        optimistic.pending_post_id = id.into();
        optimistic.message = text.into();
        posts::insert_posts(conn, &[optimistic]).unwrap();
    }

    // A channel fetch returns the first send without its pending id.
    let mut fetched = post("real-1", "c", 23);
    fetched.message = "ok".into();
    posts::insert_posts(conn, &[fetched.clone()]).unwrap();
    let window = posts::posts_window(conn, "c", Window::Latest, 10, false).unwrap();
    assert_eq!(ids(&window), ["p-2", "p-3", "real-1"]);
    assert_eq!(window[2].pending_post_id, "p-1");

    // Fetching it again, then the REST response, change nothing.
    posts::insert_posts(conn, &[fetched]).unwrap();
    let mut response = post("real-1", "c", 23);
    response.message = "ok".into();
    posts::replace_pending(conn, "p-1", &response).unwrap();
    let window = posts::posts_window(conn, "c", Window::Latest, 10, false).unwrap();
    assert_eq!(ids(&window), ["p-2", "p-3", "real-1"]);

    // Another author's identical text is not ours.
    let mut theirs = post("real-9", "c", 24);
    theirs.user_id = "u2".into();
    theirs.message = "ok".into();
    posts::insert_posts(conn, &[theirs]).unwrap();
    let window = posts::posts_window(conn, "c", Window::Latest, 10, false).unwrap();
    assert_eq!(ids(&window), ["p-2", "p-3", "real-1", "real-9"]);
}

#[test]
fn deleted_posts_leave_the_window_but_stay_stored() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let mut gone = post("gone", "c", 10);
    gone.metadata.reactions = vec![reaction("gone", "u2", "+1", 1)];
    gone.metadata.files = vec![file("f", "gone", 1)];
    posts::insert_posts(conn, &[gone, post("kept", "c", 20)]).unwrap();
    posts::mark_deleted(conn, "gone", 99).unwrap();

    let window = posts::posts_window(conn, "c", Window::Latest, 10, false).unwrap();
    assert_eq!(ids(&window), ["kept"]);
    let stored = posts::post(conn, "gone").unwrap().unwrap();
    assert_eq!(stored.delete_at, 99);
    assert_eq!(stored.message, "");
    assert!(posts::reactions(conn, "gone").unwrap().is_empty());
    assert!(posts::files_for_post(conn, "gone").unwrap().is_empty());
}

#[test]
fn reactions_add_and_remove() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    posts::insert_posts(conn, &[post("p", "c", 1)]).unwrap();
    let thumbs = reaction("p", "u2", "+1", 5);
    posts::add_reaction(conn, &thumbs).unwrap();
    posts::add_reaction(conn, &thumbs).unwrap();
    // A reaction to a post Matty has not cached is dropped, not an error.
    posts::add_reaction(conn, &reaction("unknown", "u2", "+1", 5)).unwrap();
    assert_eq!(
        posts::reactions(conn, "p").unwrap(),
        std::slice::from_ref(&thumbs)
    );
    posts::remove_reaction(conn, &thumbs).unwrap();
    assert!(posts::reactions(conn, "p").unwrap().is_empty());
}

#[test]
fn drafts_per_channel_and_thread() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let draft = |root: &str, message: &str, at: i64| Draft {
        channel_id: "c".into(),
        root_id: root.into(),
        message: message.into(),
        update_at: at,
        ..Draft::default()
    };
    posts::upsert_draft(conn, &draft("", "channel draft", 10)).unwrap();
    posts::upsert_draft(conn, &draft("root", "thread draft", 10)).unwrap();
    assert_eq!(
        posts::draft(conn, "c", "").unwrap().unwrap().message,
        "channel draft"
    );
    assert_eq!(
        posts::draft(conn, "c", "root").unwrap().unwrap().message,
        "thread draft"
    );

    // An older copy (say, from another device's late sync) does not win.
    posts::upsert_draft(conn, &draft("", "stale", 5)).unwrap();
    assert_eq!(
        posts::draft(conn, "c", "").unwrap().unwrap().message,
        "channel draft"
    );

    // Clearing the text deletes it.
    posts::upsert_draft(conn, &draft("", "", 20)).unwrap();
    assert!(posts::draft(conn, "c", "").unwrap().is_none());
    posts::delete_draft(conn, "c", "root").unwrap();
    assert!(posts::draft(conn, "c", "root").unwrap().is_none());
}

#[test]
fn threads_list_with_unread_replies() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    let thread = |id: &str, last: i64, unread: i64| UserThread {
        id: id.into(),
        reply_count: 3,
        last_reply_at: last,
        unread_replies: unread,
        participants: vec![User {
            id: "u2".into(),
            ..User::default()
        }],
        post: post(id, "c", 1),
        ..UserThread::default()
    };
    posts::upsert_threads(
        conn,
        "t",
        &[
            thread("old", 10, 0),
            thread("new", 30, 2),
            thread("mid", 20, 1),
        ],
    )
    .unwrap();

    let all = posts::threads(conn, "t", false, 10).unwrap();
    let order: Vec<&str> = all.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(order, ["new", "mid", "old"]);
    assert_eq!(all[0].participants, ["u2"]);
    // The root posts are stored too.
    assert!(posts::post(conn, "new").unwrap().is_some());

    let unread = posts::threads(conn, "t", true, 10).unwrap();
    assert_eq!(unread.len(), 2);
    posts::mark_thread_read(conn, "mid", 40, 0, 0).unwrap();
    posts::set_following(conn, "new", false).unwrap();
    assert!(posts::threads(conn, "t", true, 10).unwrap().is_empty());
    assert!(
        posts::threads(conn, "other-team", false, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn cursors_are_kept_and_since_only_moves_forward() {
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    assert!(posts::cursor(conn, posts::WS_RESUME_KEY).unwrap().is_none());
    posts::set_cursor(conn, posts::WS_RESUME_KEY, "conn-1:42").unwrap();
    assert_eq!(
        posts::cursor(conn, posts::WS_RESUME_KEY)
            .unwrap()
            .as_deref(),
        Some("conn-1:42")
    );

    let key = posts::channel_since_key("c");
    assert_eq!(posts::advance_since(conn, &key, 100).unwrap(), 100);
    assert_eq!(posts::advance_since(conn, &key, 50).unwrap(), 100);
    assert_eq!(posts::advance_since(conn, &key, 150).unwrap(), 150);
}

#[test]
fn clearing_the_outbox_drops_queued_sends_and_their_rows_only() {
    use matty_store::outbox::{self, Outgoing};
    let store = Store::open_in_memory().unwrap();
    let conn = store.conn();
    posts::insert_posts(
        conn,
        &[
            post("sent", "c", 10),
            post("me:1", "c", 20),
            post("me:2", "d", 30),
        ],
    )
    .unwrap();
    posts::upsert_files(conn, &[file("f1", "me:1", 20)]).unwrap();
    for (id, channel) in [("me:1", "c"), ("me:2", "d")] {
        outbox::push(
            conn,
            &Outgoing {
                pending_post_id: id.into(),
                channel_id: channel.into(),
                message: "queued".into(),
                ..Outgoing::default()
            },
        )
        .unwrap();
    }
    assert_eq!(outbox::clear(conn).unwrap(), 2);
    assert!(outbox::channels(conn).unwrap().is_empty());
    assert!(posts::post(conn, "me:1").unwrap().is_none());
    assert!(posts::post(conn, "me:2").unwrap().is_none());
    assert!(posts::files_for_post(conn, "me:1").unwrap().is_empty());
    assert!(posts::post(conn, "sent").unwrap().is_some());
    assert_eq!(outbox::clear(conn).unwrap(), 0);
}
