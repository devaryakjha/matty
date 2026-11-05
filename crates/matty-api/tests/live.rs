//! Runs against a real server when `MATTY_TEST_SERVER` is set, such as the
//! seeded dev server (docs/dev-server.md): `MATTY_TEST_SERVER=http://localhost:8065
//! MATTY_TEST_PASSWORD='Password1!' cargo test -p matty-api --test live`.
//! `MATTY_TEST_LOGIN` defaults to the seed's `tester`, who must be in a team
//! with a public channel. Without the server, each test returns at once.

#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use matty_api::{
    ApiError, Client, Credentials, Fetched, NewPost, PostPatch, PostsQuery, SearchQuery,
    ThreadQuery, ThreadsQuery, Upload,
    models::{PostChange, Preference},
    ws::{ConnectionState, WsClient, WsConnection, WsMessage, WsSignal},
};

fn server() -> Option<(String, String, String)> {
    let url = std::env::var("MATTY_TEST_SERVER").ok()?;
    let login = std::env::var("MATTY_TEST_LOGIN").unwrap_or_else(|_| "tester".into());
    let password = std::env::var("MATTY_TEST_PASSWORD").ok()?;
    Some((url, login, password))
}

#[tokio::test]
async fn login_load_and_logout() {
    let Some((url, login, password)) = server() else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return;
    };
    let client = Client::new(url).unwrap();
    assert!(client.ping().await.unwrap().is_ok());
    let caps = client.capabilities().await.unwrap();
    caps.minimum_supported().unwrap();

    let wrong = Credentials {
        login_id: login.clone(),
        password: "not-the-password".into(),
        ..Default::default()
    };
    let error = client.login(&wrong).await.unwrap_err();
    assert_eq!(
        error.id(),
        Some("api.user.login.invalid_credentials_email_username")
    );

    let credentials = Credentials {
        login_id: login.clone(),
        password,
        ..Default::default()
    };
    let signed_in = client.login(&credentials).await.unwrap();
    let session = signed_in.client;
    assert_eq!(signed_in.user.username, login);
    let teams = session.my_teams().await.unwrap();
    let team = &teams.first().expect("the test user is in a team").id;
    let channels = session.my_channels(team).await.unwrap();
    let members = session
        .my_channel_members(&signed_in.user.id, team)
        .await
        .unwrap();
    assert_eq!(channels.len(), members.len());
    session.sidebar_categories(team).await.unwrap();
    session.preferences().await.unwrap();
    session.all_emoji().await.unwrap();
    session.team_unreads(&signed_in.user.id).await.unwrap();

    session.logout().await.unwrap();
    let after = session.me().await.unwrap_err();
    assert!(
        matches!(after, ApiError::Server { status: 401, .. }),
        "{after:?}"
    );
}

