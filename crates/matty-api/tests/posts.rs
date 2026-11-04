//! Post, thread and read-state models decode the responses recorded from the
//! seeded Mattermost 10.5 dev server (`scripts/record-fixtures.py`), and the
//! endpoints send the queries and bodies the server expects.

#![allow(clippy::unwrap_used)]

use matty_api::{
    Client, Direction, NewPost, PostPatch, PostsQuery, ThreadQuery, ThreadsQuery,
    models::{
        ChannelUnreadAt, ChannelViewed, Draft, EmbedKind, Post, PostChange, PostList, UserThread,
        UserThreads,
    },
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, body_partial_json, method, path, query_param, query_param_is_missing},
};

fn fixture<T: DeserializeOwned>(name: &str) -> T {
    let path = format!(
        "{}/tests/fixtures/v10.5/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn post_lists_decode_order_posts_and_paging() {
    let list: PostList = fixture("channel_posts");
    assert_eq!(list.order.len(), 30);
    assert!(list.order.iter().all(|id| list.posts.contains_key(id)));
    let ordered: Vec<&Post> = list.ordered().collect();
    assert!(
        ordered.windows(2).all(|w| w[0].create_at >= w[1].create_at),
        "channel lists are newest first"
    );
    // The newest page has nothing newer, and more posts before it.
    assert_eq!(list.next_post_id, "");
    assert!(!list.prev_post_id.is_empty());
    assert!(!list.posts.contains_key(&list.prev_post_id));

    let crt: PostList = fixture("channel_posts_crt");
    assert!(
        crt.ordered().all(|p| !p.is_reply()),
        "CRT leaves replies out"
    );
    assert!(crt.ordered().any(|p| p.reply_count > 0));

    let thread: PostList = fixture("post_thread");
    let posts: Vec<&Post> = thread.ordered().collect();
    let root = posts[0];
    assert!(!root.is_reply());
    assert!(posts[1..].iter().all(|p| p.root_id == root.id));
    assert!(
        posts.windows(2).all(|w| w[0].create_at <= w[1].create_at),
        "threads are oldest first"
    );
    // perPage=10 counts the root; the thread has more when replies overflow it.
    assert_eq!(thread.has_next, root.reply_count + 1 > posts.len() as i64);

    let around: PostList = fixture("posts_around_unread");
    assert!(!around.order.is_empty());
}

#[test]
fn metadata_decodes_embeds_images_files_reactions_and_emoji() {
    let post: Post = fixture("post_opengraph");
    let embed = &post.metadata.embeds[0];
    assert_eq!(embed.kind, EmbedKind::OpenGraph);
    let og = embed.opengraph().unwrap();
    assert_eq!(og.title, "Matty preview page");
    assert_eq!(og.site_name, "Matty dev server");
    let image_url = &og.images[0].url;
    let image = &post.metadata.images[image_url];
    assert!(image.width > 0 && image.height > 0);
    assert_eq!(image.format, "png");

    let post: Post = fixture("post_image_link");
    let embed = &post.metadata.embeds[0];
    assert_eq!(embed.kind, EmbedKind::Image);
    assert_eq!(embed.opengraph(), None);
    let image = &post.metadata.images[&embed.url];
    assert_eq!((image.width, image.height), (300, 51));

    let post: Post = fixture("post_link");
    assert_eq!(post.metadata.embeds[0].kind, EmbedKind::Link);

    let post: Post = fixture("post_attachment");
    assert_eq!(post.metadata.embeds[0].kind, EmbedKind::MessageAttachment);
    let attachment = &post.attachments()[0];
    assert_eq!(attachment.title, "Build 42");
    assert_eq!(attachment.color, "#2eb67d");
    assert_eq!(attachment.fields.len(), 2);
    assert_eq!(attachment.fields[1].value, json!(95));
    assert!(attachment.fields[0].short);

    // An interactive message, as the server sends it: the integration's
    // URL stripped, each action sealed into a cookie.
    let path = format!(
        "{}/tests/fixtures/post_interactive.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let post: Post = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let attachment = &post.attachments()[0];
    assert_eq!(
        attachment.author_icon,
        "https://deploy.example.com/icon.png"
    );
    assert_eq!(attachment.thumb_url, "https://deploy.example.com/thumb.png");
    assert_eq!(
        attachment.footer_icon,
        "https://deploy.example.com/footer.png"
    );
    assert!(!attachment.fields[2].short);
    let actions: Vec<(&str, bool, bool)> = attachment
        .actions
        .iter()
        .map(|a| (a.id.as_str(), a.is_select(), a.disabled))
        .collect();
    assert_eq!(
        actions,
        [
            ("approve", false, false),
            ("reject", false, true),
            ("region", true, false),
            ("reviewer", true, false)
        ]
    );
    assert_eq!(attachment.actions[0].style, "primary");
    assert!(!attachment.actions[0].cookie.is_empty());
    let region = &attachment.actions[2];
    assert_eq!(region.default_option, "eu-central-1");
    assert_eq!(region.options[1].text, "US East");
    assert_eq!(attachment.actions[3].data_source, "users");
    assert!(attachment.actions[3].options.is_empty(), "null options");

    let post: Post = fixture("post_with_files");
    let file = &post.metadata.files[0];
    assert_eq!(post.file_ids, vec![file.id.clone()]);
    assert_eq!(file.mime_type, "image/png");
    let (w, h) = file
        .name
        .trim_start_matches("image-")
        .trim_end_matches(".png")
        .split_once('x')
        .unwrap();
    assert_eq!(
        (file.width, file.height),
        (w.parse().unwrap(), h.parse().unwrap())
    );
    assert!(file.has_preview_image);
    assert!(file.mini_preview.is_some());

    let post: Post = fixture("post_reactions");
    let names: Vec<&str> = post
        .metadata
        .reactions
        .iter()
        .map(|r| r.emoji_name.as_str())
        .collect();
    assert_eq!(names, ["matty", "+1"]);
    assert_eq!(post.metadata.emojis[0].name, "matty");
}

#[test]
fn unknown_embed_kinds_and_null_lists_decode() {
    let post: Post = serde_json::from_value(json!({
        "id": "p",
        "props": null,
        "file_ids": null,
        "participants": null,
        "metadata": {
            "embeds": [{"type": "something_new", "url": "https://example.com"}],
            "reactions": null,
            "files": null
        }
    }))
    .unwrap();
    assert_eq!(post.metadata.embeds[0].kind, EmbedKind::Unknown);
    assert!(post.props.is_empty() && post.file_ids.is_empty());
    assert!(post.attachments().is_empty());
}

#[test]
fn since_responses_surface_deletes() {
    let list: PostList = fixture("channel_posts_since");
    let changes = list.changes();
    let deleted: Vec<&str> = changes
        .iter()
        .filter_map(|c| match c {
            PostChange::Deleted { id, .. } => Some(*id),
            PostChange::Upsert(_) => None,
        })
        .collect();
    let raw: Value = fixture("channel_posts_since");
    let expected: Vec<&str> = raw["posts"]
        .as_object()
        .unwrap()
        .values()
        .filter(|p| p["delete_at"].as_i64().unwrap() > 0)
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert!(!expected.is_empty());
    assert_eq!(deleted.len(), expected.len());
    assert!(expected.iter().all(|id| deleted.contains(id)));

    // The edited post is an upsert carrying the new message.
    let edited = changes.iter().find_map(|c| match c {
        PostChange::Upsert(p) if p.edit_at > 0 => Some(p),
        _ => None,
    });
    assert_eq!(edited.unwrap().message, "Edited by the recorder");
    // Oldest change first.
    let times: Vec<i64> = list
        .changes()
        .iter()
        .map(|c| match c {
            PostChange::Upsert(p) => list.posts[&p.id].update_at,
            PostChange::Deleted { id, .. } => list.posts[*id].update_at,
        })
        .collect();
    assert!(times.windows(2).all(|w| w[0] <= w[1]));
}

#[test]
fn writes_and_read_state_decode() {
    let created: Post = fixture("post_created");
    assert!(
        created
            .pending_post_id
            .starts_with(&format!("{}:", created.user_id))
    );
    let patched: Post = fixture("post_patched");
    assert_eq!(patched.id, created.id);
    assert!(patched.edit_at > 0);

    let threads: UserThreads = fixture("user_threads");
    assert_eq!(threads.total, threads.threads.len() as i64);
    let thread = &threads.threads[0];
    assert_eq!(thread.id, thread.post.id);
    assert!(!thread.participants.is_empty());
    assert!(!thread.participants[0].username.is_empty(), "extended=true");
    let unread: UserThreads = fixture("user_threads_unread");
    assert!(unread.threads.iter().all(|t| t.unread_replies > 0));
    assert_eq!(unread.total_unread_threads, threads.total_unread_threads);

    let read: UserThread = fixture("thread_read");
    assert_eq!(read.unread_replies, 0);
    let viewed: ChannelViewed = fixture("channel_viewed");
    assert_eq!(viewed.status, "OK");
    assert_eq!(viewed.last_viewed_at_times.len(), 2);
    let unread_at: ChannelUnreadAt = fixture("channel_unread_at");
    assert!(unread_at.last_viewed_at > 0);
    assert!(!unread_at.channel_id.is_empty());
}

#[tokio::test]
async fn channel_posts_sends_paging_or_since() {
    let server = MockServer::start().await;
    let list: Value = fixture("channel_posts");
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/posts"))
        .and(query_param("before", "p9"))
        .and(query_param("per_page", "60"))
        .and(query_param("page", "0"))
        .and(query_param("collapsedThreads", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&list))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/posts"))
        .and(query_param("since", "1700000000000"))
        .and(query_param_is_missing("per_page"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&list))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    let page = client
        .channel_posts("c1", &PostsQuery::before("p9", 60).collapsed_threads(true))
        .await
        .unwrap();
    assert_eq!(page.order.len(), 30);
    client
        .channel_posts("c1", &PostsQuery::since(1_700_000_000_000))
        .await
        .unwrap();
}

#[tokio::test]
async fn thread_and_threads_queries() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/posts/r1/thread"))
        .and(query_param("perPage", "50"))
        .and(query_param("direction", "up"))
        .and(query_param("fromCreateAt", "123"))
        .and(query_param("fromPost", "p5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture::<Value>("post_thread")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/u1/teams/t1/threads"))
        .and(query_param("since", "99"))
        .and(query_param("unread", "true"))
        .and(query_param("pageSize", "25"))
        .and(query_param("extended", "true"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(fixture::<Value>("user_threads_unread")),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    client
        .post_thread("r1", &ThreadQuery::from("p5", 123, Direction::Up, 50))
        .await
        .unwrap();
    let query = ThreadsQuery {
        since: Some(99),
        unread: true,
        per_page: 25,
        extended: true,
        ..Default::default()
    };
    client.user_threads("u1", "t1", &query).await.unwrap();
}

#[tokio::test]
async fn writes_send_the_expected_bodies() {
    let server = MockServer::start().await;
    let created: Value = fixture("post_created");
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .and(body_json(json!({
            "channel_id": "c1",
            "message": "hi",
            "root_id": "r1",
            "pending_post_id": "u1:1700000000000",
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(&created))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v4/posts/p1/patch"))
        .and(body_json(json!({"message": "edited"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture::<Value>("post_patched")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v4/posts/p1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture::<Value>("status_ok")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/channels/members/me/view"))
        .and(body_json(json!({
            "channel_id": "c1",
            "prev_channel_id": "c0",
            "collapsed_threads_supported": true,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture::<Value>("channel_viewed")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/u1/posts/p1/set_unread"))
        .and(body_json(json!({"collapsed_threads_supported": true})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(fixture::<Value>("channel_unread_at")),
        )
        .expect(1)
        .mount(&server)
        .await;
    for verb in ["PUT", "DELETE"] {
        Mock::given(method(verb))
            .and(path("/api/v4/users/u1/teams/t1/threads/r1/following"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture::<Value>("status_ok")))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("PUT"))
        .and(path(
            "/api/v4/users/u1/teams/t1/threads/r1/read/1700000000000",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture::<Value>("thread_read")))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::new(server.uri()).unwrap().with_token("t");
    let post = client
        .create_post(&NewPost::new("c1", "hi", "u1:1700000000000").reply_to("r1"))
        .await
        .unwrap();
    assert_eq!(post.pending_post_id, created["pending_post_id"]);
    let patch = PostPatch {
        message: Some("edited".into()),
        ..Default::default()
    };
    client.patch_post("p1", &patch).await.unwrap();
    client.delete_post("p1").await.unwrap();
    client.view_channel("c1", "c0").await.unwrap();
    client.set_post_unread("u1", "p1", true).await.unwrap();
    client.follow_thread("u1", "t1", "r1").await.unwrap();
    client.unfollow_thread("u1", "t1", "r1").await.unwrap();
    client
        .mark_thread_read("u1", "t1", "r1", 1_700_000_000_000)
        .await
        .unwrap();
}

#[tokio::test]
async fn reactions_and_drafts_send_the_expected_requests() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/reactions"))
        .and(body_json(
            json!({"user_id": "u1", "post_id": "p1", "emoji_name": "+1"}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "user_id": "u1", "post_id": "p1", "emoji_name": "+1", "create_at": 5,
            "update_at": 5, "delete_at": 0, "remote_id": "", "channel_id": "c1",
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v4/users/u1/posts/p1/reactions/+1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture::<Value>("status_ok")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/drafts"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "create_at": 1, "update_at": 2, "delete_at": 0, "user_id": "u1",
            "channel_id": "c1", "root_id": "", "message": "draft", "props": {},
            "metadata": {},
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/u1/teams/t1/drafts"))
        .respond_with(ResponseTemplate::new(200).set_body_string("null"))
        .expect(1)
        .mount(&server)
        .await;
    for at in [
        "/api/v4/users/u1/channels/c1/drafts",
        "/api/v4/users/u1/channels/c1/drafts/r1",
    ] {
        Mock::given(method("DELETE"))
            .and(path(at))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture::<Value>("status_ok")))
            .expect(1)
            .mount(&server)
            .await;
    }

    let client = Client::new(server.uri()).unwrap().with_token("t");
    let reaction = client.add_reaction("u1", "p1", "+1").await.unwrap();
    assert_eq!(reaction.create_at, 5);
    client.remove_reaction("u1", "p1", "+1").await.unwrap();
    let draft = Draft {
        user_id: "u1".into(),
        channel_id: "c1".into(),
        message: "draft".into(),
        ..Default::default()
    };
    assert_eq!(client.save_draft(&draft).await.unwrap().update_at, 2);
    assert!(client.drafts("u1", "t1").await.unwrap().is_empty());
    client.delete_draft("u1", "c1", "").await.unwrap();
    client.delete_draft("u1", "c1", "r1").await.unwrap();
}

#[tokio::test]
async fn post_actions_send_the_choice_and_the_cookie() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts/p1/actions/approve"))
        .and(body_json(json!({ "cookie": "sealed" })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "status": "OK", "trigger_id": "" })),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts/p1/actions/region"))
        .and(body_json(json!({ "selected_option": "us-east-1" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "OK" })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts/p1/actions/gone"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "id": "api.post.do_action.action_integration.app_error",
            "message": "Action integration error.",
            "status_code": 400
        })))
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    let answer = client
        .do_post_action("p1", "approve", None, "sealed")
        .await
        .unwrap();
    assert_eq!(answer.status, "OK");
    client
        .do_post_action("p1", "region", Some("us-east-1"), "")
        .await
        .unwrap();
    let error = client
        .do_post_action("p1", "gone", None, "")
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("Action integration error"),
        "{error}"
    );
}

#[tokio::test]
async fn dialogs_submit_their_values_and_read_the_integrations_errors() {
    use matty_api::models::{SubmitDialogRequest, SubmitDialogResponse};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/actions/dialogs/submit"))
        .and(body_json(json!({
            "type": "dialog_submission",
            "url": "https://hook.example.com/dialog",
            "callback_id": "deploy",
            "state": "s",
            "user_id": "u1",
            "channel_id": "c1",
            "team_id": "t1",
            "submission": {"reason": "hi", "notify": true},
            "cancelled": false
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": "Not now.",
            "errors": {"reason": "Say more."}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/actions/dialogs/submit"))
        .and(body_partial_json(
            json!({"cancelled": true, "callback_id": "gone"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string("null"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/actions/dialogs/submit"))
        .and(body_partial_json(json!({"callback_id": "quiet"})))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    let submit = SubmitDialogRequest {
        kind: "dialog_submission".into(),
        url: "https://hook.example.com/dialog".into(),
        callback_id: "deploy".into(),
        state: "s".into(),
        user_id: "u1".into(),
        channel_id: "c1".into(),
        team_id: "t1".into(),
        submission: [
            ("reason".to_owned(), json!("hi")),
            ("notify".to_owned(), json!(true)),
        ]
        .into(),
        cancelled: false,
    };
    let answer = client.submit_dialog(&submit).await.unwrap();
    assert!(!answer.accepted());
    assert_eq!(answer.error, "Not now.");
    assert_eq!(answer.errors["reason"], "Say more.");
    let cancel = SubmitDialogRequest {
        callback_id: "gone".into(),
        submission: Default::default(),
        cancelled: true,
        ..submit.clone()
    };
    assert_eq!(
        client.submit_dialog(&cancel).await.unwrap(),
        SubmitDialogResponse::default(),
        "a null answer is a plain yes"
    );
    let quiet = SubmitDialogRequest {
        callback_id: "quiet".into(),
        ..submit
    };
    assert!(client.submit_dialog(&quiet).await.unwrap().accepted());
}
