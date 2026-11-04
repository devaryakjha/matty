//! The HTTP core against a mock server and a self-signed TLS listener.

#![allow(clippy::unwrap_used)]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use matty_api::{ApiError, Client, Sleeper};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

/// Records requested waits and returns at once.
#[derive(Clone, Default)]
struct FakeClock(Arc<Mutex<Vec<Duration>>>);

impl Sleeper for FakeClock {
    fn sleep(
        &self,
        duration: Duration,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        self.0.lock().unwrap().push(duration);
        Box::pin(async {})
    }
}

#[tokio::test]
async fn decodes_server_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "id": "api.context.session_expired.app_error",
            "message": "Invalid or expired session, please login again.",
            "request_id": "r-42",
            "status_code": 401
        })))
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    let error = client.get::<Value>("users/me").await.unwrap_err();
    assert_eq!(error.id(), Some("api.context.session_expired.app_error"));
    let ApiError::Server {
        status,
        message,
        request_id,
        ..
    } = error
    else {
        panic!("expected a server error");
    };
    assert_eq!(status, 401);
    assert_eq!(message, "Invalid or expired session, please login again.");
    assert_eq!(request_id.as_deref(), Some("r-42"));
}

#[tokio::test]
async fn sends_the_token_and_user_agent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/me"))
        .and(header("authorization", "Bearer tok"))
        .and(header(
            "user-agent",
            concat!("Matty/", env!("CARGO_PKG_VERSION")),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-RateLimit-Limit", "10")
                .insert_header("X-RateLimit-Remaining", "9")
                .insert_header("X-RateLimit-Reset", "1")
                .set_body_json(json!({"id": "u1"})),
        )
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("tok");
    let me: Value = client.get("users/me").await.unwrap();
    assert_eq!(me["id"], "u1");
    assert_eq!(client.rate_limit().remaining, Some(9));
}

#[tokio::test]
async fn retries_once_after_a_429() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/system/ping"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("X-RateLimit-Remaining", "0")
                .insert_header("X-RateLimit-Reset", "2"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/system/ping"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .mount(&server)
        .await;
    let clock = FakeClock::default();
    let client = Client::builder(server.uri())
        .sleeper(clock.clone())
        .build()
        .unwrap();
    let ping: Value = client.get("system/ping").await.unwrap();
    assert_eq!(ping["status"], "OK");
    assert_eq!(*clock.0.lock().unwrap(), vec![Duration::from_secs(2)]);
}

#[tokio::test]
async fn gives_up_after_three_429s() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429))
        .expect(3)
        .mount(&server)
        .await;
    let clock = FakeClock::default();
    let client = Client::builder(server.uri())
        .sleeper(clock.clone())
        .build()
        .unwrap();
    let error = client.get::<Value>("system/ping").await.unwrap_err();
    assert_eq!(error, ApiError::RateLimited);
    assert_eq!(
        *clock.0.lock().unwrap(),
        vec![Duration::from_secs(1), Duration::from_secs(2)]
    );
}

#[tokio::test]
async fn a_self_signed_certificate_is_a_tls_error() {
    use tokio::io::AsyncWriteExt;
    use tokio_rustls::rustls::{self, pki_types::PrivateKeyDer};

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.cert.der().clone()],
            PrivateKeyDer::Pkcs8(cert.signing_key.serialize_der().into()),
        )
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            if let Ok(mut tls) = acceptor.accept(stream).await {
                let _ = tls
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}")
                    .await;
            }
        }
    });

    let client = Client::new(format!("https://localhost:{port}")).unwrap();
    let error = client.get::<Value>("system/ping").await.unwrap_err();
    let ApiError::Tls(message) = &error else {
        panic!("expected a TLS error, got {error:?}");
    };
    assert!(message.contains("certificate"), "{message}");
}

#[tokio::test]
async fn reads_capabilities_with_format_old() {
    use wiremock::matchers::query_param;
    let server = MockServer::start().await;
    let fixture = |name: &str| -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    };
    Mock::given(method("GET"))
        .and(path("/api/v4/system/ping"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/config/client"))
        .and(query_param("format", "old"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("config_client_team.json")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/license/client"))
        .and(query_param("format", "old"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(fixture("license_client_unlicensed.json")),
        )
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap();
    assert!(client.ping().await.unwrap().is_ok());
    let caps = client.capabilities().await.unwrap();
    assert_eq!(caps.version, matty_api::Version::new(10, 5, 0));
    assert!(caps.minimum_supported().is_ok());
}

/// Mounts a server whose ping answers `ping` and whose config and license
/// are the team fixtures.
async fn probed(ping: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    let fixture = |name: &str| -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    };
    Mock::given(method("GET"))
        .and(path("/api/v4/system/ping"))
        .respond_with(ping)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/config/client"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("config_client_team.json")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/license/client"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(fixture("license_client_unlicensed.json")),
        )
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn a_probe_pings_then_reads_the_config() {
    use matty_api::ProbeError;
    let ok = ResponseTemplate::new(200).set_body_json(json!({"status": "OK"}));
    let server = probed(ok).await;
    let caps = Client::new(server.uri()).unwrap().probe().await.unwrap();
    assert_eq!(caps.version, matty_api::Version::new(10, 5, 0));

    // A web page where the ping should be is not Mattermost.
    let page =
        ResponseTemplate::new(200).set_body_raw("<!doctype html><title>Hi</title>", "text/html");
    let server = probed(page).await;
    let error = Client::new(server.uri())
        .unwrap()
        .probe()
        .await
        .unwrap_err();
    assert!(
        matches!(error, ProbeError::Ping(ApiError::Decode(_))),
        "{error:?}"
    );

    let missing = ResponseTemplate::new(404);
    let server = probed(missing).await;
    let error = Client::new(server.uri())
        .unwrap()
        .probe()
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            ProbeError::Ping(ApiError::Server { status: 404, .. })
        ),
        "{error:?}"
    );

    let unhealthy = ResponseTemplate::new(200).set_body_json(json!({"status": "UNHEALTHY"}));
    let server = probed(unhealthy).await;
    let error = Client::new(server.uri())
        .unwrap()
        .probe()
        .await
        .unwrap_err();
    assert!(
        matches!(error, ProbeError::Unhealthy(ref s) if s == "UNHEALTHY"),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_probe_reports_a_config_that_fails_after_a_good_ping() {
    use matty_api::ProbeError;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/system/ping"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/config/client"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(["not", "a", "map"])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/license/client"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    let error = Client::new(server.uri())
        .unwrap()
        .probe()
        .await
        .unwrap_err();
    assert!(matches!(error, ProbeError::Config(_)), "{error:?}");
}

#[tokio::test]
async fn a_probe_never_follows_a_redirect_to_another_host() {
    use matty_api::ProbeError;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/system/ping"))
        .respond_with(
            ResponseTemplate::new(301)
                .insert_header("location", "https://chat.example.com/api/v4/system/ping"),
        )
        .mount(&server)
        .await;
    let error = Client::new(server.uri())
        .unwrap()
        .probe()
        .await
        .unwrap_err();
    match error {
        ProbeError::Ping(ApiError::Redirected { location }) => {
            assert_eq!(location, "https://chat.example.com/api/v4/system/ping");
        }
        other => panic!("{other:?}"),
    }
}
