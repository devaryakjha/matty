//! The WebSocket connection (PLAN §6.4): connect, authenticate, `hello`,
//! server events, and requests answered by `seq_reply`.
//!
//! [`WsConnection::connect`] returns once the server has sent `hello`. A
//! background task owns the socket; events arrive on the returned receiver
//! and [`WsConnection::send`] queues a request and waits for its reply.

use std::{collections::HashMap, fmt, sync::Arc, time::Duration};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream,
    tungstenite::{self, Message, client::IntoClientRequest, http::HeaderValue},
};

use crate::{ApiError, Client, models::null_as_default, tls};

/// How long [`WsConnection::connect`] waits for the upgrade, the
/// authentication reply and `hello`, together.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long [`WsConnection::send`] waits for a reply.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Who a server event is for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Broadcast {
    /// Users the event is not sent to, by id.
    #[serde(deserialize_with = "null_as_default")]
    pub omit_users: HashMap<String, bool>,
    /// Set when the event is for one user.
    pub user_id: String,
    /// Set when the event is for a channel's members.
    pub channel_id: String,
    /// Set when the event is for a team's members.
    pub team_id: String,
}

/// A server event, before decoding its `data` by event name.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct RawEvent {
    /// Event name, such as `posted`.
    pub event: String,
    /// Event data; some values (`post`, `reaction`) are JSON in a string.
    pub data: Value,
    /// Who the event is for.
    pub broadcast: Broadcast,
    /// The server's sequence number for this connection, from 0 (`hello`).
    pub seq: i64,
}

/// The answer to a request sent with [`WsConnection::send`].
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct WsReply {
    /// `OK` or `FAIL`.
    pub status: String,
    /// The `seq` of the request this answers.
    pub seq_reply: u64,
    /// Error details when `status` is `FAIL`.
    pub error: Option<Value>,
    /// Reply data, such as statuses for `get_statuses_by_ids`.
    pub data: Option<Value>,
}

impl WsReply {
    /// Whether the server accepted the request.
    pub fn is_ok(&self) -> bool {
        self.status == "OK"
    }
}

/// Why a connection ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisconnectReason {
    /// The server closed the socket, with the close frame's code and reason.
    Closed {
        /// WebSocket close code.
        code: u16,
        /// Close reason text.
        reason: String,
    },
    /// The connection dropped without a close frame.
    Lost(String),
    /// [`WsConnection::close`] was called or the connection was dropped.
    ClientClosed,
}

/// What the connection delivers.
#[derive(Clone, Debug, PartialEq)]
pub enum WsMessage {
    /// A server event other than `hello`.
    Event(RawEvent),
    /// The connection ended; nothing follows.
    Disconnected {
        /// Why.
        reason: DisconnectReason,
    },
}

/// The `hello` event's data.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Hello {
    /// Identifies this connection; a reconnect that sends it back resumes
    /// the event stream (PLAN §6.4).
    pub connection_id: String,
    /// Server version and build, such as `10.5.14.13836...`.
    pub server_version: String,
}

enum Command {
    Send {
        action: String,
        data: Value,
        reply: oneshot::Sender<Result<WsReply, ApiError>>,
    },
    Close,
}

/// An open, authenticated WebSocket connection.
pub struct WsConnection {
    hello: Hello,
    commands: mpsc::UnboundedSender<Command>,
}

impl fmt::Debug for WsConnection {
    // Never shows the token: the connection does not keep it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WsConnection")
            .field("connection_id", &self.hello.connection_id)
            .finish_non_exhaustive()
    }
}

impl WsConnection {
    /// Connects to `api/v4/websocket` on `client`'s server, authenticates
    /// with its session token and waits for `hello`.
    pub async fn connect(
        client: &Client,
    ) -> Result<(Self, mpsc::UnboundedReceiver<WsMessage>), ApiError> {
        let token = client
            .token_value()
            .ok_or_else(|| ApiError::InvalidUrl("the client has no session token".into()))?
            .to_string();
        let url = websocket_url(client)?;
        tokio::time::timeout(CONNECT_TIMEOUT, Self::open(url, token))
            .await
            .map_err(|_| ApiError::Timeout)?
    }