#[tokio::test]
async fn post_thread_and_read_state_round_trip() {
    let Some((url, login, password)) = server() else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return;
    };
    let credentials = Credentials {
        login_id: login,
        password,
        // Signing in revokes other sessions with the same device id.
        ..Default::default()
    };
    let signed_in = Client::new(url).unwrap().login(&credentials).await.unwrap();
    let (client, me) = (signed_in.client, signed_in.user);
    let team = client.my_teams().await.unwrap().remove(0);
    let channel = client
        .my_channels(&team.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.kind == "O")
        .expect("a public channel");

    let pending = pending_id(&me.id);
    let root = client
        .create_post(&NewPost::new(&channel.id, "live test root", &pending))
        .await
        .unwrap();
    assert_eq!(
        root.pending_post_id, pending,
        "the server echoes the pending id"
    );
    let reply = client
        .create_post(
            &NewPost::new(&channel.id, "live test reply", format!("{pending}-r"))
                .reply_to(&root.id),
        )
        .await
        .unwrap();
    assert_eq!(reply.root_id, root.id);

    let edited = client
        .patch_post(
            &root.id,
            &PostPatch {
                message: Some("live test root, edited".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(edited.edit_at > 0);
    assert_eq!(
        client.get_post(&root.id).await.unwrap().message,
        edited.message
    );

    let thread = client
        .post_thread(&root.id, &ThreadQuery::all())
        .await
        .unwrap();
    assert_eq!(thread.order, vec![root.id.clone(), reply.id.clone()]);
    let latest = client
        .channel_posts(&channel.id, &PostsQuery::latest(5).collapsed_threads(true))
        .await
        .unwrap();
    // Other live tests post to the same channel at the same time, so
    // check membership rather than position.
    assert!(
        latest.order.contains(&root.id) && !latest.order.contains(&reply.id),
        "CRT lists the root, not the reply"
    );

    client
        .follow_thread(&me.id, &team.id, &root.id)
        .await
        .unwrap();
    let threads = client
        .user_threads(&me.id, &team.id, &ThreadsQuery::default())
        .await
        .unwrap();
    assert!(threads.threads.iter().any(|t| t.id == root.id));
    client
        .mark_thread_read(&me.id, &team.id, &root.id, reply.create_at)
        .await
        .unwrap();
    client
        .unfollow_thread(&me.id, &team.id, &root.id)
        .await
        .unwrap();

    client.view_channel(&channel.id, "").await.unwrap();
    let unread = client
        .set_post_unread(&me.id, &root.id, false)
        .await
        .unwrap();
    assert_eq!(unread.channel_id, channel.id);
    assert!(unread.last_viewed_at < root.create_at);
    client.view_channel(&channel.id, "").await.unwrap();

    client.delete_post(&reply.id).await.unwrap();
    client.delete_post(&root.id).await.unwrap();
    let since = client
        .channel_posts(&channel.id, &PostsQuery::since(root.create_at))
        .await
        .unwrap();
    let deleted: Vec<&str> = since
        .changes()
        .into_iter()
        .filter_map(|c| match c {
            PostChange::Deleted { id, .. } => Some(id),
            PostChange::Upsert(_) => None,
        })
        .collect();
    assert!(deleted.contains(&root.id.as_str()) && deleted.contains(&reply.id.as_str()));
}

#[tokio::test]
async fn search_pin_save_and_direct_messages() {
    let Some((url, login, password)) = server() else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return;
    };
    let credentials = Credentials {
        login_id: login,
        password,
        ..Default::default()
    };
    let signed_in = Client::new(url).unwrap().login(&credentials).await.unwrap();
    let (client, me) = (signed_in.client, signed_in.user);
    let team = client.my_teams().await.unwrap().remove(0);
    let channel = client
        .my_channels(&team.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.kind == "O")
        .expect("a public channel");

    // Search finds a post by a word only it has.
    let word = format!("needle{}", web_time_millis());
    let pending = pending_id(&me.id);
    let post = client
        .create_post(&NewPost::new(
            &channel.id,
            format!("live search {word}"),
            &pending,
        ))
        .await
        .unwrap();
    let found = client
        .search_posts(&team.id, &SearchQuery::new(&word))
        .await
        .unwrap();
    assert_eq!(found.list.order, std::slice::from_ref(&post.id));

    // Pinning shows in the channel's pinned list, and unpinning removes it.
    client.pin_post(&post.id).await.unwrap();
    assert!(client.get_post(&post.id).await.unwrap().is_pinned);
    let pinned = client.pinned_posts(&channel.id).await.unwrap();
    assert!(pinned.order.contains(&post.id));
    client.unpin_post(&post.id).await.unwrap();
    let pinned = client.pinned_posts(&channel.id).await.unwrap();
    assert!(!pinned.order.contains(&post.id));

    // Saving is a `flagged_post` preference.
    let flag = Preference {
        user_id: me.id.clone(),
        category: "flagged_post".into(),
        name: post.id.clone(),
        value: "true".into(),
    };
    client
        .save_preferences(&me.id, std::slice::from_ref(&flag))
        .await
        .unwrap();
    let saved = client.saved_posts(&me.id, 0, 200).await.unwrap();
    assert!(saved.order.contains(&post.id));
    client.delete_preferences(&me.id, &[flag]).await.unwrap();
    let saved = client.saved_posts(&me.id, 0, 200).await.unwrap();
    assert!(!saved.order.contains(&post.id));

    // A direct message with another team member, found again on a second ask.
    let others = client
        .autocomplete_users("", Some(&team.id), None)
        .await
        .unwrap();
    let other = others
        .users
        .iter()
        .find(|u| u.id != me.id)
        .expect("another user in the team");
    let direct = client.direct_channel(&me.id, &other.id).await.unwrap();
    assert_eq!(direct.kind, "D");
    assert_eq!(
        client.direct_channel(&me.id, &other.id).await.unwrap().id,
        direct.id
    );
}

#[tokio::test]
async fn upload_an_image_and_fetch_its_thumbnail_and_preview() {
    let Some((url, login, password)) = server() else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return;
    };
    let credentials = Credentials {
        login_id: login,
        password,
        ..Default::default()
    };
    let signed_in = Client::new(url).unwrap().login(&credentials).await.unwrap();
    let (client, me) = (signed_in.client, signed_in.user);
    let team = client.my_teams().await.unwrap().remove(0);
    let channel = client
        .my_channels(&team.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.kind == "O")
        .expect("a public channel");
    // The server's own PNG of the user's avatar is a real image to upload.
    let Fetched::Fresh { bytes: png, .. } = client.avatar(&me.id, None).await.unwrap() else {
        panic!("no avatar")
    };
    let reports = Arc::new(Mutex::new(Vec::new()));
    let log = reports.clone();
    let uploaded = client
        .upload_file(
            Upload {
                channel_id: channel.id.clone(),
                client_id: "live-upload".into(),
                file_name: "avatar.png".into(),
                bytes: png.clone(),
            },
            Arc::new(move |sent, total| log.lock().unwrap().push((sent, total))),
        )
        .await
        .unwrap();
    assert_eq!(uploaded.client_ids, ["live-upload"]);
    let info = &uploaded.file_infos[0];
    assert_eq!(info.name, "avatar.png");
    assert_eq!(info.size, i64::try_from(png.len()).unwrap());
    assert!(info.width > 0 && info.has_preview_image);
    let total = png.len() as u64;
    assert_eq!(reports.lock().unwrap().last(), Some(&(total, total)));

    for fetched in [
        client.file_thumbnail(&info.id, None).await.unwrap(),
        client.file_preview(&info.id, None).await.unwrap(),
    ] {
        let Fetched::Fresh {
            bytes,
            content_type,
            ..
        } = fetched
        else {
            panic!("not modified")
        };
        assert!(!bytes.is_empty());
        assert!(content_type.unwrap().starts_with("image/"));
    }
    let Fetched::Fresh { bytes, .. } = client.file(&info.id, None).await.unwrap() else {
        panic!("not modified")
    };
    assert_eq!(bytes, png);
    assert_eq!(client.file_info(&info.id).await.unwrap().id, info.id);
}

