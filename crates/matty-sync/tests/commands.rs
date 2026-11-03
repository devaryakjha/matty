//! Commands the views send (#39), against a mock server.
#![allow(clippy::unwrap_used)] // Test helpers unwrap like the tests do.

mod common;

use common::{ME, connected, get, next, server};
use matty_sync::{ChangeSet, Command, CommandError};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

fn reactions(change: &ChangeSet) -> Option<Vec<String>> {
    match change {
        ChangeSet::Posts { posts, .. } => posts.first().map(|post| {
            post.metadata
                .reactions
                .iter()
                .map(|r| format!("{}:{}", r.user_id, r.emoji_name))
                .collect()
        }),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_reaction_is_undone() {
    let server = server().await;
    get(
        &format!("users/{ME}/channels/c1/posts/unread"),
        json!({"order": ["p1"], "posts": {"p1": {
            "id": "p1", "channel_id": "c1", "user_id": "u2", "create_at": 100,
            "update_at": 100, "message": "react to me"
        }}}),
    )
    .with_priority(1)
    .mount(&server)
    .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/reactions"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "id": "api.context.permissions.app_error", "message": "No permission.",
            "status_code": 403
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    // Opening again publishes the window, whether or not its first
    // snapshot came before the load finished.
    sync.send(Command::OpenChannel {
        channel_id: "c1".into(),
    });
    next(&changes, |c| reactions(c).is_some()).await;

    sync.send(Command::ToggleReaction {
        post_id: "p1".into(),
        emoji_name: "+1".into(),
    });
    // Shown at once, then taken back when the server refuses.
    next(&changes, |c| {
        reactions(c).is_some_and(|r| r == [format!("{ME}:+1")])
    })
    .await;
    let refused = next(&changes, |c| matches!(c, ChangeSet::Failed(_))).await;
    assert!(matches!(
        refused,
        ChangeSet::Failed(CommandError::Server { action: "add reaction", target, .. }) if target == "p1"
    ));
    next(&changes, |c| reactions(c).is_some_and(|r| r.is_empty())).await;
}
