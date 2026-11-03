//! The WebSocket connection (PLAN §6.4): connect, authenticate, `hello`,
//! server events, and requests answered by `seq_reply`.
//!
//! [`WsConnection::connect`] returns once the server has sent `hello`. A
//! background task owns the socket; events arrive on the returned receiver
//! and [`WsConnection::send`] queues a request and waits for its reply.
//!
//! The task sends `{"action":"ping"}` every [`WsConfig::ping_interval`] and
//! ends the connection with [`DisconnectReason::Timeout`] when nothing at
//! all arrives for [`WsConfig::pong_timeout`] after a ping.

use std::{collections::HashMap, fmt, sync::Arc, time::Duration};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot},
    time::Instant,
};
use tokio_tungstenite::{
    Connector, WebSocketStream,
    tungstenite::{self, Message, client::IntoClientRequest, http::HeaderValue},
};

use crate::{ApiError, Client, models::null_as_default, tls};

mod events;
mod reconnect;
pub use events::{Event, PostUnread, Posted, ThreadReadChanged, WsEvent};
pub use reconnect::{
    Backoff, ClientDialer, ConnectionState, Dialed, Dialer, Jitter, WsClient, WsSignal,
};

/// Where to pick up a connection's event stream after a reconnect.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resume {
    /// The `connection_id` from the previous `hello`.
    pub connection_id: String,
    /// The `seq` of the next event the client expects.
    pub sequence_number: i64,
}

/// How long [`WsConnection::connect`] waits for the upgrade, the
/// authentication reply and `hello`, together.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long [`WsConnection::send`] waits for a reply.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// Liveness settings for a connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WsConfig {
    /// How often to send `{"action":"ping"}`. 30 seconds by default.
    pub ping_interval: Duration,
    /// How long after a ping to wait for any frame before giving up on the
    /// connection. 30 seconds by default.
    pub pong_timeout: Duration,
}

impl Default for WsConfig {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(30),
            pong_timeout: Duration::from_secs(30),
        }
    }
}

/// A byte stream a WebSocket can run over.
pub trait Transport: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Transport for T {}

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
    /// Nothing arrived for [`WsConfig::pong_timeout`] after a ping.
    Timeout,
}

/// What the connection delivers.
#[derive(Clone, Debug, PartialEq)]
pub enum WsMessage {
    /// A server event other than `hello`.
    Event(RawEvent),
    /// A `hello` after the connection opened. On a resumed connection this
    /// means the server could not resume: its `connection_id` is new and
    /// events were missed.
    Hello(Hello),
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
        Self::connect_with(client, WsConfig::default()).await
    }

    /// [`WsConnection::connect`] with liveness settings.
    pub async fn connect_with(
        client: &Client,
        config: WsConfig,
    ) -> Result<(Self, mpsc::UnboundedReceiver<WsMessage>), ApiError> {
        Self::resume(client, config, None).await
    }

    /// [`WsConnection::connect_with`], asking the server to resume the event
    /// stream `resume` names. The server either replays what was missed
    /// under the same `connection_id`, or starts over with a new one.
    pub async fn resume(
        client: &Client,
        config: WsConfig,
        resume: Option<&Resume>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<WsMessage>), ApiError> {
        let token = session_token(client)?;
        let request = upgrade_request(&websocket_url(client, resume)?, &token)?;
        tokio::time::timeout(CONNECT_TIMEOUT, async move {
            let connector = Connector::Rustls(Arc::new(tls::client_config(tls::WEBSOCKET_ALPN)?));
            let (socket, _) = tokio_tungstenite::connect_async_tls_with_config(
                request,
                None,
                false,
                Some(connector),
            )
            .await
            .map_err(from_tungstenite)?;
            Self::handshake(socket, token, config, resume).await
        })
        .await
        .map_err(|_| ApiError::Timeout)?
    }

    /// Runs the WebSocket protocol for `client`'s server over a stream that
    /// is already open, such as an in-memory pipe in tests or a proxy tunnel.
    pub async fn connect_over(
        stream: impl Transport,
        client: &Client,
        config: WsConfig,
        resume: Option<&Resume>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<WsMessage>), ApiError> {
        let token = session_token(client)?;
        let request = upgrade_request(&websocket_url(client, resume)?, &token)?;
        tokio::time::timeout(CONNECT_TIMEOUT, async move {
            let (socket, _) = tokio_tungstenite::client_async(request, stream)
                .await
                .map_err(from_tungstenite)?;
            Self::handshake(socket, token, config, resume).await
        })
        .await
        .map_err(|_| ApiError::Timeout)?
    }

    async fn handshake<S: Transport>(
        mut socket: WebSocketStream<S>,
        token: String,
        config: WsConfig,
        resume: Option<&Resume>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<WsMessage>), ApiError> {
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

        let (events, receiver) = mpsc::unbounded_channel();
        let (commands, command_rx) = mpsc::unbounded_channel();
        // A resumed connection gets no `hello`: the server replays what was
        // missed, or nothing. If it cannot resume, a `hello` with a new
        // `connection_id` follows as [`WsMessage::Hello`].
        if let Some(resume) = resume.filter(|r| !r.connection_id.is_empty()) {
            let hello = Hello {
                connection_id: resume.connection_id.clone(),
                server_version: String::new(),
            };
            tokio::spawn(run(socket, command_rx, events, config));
            return Ok((Self { hello, commands }, receiver));
        }

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

        for event in early {
            let _ = events.send(WsMessage::Event(event));
        }
        tokio::spawn(run(socket, command_rx, events, config));
        Ok((Self { hello, commands }, receiver))
    }

    /// The `hello` the server sent on connecting. For a resumed connection,
    /// which gets no `hello`, the resumed `connection_id` and an empty
    /// version.
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