#[tokio::test]
async fn websocket_hello_events_and_replies() {
    let Some((url, login, password)) = server() else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return;
    };
    let credentials = Credentials {
        login_id: login,
        password,
        // Signing in revokes other sessions with the same device id.
        ..Default::default()
    };
    let signed_in = Client::new(url).unwrap().login(&credentials).await.unwrap();
    let (client, me) = (signed_in.client, signed_in.user);
    let (connection, mut events) = WsConnection::connect(&client).await.unwrap();
    assert!(!connection.hello().connection_id.is_empty());
    assert!(
        connection
            .hello()
            .server_version
            .starts_with(char::is_numeric)
    );

    let team = client.my_teams().await.unwrap().remove(0);
    let channel = client
        .my_channels(&team.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.kind == "O")
        .expect("a public channel");

    let typing = connection.user_typing(&channel.id, "").await.unwrap();
    assert!(typing.is_ok(), "{typing:?}");
    let statuses = connection.get_statuses_by_ids(&[&me.id]).await.unwrap();
    assert!(statuses.data.unwrap().get(&me.id).is_some());

    let pending = pending_id(&me.id);
    let post = client
        .create_post(&NewPost::new(&channel.id, "live websocket test", &pending))
        .await
        .unwrap();
    let posted = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match events.recv().await {
                Some(WsMessage::Event(event)) if event.event == "posted" => {
                    let raw = event.data["post"].as_str().unwrap().to_string();
                    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
                    if value["id"] == post.id.as_str() {
                        break value;
                    }
                }
                Some(WsMessage::Event(_)) => {}
                other => panic!("connection ended: {other:?}"),
            }
        }
    })
    .await
    .expect("posted within 10s");
    assert_eq!(posted["pending_post_id"], pending.as_str());
    client.delete_post(&post.id).await.unwrap();
    connection.close();
}

