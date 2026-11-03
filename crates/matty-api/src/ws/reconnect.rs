//! A WebSocket that stays connected (PLAN §6.4): reconnects with jittered
//! exponential backoff, resumes the event stream where it left off, drops
//! events it already delivered, and says when the server could not resume so
//! the sync engine must resync.

use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use serde_json::Value;
use tokio::sync::mpsc;

use super::{
    DisconnectReason, RawEvent, Resume, SeqTracker, SeqVerdict, WsConfig, WsConnection, WsMessage,
    WsReply,
};
use crate::{ApiError, Client};

/// Exponential backoff with full jitter: before reconnect attempt `n`
/// (from 1), wait a random time in `[0, min(cap, base × factor^(n-1))]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Backoff {
    /// The ceiling for the first attempt. 1 second by default.
    pub base: Duration,
    /// How much the ceiling grows per attempt. 2 by default.
    pub factor: f64,
    /// The largest ceiling. 30 seconds by default.
    pub cap: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(1),
            factor: 2.0,
            cap: Duration::from_secs(30),
        }
    }
}

impl Backoff {
    /// The longest wait before attempt `attempt` (from 1).
    pub fn ceiling(&self, attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1).min(64) as i32;
        let secs = self.base.as_secs_f64() * self.factor.powi(exponent);
        Duration::from_secs_f64(secs.min(self.cap.as_secs_f64()))
    }

    /// The wait before attempt `attempt`, for a jitter `unit` in `[0, 1)`.
    pub fn delay(&self, attempt: u32, unit: f64) -> Duration {
        self.ceiling(attempt).mul_f64(unit.clamp(0.0, 1.0))
    }
}

/// A source of jitter in `[0, 1)`. Swapped for a fixed one in tests.
pub trait Jitter: Send + Sync + 'static {
    /// The next value in `[0, 1)`.
    fn unit(&self) -> f64;
}

/// A small xorshift generator seeded from the clock; jitter only needs to
/// spread clients apart, not be unpredictable.
struct XorShift(AtomicU64);

impl XorShift {
    fn seeded() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9e37_79b9_7f4a_7c15, |d| d.as_nanos() as u64);
        Self(AtomicU64::new(nanos | 1))
    }
}

impl Jitter for XorShift {
    fn unit(&self) -> f64 {
        let mut x = self.0.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0.store(x, Ordering::Relaxed);
        (x >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The result of one connection attempt.
pub type Dialed = Result<(WsConnection, mpsc::UnboundedReceiver<WsMessage>), ApiError>;

/// Opens a connection, resuming `resume` when given. [`ClientDialer`] is
/// the real one; tests dial in-memory servers.
pub trait Dialer: Send + Sync + 'static {
    /// Opens one connection.
    fn dial(&self, resume: Option<Resume>) -> Pin<Box<dyn Future<Output = Dialed> + Send>>;
}

/// Dials the server a [`Client`] points at, with its session token.
#[derive(Clone, Debug)]
pub struct ClientDialer {
    /// The signed-in client.
    pub client: Client,
    /// Liveness settings for each connection.
    pub config: WsConfig,
}

impl Dialer for ClientDialer {
    fn dial(&self, resume: Option<Resume>) -> Pin<Box<dyn Future<Output = Dialed> + Send>> {
        let (client, config) = (self.client.clone(), self.config);
        Box::pin(async move { WsConnection::resume(&client, config, resume.as_ref()).await })
    }
}

/// Where the connection is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    /// The first attempt is under way.
    Connecting,
    /// Connected and authenticated. A resumed connection reports the
    /// previous id at first; if the server could not resume, a
    /// [`WsSignal::ResyncRequired`] and a second `Connected` with the new id
    /// follow.
    Connected {
        /// The server's id for this connection.
        connection_id: String,
    },
    /// Waiting to retry, or retrying.
    Reconnecting {
        /// The attempt about to be made, from 1.
        attempt: u32,
        /// Why the last connection or attempt ended.
        reason: String,
    },
    /// The server refused the session token; nothing more will be tried.
    Unauthorized,
    /// [`WsClient::stop`] was called or the client was dropped.
    Stopped,
}

/// What a [`WsClient`] delivers.
#[derive(Clone, Debug, PartialEq)]
pub enum WsSignal {
    /// The connection state changed.
    State(ConnectionState),
    /// A server event, each delivered at most once and in `seq` order.
    Event(RawEvent),
    /// Events with these `seq`s never arrived; sent just before the event
    /// that followed them. The sync engine decides whether to resync.
    Gap {
        /// The missing `seq`s.
        missing: std::ops::Range<i64>,
    },
    /// The server started a new connection instead of resuming, so events
    /// were missed: the sync engine must catch up over REST (PLAN §6.5).
    ///
    /// A resumed stream is not a promise that nothing was missed, though.
    /// Mattermost 10.5 replays events addressed to the user, but channel
    /// events sent more than about a second after the old socket closed are
    /// dropped with no gap in `seq` and no new `hello` (seen against the dev
    /// server). The sync engine should still run a cheap `since` catch-up
    /// after every reconnect.
    ResyncRequired,
}

enum Control {
    ReconnectNow,
    Stop,
}

/// A WebSocket that reconnects by itself. Dropping it stops it.
pub struct WsClient {
    control: mpsc::UnboundedSender<Control>,
    current: Arc<Mutex<Option<Arc<WsConnection>>>>,
}

impl fmt::Debug for WsClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WsClient").finish_non_exhaustive()
    }
}

