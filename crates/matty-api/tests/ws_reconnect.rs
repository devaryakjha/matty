//! Reconnect, resume and resync, against scripted in-memory servers on
//! tokio's paused clock.

#![allow(clippy::unwrap_used)]
// The handshake callback's signature is tungstenite's, with its large error type.
#![allow(clippy::result_large_err)]

use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use matty_api::{
    ApiError, Client,
    ws::{
        Backoff, ConnectionState, Dialed, Dialer, Jitter, Resume, WsClient, WsConfig, WsConnection,
        WsSignal,
    },
};
use proptest::prelude::*;
use serde_json::{Value, json};
use tokio::{io::DuplexStream, sync::mpsc, time::Instant};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

type Server = WebSocketStream<DuplexStream>;
/// When each dial happened, what it asked to resume, and its query string.
type Dials = Arc<Mutex<Vec<(Instant, Option<Resume>, String)>>>;
type Script = Box<dyn FnOnce(Server) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// One scripted connection: accept it as `connection_id` and run `script`,
/// or refuse it with `error`.
enum Plan {
    Accept {
        connection_id: &'static str,
        script: Script,
    },
    Refuse(ApiError),
}

fn accept<F, Fut>(connection_id: &'static str, script: F) -> Plan
where
    F: FnOnce(Server) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    Plan::Accept {
        connection_id,
        script: Box::new(move |socket| Box::pin(script(socket))),
    }
}

/// Dials the next plan; records each dial's time and the query it asked for.
#[derive(Clone, Default)]
struct ScriptedDialer {
    plans: Arc<Mutex<VecDeque<Plan>>>,
    dials: Dials,
}

impl ScriptedDialer {
    fn new(plans: Vec<Plan>) -> Self {
        Self {
            plans: Arc::new(Mutex::new(plans.into())),
            dials: Arc::default(),
        }
    }
}

impl Dialer for ScriptedDialer {
    fn dial(&self, resume: Option<Resume>) -> Pin<Box<dyn Future<Output = Dialed> + Send>> {
        let plan = self.plans.lock().unwrap().pop_front();
        let dials = self.dials.clone();
        Box::pin(async move {
            let Some(plan) = plan else {
                // Out of plans: refuse forever.
                dials
                    .lock()
                    .unwrap()
                    .push((Instant::now(), resume, String::new()));
                return Err(ApiError::Network("no more plans".into()));
            };
            let (connection_id, script) = match plan {
                Plan::Refuse(error) => {
                    dials
                        .lock()
                        .unwrap()
                        .push((Instant::now(), resume, String::new()));
                    return Err(error);
                }
                Plan::Accept {
                    connection_id,
                    script,
                } => (connection_id, script),
            };
            let resume_id = resume.as_ref().map(|r| r.connection_id.clone());
            let (client_end, server_end) = tokio::io::duplex(64 * 1024);
            let (query_tx, query_rx) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                let callback =
                    |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                     response| {
                        let _ =
                            query_tx.send(request.uri().query().unwrap_or_default().to_string());
                        Ok(response)
                    };
                let mut socket = tokio_tungstenite::accept_hdr_async(server_end, callback)
                    .await
                    .unwrap();
                next_json(&mut socket).await.unwrap(); // the challenge
                // Like Mattermost: a resumed connection gets no hello; a new
                // one (or one that could not be resumed) does.
                let resumed = resume_id.as_deref() == Some(connection_id);
                if !resumed {
                    send(
                        &mut socket,
                        json!({"event": "hello", "data": {"connection_id": connection_id, "server_version": "10.5.0"}, "seq": 0}),
                    )
                    .await;
                }
                script(socket).await;
            });
            let client = Client::new("http://matty.test").unwrap().with_token("t");
            let dialed = WsConnection::connect_over(
                client_end,
                &client,
                WsConfig::default(),
                resume.as_ref(),
            )
            .await;
            let query = query_rx.await.unwrap_or_default();
            dials.lock().unwrap().push((Instant::now(), resume, query));
            dialed
        })
    }
}