#[tokio::test]
async fn websocket_resumes_after_reconnect() {
    let Some((url, login, password)) = server() else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return;
    };
    let credentials = Credentials {
        login_id: login,
        password,
        ..Default::default()
    };
    let signed_in = Client::new(url).unwrap().login(&credentials).await.unwrap();
    let (client, me) = (signed_in.client, signed_in.user);
    let team = client.my_teams().await.unwrap().remove(0);
    let channel = client
        .my_channels(&team.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.kind == "O")
        .expect("a public channel");
    let (ws, mut signals) = WsClient::connect(client.clone());

    let first = wait_for(&mut signals, |s| {
        matches!(s, WsSignal::State(ConnectionState::Connected { .. }))
    })
    .await;
    let Some(WsSignal::State(ConnectionState::Connected { connection_id })) = first.last() else {
        unreachable!()
    };
    let connection_id = connection_id.clone();

    let pending = pending_id(&me.id);
    let one = client
        .create_post(&NewPost::new(&channel.id, "resume test one", &pending))
        .await
        .unwrap();
    let seen = wait_for(
        &mut signals,
        |s| matches!(s, WsSignal::Event(e) if e.event == "posted"),
    )
    .await;
    let WsSignal::Event(before) = seen.last().unwrap() else {
        unreachable!()
    };
    let before_seq = before.seq;

    ws.reconnect_now();
    let reconnected = wait_for(&mut signals, |s| {
        matches!(s, WsSignal::State(ConnectionState::Connected { .. }))
    })
    .await;
    // reconnect_now redials at once, often before the server has retired
    // the old socket, so it may resume or start over; either way the
    // signals must say which. (websocket_connection_resumes checks a resume.)
    let resynced = reconnected.contains(&WsSignal::ResyncRequired);
    if !resynced {
        assert_eq!(
            reconnected.last(),
            Some(&WsSignal::State(ConnectionState::Connected {
                connection_id: connection_id.clone()
            })),
        );
    }
    let two = client
        .create_post(&NewPost::new(
            &channel.id,
            "resume test two",
            format!("{pending}-2"),
        ))
        .await
        .unwrap();
    let seen = wait_for(&mut signals, |s| {
        matches!(s, WsSignal::Event(e) if e.event == "posted" && e.data["post"].as_str().unwrap().contains(&two.id))
    })
    .await;
    let resynced = resynced || seen.contains(&WsSignal::ResyncRequired);

    let WsSignal::Event(after) = seen.last().unwrap() else {
        unreachable!()
    };
    if !resynced {
        assert!(after.seq > before_seq, "a resumed stream keeps counting");
    }
    let posted: Vec<i64> = seen
        .iter()
        .filter_map(|s| match s {
            WsSignal::Event(e) => Some(e.seq),
            _ => None,
        })
        .collect();
    assert!(
        posted.windows(2).all(|w| w[0] < w[1]),
        "in order, no duplicates: {posted:?}"
    );

    client.delete_post(&one.id).await.unwrap();
    client.delete_post(&two.id).await.unwrap();
    ws.stop();
}

#[tokio::test]
async fn websocket_connection_resumes() {
    let Some((url, login, password)) = server() else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return;
    };
    let credentials = Credentials {
        login_id: login,
        password,
        ..Default::default()
    };
    let client = Client::new(url)
        .unwrap()
        .login(&credentials)
        .await
        .unwrap()
        .client;
    let (connection, mut events) = WsConnection::connect(&client).await.unwrap();
    let id = connection.hello().connection_id.clone();
    connection.get_statuses_by_ids(&[]).await.unwrap();
    // Read whatever the server sent in the first moment.
    let mut next = 1;
    while let Ok(Some(WsMessage::Event(event))) =
        tokio::time::timeout(std::time::Duration::from_millis(500), events.recv()).await
    {
        next = event.seq + 1;
    }
    connection.close();
    // Give the server a moment to retire the socket, as a backoff would.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let resume = matty_api::ws::Resume {
        connection_id: id.clone(),
        sequence_number: next,
    };
    let (resumed, mut events) = WsConnection::resume(&client, Default::default(), Some(&resume))
        .await
        .unwrap();
    resumed.user_typing("", "").await.ok();
    let mut seqs = Vec::new();
    while let Ok(Some(message)) =
        tokio::time::timeout(std::time::Duration::from_millis(1500), events.recv()).await
    {
        match message {
            WsMessage::Event(event) => seqs.push(event.seq),
            WsMessage::Hello(hello) => panic!("not resumed: new id {}", hello.connection_id),
            WsMessage::Disconnected { reason } => panic!("{reason:?}"),
        }
    }
    // The server sends status_change events on reconnecting; they continue
    // the old numbering.
    assert!(!seqs.is_empty(), "events on the resumed stream");
    assert_eq!(seqs[0], next, "{seqs:?}");
}

async fn wait_for(
    signals: &mut tokio::sync::mpsc::UnboundedReceiver<WsSignal>,
    want: impl Fn(&WsSignal) -> bool,
) -> Vec<WsSignal> {
    let mut seen = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let signal = signals.recv().await.unwrap();
            let done = want(&signal);
            seen.push(signal);
            if done {
                return;
            }
        }
    })
    .await
    .expect("signal within 15s");
    seen
}

/// A pending post id no other test uses: the tests post as the same user
/// at once, and the server refuses a second post with a pending id it is
/// still creating.
fn pending_id(user_id: &str) -> String {
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{user_id}:{}{n:03}", web_time_millis())
}

fn web_time_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

/// The host name the server reaches this machine by, for the integrations
/// the tests below run (docker/compose.yml maps `host.docker.internal`).
fn integration_host() -> String {
    std::env::var("MATTY_TEST_INTEGRATION_HOST").unwrap_or_else(|_| "host.docker.internal".into())
}

