//! Runs against a real server when `MATTY_TEST_SERVER` is set, such as the
//! Docker server from #46: `MATTY_TEST_SERVER=http://localhost:8065
//! MATTY_TEST_LOGIN=arya MATTY_TEST_PASSWORD=... cargo test -p matty-api --test live`.
//! Without it, each test returns at once.

#![allow(clippy::unwrap_used)]

use matty_api::{ApiError, Client, Credentials};

fn server() -> Option<(String, String, String)> {
    let url = std::env::var("MATTY_TEST_SERVER").ok()?;
    let login = std::env::var("MATTY_TEST_LOGIN").unwrap_or_else(|_| "arya".into());
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
    assert_eq!(error.id(), Some("api.user.login.invalid_credentials_email_username"));

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
    let members = session.my_channel_members(&signed_in.user.id, team).await.unwrap();
    assert_eq!(channels.len(), members.len());
    session.sidebar_categories(team).await.unwrap();
    session.preferences().await.unwrap();
    session.all_emoji().await.unwrap();
    session.team_unreads(&signed_in.user.id).await.unwrap();

    session.logout().await.unwrap();
    let after = session.me().await.unwrap_err();
    assert!(matches!(after, ApiError::Server { status: 401, .. }), "{after:?}");
}
