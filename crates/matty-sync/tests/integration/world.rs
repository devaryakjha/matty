//! Per-test users, channel and engine.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use matty_api::models::{Channel, Post};
use matty_api::ws::ConnectionState;
use matty_api::{Client, Credentials, NewPost};
use matty_store::Store;
use matty_store::sidebar::SidebarChannel;
use matty_sync::{ChangeSet, Command, Connection, SyncHandle};
use serde_json::{Value, json};

/// How long any one wait may take.
pub const WAIT: Duration = Duration::from_secs(30);

/// The server's URL and the shared password, or `None` to skip.
pub fn server() -> Option<(String, String)> {
    let Ok(url) = std::env::var("MATTY_TEST_SERVER") else {
        eprintln!("MATTY_TEST_SERVER not set; skipping");
        return None;
    };
    Some((url, std::env::var("MATTY_TEST_PASSWORD").unwrap()))
}

/// Signs in to `url` as `login`.
pub async fn sign_in(url: &str, login: &str, password: &str) -> (Client, String) {
    let signed_in = Client::new(url)
        .unwrap()
        .login(&Credentials {
            login_id: login.into(),
            password: password.into(),
            ..Default::default()
        })
        .await
        .unwrap();
    (signed_in.client, signed_in.user.id)
}

/// A name no other run has used: lowercase letters and digits.
fn unique() -> String {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut n = (nanos % 36u128.pow(9)) * 100 + u128::from(COUNT.fetch_add(1, Ordering::Relaxed));
    let mut name = String::new();
    while n > 0 {
        name.push(char::from_digit((n % 36) as u32, 36).unwrap());
        n /= 36;
    }
    name
}

/// A signed-in test user.
#[derive(Clone)]
pub struct User {
    pub client: Client,
    pub id: String,
    pub username: String,
}

/// Two fresh users on a fresh team, both in a fresh public channel.
pub struct World {
    pub team_id: String,
    pub channel: Channel,
    /// The user whose engine the test runs.
    pub me: User,
    /// Someone else, who posts and reacts.
    pub other: User,
}

impl World {
    /// Makes the users and the channel; `None` when no server is set.
    pub async fn new() -> Option<Self> {
        let (url, password) = server()?;
        let admin_login = std::env::var("MATTY_TEST_ADMIN").unwrap_or_else(|_| "admin".into());
        let (admin, _) = sign_in(&url, &admin_login, &password).await;
        // A team of its own: teams cap their members, and joining posts in
        // a team's default channels, which other tests would see.
        let team_name = format!("it{}", unique());
        let team: Value = admin
            .post(
                "teams",
                &json!({"name": team_name, "display_name": team_name, "type": "O"}),
            )
            .await
            .unwrap();
        let team_id = team["id"].as_str().unwrap().to_owned();
        let mut users = Vec::new();
        for _ in 0..2 {
            let username = format!("it{}", unique());
            let created: Value = admin
                .post(
                    "users",
                    &json!({
                        "username": username,
                        "email": format!("{username}@example.com"),
                        "password": password,
                    }),
                )
                .await
                .unwrap();
            let id = created["id"].as_str().unwrap().to_owned();
            admin
                .post::<_, Value>(
                    &format!("teams/{team_id}/members"),
                    &json!({"team_id": team_id, "user_id": id}),
                )
                .await
                .unwrap();
            let (client, _) = sign_in(&url, &username, &password).await;
            users.push(User {
                client,
                id,
                username,
            });
        }
        let name = format!("it-{}", unique());
        let channel: Channel = admin
            .post(
                "channels",
                &json!({"team_id": team_id, "name": name, "display_name": name, "type": "O"}),
            )
            .await
            .unwrap();
        for user in &users {
            admin
                .post::<_, Value>(
                    &format!("channels/{}/members", channel.id),
                    &json!({"user_id": user.id}),
                )
                .await
                .unwrap();
        }
        // Both start with the channel read: the join messages count as
        // unread, and as a mention of whoever was added.
        for user in &users {
            user.client.view_channel(&channel.id, "").await.unwrap();
        }
        let other = users.pop().unwrap();
        let me = users.pop().unwrap();
        Some(Self {
            team_id,
            channel,
            me,
            other,
        })
    }

