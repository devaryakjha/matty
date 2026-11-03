//! Pings and the liveness timeout, on tokio's paused clock over an
//! in-memory pipe, so minutes pass instantly and deterministically.

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use matty_api::{
    Client,
    ws::{DisconnectReason, WsConfig, WsConnection, WsMessage},
};
use serde_json::{Value, json};
use tokio::{io::DuplexStream, sync::mpsc, time::Instant};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

type Server = WebSocketStream<DuplexStream>;

/// Accepts the challenge and sends `hello`.
async fn handshake(socket: &mut Server) {
    let challenge = next_json(socket).await.unwrap();
    assert_eq!(challenge["action"], "authentication_challenge");
    send(socket, json!({"status": "OK", "seq_reply": 1})).await;
    send(
        socket,
        json!({"event": "hello", "data": {"connection_id": "c", "server_version": "10.5.0"}, "seq": 0}),
    )
    .await;
}

async fn next_json(socket: &mut Server) -> Option<Value> {
    loop {
        match socket.next().await? {
            Ok(Message::Text(text)) => return Some(serde_json::from_str(text.as_str()).unwrap()),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

async fn send(socket: &mut Server, value: Value) {
    socket.send(Message::text(value.to_string())).await.unwrap();
}

/// Connects a client over a pipe to a server running `script`.
async fn connect<F, Fut>(
    config: WsConfig,
    script: F,
) -> (WsConnection, mpsc::UnboundedReceiver<WsMessage>)
where
    F: FnOnce(Server) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let mut socket = tokio_tungstenite::accept_async(server_end).await.unwrap();
        handshake(&mut socket).await;
        script(socket).await;
    });
    let client = Client::new("http://matty.test").unwrap().with_token("t");
    WsConnection::connect_over(client_end, &client, config, None)
        .await
        .unwrap()
}

async fn disconnected(events: &mut mpsc::UnboundedReceiver<WsMessage>) -> DisconnectReason {
    loop {
        match events.recv().await.unwrap() {
            WsMessage::Disconnected { reason } => return reason,
            WsMessage::Event(_) | WsMessage::Hello(_) => {}
        }
    }
}

#[tokio::test(start_paused = true)]
async fn pings_every_30_seconds_and_stays_up_while_answered() {
    let (pings_tx, mut pings) = mpsc::unbounded_channel();
    let start = Instant::now();
    let (connection, mut events) = connect(WsConfig::default(), move |mut socket| async move {
        while let Some(request) = next_json(&mut socket).await {
            assert_eq!(request["action"], "ping");
            pings_tx.send(Instant::now()).unwrap();
            let reply =
                json!({"status": "OK", "seq_reply": request["seq"], "data": {"text": "pong"}});
            send(&mut socket, reply).await;
        }
    })
    .await;

    let mut times = Vec::new();
    for _ in 0..10 {
        times.push(pings.recv().await.unwrap() - start);
    }
    for (n, at) in times.iter().enumerate() {
        let expected = Duration::from_secs(30 * (n as u64 + 1));
        assert!(
            at.abs_diff(expected) < Duration::from_millis(100),
            "ping {n} at {at:?}"
        );
    }
    assert!(
        events.try_recv().is_err(),
        "still connected after 5 minutes"
    );
    drop(connection);
}

#[tokio::test(start_paused = true)]
async fn a_silent_server_times_out_30_to_35_seconds_after_the_ping() {
    let (pings_tx, mut pings) = mpsc::unbounded_channel();
    let (_connection, mut events) = connect(WsConfig::default(), move |mut socket| async move {
        // Read pings but never answer, and keep the pipe open.
        while next_json(&mut socket).await.is_some() {
            pings_tx.send(Instant::now()).unwrap();
        }
    })
    .await;
    let reason = disconnected(&mut events).await;
    let ended = Instant::now();
    assert_eq!(reason, DisconnectReason::Timeout);
    let first_ping = pings.recv().await.unwrap();
    let silence = ended - first_ping;
    assert!(
        (Duration::from_secs(30)..=Duration::from_secs(35)).contains(&silence),
        "timed out {silence:?} after the unanswered ping"
    );
}

#[tokio::test(start_paused = true)]
async fn any_frame_counts_as_alive() {
    // The server never answers pings but sends an event every 20 seconds.
    let (_connection, mut events) = connect(WsConfig::default(), |mut socket| async move {
        for seq in 1..=12 {
            tokio::time::sleep(Duration::from_secs(20)).await;
            send(
                &mut socket,
                json!({"event": "typing", "data": {}, "seq": seq}),
            )
            .await;
        }
        while next_json(&mut socket).await.is_some() {}
    })
    .await;
    let start = Instant::now();
    let reason = disconnected(&mut events).await;
    assert_eq!(reason, DisconnectReason::Timeout);
    // 12 events cover 240 s; the timeout comes after the next unanswered ping.
    assert!(Instant::now() - start > Duration::from_secs(240));
}

#[tokio::test(start_paused = true)]
async fn interval_and_timeout_are_configurable() {
    let config = WsConfig {
        ping_interval: Duration::from_secs(5),
        pong_timeout: Duration::from_secs(2),
    };
    let (_connection, mut events) = connect(config, |mut socket| async move {
        while next_json(&mut socket).await.is_some() {}
    })
    .await;
    let start = Instant::now();
    assert_eq!(disconnected(&mut events).await, DisconnectReason::Timeout);
    let elapsed = Instant::now() - start;
    assert!(
        elapsed.abs_diff(Duration::from_secs(7)) < Duration::from_millis(100),
        "{elapsed:?}"
    );
}
