//! The Threads view (#124): opening it reads the followed threads from the
//! server, and reading or unfollowing one shows at once and reaches the
//! server.
#![allow(clippy::unwrap_used)]

mod common;

use common::{ME, connected, next, server};
use matty_sync::{ChangeSet, Command};
use serde_json::json;
use wiremock::matchers::{method, path, path_regex, query_param, query_param_is_missing};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn the_thread_list_loads_reads_and_unfollows() {
    let server = server().await;
    let root = json!({"id": "root1", "channel_id": "c1", "user_id": "ada", "root_id": "",
                      "create_at": 100, "update_at": 100, "message": "Plan?"});
    let thread = json!({"id": "root1", "reply_count": 2, "last_reply_at": 300,
                        "last_viewed_at": 100, "participants": [], "post": root,
                        "unread_replies": 2, "unread_mentions": 1});
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/users/{ME}/teams/t1/threads")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "threads": [thread], "total": 1,
        })))
        .expect(1)
        .mount(&server)
        .await;
    let mut read = thread.clone();
    read["unread_replies"] = json!(0);
    read["unread_mentions"] = json!(0);
    Mock::given(method("PUT"))
        .and(path_regex(format!(
            "^/api/v4/users/{ME}/teams/t1/threads/root1/read/[0-9]+$"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(read))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/api/v4/users/{ME}/teams/t1/threads/root1/following"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;

    sync.send(Command::OpenThreadList {
        team_id: "t1".into(),
    });
    let ChangeSet::ThreadList { threads, roots, .. } = next(
        &changes,
        |c| matches!(c, ChangeSet::ThreadList { threads, .. } if threads.len() == 1),
    )
    .await
    else {
        unreachable!()
    };
    assert_eq!(
        (threads[0].unread_replies, threads[0].unread_mentions),
        (2, 1)
    );
    assert_eq!(roots[0].message, "Plan?");

    sync.send(Command::ReadThread {
        team_id: "t1".into(),
        root_id: "root1".into(),
    });
    next(&changes, |c| {
        matches!(c, ChangeSet::ThreadList { threads, .. }
            if threads.len() == 1 && threads[0].unread_replies == 0)
    })
    .await;

    sync.send(Command::FollowThread {
        team_id: "t1".into(),
        root_id: "root1".into(),
        follow: false,
    });
    next(
        &changes,
        |c| matches!(c, ChangeSet::ThreadList { threads, .. } if threads.is_empty()),
    )
    .await;
    // Let the server calls land before the mocks check their counts.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

/// The Threads list pages past the server's first page (#122): it says
/// more threads remain until a page comes back short.
#[tokio::test(flavor = "multi_thread")]
async fn the_thread_list_pages_until_a_short_page() {
    let server = server().await;
    let thread = |id: &str, at: i64| {
        json!({"id": id, "reply_count": 1, "last_reply_at": at, "last_viewed_at": at,
               "participants": [], "unread_replies": 0, "unread_mentions": 0,
               "post": {"id": id, "channel_id": "c1", "user_id": "ada", "root_id": "",
                        "create_at": at, "update_at": at, "message": id}})
    };
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/users/{ME}/teams/t1/threads")))
        .and(query_param_is_missing("before"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "threads": [thread("new", 300)], "total": 2,
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/users/{ME}/teams/t1/threads")))
        .and(query_param("before", "new"))
        .and(query_param("pageSize", "25"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "threads": [thread("old", 100)], "total": 2,
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;

    sync.send(Command::OpenThreadList {
        team_id: "t1".into(),
    });
    next(&changes, |c| {
        matches!(c, ChangeSet::ThreadList { threads, more: true, failed: false, .. }
            if threads.len() == 1)
    })
    .await;
    sync.send(Command::LoadMoreThreads {
        team_id: "t1".into(),
    });
    let ChangeSet::ThreadList { threads, .. } = next(
        &changes,
        |c| matches!(c, ChangeSet::ThreadList { threads, more: false, .. } if threads.len() == 2),
    )
    .await
    else {
        unreachable!()
    };
    let ids: Vec<_> = threads.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["new", "old"]);
    // A short page was the last: nothing more to ask for.
    sync.send(Command::LoadMoreThreads {
        team_id: "t1".into(),
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}