    /// `user` posts `message` in the channel, as a reply to `root_id` when
    /// it is not empty.
    pub async fn post(&self, user: &User, message: &str, root_id: &str) -> Post {
        user.client
            .create_post(&NewPost {
                channel_id: self.channel.id.clone(),
                message: message.into(),
                root_id: root_id.into(),
                ..NewPost::default()
            })
            .await
            .unwrap()
    }
}

/// A running engine over a database in a temporary folder.
pub struct Engine {
    pub sync: SyncHandle,
    pub changes: async_channel::Receiver<ChangeSet>,
    pub path: PathBuf,
    /// Each channel as the latest sidebar change set showed it.
    seen: Mutex<HashMap<String, SidebarChannel>>,
    _dir: Option<tempfile::TempDir>,
}

impl Engine {
    /// Starts an engine for `user` in a new folder, connects, and waits for
    /// the load and the WebSocket.
    pub async fn start(user: &User) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("account.sqlite");
        let mut engine = Self::start_at(user, path).await;
        engine._dir = Some(dir);
        engine
    }

    /// [`Engine::start`] on the database at `path`, which outlives it.
    pub async fn start_at(user: &User, path: PathBuf) -> Self {
        let sync = matty_sync::spawn(Store::open(&path).unwrap()).unwrap();
        let changes = sync.subscribe();
        sync.send(Command::Connect {
            client: user.client.clone(),
        });
        let engine = Self {
            sync,
            changes,
            path,
            seen: Mutex::default(),
            _dir: None,
        };
        // Either may come first.
        let (mut loaded, mut socket) = (false, false);
        engine
            .wait("the load and the WebSocket", |change| {
                match change {
                    ChangeSet::Connection(Connection::Loaded) => loaded = true,
                    ChangeSet::Connection(Connection::Socket(ConnectionState::Connected {
                        ..
                    })) => socket = true,
                    ChangeSet::Connection(
                        Connection::Failed { error } | Connection::TeamFailed { error, .. },
                    ) => panic!("the load failed: {error}"),
                    _ => {}
                }
                loaded && socket
            })
            .await;
        engine
    }

    pub fn send(&self, command: Command) {
        self.sync.send(command);
    }

    /// Waits for a change set `stop` accepts, and returns it.
    pub async fn wait(&self, what: &str, mut stop: impl FnMut(&ChangeSet) -> bool) -> ChangeSet {
        tokio::time::timeout(WAIT, async {
            loop {
                let change = self.changes.recv().await.unwrap();
                if let ChangeSet::Sidebar { sections, .. } = &change {
                    let mut seen = self.seen.lock().unwrap();
                    for channel in sections.iter().flat_map(|s| s.channels.iter()) {
                        seen.insert(channel.id.clone(), channel.clone());
                    }
                }
                if stop(&change) {
                    return change;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
    }

    /// Opens `channel_id` and waits for a window `stop` accepts.
    pub async fn open(&self, channel_id: &str, mut stop: impl FnMut(&[Post]) -> bool) -> Vec<Post> {
        self.send(Command::OpenChannel {
            channel_id: channel_id.into(),
        });
        let ChangeSet::Posts { posts, .. } = self
            .wait("the channel's posts", |change| {
                matches!(change, ChangeSet::Posts { channel_id: id, posts } if id == channel_id && stop(posts))
            })
            .await
        else {
            unreachable!()
        };
        posts.to_vec()
    }

    /// Waits for the sidebar to show `channel_id` as `stop` accepts it,
    /// which it may already have.
    pub async fn sidebar(
        &self,
        channel_id: &str,
        mut stop: impl FnMut(&SidebarChannel) -> bool,
    ) -> SidebarChannel {
        let shown = self.seen.lock().unwrap().get(channel_id).cloned();
        if let Some(channel) = shown.filter(|c| stop(c)) {
            return channel;
        }
        let mut found = None;
        self.wait("the channel in the sidebar", |change| {
            let ChangeSet::Sidebar { sections, .. } = change else {
                return false;
            };
            found = sections
                .iter()
                .flat_map(|s| s.channels.iter())
                .find(|c| c.id == channel_id && stop(c))
                .cloned();
            found.is_some()
        })
        .await;
        found.unwrap()
    }

    /// Stops the engine and waits for it to let go of the database.
    pub async fn stop(self) -> Option<tempfile::TempDir> {
        drop(self.sync);
        tokio::time::sleep(Duration::from_millis(300)).await;
        self._dir
    }
}

/// Polls `check` until it holds.
pub async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + WAIT;
    while tokio::time::Instant::now() < deadline {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}