/// Always the top of the backoff range, so waits are predictable.
struct Max;
impl Jitter for Max {
    fn unit(&self) -> f64 {
        0.999_999
    }
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

async fn events(socket: &mut Server, seqs: impl IntoIterator<Item = i64>) {
    for seq in seqs {
        send(
            socket,
            json!({"event": "typing", "data": {"user_id": format!("u{seq}")}, "seq": seq}),
        )
        .await;
    }
}

/// Collects signals until `stop` says so.
async fn collect(
    signals: &mut mpsc::UnboundedReceiver<WsSignal>,
    mut stop: impl FnMut(&[WsSignal]) -> bool,
) -> Vec<WsSignal> {
    let mut seen = Vec::new();
    while !stop(&seen) {
        let signal = tokio::time::timeout(Duration::from_secs(600), signals.recv())
            .await
            .expect("a signal within 10 simulated minutes")
            .expect("signals stay open");
        seen.push(signal);
    }
    seen
}

fn seqs(signals: &[WsSignal]) -> Vec<i64> {
    signals
        .iter()
        .filter_map(|s| match s {
            WsSignal::Event(event) => Some(event.seq),
            _ => None,
        })
        .collect()
}

fn resyncs(signals: &[WsSignal]) -> usize {
    signals
        .iter()
        .filter(|s| matches!(s, WsSignal::ResyncRequired))
        .count()
}

fn connected(signals: &[WsSignal]) -> usize {
    signals
        .iter()
        .filter(|s| matches!(s, WsSignal::State(ConnectionState::Connected { .. })))
        .count()
}

#[tokio::test(start_paused = true)]
async fn a_resumed_stream_has_no_gap_and_no_duplicates() {
    let dialer = ScriptedDialer::new(vec![
        accept("conn-a", |mut socket| async move {
            events(&mut socket, 1..=3).await;
            // Drop the connection without a close frame.
        }),
        accept("conn-a", |mut socket| async move {
            // The server replays from what it thinks was last seen,
            // overlapping what the client already has.
            events(&mut socket, [2, 3, 4, 5, 6]).await;
            while next_json(&mut socket).await.is_some() {}
        }),
    ]);
    let (client, mut signals) = WsClient::start(dialer.clone(), Backoff::default(), Max);
    let seen = collect(&mut signals, |s| seqs(s).len() >= 6).await;

    assert_eq!(seqs(&seen), vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(resyncs(&seen), 0);
    assert_eq!(connected(&seen), 2);
    let dials = dialer.dials.lock().unwrap().clone();
    assert_eq!(dials[0].1, None);
    assert_eq!(
        dials[1].1,
        Some(Resume {
            connection_id: "conn-a".into(),
            sequence_number: 4
        })
    );
    assert_eq!(dials[1].2, "connection_id=conn-a&sequence_number=4");
    drop(client);
}

#[tokio::test(start_paused = true)]
async fn a_new_connection_id_requires_one_resync_per_reconnect() {
    let dialer = ScriptedDialer::new(vec![
        accept("conn-a", |mut socket| async move {
            events(&mut socket, 1..=2).await;
        }),
        // Not resumable: a new id, numbering from 1 again.
        accept("conn-b", |mut socket| async move {
            events(&mut socket, 1..=2).await;
        }),
        // Resumed: same id, no resync.
        accept("conn-b", |mut socket| async move {
            events(&mut socket, 3..=4).await;
        }),
        accept("conn-c", |mut socket| async move {
            events(&mut socket, [1]).await;
            while next_json(&mut socket).await.is_some() {}
        }),
    ]);
    let (_client, mut signals) = WsClient::start(dialer, Backoff::default(), Max);
    let seen = collect(&mut signals, |s| seqs(s).len() >= 7).await;

    assert_eq!(seqs(&seen), vec![1, 2, 1, 2, 3, 4, 1]);
    assert_eq!(resyncs(&seen), 2, "conn-a → conn-b and conn-b → conn-c");
    // Each resync comes after its reconnect and before that connection's events.
    let order: Vec<String> = seen
        .iter()
        .filter_map(|s| match s {
            WsSignal::ResyncRequired => Some("resync".into()),
            WsSignal::State(ConnectionState::Connected { connection_id }) => {
                Some(connection_id.clone())
            }
            WsSignal::Event(e) => Some(e.seq.to_string()),
            WsSignal::State(_) | WsSignal::Gap { .. } => None,
        })
        .collect();
    assert_eq!(
        order,
        [
            // A resumed connection reports the previous id until the
            // server's hello says it started over.
            "conn-a", "1", "2", "conn-a", "resync", "conn-b", "1", "2", "conn-b", "3", "4",
            "conn-b", "resync", "conn-c", "1"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn failed_dials_back_off_exponentially_up_to_30_seconds() {
    let dialer = ScriptedDialer::new(vec![]);
    let start = Instant::now();
    let (_client, mut signals) = WsClient::start(dialer.clone(), Backoff::default(), Max);
    let seen = collect(&mut signals, |s| {
        s.iter()
            .filter(|x| matches!(x, WsSignal::State(ConnectionState::Reconnecting { .. })))
            .count()
            >= 9
    })
    .await;
    let attempts: Vec<u32> = seen
        .iter()
        .filter_map(|s| match s {
            WsSignal::State(ConnectionState::Reconnecting { attempt, .. }) => Some(*attempt),
            _ => None,
        })
        .collect();
    assert_eq!(attempts, (1..=9).collect::<Vec<_>>());
    let times: Vec<f64> = dialer
        .dials
        .lock()
        .unwrap()
        .iter()
        .map(|(at, _, _)| (*at - start).as_secs_f64())
        .collect();
    let gaps: Vec<f64> = times.windows(2).map(|w| (w[1] - w[0]).round()).collect();
    assert_eq!(gaps, [1.0, 2.0, 4.0, 8.0, 16.0, 30.0, 30.0, 30.0]);
}

#[tokio::test(start_paused = true)]
async fn reconnect_now_skips_the_wait_and_replaces_a_live_connection() {
    let dialer = ScriptedDialer::new(vec![
        Plan::Refuse(ApiError::Network("down".into())),
        Plan::Refuse(ApiError::Network("down".into())),
        Plan::Refuse(ApiError::Network("down".into())),
        Plan::Refuse(ApiError::Network("down".into())),
        Plan::Refuse(ApiError::Network("down".into())),
        accept("conn-a", |mut socket| async move {
            events(&mut socket, [1]).await;
            while next_json(&mut socket).await.is_some() {}
        }),
        accept("conn-a", |mut socket| async move {
            events(&mut socket, [2]).await;
            while next_json(&mut socket).await.is_some() {}
        }),
    ]);
    let (client, mut signals) = WsClient::start(dialer.clone(), Backoff::default(), Max);
    // Five failures: the next wait would be 16 s.
    collect(&mut signals, |s| {
        s.iter().any(|x| {
            matches!(
                x,
                WsSignal::State(ConnectionState::Reconnecting { attempt: 5, .. })
            )
        })
    })
    .await;
    let before = Instant::now();
    client.reconnect_now();
    let seen = collect(&mut signals, |s| seqs(s) == [1]).await;
    assert!(
        Instant::now() - before < Duration::from_millis(100),
        "no backoff wait"
    );
    assert_eq!(connected(&seen), 1);

    // On a live connection, it reconnects too, resuming the stream.
    client.reconnect_now();
    let seen = collect(&mut signals, |s| seqs(s) == [2]).await;
    assert_eq!(resyncs(&seen), 0);
    let last = dialer.dials.lock().unwrap().last().unwrap().1.clone();
    assert_eq!(
        last,
        Some(Resume {
            connection_id: "conn-a".into(),
            sequence_number: 2
        })
    );
}

#[tokio::test(start_paused = true)]
async fn a_burst_of_reconnect_requests_dials_once() {
    let down = || Plan::Refuse(ApiError::Network("down".into()));
    let dialer = ScriptedDialer::new(vec![down(), down(), down(), down(), down()]);
    let (client, mut signals) = WsClient::start(dialer.clone(), Backoff::default(), Max);
    let attempt = |n: u32| {
        move |s: &[WsSignal]| {
            s.iter().any(|x| {
                matches!(x, WsSignal::State(ConnectionState::Reconnecting { attempt, .. }) if *attempt == n)
            })
        }
    };
    collect(&mut signals, attempt(3)).await;
    assert_eq!(dialer.dials.lock().unwrap().len(), 3);
    // A flapping network: many requests while waiting to retry.
    for _ in 0..10 {
        client.reconnect_now();
    }
    collect(&mut signals, attempt(4)).await;
    assert_eq!(dialer.dials.lock().unwrap().len(), 4);
    // The rest were dropped, not left to cut the next wait short: the
    // fifth attempt waits out its full backoff.
    let before = Instant::now();
    collect(&mut signals, attempt(5)).await;
    assert_eq!(Instant::now() - before, Duration::from_secs(8));
    assert_eq!(dialer.dials.lock().unwrap().len(), 5);
}

#[tokio::test(start_paused = true)]
async fn a_refused_token_stops_reconnecting() {
    let dialer = ScriptedDialer::new(vec![Plan::Refuse(ApiError::Server {
        status: 401,
        id: "api.web_socket_router.not_authenticated.app_error".into(),
        message: String::new(),
        request_id: None,
    })]);
    let (_client, mut signals) = WsClient::start(dialer.clone(), Backoff::default(), Max);
    let seen = collect(&mut signals, |s| {
        s.contains(&WsSignal::State(ConnectionState::Unauthorized))
    })
    .await;
    assert_eq!(
        seen,
        [
            WsSignal::State(ConnectionState::Connecting),
            WsSignal::State(ConnectionState::Unauthorized)
        ]
    );
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert_eq!(dialer.dials.lock().unwrap().len(), 1, "no retries");
}

#[tokio::test(start_paused = true)]
async fn requests_go_to_the_current_connection_and_stop_ends_it() {
    let dialer = ScriptedDialer::new(vec![accept("conn-a", |mut socket| async move {
        while let Some(request) = next_json(&mut socket).await {
            if request["action"] == "user_typing" {
                send(
                    &mut socket,
                    json!({"status": "OK", "seq_reply": request["seq"]}),
                )
                .await;
            }
        }
    })]);
    let (client, mut signals) = WsClient::start(dialer, Backoff::default(), Max);
    collect(&mut signals, |s| connected(s) == 1).await;
    let reply = client
        .send("user_typing", json!({"channel_id": "c1"}))
        .await
        .unwrap();
    assert!(reply.is_ok());
    client.stop();
    let seen = collect(&mut signals, |s| {
        s.contains(&WsSignal::State(ConnectionState::Stopped))
    })
    .await;
    assert_eq!(
        seen.last(),
        Some(&WsSignal::State(ConnectionState::Stopped))
    );
    assert!(client.send("user_typing", json!({})).await.is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn backoff_stays_within_zero_and_min_30_or_2_to_the_n(attempt in 1u32..200, unit in 0.0f64..1.0) {
        let backoff = Backoff::default();
        let delay = backoff.delay(attempt, unit).as_secs_f64();
        let bound = 30f64.min(2f64.powi(attempt.min(64) as i32));
        prop_assert!(delay >= 0.0);
        prop_assert!(delay <= bound, "attempt {attempt}: {delay} > {bound}");
        prop_assert!(backoff.ceiling(attempt) <= backoff.ceiling(attempt + 1));
    }
}

#[tokio::test(start_paused = true)]
async fn the_client_reports_gaps_and_drops_late_and_duplicate_events() {
    let dialer = ScriptedDialer::new(vec![accept("conn-a", |mut socket| async move {
        events(&mut socket, [1, 2, 2, 5, 4, 3, 6, 9]).await;
        while next_json(&mut socket).await.is_some() {}
    })]);
    let (_client, mut signals) = WsClient::start(dialer, Backoff::default(), Max);
    let seen = collect(&mut signals, |s| seqs(s).last() == Some(&9)).await;
    let stream: Vec<String> = seen
        .iter()
        .filter_map(|s| match s {
            WsSignal::Event(e) => Some(e.seq.to_string()),
            WsSignal::Gap { missing } => Some(format!("gap {}..{}", missing.start, missing.end)),
            _ => None,
        })
        .collect();
    assert_eq!(stream, ["1", "2", "gap 3..5", "5", "6", "gap 7..9", "9"]);
}
