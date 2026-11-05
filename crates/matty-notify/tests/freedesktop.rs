//! The Linux backend against a stand-in notification service on a private
//! session bus (#216): a notification posts with Matty's name and desktop
//! entry, clicking it reports the post, reading its channel closes it, and
//! the badge goes out as the Unity launcher signal.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_lite::{StreamExt as _, future};
use matty_notify::{Badge, Clicked, Notification};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

#[derive(Default)]
struct Calls {
    notified: Vec<(String, String, String, Vec<String>)>,
    closed: Vec<u32>,
}

struct Service(Arc<Mutex<Calls>>);

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Service {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        _replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let mut calls = self.0.lock().unwrap();
        calls.notified.push((app_name, summary, body, actions));
        u32::try_from(calls.notified.len()).unwrap()
    }

    async fn close_notification(
        &self,
        id: u32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        self.0.lock().unwrap().closed.push(id);
        Self::notification_closed(&emitter, id, 3).await.unwrap();
    }

    fn get_capabilities(&self) -> Vec<String> {
        vec!["actions".into(), "body".into()]
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        ("stand-in".into(), "matty".into(), "1".into(), "1.2".into())
    }

    #[zbus(signal)]
    async fn action_invoked(emitter: &SignalEmitter<'_>, id: u32, action: &str)
    -> zbus::Result<()>;

    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;
}

/// Set in the copy of this test that runs on the private bus.
const ON_PRIVATE_BUS: &str = "MATTY_NOTIFY_ON_PRIVATE_BUS";

/// Runs `test` again in a child process whose session bus is a private
/// `dbus-daemon`, so the backend finds the stand-in service and nothing
/// reaches the desktop's own. Returns whether this is that child.
fn on_private_bus(test: &str) -> bool {
    if std::env::var_os(ON_PRIVATE_BUS).is_some() {
        return true;
    }
    let mut daemon = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("dbus-daemon runs");
    let mut address = String::new();
    let stdout = daemon.stdout.take().unwrap();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(stdout), &mut address).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env("DBUS_SESSION_BUS_ADDRESS", address.trim())
        .env(ON_PRIVATE_BUS, "1")
        .status();
    let _ = daemon.kill();
    let _ = daemon.wait();
    assert!(
        status.unwrap().success(),
        "{test} failed on the private bus"
    );
    false
}

async fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        async_io::Timer::after(Duration::from_millis(10)).await;
    }
    panic!("{what} never happened");
}

#[test]
fn notifications_post_open_and_close_through_the_service() {
    if !on_private_bus("notifications_post_open_and_close_through_the_service") {
        return;
    }
    future::block_on(async {
        let calls = Arc::new(Mutex::new(Calls::default()));
        let service = zbus::connection::Builder::session()
            .unwrap()
            .name("org.freedesktop.Notifications")
            .unwrap()
            .serve_at("/org/freedesktop/Notifications", Service(calls.clone()))
            .unwrap()
            .build()
            .await
            .unwrap();
        let watcher = zbus::Connection::session().await.unwrap();
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("com.canonical.Unity.LauncherEntry")
            .unwrap()
            .build();
        let mut badges = zbus::MessageStream::for_match_rule(rule, &watcher, None)
            .await
            .unwrap();

        let (clicked_tx, clicked_rx) = async_channel::unbounded();
        let notifier = matty_notify::backend::platform(clicked_tx);
        notifier.show(&Notification {
            title: "town-square".into(),
            body: "Ada: ship it <now>".into(),
            channel_id: "c1".into(),
            post_id: "p1".into(),
            root_id: "r1".into(),
            mention: true,
            sound: None,
        });
        eventually("the notification", || {
            !calls.lock().unwrap().notified.is_empty()
        })
        .await;
        let (app, summary, body, actions) = calls.lock().unwrap().notified[0].clone();
        assert_eq!(app, "Matty");
        assert_eq!(summary, "town-square");
        assert_eq!(body, "Ada: ship it &lt;now&gt;");
        assert_eq!(actions, ["default", "Open"]);

        // Clicking it reports the post and its thread.
        let emitter = service
            .object_server()
            .interface::<_, Service>("/org/freedesktop/Notifications")
            .await
            .unwrap();
        // The backend starts listening for the click only after the service
        // answers Notify, so a click sent straight away can land before it
        // listens. Click again until it arrives, as a person would.
        let click = async {
            for _ in 0..40 {
                Service::action_invoked(emitter.signal_emitter(), 1, "default")
                    .await
                    .unwrap();
                async_io::Timer::after(Duration::from_millis(50)).await;
            }
            None
        };
        let opened = future::or(async { clicked_rx.recv().await.ok() }, click).await;
        assert_eq!(
            opened,
            Some(Clicked {
                channel_id: "c1".into(),
                post_id: "p1".into(),
                root_id: "r1".into(),
            })
        );

        // A second one closes when its channel is read.
        notifier.show(&Notification {
            title: "town-square".into(),
            body: "Ada: and this".into(),
            channel_id: "c1".into(),
            post_id: "p2".into(),
            root_id: String::new(),
            mention: false,
            sound: None,
        });
        eventually("the second notification", || {
            calls.lock().unwrap().notified.len() == 2
        })
        .await;
        notifier.withdraw("c1");
        eventually("closing it", || calls.lock().unwrap().closed == [2]).await;

        // The badge reaches the launcher.
        notifier.set_badge(Badge::Mentions(3));
        let message = future::or(async { badges.next().await }, async {
            async_io::Timer::after(Duration::from_secs(2)).await;
            None
        })
        .await
        .expect("a badge signal")
        .unwrap();
        let (app_uri, properties): (String, HashMap<String, OwnedValue>) =
            message.body().deserialize().unwrap();
        assert_eq!(app_uri, "application://dev.arya.matty.desktop");
        assert_eq!(i64::try_from(&properties["count"]).unwrap(), 3);
        assert!(bool::try_from(&properties["count-visible"]).unwrap());
    });
}
