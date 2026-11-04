//! A saved server's sync while another is shown (#206, PLAN §3 M2). Every
//! saved server with a session keeps its own engine running, so its
//! unreads, mentions and notifications stay current. A [`ServerWatch`]
//! follows that engine for the rail's badge and the dock's total, and keeps
//! what the engine said once per connection (the user, the server's
//! configuration, preferences, presence) so the window can show the server
//! at once on a switch, over the same engine, without connecting again.

use std::collections::HashMap;
use std::sync::Arc;

use matty_api::ServerCapabilities;
use matty_api::models::{Emoji, Preference, Status, User};
use matty_api::ws::ConnectionState as Socket;
use matty_sync::{ChangeSet, Connection, SidebarSection, SyncHandle, TeamWithUnreads};

use crate::live::Arrival;
use crate::settings::SavedAccount;

/// One saved server's engine and what the rail needs from it.
pub struct ServerWatch {
    /// Names the server's files, and so the server among the saved ones.
    data_key: String,
    sync: SyncHandle,
    /// The signed-in user's id.
    me: String,
    /// The user's notification preferences, from their `notify_props`.
    prefs: matty_notify::UserPrefs,
    /// Every team's sidebar, for the server's unread and mention totals.
    sections: HashMap<String, Arc<[SidebarSection]>>,
    /// The teams, in rail order.
    teams: Option<Arc<[TeamWithUnreads]>>,
    /// The engine has started a session or shown its cache.
    started: bool,
    /// What a window showing the server needs from before it subscribed.
    me_user: Option<Arc<User>>,
    config: Option<Arc<ServerCapabilities>>,
    collapsed_threads: Option<bool>,
    link_previews: Option<bool>,
    preferences: bool,
    account_preferences: Option<Arc<[Preference]>>,
    custom_emoji: Option<Arc<[Emoji]>>,
    users: HashMap<String, User>,
    presence: HashMap<String, Status>,
    socket: Option<Socket>,
}

/// What a change set meant for the rail and notifications.
#[derive(Debug, PartialEq)]
pub enum Heard {
    Nothing,
    /// The server's unread or mention totals may have changed.
    Unreads,
    /// Someone else's post arrived; the window decides whether it notifies.
    Arrived(Box<Arrival>),
    /// The server refused the session, or the user signed out of it: the
    /// engine should stop.
    Ended,
}

impl ServerWatch {
    /// Follows `sync`, the engine for `account`.
    pub fn new(
        sync: SyncHandle,
        account: &SavedAccount,
    ) -> (Self, async_channel::Receiver<ChangeSet>) {
        let changes = sync.subscribe();
        (
            Self {
                data_key: account.data_key(),
                sync,
                me: account.user_id.clone(),
                prefs: matty_notify::UserPrefs::from_props(&account.username, "", &HashMap::new()),
                sections: HashMap::new(),
                teams: None,
                started: false,
                me_user: None,
                config: None,
                collapsed_threads: None,
                link_previews: None,
                preferences: false,
                account_preferences: None,
                custom_emoji: None,
                users: HashMap::new(),
                presence: HashMap::new(),
                socket: None,
            },
            changes,
        )
    }

    pub fn data_key(&self) -> &str {
        &self.data_key
    }

    /// The engine, for a window to show the server over.
    pub fn sync(&self) -> SyncHandle {
        self.sync.clone()
    }

    pub fn send(&self, command: matty_sync::Command) {
        self.sync.send(command);
    }

    /// Whether the engine already connected or showed its cache, so a
    /// window showing the server needs only [`ServerWatch::replay`].
    pub fn started(&self) -> bool {
        self.started
    }