async fn run<S: Transport>(
    mut socket: WebSocketStream<S>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<WsMessage>,
    config: WsConfig,
) {
    let mut next_seq: u64 = 2;
    let mut pending: HashMap<u64, oneshot::Sender<Result<WsReply, ApiError>>> = HashMap::new();
    let mut ping =
        tokio::time::interval_at(Instant::now() + config.ping_interval, config.ping_interval);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Set when a ping goes out with nothing heard since; any frame clears it.
    let mut deadline: Option<Instant> = None;
    let reason = loop {
        let silent_until = deadline;
        tokio::select! {
            () = async { tokio::time::sleep_until(silent_until.unwrap_or_else(Instant::now)).await },
                if silent_until.is_some() => {
                break DisconnectReason::Timeout;
            }
            _ = ping.tick() => {
                let request = json!({ "seq": next_seq, "action": "ping" });
                next_seq += 1;
                if let Err(error) = socket.send(Message::text(request.to_string())).await {
                    break DisconnectReason::Lost(error.to_string());
                }
                deadline.get_or_insert(Instant::now() + config.pong_timeout);
            }
            frame = socket.next() => {
                deadline = None;
                match frame {
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
                    Some(Incoming::Event(event)) if event.event == "hello" => {
                        if let Ok(hello) = serde_json::from_value(event.data) {
                            let _ = events.send(WsMessage::Hello(hello));
                        }
                    }
                    Some(Incoming::Event(event)) => {
                        let _ = events.send(WsMessage::Event(event));
                    }
                    None => {}
                },
                }
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

fn session_token(client: &Client) -> Result<String, ApiError> {
    client
        .token_value()
        .map(str::to_string)
        .ok_or_else(|| ApiError::InvalidUrl("the client has no session token".into()))
}

/// The upgrade request, authenticated with `token`. Mattermost only resumes
/// a connection when the session is known at upgrade time, so the token goes
/// in the `Authorization` header as well as in the challenge after it (which
/// the server ignores when the header already signed the socket in, and
/// which still works behind a proxy that strips the header).
fn upgrade_request(
    url: &url::Url,
    token: &str,
) -> Result<tungstenite::handshake::client::Request, ApiError> {
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| ApiError::InvalidUrl(e.to_string()))?;
    let headers = request.headers_mut();
    headers.insert(
        "User-Agent",
        HeaderValue::from_static(concat!("Matty/", env!("CARGO_PKG_VERSION"))),
    );
    let mut bearer = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| ApiError::InvalidUrl("the session token is not a valid header".into()))?;
    bearer.set_sensitive(true);
    headers.insert("Authorization", bearer);
    Ok(request)
}

fn websocket_url(client: &Client, resume: Option<&Resume>) -> Result<url::Url, ApiError> {
    let mut url = client.api_url("websocket")?;
    if let Some(resume) = resume {
        url.query_pairs_mut()
            .append_pair("connection_id", &resume.connection_id)
            .append_pair("sequence_number", &resume.sequence_number.to_string());
    }
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