    async fn open(
        url: url::Url,
        token: String,
    ) -> Result<(Self, mpsc::UnboundedReceiver<WsMessage>), ApiError> {
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| ApiError::InvalidUrl(e.to_string()))?;
        request.headers_mut().insert(
            "User-Agent",
            HeaderValue::from_static(concat!("Matty/", env!("CARGO_PKG_VERSION"))),
        );
        let connector = Connector::Rustls(Arc::new(tls::client_config(tls::WEBSOCKET_ALPN)?));
        let (mut socket, _) =
            tokio_tungstenite::connect_async_tls_with_config(request, None, false, Some(connector))
                .await
                .map_err(from_tungstenite)?;

        // seq 1 is the authentication challenge; requests count up from 2.
        let challenge = json!({
            "seq": 1,
            "action": "authentication_challenge",
            "data": { "token": token },
        });
        socket
            .send(Message::text(challenge.to_string()))
            .await
            .map_err(from_tungstenite)?;
        drop(token);

        let mut early = Vec::new();
        let hello = loop {
            let frame = socket
                .next()
                .await
                .ok_or_else(|| ApiError::Network("the server closed the WebSocket".into()))?
                .map_err(from_tungstenite)?;
            match parse(&frame) {
                Some(Incoming::Reply(reply)) if reply.seq_reply == 1 && !reply.is_ok() => {
                    return Err(auth_error(&reply));
                }
                Some(Incoming::Reply(_)) | None => {}
                Some(Incoming::Event(event)) if event.event == "hello" => {
                    break serde_json::from_value::<Hello>(event.data)
                        .map_err(|e| ApiError::Decode(e.to_string()))?;
                }
                Some(Incoming::Event(event)) => early.push(event),
            }
            if let Message::Close(close) = frame {
                let reason = close.map(|c| c.reason.to_string()).unwrap_or_default();
                return Err(ApiError::Server {
                    status: 401,
                    id: String::new(),
                    message: format!("the server closed the WebSocket: {reason}"),
                    request_id: None,
                });
            }
        };

        let (events, receiver) = mpsc::unbounded_channel();
        for event in early {
            let _ = events.send(WsMessage::Event(event));
        }
        let (commands, command_rx) = mpsc::unbounded_channel();
        tokio::spawn(run(socket, command_rx, events));
        Ok((Self { hello, commands }, receiver))
    }

    /// The `hello` the server sent on connecting.
    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    /// Sends `action` with `data` and waits for the server's reply.
    pub async fn send(&self, action: &str, data: Value) -> Result<WsReply, ApiError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Send {
                action: action.to_string(),
                data,
                reply,
            })
            .map_err(|_| closed())?;
        tokio::time::timeout(REPLY_TIMEOUT, answer)
            .await
            .map_err(|_| ApiError::Timeout)?
            .map_err(|_| closed())?
    }

    /// Tells the channel's members the user is typing; `parent_id` is the
    /// thread root when typing a reply.
    pub async fn user_typing(
        &self,
        channel_id: &str,
        parent_id: &str,
    ) -> Result<WsReply, ApiError> {
        self.send(
            "user_typing",
            json!({ "channel_id": channel_id, "parent_id": parent_id }),
        )
        .await
    }

    /// Asks for the statuses of `user_ids`; the reply's `data` maps user id
    /// to status.
    pub async fn get_statuses_by_ids(&self, user_ids: &[&str]) -> Result<WsReply, ApiError> {
        self.send("get_statuses_by_ids", json!({ "user_ids": user_ids }))
            .await
    }

    /// Closes the connection. The receiver gets
    /// [`DisconnectReason::ClientClosed`] last.
    pub fn close(&self) {
        let _ = self.commands.send(Command::Close);
    }
}

impl Drop for WsConnection {
    fn drop(&mut self) {
        self.close();
    }
}