/// A stand-in integration the server can call: a mock server on every
/// interface, and its URL as the server sees it.
async fn integration() -> (wiremock::MockServer, String) {
    let listener = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mock = wiremock::MockServer::builder()
        .listener(listener)
        .start()
        .await;
    (mock, format!("http://{}:{port}", integration_host()))
}

/// The next event called `name` that `want` accepts, within 15 seconds.
async fn next_event(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<WsMessage>,
    name: &str,
    want: impl Fn(&matty_api::ws::RawEvent) -> bool,
) -> matty_api::ws::RawEvent {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            match events.recv().await {
                Some(WsMessage::Event(event)) if event.event == name && want(&event) => {
                    return event;
                }
                Some(WsMessage::Event(_) | WsMessage::Hello(_)) => {}
                other => panic!("connection ended: {other:?}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{name} within 15s"))
}

/// The bodies the integration got on `path`, oldest first.
async fn received(mock: &wiremock::MockServer, path: &str) -> Vec<String> {
    mock.received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.url.path() == path)
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect()
}

async fn sign_in_tester() -> Option<(Client, matty_api::models::User)> {
    let (url, login, password) = server()?;
    let credentials = Credentials {
        login_id: login,
        password,
        ..Default::default()
    };
    let signed_in = Client::new(url).unwrap().login(&credentials).await.unwrap();
    Some((signed_in.client, signed_in.user))
}

/// The tester's first team and its first public channel.
async fn team_and_channel(client: &Client) -> (String, String) {
    let team = client.my_teams().await.unwrap().remove(0);
    let channel = client
        .my_channels(&team.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.kind == "O")
        .expect("a public channel");
    (team.id, channel.id)
}

