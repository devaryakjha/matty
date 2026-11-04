//! Opening a thread reads it from the server, so a thread whose replies
//! the cache never saw still shows them.
#![allow(clippy::unwrap_used)]

mod common;

use common::{connected, next, server};
use matty_sync::{ChangeSet, Command};
use serde_json::json;
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn an_opened_thread_comes_from_the_server() {
    let server = server().await;
    let post = |id: &str, root: &str, at: i64, message: &str| {
        json!({"id": id, "channel_id": "c1", "user_id": "ada", "root_id": root,
               "create_at": at, "update_at": at, "message": message})
    };
    Mock::given(method("GET"))
        .and(path("/api/v4/posts/root1/thread"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "order": ["r1", "root1"],
            "posts": {
                "root1": post("root1", "", 100, "Plan?"),
                "r1": post("r1", "root1", 200, "Yes"),
            },
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;

    sync.send(Command::OpenThread {
        root_id: "root1".into(),
    });
    let ChangeSet::Thread { posts, .. } = next(&changes, |c| {
        matches!(c, ChangeSet::Thread { root_id, posts, .. } if root_id == "root1" && posts.len() == 2)
    })
    .await
    else {
        unreachable!()
    };
    let messages: Vec<_> = posts.iter().map(|p| p.message.as_str()).collect();
    assert_eq!(messages, ["Plan?", "Yes"]);
}

/// A long thread opens on its newest page and pages back (#122): the
/// snapshot says older replies remain until the page that reaches the root.
#[tokio::test(flavor = "multi_thread")]
async fn a_long_thread_pages_back_to_its_root() {
    let server = server().await;
    let post = |id: &str, root: &str, at: i64| {
        json!({"id": id, "channel_id": "c1", "user_id": "ada", "root_id": root,
               "create_at": at, "update_at": at, "message": id})
    };
    Mock::given(method("GET"))
        .and(path("/api/v4/posts/root1/thread"))
        .and(query_param("direction", "up"))
        .and(query_param("perPage", "50"))
        .and(query_param_is_missing("fromPost"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "order": ["r4", "r3", "root1"],
            "posts": {
                "root1": post("root1", "", 100),
                "r3": post("r3", "root1", 300),
                "r4": post("r4", "root1", 400),
            },
            "has_next": true,
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/posts/root1/thread"))
        .and(query_param("direction", "up"))
        .and(query_param("fromPost", "r3"))
        .and(query_param("fromCreateAt", "300"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "order": ["r2", "r1", "root1"],
            "posts": {
                "root1": post("root1", "", 100),
                "r1": post("r1", "root1", 150),
                "r2": post("r2", "root1", 200),
            },
            "has_next": false,
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;

    sync.send(Command::OpenThread {
        root_id: "root1".into(),
    });
    let ChangeSet::Thread { posts, failed, .. } = next(
        &changes,
        |c| matches!(c, ChangeSet::Thread { posts, older: true, .. } if posts.len() == 3),
    )
    .await
    else {
        unreachable!()
    };
    let ids: Vec<_> = posts.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["root1", "r3", "r4"]);
    assert!(!failed);

    sync.send(Command::LoadOlderReplies {
        root_id: "root1".into(),
    });
    let ChangeSet::Thread { posts, .. } = next(
        &changes,
        |c| matches!(c, ChangeSet::Thread { older: false, posts, .. } if posts.len() == 5),
    )
    .await
    else {
        unreachable!()
    };
    let ids: Vec<_> = posts.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["root1", "r1", "r2", "r3", "r4"]);
    // At the root: nothing more to ask for.
    sync.send(Command::LoadOlderReplies {
        root_id: "root1".into(),
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

/// An older page that fails says so, and asking again retries it.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_older_page_is_reported_and_retried() {
    let server = server().await;
    let post = |id: &str, root: &str, at: i64| {
        json!({"id": id, "channel_id": "c1", "user_id": "ada", "root_id": root,
               "create_at": at, "update_at": at, "message": id})
    };
    Mock::given(method("GET"))
        .and(path("/api/v4/posts/root1/thread"))
        .and(query_param_is_missing("fromPost"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "order": ["r2", "root1"],
            "posts": {"root1": post("root1", "", 100), "r2": post("r2", "root1", 200)},
            "has_next": true,
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/posts/root1/thread"))
        .and(query_param("fromPost", "r2"))
        .respond_with(ResponseTemplate::new(500))
        .expect(2)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    sync.send(Command::OpenThread {
        root_id: "root1".into(),
    });
    next(&changes, |c| {
        matches!(c, ChangeSet::Thread { older: true, .. })
    })
    .await;
    for _ in 0..2 {
        sync.send(Command::LoadOlderReplies {
            root_id: "root1".into(),
        });
        next(&changes, |c| {
            matches!(c, ChangeSet::Thread { older: true, failed: true, posts, .. } if posts.len() == 2)
        })
        .await;
    }
}

/// With collapsed reply threads off, a reply whose root is older than the
/// channel's window comes with that root, for the "Commented on" line
/// (#123).
#[tokio::test(flavor = "multi_thread")]
async fn replies_come_with_roots_the_window_leaves_out() {
    let server = common::server_with_config(json!({"CollapsedThreads": "default_off"})).await;
    let post = |id: &str, root: &str, at: i64| {
        json!({"id": id, "channel_id": "c1", "user_id": "ada", "root_id": root,
               "create_at": at, "update_at": at, "message": id})
    };
    let mut posts = serde_json::Map::new();
    let mut order = Vec::new();
    posts.insert("old".into(), post("old", "", 1));
    for at in 10..10 + matty_sync::POSTS_WINDOW as i64 {
        let id = format!("p{at}");
        posts.insert(id.clone(), post(&id, "", at));
        order.push(id);
    }
    posts.insert("reply".into(), post("reply", "old", 1000));
    order.push("reply".into());
    order.reverse();
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v4/users/{}/channels/c1/posts/unread",
            common::ME
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"order": order, "posts": posts})),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    let (_sync, changes) = connected(&server).await;

    let ChangeSet::ReplyRoots { roots, .. } =
        next(&changes, |c| matches!(c, ChangeSet::ReplyRoots { .. })).await
    else {
        unreachable!()
    };
    let ids: Vec<_> = roots.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["old"]);
    let ChangeSet::Posts { posts, .. } =
        next(&changes, |c| matches!(c, ChangeSet::Posts { .. })).await
    else {
        unreachable!()
    };
    assert!(posts.iter().all(|p| p.id != "old"));
    assert_eq!(posts.last().map(|p| p.id.as_str()), Some("reply"));
}
