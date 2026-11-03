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

    let pending = format!("{}:{}", me.id, web_time_millis());
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
    let pending = format!("{}:{}", me.id, web_time_millis());
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

    let pending = format!("{}:{}", me.id, web_time_millis());
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

    let pending = format!("{}:{}", me.id, web_time_millis());
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

fn web_time_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
}