impl WsClient {
    /// Keeps a connection to `client`'s server open with default settings.
    pub fn connect(client: Client) -> (Self, mpsc::UnboundedReceiver<WsSignal>) {
        let dialer = ClientDialer {
            client,
            config: WsConfig::default(),
        };
        Self::start(dialer, Backoff::default(), XorShift::seeded())
    }

    /// Keeps a connection open through `dialer`. Must be called inside a
    /// tokio runtime.
    pub fn start(
        dialer: impl Dialer,
        backoff: Backoff,
        jitter: impl Jitter,
    ) -> (Self, mpsc::UnboundedReceiver<WsSignal>) {
        let (control, control_rx) = mpsc::unbounded_channel();
        let (signals, signal_rx) = mpsc::unbounded_channel();
        let current = Arc::new(Mutex::new(None));
        let supervisor = Supervisor {
            dialer: Box::new(dialer),
            backoff,
            jitter: Box::new(jitter),
            signals,
            control: control_rx,
            current: current.clone(),
        };
        tokio::spawn(supervisor.run());
        (Self { control, current }, signal_rx)
    }

    /// Drops the current connection, if any, and reconnects at once,
    /// skipping any backoff wait. For OS network-change and wake signals.
    pub fn reconnect_now(&self) {
        let _ = self.control.send(Control::ReconnectNow);
    }

    /// The current connection, to send on from another task; `None` while
    /// reconnecting.
    pub fn connection(&self) -> Option<Arc<WsConnection>> {
        self.current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Sends a request on the current connection.
    pub async fn send(&self, action: &str, data: Value) -> Result<WsReply, ApiError> {
        let connection = self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or_else(|| ApiError::Network("the WebSocket is not connected".into()))?;
        connection.send(action, data).await
    }

    /// Stops reconnecting and closes the connection; the last signal is
    /// [`ConnectionState::Stopped`].
    pub fn stop(&self) {
        let _ = self.control.send(Control::Stop);
    }
}

impl Drop for WsClient {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Supervisor {
    dialer: Box<dyn Dialer>,
    backoff: Backoff,
    jitter: Box<dyn Jitter>,
    signals: mpsc::UnboundedSender<WsSignal>,
    control: mpsc::UnboundedReceiver<Control>,
    current: Arc<Mutex<Option<Arc<WsConnection>>>>,
}

enum Ended {
    Lost(String),
    ReconnectNow,
    Stop,
}

impl Supervisor {
    fn emit(&self, signal: WsSignal) {
        let _ = self.signals.send(signal);
    }

    fn set_current(&self, connection: Option<Arc<WsConnection>>) {
        *self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = connection;
    }

