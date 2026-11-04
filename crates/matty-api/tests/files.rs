//! Uploads with progress and cancellation, downloads, and the redirect rule
//! (#18), against mock servers.

#![allow(clippy::unwrap_used)]

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use matty_api::{ApiError, Client, Fetched, Upload};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{header, method, path, query_param},
};

fn upload(bytes: Vec<u8>) -> Upload {
    Upload {
        channel_id: "c1".into(),
        client_id: "local-1".into(),
        file_name: "photo.png".into(),
        bytes,
    }
}

#[tokio::test]
async fn upload_reports_rising_progress_to_the_total() {
    let server = MockServer::start().await;
    let received = Arc::new(Mutex::new(Vec::new()));
    let seen = received.clone();
    Mock::given(method("POST"))
        .and(path("/api/v4/files"))
        .respond_with(move |request: &Request| {
            *seen.lock().unwrap() = request.body.clone();
            ResponseTemplate::new(201).set_body_json(json!({
                "file_infos": [{ "id": "f1", "name": "photo.png", "size": 5_000_000 }],
                "client_ids": ["local-1"]
            }))
        })
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    let reports = Arc::new(Mutex::new(Vec::new()));
    let log = reports.clone();
    let body: Vec<u8> = (0..5_000_000u32).map(|i| (i % 251) as u8).collect();
    let uploaded = client
        .upload_file(
            upload(body.clone()),
            Arc::new(move |sent, total| log.lock().unwrap().push((sent, total))),
        )
        .await
        .unwrap();
    assert_eq!(uploaded.file_infos[0].id, "f1");
    assert_eq!(uploaded.client_ids, ["local-1"]);

    let reports = reports.lock().unwrap();
    assert!(reports.len() > 2, "{} reports", reports.len());
    assert!(reports.iter().all(|&(_, total)| total == 5_000_000));
    assert!(reports.windows(2).all(|w| w[0].0 <= w[1].0), "monotonic");
    assert_eq!(reports.first().unwrap().0, 0);
    assert_eq!(reports.last().unwrap().0, 5_000_000);

    // The channel comes before the file, and the file arrives whole.
    let sent = received.lock().unwrap();
    let text = String::from_utf8_lossy(&sent);
    let channel = text.find("name=\"channel_id\"").unwrap();
    let client_ids = text.find("name=\"client_ids\"").unwrap();
    let files = text.find("name=\"files\"; filename=\"photo.png\"").unwrap();
    assert!(channel < files && client_ids < files);
    assert!(sent.windows(body.len()).any(|w| w == body.as_slice()));
}

