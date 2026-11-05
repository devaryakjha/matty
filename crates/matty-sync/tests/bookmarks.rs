//! Channel bookmarks (#226) and Apps calls reach the server, the store and
//! the app.
#![allow(clippy::unwrap_used)]

mod common;

use common::{connected, next, requested, server, server_licensed};
use matty_api::models::{AppBinding, AppCall, AppContext, ChannelBookmark};
use matty_api::ws::WsEvent;
use matty_sync::{AppAnswer, AppCommand, BookmarkCommand, ChangeSet, Command, CommandError};
use serde_json::json;
use wiremock::matchers::{body_json, body_partial_json, method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

fn bookmark(id: &str, order: i64) -> serde_json::Value {
    json!({
        "id": id, "create_at": order + 1, "update_at": 1, "delete_at": 0,
        "channel_id": "c1", "owner_id": "me", "file_id": "",
        "display_name": id.to_uppercase(), "sort_order": order,
        "link_url": format!("https://example.com/{id}"), "type": "link"
    })
}

fn ids(change: &ChangeSet) -> Vec<String> {
    let ChangeSet::Bookmarks { bookmarks, .. } = change else {
        panic!("{change:?}")
    };
    bookmarks.iter().map(|b| b.id.clone()).collect()
}

/// A connected engine with `c1` open, before the account has loaded, so
/// the first bookmarks fetch is not missed.
fn opened(
    server: &wiremock::MockServer,
) -> (matty_sync::SyncHandle, async_channel::Receiver<ChangeSet>) {
    let sync = matty_sync::spawn(matty_store::Store::open_in_memory().unwrap()).unwrap();
    let changes = sync.subscribe();
    sync.send(Command::OpenChannel {
        channel_id: "c1".into(),
    });
    sync.send(Command::Connect {
        client: matty_api::Client::new(server.uri())
            .unwrap()
            .with_token("token"),
    });
    (sync, changes)
}

fn is_bookmarks(change: &ChangeSet) -> bool {
    matches!(change, ChangeSet::Bookmarks { channel_id, .. } if channel_id == "c1")
}

#[tokio::test(flavor = "multi_thread")]
async fn bookmarks_load_change_and_undo_a_refusal() {
    let server = server_licensed(json!({})).await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/bookmarks"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([bookmark("b", 1), bookmark("a", 0)])),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/channels/c1/bookmarks"))
        .and(body_partial_json(json!({
            "display_name": "Docs", "link_url": "https://example.com/docs", "type": "link"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(bookmark("c", 2)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/channels/c1/bookmarks/c/sort_order"))
        .and(body_json(json!(0)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            bookmark("c", 0),
            bookmark("a", 1),
            bookmark("b", 2)
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v4/channels/c1/bookmarks/a"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "id": "api.context.permissions.app_error", "message": "No."
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = opened(&server);
    // The server's list, in bar order.
    let first = next(&changes, |c| is_bookmarks(c) && ids(c).len() == 2).await;
    assert_eq!(ids(&first), ["a", "b"]);

    sync.send(Command::Bookmark(BookmarkCommand::AddLink {
        channel_id: "c1".into(),
        display_name: "Docs".into(),
        link_url: "https://example.com/docs".into(),
        emoji: String::new(),
    }));
    let added = next(&changes, |c| is_bookmarks(c) && ids(c).len() == 3).await;
    assert_eq!(ids(&added), ["a", "b", "c"]);

    // A move shows at once, then the server's answer agrees.
    sync.send(Command::Bookmark(BookmarkCommand::Move {
        channel_id: "c1".into(),
        bookmark_id: "c".into(),
        index: 0,
    }));
    let moved = next(&changes, is_bookmarks).await;
    assert_eq!(ids(&moved), ["c", "a", "b"]);

    // A refused delete shows at once, then comes back with the reason.
    sync.send(Command::Bookmark(BookmarkCommand::Delete {
        channel_id: "c1".into(),
        bookmark_id: "a".into(),
    }));
    let gone = next(&changes, |c| {
        is_bookmarks(c) && !ids(c).contains(&"a".into())
    })
    .await;
    assert_eq!(ids(&gone).len(), 2);
    let refused = next(&changes, |c| matches!(c, ChangeSet::Failed(_))).await;
    let ChangeSet::Failed(CommandError::Server { action, error, .. }) = refused else {
        panic!("{refused:?}")
    };
    assert_eq!(action, "delete bookmark");
    assert!(error.contains("permission"), "{error}");
    // The server's list again, from the GET mock.
    let back = next(&changes, |c| {
        is_bookmarks(c) && ids(c).contains(&"a".into())
    })
    .await;
    assert_eq!(ids(&back), ["a", "b"]);
}

/// A file bookmark uploads its file as a bookmark's, then points the new
/// bookmark at it.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_bookmark_uploads_its_file_first() {
    let server = server_licensed(json!({})).await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/bookmarks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/files"))
        .and(query_param("bookmark", "true"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "file_infos": [{"id": "f1", "name": "plan.pdf", "extension": "pdf", "size": 8}],
            "client_ids": ["bookmark"]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let mut made = bookmark("p", 0);
    made["type"] = json!("file");
    made["file_id"] = json!("f1");
    made["link_url"] = json!("");
    Mock::given(method("POST"))
        .and(path("/api/v4/channels/c1/bookmarks"))
        .and(body_partial_json(json!({
            "display_name": "Plan", "type": "file", "file_id": "f1", "emoji": "memo"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(made))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = opened(&server);
    next(&changes, |c| is_bookmarks(c) && ids(c).is_empty()).await;
    sync.send(Command::Bookmark(BookmarkCommand::AddFile {
        channel_id: "c1".into(),
        display_name: "Plan".into(),
        emoji: "memo".into(),
        file_name: "plan.pdf".into(),
        bytes: b"%PDF-1.7".to_vec(),
    }));
    let added = next(&changes, |c| is_bookmarks(c) && !ids(c).is_empty()).await;
    let ChangeSet::Bookmarks { bookmarks, .. } = added else {
        unreachable!()
    };
    assert_eq!(bookmarks[0].file_id, "f1");
}

#[tokio::test(flavor = "multi_thread")]
async fn events_keep_the_bar_current_and_it_shows_from_the_cache() {
    let server = server_licensed(json!({})).await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/bookmarks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let (sync, changes) = opened(&server);
    next(&changes, |c| is_bookmarks(c) && ids(c).is_empty()).await;
    let made: ChannelBookmark = serde_json::from_value(bookmark("x", 0)).unwrap();
    sync.send(Command::Event(Box::new(WsEvent::ChannelBookmarkCreated {
        channel_id: "c1".into(),
        bookmark: Box::new(made),
    })));
    let shown = next(&changes, |c| is_bookmarks(c) && !ids(c).is_empty()).await;
    assert_eq!(ids(&shown), ["x"]);

    // Opening the channel again publishes the stored list at once.
    sync.send(Command::CloseChannel {
        channel_id: "c1".into(),
    });
    sync.send(Command::OpenChannel {
        channel_id: "c1".into(),
    });
    let again = next(&changes, is_bookmarks).await;
    assert_eq!(ids(&again), ["x"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_without_bookmarks_is_not_asked() {
    // Turned off, or unlicensed: the recorded 10.5 reports the flag on
    // and refuses every bookmark call without a license.
    for server in [
        server_licensed(json!({"FeatureFlagChannelBookmarks": "false"})).await,
        server().await,
    ] {
        let (_sync, _changes) = connected(&server).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            !requested(&server)
                .await
                .iter()
                .any(|path| path.ends_with("/bookmarks")),
            "no bookmarks request"
        );
    }
}

fn context() -> AppContext {
    AppContext {
        app_id: "hello".into(),
        location: "/post_menu/send".into(),
        user_agent: "desktop".into(),
        channel_id: "c1".into(),
        team_id: "t1".into(),
        post_id: "p1".into(),
        ..AppContext::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_form_opens_as_a_dialog_and_its_errors_come_back_to_it() {
    let server = server().await;
    Mock::given(method("GET"))
        .and(path("/plugins/com.mattermost.apps/api/v1/bindings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "location": "/post_menu",
            "bindings": [{"app_id": "hello", "location": "send", "label": "Send hello",
                          "submit": {"path": "/send"}}]
        }])))
        .mount(&server)
        .await;
    // The call answers with a form; its submission first fails on a
    // field, then goes through with a message.
    Mock::given(method("POST"))
        .and(path("/plugins/com.mattermost.apps/api/v1/call"))
        .and(body_partial_json(json!({"path": "/send"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "type": "form",
            "form": {"title": "Send hello", "submit": {"path": "/submit"},
                     "fields": [{"name": "to", "type": "text", "label": "To", "is_required": true}]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/plugins/com.mattermost.apps/api/v1/call"))
        .and(body_partial_json(json!({"path": "/submit", "values": {"to": "nobody"}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "type": "error", "text": "Check the form.", "data": {"errors": {"to": "No such person."}}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/plugins/com.mattermost.apps/api/v1/call"))
        .and(body_partial_json(
            json!({"path": "/submit", "values": {"to": "ada"}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "type": "ok", "text": "Hello sent to ada"
        })))
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    sync.send(Command::App(AppCommand::Bindings {
        channel_id: "c1".into(),
        team_id: "t1".into(),
    }));
    let ChangeSet::AppBindings { bindings, .. } =
        next(&changes, |c| matches!(c, ChangeSet::AppBindings { .. })).await
    else {
        unreachable!()
    };
    let binding: AppBinding = bindings[0].bindings[0].clone();

    sync.send(Command::App(AppCommand::Run {
        call_id: "7".into(),
        binding: Box::new(binding),
        context: Box::new(context()),
    }));
    let ChangeSet::OpenDialog(dialog) =
        next(&changes, |c| matches!(c, ChangeSet::OpenDialog(_))).await
    else {
        unreachable!()
    };
    assert!(dialog.url.starts_with(matty_sync::APP_DIALOG_URL));
    assert_eq!(dialog.dialog.elements[0].name, "to");

    let submit = |to: &str| Command::SubmitDialog {
        url: dialog.url.clone(),
        callback_id: dialog.dialog.callback_id.clone(),
        state: dialog.dialog.state.clone(),
        channel_id: "c1".into(),
        team_id: "t1".into(),
        submission: [("to".to_owned(), json!(to))].into(),
        cancelled: false,
    };
    sync.send(submit("nobody"));
    let ChangeSet::DialogSubmitted { answer, .. } =
        next(&changes, |c| matches!(c, ChangeSet::DialogSubmitted { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(answer.errors["to"], "No such person.");
    assert_eq!(answer.error, "Check the form.");

    sync.send(submit("ada"));
    let ChangeSet::DialogSubmitted { answer, .. } =
        next(&changes, |c| matches!(c, ChangeSet::DialogSubmitted { .. })).await
    else {
        unreachable!()
    };
    assert!(answer.accepted());
    let ChangeSet::Ephemeral(post) = next(&changes, |c| matches!(c, ChangeSet::Ephemeral(_))).await
    else {
        unreachable!()
    };
    assert_eq!(post.message, "Hello sent to ada");
    assert_eq!(post.channel_id, "c1");
    let answered = next(&changes, |c| matches!(c, ChangeSet::AppAnswered { .. })).await;
    assert_eq!(
        answered,
        ChangeSet::AppAnswered {
            call_id: "7".into(),
            answer: AppAnswer::Done
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_the_apps_plugin_nothing_shows_and_a_call_failure_is_told() {
    // A server of its own: its 404 is remembered by address.
    let server = common::server_unpooled().await;
    Mock::given(method("GET"))
        .and(path("/plugins/com.mattermost.apps/api/v1/bindings"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/plugins/com.mattermost.apps/api/v1/call"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    let bindings = || {
        Command::App(AppCommand::Bindings {
            channel_id: "c1".into(),
            team_id: "t1".into(),
        })
    };
    sync.send(bindings());
    // Once the 404 is in, the next channel does not ask again.
    for _ in 0..100 {
        if requested(&server)
            .await
            .iter()
            .any(|p| p.ends_with("/bindings"))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    sync.send(bindings());
    sync.send(Command::App(AppCommand::Run {
        call_id: "8".into(),
        binding: Box::new(AppBinding {
            submit: Some(AppCall {
                path: "/x".into(),
                ..AppCall::default()
            }),
            ..AppBinding::default()
        }),
        context: Box::new(context()),
    }));
    let answered = next(&changes, |c| {
        matches!(
            c,
            ChangeSet::AppAnswered { .. } | ChangeSet::AppBindings { .. }
        )
    })
    .await;
    assert!(
        matches!(
            &answered,
            ChangeSet::AppAnswered {
                answer: AppAnswer::Failed(_),
                ..
            }
        ),
        "{answered:?}"
    );
}

/// A dynamic select's options come from the App's lookup call, with the
/// field and what the user typed.
#[tokio::test(flavor = "multi_thread")]
async fn a_dynamic_select_looks_up_its_options() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/plugins/com.mattermost.apps/api/v1/call"))
        .and(body_partial_json(json!({
            "path": "/issues/lookup", "selected_field": "issue", "query": "crash",
            "values": {"to": "ada"}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "type": "ok",
            "data": {"items": [
                {"label": "MM-1 Crash on start", "value": "MM-1"},
                {"label": "MM-7 Crash on quit", "value": "MM-7"}
            ]}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (sync, changes) = connected(&server).await;
    // A binding with its form given whole shows it without a call.
    let binding: AppBinding = serde_json::from_value(json!({
        "app_id": "hello", "location": "file", "label": "File an issue",
        "form": {
            "submit": {"path": "/submit"},
            "fields": [
                {"name": "to", "type": "text"},
                {"name": "issue", "type": "dynamic_select", "lookup": {"path": "/issues/lookup"}}
            ]
        }
    }))
    .unwrap();
    sync.send(Command::App(AppCommand::Run {
        call_id: "9".into(),
        binding: Box::new(binding),
        context: Box::new(context()),
    }));
    let ChangeSet::OpenDialog(dialog) =
        next(&changes, |c| matches!(c, ChangeSet::OpenDialog(_))).await
    else {
        unreachable!()
    };
    assert_eq!(
        dialog.dialog.elements[1].data_source,
        matty_sync::APP_LOOKUP_SOURCE
    );
    sync.send(Command::App(AppCommand::Lookup {
        callback_id: dialog.dialog.callback_id.clone(),
        state: dialog.dialog.state.clone(),
        field: "issue".into(),
        query: "crash".into(),
        values: [("to".to_owned(), json!("ada"))].into(),
    }));
    let ChangeSet::AppLookup {
        callback_id,
        field,
        query,
        options,
    } = next(&changes, |c| matches!(c, ChangeSet::AppLookup { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(
        (callback_id.as_str(), field.as_str(), query.as_str()),
        ("9", "issue", "crash")
    );
    let values: Vec<_> = options.iter().map(|o| o.value.as_str()).collect();
    assert_eq!(values, ["MM-1", "MM-7"]);
}
