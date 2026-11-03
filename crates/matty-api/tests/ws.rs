//! The WebSocket client against a scripted mock server: authentication,
//! `hello`, events, request replies by `seq`, pings and closes.

#![allow(clippy::unwrap_used)]
// The handshake callback's signature is tungstenite's, with its large error type.
#![allow(clippy::result_large_err)]

use std::future::Future;

use futures_util::{SinkExt, StreamExt};
use matty_api::{
    ApiError, Client,
    ws::{DisconnectReason, WsConnection, WsMessage},
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        protocol::{CloseFrame, frame::coding::CloseCode},
    },
};

const TOKEN: &str = "secret-session-token-123";

type Server = WebSocketStream<TcpStream>;

/// Starts a server that accepts one WebSocket on `/api/v4/websocket` and
/// runs `script` on it. Returns a client for it.
async fn mock<F, Fut>(script: F) -> (Client, tokio::task::JoinHandle<()>)
where
    F: FnOnce(Server) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let callback = |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                        response| {
            assert_eq!(request.uri().path(), "/api/v4/websocket");
            Ok(response)
        };
        let socket = tokio_tungstenite::accept_hdr_async(stream, callback)
            .await
            .unwrap();
        script(socket).await;
    });
    let client = Client::new(format!("http://{addr}"))
        .unwrap()
        .with_token(TOKEN);
    (client, task)
}

async fn recv_json(socket: &mut Server) -> Value {
    loop {
        match socket.next().await.unwrap().unwrap() {
            Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
            Message::Pong(_) => {}
            other => panic!("expected text, got {other:?}"),
        }
    }
}

async fn send_json(socket: &mut Server, value: Value) {
    socket.send(Message::text(value.to_string())).await.unwrap();
}

/// Checks the challenge, accepts it and sends `hello`.
async fn handshake(socket: &mut Server) {
    let challenge = recv_json(socket).await;
    assert_eq!(
        challenge,
        json!({"seq": 1, "action": "authentication_challenge", "data": {"token": TOKEN}})
    );
    send_json(socket, json!({"status": "OK", "seq_reply": 1})).await;
    send_json(
        socket,
        json!({
            "event": "hello",
            "data": {"connection_id": "conn-1", "server_version": "10.5.14"},
            "broadcast": {"omit_users": null, "user_id": "u1", "channel_id": "", "team_id": ""},
            "seq": 0
        }),
    )
    .await;
}

async fn next_message(rx: &mut tokio::sync::mpsc::UnboundedReceiver<WsMessage>) -> WsMessage {
    tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("a message within 5s")
        .expect("the channel is open")
}

#[tokio::test]
async fn authenticates_receives_hello_and_events() {
    let (client, server) = mock(|mut socket| async move {
        handshake(&mut socket).await;
        send_json(
            &mut socket,
            json!({
                "event": "posted",
                "data": {"channel_type": "O", "post": "{\"id\":\"p1\",\"message\":\"hi\"}"},
                "broadcast": {"omit_users": null, "user_id": "", "channel_id": "c1", "team_id": ""},
                "seq": 1
            }),
        )
        .await;
        // Hold the socket open until the client closes it.
        while let Some(Ok(frame)) = socket.next().await {
            if frame.is_close() {
                break;
            }
        }
    })
    .await;

    let (connection, mut events) = WsConnection::connect(&client).await.unwrap();
    assert_eq!(connection.hello().connection_id, "conn-1");
    assert_eq!(connection.hello().server_version, "10.5.14");

    let WsMessage::Event(event) = next_message(&mut events).await else {
        panic!("expected an event");
    };
    assert_eq!(event.event, "posted");
    assert_eq!(event.seq, 1);
    assert_eq!(event.broadcast.channel_id, "c1");
    assert!(event.data["post"].is_string());

    connection.close();
    assert_eq!(
        next_message(&mut events).await,
        WsMessage::Disconnected {
            reason: DisconnectReason::ClientClosed
        }
    );
    server.await.unwrap();
}