    /// The server's mentions, and whether anything unmuted is unread, across
    /// its teams and direct messages, as the sidebar counts them.
    pub fn unreads(&self) -> (usize, bool) {
        let mut seen = std::collections::HashSet::new();
        let mut mentions = 0;
        let mut unread = false;
        // Direct messages show in every team's sidebar; each counts once.
        let channels = self
            .sections
            .values()
            .flat_map(|sections| sections.iter())
            .flat_map(|section| section.channels.iter());
        // A channel in a muted category is muted itself.
        for channel in channels {
            if !seen.insert(channel.id.as_str()) {
                continue;
            }
            mentions += usize::try_from(channel.unreads.mentions).unwrap_or(0);
            unread |= channel.unreads.messages > 0 && !channel.muted;
        }
        (mentions, unread || mentions > 0)
    }

    /// The user's own status on this server: whether Do not disturb holds
    /// now, for its notifications.
    pub fn dnd(&self, now: i64) -> bool {
        self.presence.get(&self.me).is_some_and(|status| {
            status.status == "dnd"
                && matty_notify::dnd_active(
                    (status.dnd_end_time > 0).then_some(status.dnd_end_time),
                    now,
                )
        })
    }

    pub fn apply(&mut self, change: &ChangeSet) -> Heard {
        match change {
            ChangeSet::Sidebar { team_id, sections } => {
                self.sections.insert(team_id.clone(), sections.clone());
                return Heard::Unreads;
            }
            ChangeSet::Unreads { teams } => {
                // A team the user left takes its channels with it.
                self.sections
                    .retain(|id, _| id.is_empty() || teams.iter().any(|team| &team.id == id));
                self.teams = Some(teams.clone());
                return Heard::Unreads;
            }
            ChangeSet::Me(user) => {
                self.take_me(user);
                self.me_user = Some(user.clone());
            }
            ChangeSet::Users(users) => {
                for user in users.iter() {
                    self.take_me(user);
                    self.users.insert(user.id.clone(), user.clone());
                }
            }
            ChangeSet::Presence(statuses) => {
                for status in statuses.iter() {
                    self.presence.insert(status.user_id.clone(), status.clone());
                }
            }
            ChangeSet::Config(config) => self.config = Some(config.clone()),
            ChangeSet::CollapsedThreads(on) => self.collapsed_threads = Some(*on),
            ChangeSet::LinkPreviews(on) => self.link_previews = Some(*on),
            ChangeSet::Preferences => self.preferences = true,
            ChangeSet::AccountPreferences(preferences) => {
                self.account_preferences = Some(preferences.clone());
            }
            ChangeSet::CustomEmoji(emoji) => self.custom_emoji = Some(emoji.clone()),
            ChangeSet::Arrived(arrived) => {
                return Heard::Arrived(Box::new(Arrival::new(arrived, &self.me, &self.prefs)));
            }
            ChangeSet::Connection(connection) => match connection {
                Connection::Socket(Socket::Unauthorized)
                | Connection::Expired
                | Connection::SignedOut => return Heard::Ended,
                Connection::Socket(state) => {
                    self.started = true;
                    self.socket = Some(state.clone());
                }
                Connection::Loading | Connection::Loaded | Connection::Resynced => {
                    self.started = true;
                }
                _ => {}
            },
            _ => {}
        }
        Heard::Nothing
    }

    fn take_me(&mut self, user: &User) {
        if user.id == self.me && !user.notify_props.is_empty() {
            self.prefs = matty_notify::UserPrefs::from_props(
                &user.username,
                &user.first_name,
                &user.notify_props,
            );
        }
    }

