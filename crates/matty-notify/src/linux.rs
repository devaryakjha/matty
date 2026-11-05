//! Linux notifications through the freedesktop notification service, and
//! the launcher badge through the Unity LauncherEntry signal that GNOME
//! docks, KDE's task manager and others read (#216, PLAN §6.7).
//!
//! The service is on the session bus, which Flatpak's sandbox exposes for
//! `org.freedesktop.Notifications`. Everything runs on one thread with its
//! own executor, so a slow or missing service never holds up the app: the
//! app's calls only queue a command.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use async_channel::{Receiver, Sender};
use notify_rust::{Hint, NotificationHandle, NotificationResponse};

use crate::{Badge, Clicked, Notification, Notifier};

/// The desktop entry Matty installs, `packaging/linux/dev.arya.matty.desktop`,
/// named for the app id the windows carry.
pub const DESKTOP_ENTRY: &str = "dev.arya.matty";

/// Where the badge signal comes from. Docks match it to the app by the
/// desktop entry it names, not by this path.
const LAUNCHER_PATH: &str = "/com/canonical/unity/launcherentry/matty";

enum Command {
    Show(Notification),
    Withdraw(String),
    Badge(Badge),
}

/// Posts to the freedesktop notification service.
pub struct Freedesktop {
    commands: Sender<Command>,
}

impl Freedesktop {
    /// Starts the notification thread. Clicks arrive on `clicked`.
    pub fn new(clicked: Sender<Clicked>) -> Option<Self> {
        let (commands, receiver) = async_channel::unbounded();
        std::thread::Builder::new()
            .name("notifications".into())
            .spawn(move || run(receiver, clicked))
            .map_err(|error| tracing::warn!(%error, "no notification thread"))
            .ok()?;
        Some(Self { commands })
    }

    fn send(&self, command: Command) {
        if self.commands.try_send(command).is_err() {
            tracing::debug!("the notification thread has stopped");
        }
    }
}

impl Notifier for Freedesktop {
    fn show(&self, notification: &Notification) {
        self.send(Command::Show(notification.clone()));
    }

    fn withdraw(&self, channel_id: &str) {
        self.send(Command::Withdraw(channel_id.to_owned()));
    }

    fn set_badge(&self, badge: Badge) {
        self.send(Command::Badge(badge));
    }
}

/// The notifications on screen, by channel, so reading a channel can
/// close its own.
#[derive(Default)]
struct Shown {
    next: u64,
    by_channel: HashMap<String, Vec<(u64, Rc<NotificationHandle>)>>,
}

impl Shown {
    fn add(&mut self, channel_id: &str, handle: Rc<NotificationHandle>) -> u64 {
        self.next += 1;
        self.by_channel
            .entry(channel_id.to_owned())
            .or_default()
            .push((self.next, handle));
        self.next
    }

    fn remove(&mut self, channel_id: &str, key: u64) {
        if let Some(handles) = self.by_channel.get_mut(channel_id) {
            handles.retain(|(shown, _)| *shown != key);
            if handles.is_empty() {
                self.by_channel.remove(channel_id);
            }
        }
    }

    fn take(&mut self, channel_id: &str) -> Vec<Rc<NotificationHandle>> {
        self.by_channel
            .remove(channel_id)
            .unwrap_or_default()
            .into_iter()
            .map(|(_, handle)| handle)
            .collect()
    }
}

fn run(commands: Receiver<Command>, clicked: Sender<Clicked>) {
    let executor = Rc::new(async_executor::LocalExecutor::new());
    let shown = Rc::new(RefCell::new(Shown::default()));
    let main = {
        let executor = executor.clone();
        async move {
            let mut session: Option<zbus::Connection> = None;
            while let Ok(command) = commands.recv().await {
                match command {
                    Command::Show(notification) => {
                        let Some(handle) = show(&notification).await else {
                            continue;
                        };
                        let handle = Rc::new(handle);
                        let channel_id = notification.channel_id.clone();
                        let key = shown.borrow_mut().add(&channel_id, handle.clone());
                        let shown = shown.clone();
                        let clicked = clicked.clone();
                        let target = Clicked {
                            channel_id: notification.channel_id,
                            post_id: notification.post_id,
                            root_id: notification.root_id,
                        };
                        executor
                            .spawn(async move {
                                let mut opened = false;
                                handle
                                    .wait_for_action_async(|response| {
                                        opened = is_open(response);
                                    })
                                    .await;
                                shown.borrow_mut().remove(&channel_id, key);
                                if opened {
                                    let _ = clicked.send(target).await;
                                }
                            })
                            .detach();
                    }
                    Command::Withdraw(channel_id) => {
                        let handles = shown.borrow_mut().take(&channel_id);
                        for handle in handles {
                            handle.close_async().await;
                        }
                    }
                    Command::Badge(badge) => {
                        if session.is_none() {
                            session = zbus::Connection::session()
                                .await
                                .map_err(|error| tracing::debug!(%error, "no session bus"))
                                .ok();
                        }
                        if let Some(session) = &session {
                            set_badge(session, badge).await;
                        }
                    }
                }
            }
        }
    };
    futures_lite::future::block_on(executor.run(main));
}