#[tokio::test]
async fn interactive_message_actions_and_dialogs() {
    use matty_api::{models::SubmitDialogRequest, ws::WsEvent};
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{body_partial_json, method, path},
    };
    let Some((client, me)) = sign_in_tester().await else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return;
    };
    let (team_id, channel_id) = team_and_channel(&client).await;
    let (mock, hook) = integration().await;
    let answer = |action: &str, body: serde_json::Value| {
        Mock::given(method("POST"))
            .and(path("/action"))
            .and(body_partial_json(
                serde_json::json!({ "context": { "action": action } }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
    };
    answer(
        "ephemeral",
        serde_json::json!({ "ephemeral_text": "only you see this" }),
    )
    .mount(&mock)
    .await;
    answer(
        "update",
        serde_json::json!({ "update": { "message": "updated by the integration" } }),
    )
    .mount(&mock)
    .await;
    answer("pick", serde_json::json!({})).mount(&mock).await;
    answer("dialog", serde_json::json!({})).mount(&mock).await;
    Mock::given(method("POST"))
        .and(path("/dialog"))
        .and(body_partial_json(
            serde_json::json!({ "submission": { "name": "bad" } }),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "errors": { "name": "taken" } })),
        )
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/dialog"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock)
        .await;

    let action = |id: &str, kind: &str, act: &str| {
        serde_json::json!({
            "id": id, "name": id, "type": kind,
            "options": [{ "text": "Europe", "value": "eu" }],
            "integration": { "url": format!("{hook}/action"), "context": { "action": act } },
        })
    };
    let mut new = NewPost::new(&channel_id, "", pending_id(&me.id));
    new.props.insert(
        "attachments".into(),
        serde_json::json!([{ "text": "live interactive", "actions": [
            action("tell", "button", "ephemeral"),
            action("change", "button", "update"),
            action("region", "select", "pick"),
            action("ask", "button", "dialog"),
        ] }]),
    );
    let post = client.create_post(&new).await.unwrap();
    // The server keeps the integration's URL to itself.
    let fetched = client.get_post(&post.id).await.unwrap();
    let actions = &fetched.attachments()[0].actions;
    assert_eq!(actions.len(), 4);
    assert!(actions[2].is_select());
    assert_eq!(actions[2].options[0].value, "eu");
    assert!(
        !serde_json::to_string(&fetched.props)
            .unwrap()
            .contains(&hook)
    );

    let (connection, mut events) = WsConnection::connect(&client).await.unwrap();
    let told = client
        .do_post_action(&post.id, "tell", None, "")
        .await
        .unwrap();
    assert_eq!(told.status, "OK");
    let ephemeral = next_event(&mut events, "ephemeral_message", |_| true).await;
    match WsEvent::decode(&ephemeral) {
        // The server threads the answer under the post with the button.
        WsEvent::EphemeralMessage(answer) => {
            assert_eq!(answer.message, "only you see this");
            assert_eq!(answer.root_id, post.id);
            assert_eq!(answer.kind, "system_ephemeral");
        }
        other => panic!("{other:?}"),
    }

    client
        .do_post_action(&post.id, "region", Some("eu"), "")
        .await
        .unwrap();
    let picks = received(&mock, "/action").await;
    assert!(
        picks
            .iter()
            .any(|b| b.contains(r#""selected_option":"eu""#)),
        "{picks:?}"
    );

    client
        .do_post_action(&post.id, "change", None, "")
        .await
        .unwrap();
    assert_eq!(
        client.get_post(&post.id).await.unwrap().message,
        "updated by the integration"
    );

    // A button that opens a dialog: the integration gets a trigger id and
    // opens the dialog with it, which reaches the user as `open_dialog`.
    let asked = client
        .do_post_action(&post.id, "ask", None, "")
        .await
        .unwrap();
    assert!(!asked.trigger_id.is_empty());
    let request: serde_json::Value = serde_json::from_str(
        received(&mock, "/action")
            .await
            .iter()
            .rev()
            .find(|b| b.contains(r#""action":"dialog""#))
            .unwrap(),
    )
    .unwrap();
    let opened = reqwest::Client::new()
        .post(
            client
                .base_url()
                .join("api/v4/actions/dialogs/open")
                .unwrap(),
        )
        .json(&serde_json::json!({
            "trigger_id": request["trigger_id"],
            "url": format!("{hook}/dialog"),
            "dialog": {
                "callback_id": "live", "title": "Live", "state": "s1", "notify_on_cancel": true,
                "elements": [
                    { "display_name": "Name", "name": "name", "type": "text" },
                    { "display_name": "Agree", "name": "agree", "type": "bool", "default": "true" },
                    { "display_name": "Owner", "name": "owner", "type": "select",
                      "data_source": "users", "optional": true },
                ],
            },
        }))
        .send()
        .await
        .unwrap();
    assert!(opened.status().is_success(), "{:?}", opened.text().await);
    let event = next_event(&mut events, "open_dialog", |_| true).await;
    let WsEvent::OpenDialog(dialog) = WsEvent::decode(&event) else {
        panic!("{event:?}");
    };
    assert_eq!(dialog.url, format!("{hook}/dialog"));
    assert_eq!(dialog.dialog.elements.len(), 3);
    assert_eq!(dialog.dialog.elements[1].default, "true");
    assert_eq!(dialog.dialog.elements[2].data_source, "users");

    let submit = |name: &str, cancelled: bool| SubmitDialogRequest {
        kind: "dialog_submission".into(),
        url: dialog.url.clone(),
        callback_id: dialog.dialog.callback_id.clone(),
        state: dialog.dialog.state.clone(),
        user_id: me.id.clone(),
        channel_id: channel_id.clone(),
        team_id: team_id.clone(),
        submission: if cancelled {
            Default::default()
        } else {
            [
                ("name".to_string(), serde_json::json!(name)),
                ("agree".to_string(), serde_json::json!(true)),
            ]
            .into()
        },
        cancelled,
    };
    let refused = client.submit_dialog(&submit("bad", false)).await.unwrap();
    assert!(!refused.accepted());
    assert_eq!(refused.errors["name"], "taken");
    assert!(
        client
            .submit_dialog(&submit("Ada", false))
            .await
            .unwrap()
            .accepted()
    );
    assert!(
        client
            .submit_dialog(&submit("", true))
            .await
            .unwrap()
            .accepted()
    );
    let submissions = received(&mock, "/dialog").await;
    assert_eq!(submissions.len(), 3, "{submissions:?}");
    assert!(submissions[2].contains(r#""cancelled":true"#));

    client.delete_post(&post.id).await.unwrap();
    connection.close();
}

/// Calls the REST API as the seed's admin, for setup the client has no
/// method for (Matty never manages integrations).
async fn as_admin(
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> serde_json::Value {
    let (url, _, password) = server().unwrap();
    let login = std::env::var("MATTY_TEST_ADMIN").unwrap_or_else(|_| "admin".into());
    let http = reqwest::Client::new();
    let api = format!("{}/api/v4", url.trim_end_matches('/'));
    let signed_in = http
        .post(format!("{api}/users/login"))
        .json(&serde_json::json!({ "login_id": login, "password": password }))
        .send()
        .await
        .unwrap();
    let token = signed_in.headers()["Token"].to_str().unwrap().to_owned();
    let mut request = http
        .request(method, format!("{api}/{path}"))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    assert!(response.status().is_success(), "{path}: {response:?}");
    response.json().await.unwrap_or_default()
}

#[tokio::test]
async fn slash_commands_suggest_and_run() {
    use matty_api::{models::ExecuteCommand, ws::WsEvent};
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{body_string_contains, method, path},
    };
    let Some((client, me)) = sign_in_tester().await else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return;
    };
    let (team_id, channel_id) = team_and_channel(&client).await;
    let (mock, hook) = integration().await;
    let reply = |text: &str, body: serde_json::Value| {
        Mock::given(method("POST"))
            .and(path("/command"))
            .and(body_string_contains(format!("text={text}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
    };
    reply(
        "hello",
        serde_json::json!({ "response_type": "ephemeral", "text": "hi from the integration" }),
    )
    .mount(&mock)
    .await;
    reply(
        "goto",
        serde_json::json!({ "goto_location": "/dev/channels/design" }),
    )
    .mount(&mock)
    .await;
    reply(
        "buttons",
        serde_json::json!({ "response_type": "ephemeral", "text": "pick one", "attachments": [{
            "actions": [{ "id": "press", "name": "Press", "integration": {
                "url": format!("{hook}/action"), "context": { "from": "command" } } }],
        }] }),
    )
    .mount(&mock)
    .await;
    Mock::given(method("POST"))
        .and(path("/action"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "ephemeral_text": "pressed" })),
        )
        .mount(&mock)
        .await;

    // Its own trigger, since the tests share a server.
    let trigger = format!("live{}", web_time_millis() % 1_000_000_000);
    let command = as_admin(
        reqwest::Method::POST,
        "commands",
        Some(serde_json::json!({
            "team_id": team_id, "trigger": trigger, "method": "P",
            "url": format!("{hook}/command"), "display_name": "Live",
            "auto_complete": true, "auto_complete_desc": "Live test command",
            "auto_complete_hint": "[hello|goto|buttons]",
        })),
    )
    .await;

    let listed = client.autocomplete_commands(&team_id).await.unwrap();
    let ours = listed
        .iter()
        .find(|c| c.trigger == trigger)
        .expect("the new command");
    assert_eq!(ours.auto_complete_hint, "[hello|goto|buttons]");
    assert!(listed.iter().any(|c| c.trigger == "away"));
    let suggested = client
        .command_suggestions(&team_id, &format!("/{}", &trigger[..6]), &channel_id, "")
        .await
        .unwrap();
    let row = suggested
        .iter()
        .find(|s| s.complete == trigger)
        .expect("suggested");
    assert_eq!(row.description, "Live test command");
    let builtin = client
        .command_suggestions(&team_id, "/aw", &channel_id, "")
        .await
        .unwrap();
    assert!(
        builtin.iter().any(|s| s.suggestion == "away"),
        "{builtin:?}"
    );

    let run = |line: String| ExecuteCommand {
        command: line,
        channel_id: channel_id.clone(),
        team_id: team_id.clone(),
        root_id: String::new(),
    };
    let (connection, mut events) = WsConnection::connect(&client).await.unwrap();
    let hello = client
        .execute_command(&run(format!("/{trigger} hello")))
        .await
        .unwrap();
    assert!(hello.is_ephemeral());
    assert_eq!(hello.text, "hi from the integration");
    let goto = client
        .execute_command(&run(format!("/{trigger} goto")))
        .await
        .unwrap();
    assert_eq!(goto.goto_location, "/dev/channels/design");

    // An ephemeral answer with a button: the server sends it with a cookie,
    // which pressing the button must send back.
    client
        .execute_command(&run(format!("/{trigger} buttons")))
        .await
        .unwrap();
    let event = next_event(&mut events, "ephemeral_message", |e| {
        e.data["post"].as_str().is_some_and(|p| p.contains("Press"))
    })
    .await;
    let WsEvent::EphemeralMessage(post) = WsEvent::decode(&event) else {
        panic!("{event:?}");
    };
    let press = &post.attachments()[0].actions[0];
    assert!(!press.cookie.is_empty(), "{press:?}");
    client
        .do_post_action(&post.id, &press.id, None, &press.cookie)
        .await
        .unwrap();
    let pressed = received(&mock, "/action").await;
    assert!(pressed[0].contains(r#""from":"command""#), "{pressed:?}");

    let away = client.execute_command(&run("/away".into())).await.unwrap();
    assert!(away.is_ephemeral());
    assert!(!away.text.is_empty());
    client
        .execute_command(&run("/online".into()))
        .await
        .unwrap();
    let missing = client
        .execute_command(&run(format!("/{trigger}nope")))
        .await
        .unwrap_err();
    assert!(
        matches!(missing, ApiError::Server { status: 404, .. }),
        "{missing:?}"
    );
    assert_eq!(
        missing.id(),
        Some("api.command.execute_command.not_found.app_error")
    );

    as_admin(
        reqwest::Method::DELETE,
        &format!("commands/{}", command["id"].as_str().unwrap()),
        None,
    )
    .await;
    connection.close();
    let _ = me;
}

#[tokio::test]
async fn bookmarks_round_trip_where_the_license_allows() {
    use matty_api::models::{BookmarkKind, BookmarkPatch, NewBookmark};
    let Some((client, _me)) = sign_in_tester().await else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return;
    };
    let (_, channel_id) = team_and_channel(&client).await;
    let caps = client.capabilities().await.unwrap();
    if !caps.channel_bookmarks {
        // Without a license the server refuses bookmarks even where its
        // config reports the feature flag on, which is why the capability
        // needs the license too.
        let refused = client.channel_bookmarks(&channel_id).await.unwrap_err();
        if !caps.licensed {
            assert!(
                matches!(refused, ApiError::Server { status: 501, .. }),
                "{refused:?}"
            );
            assert_eq!(
                refused.id(),
                Some("api.channel.bookmark.channel_bookmark.license.error")
            );
        }
        return;
    }
    let new = |name: &str| NewBookmark {
        channel_id: channel_id.clone(),
        display_name: name.into(),
        kind: BookmarkKind::Link,
        link_url: format!("https://example.com/{name}"),
        ..Default::default()
    };
    let first = client.create_bookmark(&new("live-one")).await.unwrap();
    let second = client.create_bookmark(&new("live-two")).await.unwrap();
    assert_eq!(first.kind, BookmarkKind::Link);
    let listed = client.channel_bookmarks(&channel_id).await.unwrap();
    assert!(listed.iter().any(|b| b.id == first.id));
    let patched = client
        .patch_bookmark(
            &channel_id,
            &first.id,
            &BookmarkPatch {
                display_name: Some("live-renamed".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let renamed = patched.updated.expect("the updated bookmark");
    assert_eq!(renamed.display_name, "live-renamed");
    let moved = client
        .sort_bookmark(&channel_id, &second.id, 0)
        .await
        .unwrap();
    assert!(moved.iter().any(|b| b.id == second.id && b.sort_order == 0));
    for bookmark in [&second, &renamed] {
        let deleted = client
            .delete_bookmark(&channel_id, &bookmark.id)
            .await
            .unwrap();
        assert!(deleted.delete_at > 0);
    }
}

#[tokio::test]
async fn custom_emoji_upload_and_delete() {
    let Some((client, me)) = sign_in_tester().await else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return;
    };
    if !client.capabilities().await.unwrap().custom_emoji {
        return;
    }
    // A 1×1 PNG.
    let png = vec![
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    let name = format!(
        "live-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let emoji = client
        .create_emoji(&me.id, &name, png, "dot.png".into())
        .await
        .unwrap();
    assert_eq!(
        (emoji.name.as_str(), emoji.creator_id.as_str()),
        (name.as_str(), me.id.as_str())
    );
    assert!(
        client
            .all_emoji()
            .await
            .unwrap()
            .iter()
            .any(|e| e.id == emoji.id)
    );
    client.delete_emoji(&emoji.id).await.unwrap();
    assert!(
        !client
            .all_emoji()
            .await
            .unwrap()
            .iter()
            .any(|e| e.id == emoji.id)
    );
}

/// Changing the email needs the current password (#212); the test puts the
/// address back afterwards. Runs as a seeded user nobody else's test uses.
#[tokio::test]
async fn changing_the_email_checks_the_password() {
    let Some((url, _, password)) = server() else {
        return;
    };
    let credentials = Credentials {
        login_id: "alan.allen".into(),
        password: password.clone(),
        ..Default::default()
    };
    let signed_in = Client::new(url).unwrap().login(&credentials).await.unwrap();
    let (client, me) = (signed_in.client, signed_in.user);
    let patch = |email: &str, password: &str| matty_api::ProfilePatch {
        email: Some(email.into()),
        password: Some(password.into()),
        ..Default::default()
    };

    let wrong = client
        .patch_profile(&me.id, &patch("alan@allen.dev", "not-the-password"))
        .await;
    assert!(wrong.is_err(), "a wrong password changed the email");
    assert_eq!(client.me().await.unwrap().email, me.email);

    let changed = client
        .patch_profile(&me.id, &patch("alan@allen.dev", &password))
        .await
        .unwrap();
    assert_eq!(changed.email, "alan@allen.dev");
    assert_eq!(client.me().await.unwrap().email, "alan@allen.dev");

    client
        .patch_profile(&me.id, &patch(&me.email, &password))
        .await
        .unwrap();
    assert_eq!(client.me().await.unwrap().email, me.email);
}