    /// What the engine said that a window showing the server from now on
    /// would have missed, in the order the engine first says it, so the
    /// server shows at once: its sidebars and teams among them, so a
    /// notification's channel can open straight after a switch.
    /// [`matty_sync::Command::ShowCache`] then brings the saved posts and
    /// drafts, and the sidebars again.
    pub fn replay(&self) -> Vec<ChangeSet> {
        let mut changes = Vec::new();
        if let Some(on) = self.collapsed_threads {
            changes.push(ChangeSet::CollapsedThreads(on));
        }
        if let Some(on) = self.link_previews {
            changes.push(ChangeSet::LinkPreviews(on));
        }
        if let Some(user) = &self.me_user {
            changes.push(ChangeSet::Me(user.clone()));
        }
        if let Some(config) = &self.config {
            changes.push(ChangeSet::Config(config.clone()));
        }
        if self.preferences {
            changes.push(ChangeSet::Preferences);
        }
        if let Some(preferences) = &self.account_preferences {
            changes.push(ChangeSet::AccountPreferences(preferences.clone()));
        }
        if let Some(emoji) = &self.custom_emoji {
            changes.push(ChangeSet::CustomEmoji(emoji.clone()));
        }
        if !self.users.is_empty() {
            changes.push(ChangeSet::Users(self.users.values().cloned().collect()));
        }
        if !self.presence.is_empty() {
            changes.push(ChangeSet::Presence(
                self.presence.values().cloned().collect(),
            ));
        }
        if let Some(state) = &self.socket {
            changes.push(ChangeSet::Connection(Connection::Socket(state.clone())));
        }
        // Sidebars ahead of the teams, whose change picks the team to show.
        for (team_id, sections) in &self.sections {
            changes.push(ChangeSet::Sidebar {
                team_id: team_id.clone(),
                sections: sections.clone(),
            });
        }
        if let Some(teams) = &self.teams {
            changes.push(ChangeSet::Unreads {
                teams: teams.clone(),
            });
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use matty_sync::{SidebarChannel, Unreads};

    fn account() -> SavedAccount {
        SavedAccount {
            server: "https://chat.example.com".into(),
            user_id: "me".into(),
            username: "ada".into(),
            ..SavedAccount::default()
        }
    }

    fn watch() -> ServerWatch {
        let dir = tempfile::tempdir().unwrap();
        let sync = matty_sync::spawn_at(dir.path().join("account.sqlite"), None).unwrap();
        // The engine may still be opening the file; let the folder outlive it.
        std::mem::forget(dir);
        ServerWatch::new(sync, &account()).0
    }

    fn channel(id: &str, messages: i64, mentions: i64, muted: bool) -> SidebarChannel {
        SidebarChannel {
            id: id.into(),
            kind: "O".into(),
            name: id.into(),
            display_name: id.into(),
            dm_user_id: None,
            dm_user: None,
            unreads: Unreads {
                messages,
                mentions,
                urgent_mentions: 0,
            },
            muted,
            last_post_at: 0,
        }
    }

    fn sidebar(team: &str, channels: Vec<SidebarChannel>) -> ChangeSet {
        ChangeSet::Sidebar {
            team_id: team.into(),
            sections: Arc::from(vec![SidebarSection {
                id: format!("{team}-channels"),
                kind: "channels".into(),
                display_name: "Channels".into(),
                sorting: String::new(),
                collapsed: false,
                muted: false,
                channels,
            }]),
        }
    }

    #[test]
    fn totals_count_each_channel_once_across_teams() {
        let mut watch = watch();
        assert_eq!(watch.unreads(), (0, false));
        let dm = channel("dm", 1, 1, false);
        assert_eq!(
            watch.apply(&sidebar("t1", vec![channel("a", 3, 2, false), dm.clone()])),
            Heard::Unreads
        );
        watch.apply(&sidebar("t2", vec![channel("b", 0, 0, false), dm]));
        assert_eq!(watch.unreads(), (3, true));
    }

    #[test]
    fn a_muted_channel_counts_its_mentions_but_not_its_unreads() {
        let mut watch = watch();
        watch.apply(&sidebar("t1", vec![channel("a", 5, 0, true)]));
        assert_eq!(watch.unreads(), (0, false));
        watch.apply(&sidebar("t1", vec![channel("a", 5, 1, true)]));
        assert_eq!(watch.unreads(), (1, true));
    }

    #[test]
    fn a_left_team_stops_counting() {
        let mut watch = watch();
        watch.apply(&sidebar("t1", vec![channel("a", 1, 0, false)]));
        watch.apply(&ChangeSet::Unreads {
            teams: Arc::from(Vec::new()),
        });
        assert_eq!(watch.unreads(), (0, false));
    }

    #[test]
    fn a_refused_session_ends_the_watch() {
        let mut watch = watch();
        assert_eq!(
            watch.apply(&ChangeSet::Connection(Connection::Expired)),
            Heard::Ended
        );
        assert_eq!(
            watch.apply(&ChangeSet::Connection(Connection::Socket(
                Socket::Unauthorized
            ))),
            Heard::Ended
        );
    }

    #[test]
    fn what_the_engine_said_once_replays_for_the_window() {
        let mut watch = watch();
        assert!(!watch.started() && watch.replay().is_empty());
        let me = Arc::new(User {
            id: "me".into(),
            username: "ada".into(),
            ..User::default()
        });
        watch.apply(&ChangeSet::Connection(Connection::Loading));
        watch.apply(&ChangeSet::CollapsedThreads(true));
        watch.apply(&ChangeSet::Me(me.clone()));
        watch.apply(&ChangeSet::Presence(Arc::from(vec![Status {
            user_id: "me".into(),
            status: "dnd".into(),
            ..Status::default()
        }])));
        assert!(watch.started());
        assert!(watch.dnd(0));
        let teams = ChangeSet::Unreads {
            teams: Arc::from(Vec::new()),
        };
        watch.apply(&teams);
        assert_eq!(
            watch.replay(),
            vec![
                ChangeSet::CollapsedThreads(true),
                ChangeSet::Me(me),
                ChangeSet::Presence(Arc::from(vec![Status {
                    user_id: "me".into(),
                    status: "dnd".into(),
                    ..Status::default()
                }])),
                teams,
            ]
        );
    }

    #[test]
    fn sidebars_replay_ahead_of_the_teams() {
        let mut watch = watch();
        let t1 = sidebar("t1", vec![channel("a", 1, 0, false)]);
        let teams = ChangeSet::Unreads {
            teams: Arc::from(vec![TeamWithUnreads {
                id: "t1".into(),
                name: "one".into(),
                display_name: "One".into(),
                has_unreads: true,
                mentions: 0,
                icon: 0,
            }]),
        };
        watch.apply(&t1);
        watch.apply(&teams);
        assert_eq!(watch.replay(), vec![t1, teams]);
    }

    #[test]
    fn a_direct_message_notifies_unless_do_not_disturb() {
        let mut watch = watch();
        let dm = ChangeSet::Arrived(Arc::new(matty_sync::Arrived {
            post: matty_api::models::Post {
                id: "p1".into(),
                channel_id: "dm".into(),
                user_id: "sam".into(),
                message: "lunch?".into(),
                ..Default::default()
            },
            channel_type: "D".into(),
            channel_display_name: "sam".into(),
            sender_name: "@sam".into(),
            mentioned: false,
            followed: false,
            channel_props: HashMap::new(),
            collapsed_threads: false,
        }));
        let Heard::Arrived(arrival) = watch.apply(&dm) else {
            panic!("a post arrived");
        };
        let shown = arrival
            .decide(matty_notify::AppState::default())
            .expect("a direct message notifies");
        assert_eq!(
            (shown.channel_id.as_str(), shown.post_id.as_str()),
            ("dm", "p1")
        );
        let dnd = matty_notify::AppState {
            dnd: true,
            ..matty_notify::AppState::default()
        };
        assert_eq!(arrival.decide(dnd), None);
    }

    #[test]
    fn do_not_disturb_ends_on_time() {
        let mut watch = watch();
        watch.apply(&ChangeSet::Presence(Arc::from(vec![Status {
            user_id: "me".into(),
            status: "dnd".into(),
            dnd_end_time: 100,
            ..Status::default()
        }])));
        assert!(watch.dnd(99));
        assert!(!watch.dnd(100));
    }
}