enum Incoming {
    Event(RawEvent),
    Reply(WsReply),
}

fn parse(frame: &Message) -> Option<Incoming> {
    let Message::Text(text) = frame else {
        return None;
    };
    let value: Value = serde_json::from_str(text.as_str()).ok()?;
    if value.get("seq_reply").is_some() {
        serde_json::from_value(value).ok().map(Incoming::Reply)
    } else if value.get("event").is_some() {
        serde_json::from_value(value).ok().map(Incoming::Event)
    } else {
        None
    }
}

async fn run(
    mut socket: Socket,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<WsMessage>,
) {
    let mut next_seq: u64 = 2;
    let mut pending: HashMap<u64, oneshot::Sender<Result<WsReply, ApiError>>> = HashMap::new();
    let reason = loop {
        tokio::select! {
            frame = socket.next() => match frame {
                None => break DisconnectReason::Lost("the stream ended".into()),
                Some(Err(error)) => break DisconnectReason::Lost(error.to_string()),
                Some(Ok(Message::Close(close))) => {
                    break match close {
                        Some(frame) => DisconnectReason::Closed {
                            code: frame.code.into(),
                            reason: frame.reason.to_string(),
                        },
                        None => DisconnectReason::Closed { code: 1005, reason: String::new() },
                    };
                }
                // tungstenite queues the pong for a ping itself; flushing
                // sends it now rather than with the next request.
                Some(Ok(Message::Ping(_))) => {
                    let _ = socket.flush().await;
                }
                Some(Ok(frame)) => match parse(&frame) {
                    Some(Incoming::Reply(reply)) => {
                        if let Some(waiter) = pending.remove(&reply.seq_reply) {
                            let _ = waiter.send(Ok(reply));
                        }
                    }
                    Some(Incoming::Event(event)) => {
                        let _ = events.send(WsMessage::Event(event));
                    }
                    None => {}
                },
            },
            command = commands.recv() => match command {
                Some(Command::Send { action, data, reply }) => {
                    let seq = next_seq;
                    next_seq += 1;
                    let request = json!({ "seq": seq, "action": action, "data": data });
                    match socket.send(Message::text(request.to_string())).await {
                        Ok(()) => {
                            pending.insert(seq, reply);
                        }
                        Err(error) => {
                            let _ = reply.send(Err(from_tungstenite(error)));
                        }
                    }
                }
                Some(Command::Close) | None => {
                    let _ = socket.close(None).await;
                    break DisconnectReason::ClientClosed;
                }
            },
        }
    };
    for (_, waiter) in pending.drain() {
        let _ = waiter.send(Err(closed()));
    }
    let _ = events.send(WsMessage::Disconnected { reason });
}

fn websocket_url(client: &Client) -> Result<url::Url, ApiError> {
    let mut url = client.api_url("websocket")?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme)
        .map_err(|()| ApiError::InvalidUrl(format!("cannot use {scheme} for {url}")))?;
    Ok(url)
}

fn auth_error(reply: &WsReply) -> ApiError {
    let field = |name: &str| {
        reply
            .error
            .as_ref()
            .and_then(|e| e.get(name))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    ApiError::Server {
        status: 401,
        id: field("id"),
        message: field("message"),
        request_id: None,
    }
}

fn closed() -> ApiError {
    ApiError::Network("the WebSocket is closed".into())
}

fn from_tungstenite(error: tungstenite::Error) -> ApiError {
    use tungstenite::Error;
    match error {
        Error::Http(response) => ApiError::Server {
            status: response.status().as_u16(),
            id: String::new(),
            message: format!("the WebSocket upgrade was refused ({})", response.status()),
            request_id: None,
        },
        Error::Url(e) => ApiError::InvalidUrl(e.to_string()),
        Error::Tls(e) => ApiError::Tls(e.to_string()),
        Error::Io(e) => match crate::error::tls_cause(&e) {
            Some(tls) => ApiError::Tls(tls),
            None => ApiError::Network(e.to_string()),
        },
        other => ApiError::Network(other.to_string()),
    }
}