#[tokio::test]
async fn cancelling_an_upload_closes_the_connection_quickly() {
    // A server that reads the request slowly, so the upload is mid-body.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let cancel = Arc::new(tokio::sync::Notify::new());
    let cancelled_signal = cancel.clone();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = vec![0u8; 1024 * 1024];
        // Read slowly until the client gives up, then drain what the kernel
        // buffered: the connection is closed when the read hits the end.
        let mut draining = false;
        loop {
            if !draining {
                tokio::select! {
                    () = cancelled_signal.notified() => draining = true,
                    () = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            }
            match socket
                .read(&mut buffer[..if draining { 1024 * 1024 } else { 16 * 1024 }])
                .await
            {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _ = closed_tx.send(Instant::now());
        let _ = socket.shutdown().await;
    });

    let client = Client::new(format!("http://{address}"))
        .unwrap()
        .with_token("t");
    let started = Arc::new(tokio::sync::Notify::new());
    let notify = started.clone();
    let task = tokio::spawn(async move {
        client
            .upload_file(
                upload(vec![7u8; 50_000_000]),
                Arc::new(move |sent, _| {
                    if sent >= 1_000_000 {
                        notify.notify_one();
                    }
                }),
            )
            .await
    });
    started.notified().await;
    let cancelled = Instant::now();
    task.abort();
    cancel.notify_one();
    let closed = tokio::time::timeout(Duration::from_secs(2), closed_rx)
        .await
        .expect("the server saw the connection close")
        .unwrap();
    let took = closed.saturating_duration_since(cancelled);
    assert!(took < Duration::from_millis(100), "took {took:?}");
}

#[tokio::test]
async fn downloads_return_bytes_and_content_type() {
    let server = MockServer::start().await;
    for (route, kind) in [
        ("/api/v4/files/f1", "application/pdf"),
        ("/api/v4/files/f1/thumbnail", "image/jpeg"),
        ("/api/v4/files/f1/preview", "image/jpeg"),
        ("/api/v4/emoji/e1/image", "image/png"),
        ("/api/v4/users/u1/image", "image/png"),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", kind)
                    .insert_header("etag", "\"v1\"")
                    .set_body_bytes(route.as_bytes().to_vec()),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/v4/image"))
        .and(query_param("url", "https://example.com/a b.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(b"proxied".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/files/f1/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "f1", "name": "report.pdf", "extension": "pdf", "size": 1234,
            "mime_type": "application/pdf", "has_preview_image": false
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/files/f1/link"))
        .and(header("authorization", "Bearer t"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "link": "https://chat.example.com/files/f1/public?h=x" })),
        )
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");

    let expect = |fetched: Fetched, kind: &str, body: &[u8]| {
        let Fetched::Fresh {
            bytes,
            content_type,
            ..
        } = fetched
        else {
            panic!("not modified")
        };
        assert_eq!(content_type.as_deref(), Some(kind));
        assert_eq!(bytes, body);
    };
    expect(
        client.file("f1", None).await.unwrap(),
        "application/pdf",
        b"/api/v4/files/f1",
    );
    expect(
        client.file_thumbnail("f1", None).await.unwrap(),
        "image/jpeg",
        b"/api/v4/files/f1/thumbnail",
    );
    expect(
        client.file_preview("f1", None).await.unwrap(),
        "image/jpeg",
        b"/api/v4/files/f1/preview",
    );
    expect(
        client.emoji_image("e1", None).await.unwrap(),
        "image/png",
        b"/api/v4/emoji/e1/image",
    );
    expect(
        client.avatar("u1", None).await.unwrap(),
        "image/png",
        b"/api/v4/users/u1/image",
    );
    expect(
        client
            .proxied_image("https://example.com/a b.png")
            .await
            .unwrap(),
        "image/png",
        b"proxied",
    );
    let info = client.file_info("f1").await.unwrap();
    assert_eq!((info.name.as_str(), info.size), ("report.pdf", 1234));
    assert!(client.file_link("f1").await.unwrap().ends_with("?h=x"));
}

#[tokio::test]
async fn redirects_stay_on_the_server() {
    let server = MockServer::start().await;
    let elsewhere = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/files/moved"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/api/v4/files/here"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/files/here"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"same host".to_vec()))
        .mount(&server)
        .await;
    // Another port on the same machine is another server.
    let outside = format!("{}/secret.png", elsewhere.uri());
    Mock::given(method("GET"))
        .and(path("/api/v4/image"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", outside.as_str()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&elsewhere)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");

    let Fetched::Fresh { bytes, .. } = client.file("moved", None).await.unwrap() else {
        panic!("not modified")
    };
    assert_eq!(bytes, b"same host");
    let error = client
        .proxied_image("https://example.com/x.png")
        .await
        .unwrap_err();
    assert_eq!(error, ApiError::Redirected { location: outside });
}

#[tokio::test]
async fn an_outside_image_goes_without_the_session_token() {
    let outside = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/a.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(b"png".to_vec()),
        )
        .mount(&outside)
        .await;
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_string("<html>"),
        )
        .mount(&outside)
        .await;
    let client = Client::new("https://chat.example.com")
        .unwrap()
        .with_token("secret");

    let Fetched::Fresh {
        bytes,
        content_type,
        ..
    } = client
        .outside_image(&format!("{}/a.png", outside.uri()))
        .await
        .unwrap()
    else {
        panic!("not modified")
    };
    assert_eq!(
        (bytes.as_slice(), content_type.as_deref()),
        (&b"png"[..], Some("image/png"))
    );
    let requests = outside.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|r| !r.headers.contains_key("authorization"))
    );

    let page = client
        .outside_image(&format!("{}/page", outside.uri()))
        .await;
    assert!(matches!(page, Err(ApiError::Decode(_))), "{page:?}");
    let script = client.outside_image("javascript:alert(1)").await;
    assert!(matches!(script, Err(ApiError::InvalidUrl(_))), "{script:?}");
}

#[tokio::test]
async fn a_download_streams_the_servers_bytes_with_rising_progress() {
    let server = MockServer::start().await;
    let body: Vec<u8> = (0..3_000_000u32).map(|i| (i % 253) as u8).collect();
    Mock::given(method("GET"))
        .and(path("/api/v4/files/f1"))
        .and(header("authorization", "Bearer t"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    let mut written = Vec::new();
    let mut reports = Vec::new();
    let received = client
        .download_file("f1", |chunk, received, total| {
            written.extend_from_slice(chunk);
            reports.push((received, total));
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(received, 3_000_000);
    assert!(written == body, "the bytes arrive whole and in order");
    assert!(reports.iter().all(|&(_, total)| total == Some(3_000_000)));
    assert!(reports.windows(2).all(|w| w[0].0 < w[1].0), "monotonic");
    assert_eq!(reports.last().unwrap().0, 3_000_000);
}

#[tokio::test]
async fn a_download_stops_when_the_file_cannot_be_written() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/files/f1"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![7u8; 1024]))
        .mount(&server)
        .await;
    let client = Client::new(server.uri()).unwrap().with_token("t");
    let error = client
        .download_file("f1", |_, _, _| Err(std::io::Error::other("disk full")))
        .await
        .unwrap_err();
    assert_eq!(error, ApiError::Io("disk full".into()));
}