/// Whether `response` means the person opened the notification: its body,
/// or the Open button on desktops that show actions as buttons.
fn is_open(response: &NotificationResponse) -> bool {
    match response {
        NotificationResponse::Default => true,
        NotificationResponse::Action(action) => action == "default",
        _ => false,
    }
}

async fn show(notification: &Notification) -> Option<NotificationHandle> {
    build(notification)
        .show_async()
        .await
        .map_err(|error| tracing::debug!(%error, "no notification service"))
        .ok()
}

/// The freedesktop notification for `notification`. Matty plays its own
/// sounds, so the service is asked to stay quiet.
fn build(notification: &Notification) -> notify_rust::Notification {
    let mut built = notify_rust::Notification::new();
    built
        .appname("Matty")
        .summary(&notification.title)
        .body(&escape(&notification.body))
        .icon(DESKTOP_ENTRY)
        .action("default", "Open")
        .hint(Hint::DesktopEntry(DESKTOP_ENTRY.into()))
        .hint(Hint::Category("im.received".into()))
        .hint(Hint::SuppressSound(true));
    built
}

/// The body as plain text: services that support markup read `<`, `>` and
/// `&` in a message as tags.
fn escape(body: &str) -> String {
    let mut escaped = String::with_capacity(body.len());
    for character in body.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// The Unity LauncherEntry properties for `badge`. Docks show a number, so
/// an unread channel without mentions shows nothing.
fn launcher_properties(badge: Badge) -> (i64, bool) {
    match badge {
        Badge::Mentions(count) => (i64::try_from(count).unwrap_or(i64::MAX), true),
        Badge::Unread | Badge::Clear => (0, false),
    }
}

async fn set_badge(session: &zbus::Connection, badge: Badge) {
    use zbus::zvariant::Value;
    let (count, visible) = launcher_properties(badge);
    let properties: HashMap<&str, Value<'_>> = HashMap::from([
        ("count", Value::from(count)),
        ("count-visible", Value::from(visible)),
    ]);
    let app_uri = format!("application://{DESKTOP_ENTRY}.desktop");
    if let Err(error) = session
        .emit_signal(
            None::<()>,
            LAUNCHER_PATH,
            "com.canonical.Unity.LauncherEntry",
            "Update",
            &(app_uri, properties),
        )
        .await
    {
        tracing::debug!(%error, "badge");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_body_is_escaped_for_services_that_read_markup() {
        assert_eq!(escape("a <b> & c"), "a &lt;b&gt; &amp; c");
        assert_eq!(escape("plain"), "plain");
    }

    #[test]
    fn the_launcher_shows_mentions_only() {
        assert_eq!(launcher_properties(Badge::Mentions(4)), (4, true));
        assert_eq!(launcher_properties(Badge::Unread), (0, false));
        assert_eq!(launcher_properties(Badge::Clear), (0, false));
    }

    #[test]
    fn opening_is_the_default_action_and_nothing_else() {
        assert!(is_open(&NotificationResponse::Default));
        assert!(is_open(&NotificationResponse::Action("default".into())));
        assert!(!is_open(&NotificationResponse::Action("other".into())));
    }

    #[test]
    fn a_notification_names_matty_and_its_desktop_entry() {
        let built = build(&Notification {
            title: "town-square".into(),
            body: "Ada: <hi>".into(),
            channel_id: "c1".into(),
            post_id: "p1".into(),
            root_id: String::new(),
            mention: true,
            sound: None,
        });
        assert_eq!(built.appname, "Matty");
        assert_eq!(built.summary, "town-square");
        assert_eq!(built.body, "Ada: &lt;hi&gt;");
        assert_eq!(built.actions, ["default", "Open"]);
    }
}