#[tokio::test]
async fn replies_are_matched_to_requests_by_seq() {
    let (client, server) = mock(|mut socket| async move {
        handshake(&mut socket).await;
        let typing = recv_json(&mut socket).await;
        assert_eq!(typing["action"], "user_typing");
        assert_eq!(typing["data"], json!({"channel_id": "c1", "parent_id": "r1"}));
        let statuses = recv_json(&mut socket).await;
        assert_eq!(statuses["action"], "get_statuses_by_ids");
        assert_eq!(statuses["data"], json!({"user_ids": ["u1", "u2"]}));
        let (typing_seq, statuses_seq) = (typing["seq"].clone(), statuses["seq"].clone());
        assert_eq!(typing_seq, 2, "requests count up from 2 after the challenge");
        // A stray reply, then the two answers in reverse order.
        send_json(&mut socket, json!({"status": "OK", "seq_reply": 99})).await;
        send_json(
            &mut socket,
            json!({"status": "OK", "seq_reply": statuses_seq, "data": {"u1": "online", "u2": "away"}}),
        )
        .await;
        send_json(&mut socket, json!({"status": "OK", "seq_reply": typing_seq})).await;
        while let Some(Ok(frame)) = socket.next().await {
            if frame.is_close() {
                break;
            }
        }
    })
    .await;

    let (connection, _events) = WsConnection::connect(&client).await.unwrap();
    let (typing, statuses) = tokio::join!(
        connection.user_typing("c1", "r1"),
        connection.get_statuses_by_ids(&["u1", "u2"]),
    );
    let (typing, statuses) = (typing.unwrap(), statuses.unwrap());
    assert!(typing.is_ok());
    assert_eq!(typing.seq_reply, 2);
    assert_eq!(statuses.seq_reply, 3);
    assert_eq!(statuses.data.unwrap()["u2"], "away");
    drop(connection);
    server.await.unwrap();
}

#[tokio::test]
async fn a_rejected_token_fails_connect() {
    let (client, server) = mock(|mut socket| async move {
        recv_json(&mut socket).await;
        send_json(
            &mut socket,
            json!({
                "status": "FAIL",
                "seq_reply": 1,
                "error": {"id": "api.web_socket_router.not_authenticated.app_error", "message": "Not authenticated"}
            }),
        )
        .await;
        let _ = socket.close(None).await;
    })
    .await;
    let error = WsConnection::connect(&client).await.unwrap_err();
    assert_eq!(
        error,
        ApiError::Server {
            status: 401,
            id: "api.web_socket_router.not_authenticated.app_error".into(),
            message: "Not authenticated".into(),
            request_id: None,
        }
    );
    assert!(!error.to_string().contains(TOKEN));
    server.await.unwrap();
}

#[tokio::test]
async fn no_token_no_connection() {
    let client = Client::new("http://127.0.0.1:9").unwrap();
    assert!(WsConnection::connect(&client).await.is_err());
}

#[tokio::test]
async fn server_pings_are_answered_and_closes_reported() {
    let (client, server) = mock(|mut socket| async move {
        handshake(&mut socket).await;
        socket
            .send(Message::Ping(b"are you there".to_vec().into()))
            .await
            .unwrap();
        match socket.next().await.unwrap().unwrap() {
            Message::Pong(payload) => assert_eq!(&payload[..], b"are you there"),
            other => panic!("expected a pong, got {other:?}"),
        }
        // Take a request, then close without answering it.
        recv_json(&mut socket).await;
        socket
            .close(Some(CloseFrame {
                code: CloseCode::Away,
                reason: "server restarting".into(),
            }))
            .await
            .unwrap();
    })
    .await;

    let (connection, mut events) = WsConnection::connect(&client).await.unwrap();
    let pending = connection.user_typing("c1", "").await;
    assert_eq!(
        pending.unwrap_err(),
        ApiError::Network("the WebSocket is closed".into())
    );
    assert_eq!(
        next_message(&mut events).await,
        WsMessage::Disconnected {
            reason: DisconnectReason::Closed {
                code: 1001,
                reason: "server restarting".into()
            }
        }
    );
    server.await.unwrap();
}

#[tokio::test]
async fn debug_output_never_shows_the_token() {
    let (client, _server) = mock(|mut socket| async move {
        handshake(&mut socket).await;
        while let Some(Ok(frame)) = socket.next().await {
            if frame.is_close() {
                break;
            }
        }
    })
    .await;
    assert!(!format!("{client:?}").contains(TOKEN));
    let (connection, _events) = WsConnection::connect(&client).await.unwrap();
    let debug = format!("{connection:?}");
    assert!(debug.contains("conn-1") && !debug.contains(TOKEN));
}

#[tokio::test]
async fn wss_uses_the_same_tls_checks_as_https() {
    use std::sync::Arc;
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
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let _ = acceptor.accept(stream).await;
        }
    });

    let client = Client::new(format!("https://localhost:{port}"))
        .unwrap()
        .with_token(TOKEN);
    let error = WsConnection::connect(&client).await.unwrap_err();
    let ApiError::Tls(message) = &error else {
        panic!("expected a TLS error, got {error:?}");
    };
    assert!(message.contains("certificate"), "{message}");
}