    async fn run(mut self) {
        let mut resume: Option<Resume> = None;
        let mut attempt: u32 = 0;
        self.emit(WsSignal::State(ConnectionState::Connecting));
        loop {
            let dialed = tokio::select! {
                dialed = self.dialer.dial(resume.clone()) => dialed,
                control = self.control.recv() => match control {
                    Some(Control::ReconnectNow) => continue,
                    Some(Control::Stop) | None => break,
                },
            };
            let reason = match dialed {
                Ok((connection, events)) => {
                    attempt = 0;
                    match self.serve(connection, events, &mut resume).await {
                        Ended::Stop => break,
                        Ended::ReconnectNow => {
                            self.emit(WsSignal::State(ConnectionState::Reconnecting {
                                attempt: 1,
                                reason: "reconnect requested".into(),
                            }));
                            continue;
                        }
                        Ended::Lost(reason) => reason,
                    }
                }
                Err(ApiError::Server { status: 401, .. }) => {
                    self.emit(WsSignal::State(ConnectionState::Unauthorized));
                    return;
                }
                Err(error) => error.to_string(),
            };
            attempt = attempt.saturating_add(1);
            self.emit(WsSignal::State(ConnectionState::Reconnecting {
                attempt,
                reason,
            }));
            let wait = self.backoff.delay(attempt, self.jitter.unit());
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                control = self.control.recv() => match control {
                    Some(Control::ReconnectNow) => {}
                    Some(Control::Stop) | None => break,
                },
            }
        }
        self.set_current(None);
        self.emit(WsSignal::State(ConnectionState::Stopped));
    }

    /// Delivers one connection's events until it ends.
    async fn serve(
        &mut self,
        connection: WsConnection,
        mut events: mpsc::UnboundedReceiver<WsMessage>,
        resume: &mut Option<Resume>,
    ) -> Ended {
        let mut connection_id = connection.hello().connection_id.clone();
        let resumed = match resume.as_ref() {
            Some(previous) if previous.connection_id == connection_id => true,
            Some(_) => {
                self.emit(WsSignal::ResyncRequired);
                false
            }
            None => false,
        };
        // A new connection's events count from 1 (`hello` is 0).
        let mut seq = match resume.as_ref() {
            Some(previous) if resumed => SeqTracker::starting_at(previous.sequence_number),
            _ => SeqTracker::default(),
        };
        let connection = Arc::new(connection);
        self.set_current(Some(connection.clone()));
        self.emit(WsSignal::State(ConnectionState::Connected {
            connection_id: connection_id.clone(),
        }));

        let ended = loop {
            tokio::select! {
                message = events.recv() => match message {
                    Some(WsMessage::Event(event)) => match seq.accept(event.seq) {
                        // Replays and late arrivals the client is past.
                        SeqVerdict::Stale => {}
                        SeqVerdict::Deliver { gap } => {
                            if let Some(missing) = gap {
                                self.emit(WsSignal::Gap { missing });
                            }
                            self.emit(WsSignal::Event(event));
                        }
                    },
                    // The server could not resume and started over.
                    Some(WsMessage::Hello(hello)) if hello.connection_id != connection_id => {
                        self.emit(WsSignal::ResyncRequired);
                        connection_id = hello.connection_id;
                        seq = SeqTracker::default();
                        self.emit(WsSignal::State(ConnectionState::Connected {
                            connection_id: connection_id.clone(),
                        }));
                    }
                    Some(WsMessage::Hello(_)) => {}
                    Some(WsMessage::Disconnected { reason }) => break Ended::Lost(describe(&reason)),
                    None => break Ended::Lost("the connection ended".into()),
                },
                control = self.control.recv() => match control {
                    Some(Control::ReconnectNow) => break Ended::ReconnectNow,
                    Some(Control::Stop) | None => break Ended::Stop,
                },
            }
        };
        self.set_current(None);
        connection.close();
        *resume = Some(Resume {
            connection_id,
            sequence_number: seq.next(),
        });
        ended
    }
}

fn describe(reason: &DisconnectReason) -> String {
    match reason {
        DisconnectReason::Closed { code, reason } => format!("closed ({code}) {reason}"),
        DisconnectReason::Lost(why) => format!("lost: {why}"),
        DisconnectReason::ClientClosed => "closed by Matty".into(),
        DisconnectReason::Timeout => "the server stopped answering".into(),
    }
}
