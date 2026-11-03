//! Sign-in endpoints against a mock server.

#![allow(clippy::unwrap_used)]

use matty_api::{ApiError, Client, Credentials};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{body_partial_json, header, method, path},
};

fn user() -> Value {
    json!({"id": "u1", "username": "arya", "email": "arya@example.com", "roles": "system_user"})
}

fn credentials() -> Credentials {
    Credentials {
        login_id: "arya".into(),
        password: "hunter2".into(),
        device_id: "apple:matty-test".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn login_takes_the_token_from_the_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/login"))
        .and(body_partial_json(
            json!({"login_id": "arya", "password": "hunter2", "device_id": "apple:matty-test"}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Token", "session-from-header")
                // A token in the body must never be used.
                .set_body_json(json!({"id": "u1", "username": "arya", "token": "not-this"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/me"))
        .and(header("authorization", "Bearer session-from-header"))
        .respond_with(ResponseTemplate::new(200).set_body_json(user()))
        .mount(&server)
        .await;

    let signed_in = Client::new(server.uri())
        .unwrap()
        .login(&credentials())
        .await
        .unwrap();
    assert_eq!(signed_in.user.username, "arya");
    assert!(signed_in.client.is_signed_in());
    assert_eq!(signed_in.client.me().await.unwrap().id, "u1");
}

#[tokio::test]
async fn mfa_is_detected_by_error_id_not_status() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/login"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "id": "mfa.validate_token.authenticate.app_error",
            "message": "Invalid MFA token.",
            "status_code": 401
        })))
        .mount(&server)
        .await;
    let error = Client::new(server.uri())
        .unwrap()
        .login(&credentials())
        .await
        .unwrap_err();
    assert_eq!(error, ApiError::MfaRequired);
}

#[tokio::test]
async fn other_login_errors_pass_through() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/login"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "id": "api.user.login.invalid_credentials_email_username",
            "message": "Enter a valid email or username and/or password.",
            "status_code": 401
        })))
        .mount(&server)
        .await;
    let error = Client::new(server.uri())
        .unwrap()
        .login(&credentials())
        .await
        .unwrap_err();
    assert_eq!(
        error.id(),
        Some("api.user.login.invalid_credentials_email_username")
    );
}

async fn login_body(ldap_only: bool, mfa: Option<&str>) -> Value {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/login"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Token", "t")
                .set_body_json(user()),
        )
        .mount(&server)
        .await;
    let credentials = Credentials {
        ldap_only,
        mfa_token: mfa.map(str::to_string),
        ..credentials()
    };
    Client::new(server.uri())
        .unwrap()
        .login(&credentials)
        .await
        .unwrap();
    let requests: Vec<Request> = server.received_requests().await.unwrap();
    serde_json::from_slice(&requests[0].body).unwrap()
}

#[tokio::test]
async fn ldap_only_is_sent_only_when_asked() {
    let plain = login_body(false, None).await;
    assert!(plain.get("ldap_only").is_none(), "{plain}");
    assert!(plain.get("token").is_none(), "{plain}");
    let ldap = login_body(true, Some("123456")).await;
    assert_eq!(ldap["ldap_only"], true);
    assert_eq!(ldap["token"], "123456");
}

#[tokio::test]
async fn desktop_token_and_logout() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/login/desktop_token"))
        .and(body_partial_json(
            json!({"token": "server-token", "device_id": "d1"}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Token", "sso-session")
                .set_body_json(user()),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/logout"))
        .and(header("authorization", "Bearer sso-session"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap();
    let signed_in = client
        .login_desktop_token("server-token", "d1")
        .await
        .unwrap();
    signed_in.client.logout().await.unwrap();
}

#[tokio::test]
async fn personal_access_token_and_sessions() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/me"))
        .and(header("authorization", "Bearer pat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(user()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/u1/sessions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "s1", "device_id": "matty-test", "create_at": 1, "expires_at": 0, "last_activity_at": 2, "token": "never-read"}
        ])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/u1/sessions/revoke"))
        .and(body_partial_json(json!({"session_id": "s1"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .expect(1)
        .mount(&server)
        .await;
    let signed_in = Client::new(server.uri())
        .unwrap()
        .login_personal_access_token("pat")
        .await
        .unwrap();
    let sessions = signed_in.client.sessions(&signed_in.user.id).await.unwrap();
    assert_eq!(sessions[0].device_id, "matty-test");
    signed_in.client.revoke_session("u1", "s1").await.unwrap();
}

#[test]
fn credentials_debug_hides_the_password() {
    let printed = format!(
        "{:?}",
        Credentials {
            mfa_token: Some("654321".into()),
            ..credentials()
        }
    );
    assert!(
        !printed.contains("hunter2") && !printed.contains("654321"),
        "{printed}"
    );
}
