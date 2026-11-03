//! Runs against a real server when `MATTY_TEST_SERVER` is set, such as the
//! seeded dev server (docs/dev-server.md): `MATTY_TEST_SERVER=http://localhost:8065
//! MATTY_TEST_PASSWORD='Password1!' cargo test -p matty-api --test live`.
//! `MATTY_TEST_LOGIN` defaults to the seed's `tester`, who must be in a team
//! with a public channel. Without the server, each test returns at once.

#![allow(clippy::unwrap_used)]

use matty_api::{
    ApiError, Client, Credentials, NewPost, PostPatch, PostsQuery, ThreadQuery, ThreadsQuery,
    models::PostChange,
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
        device_id: "matty-live-test".into(),
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
        device_id: "matty-live-test-posts".into(),
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
    assert_eq!(
        latest.order[0], root.id,
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

fn web_time_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
}
