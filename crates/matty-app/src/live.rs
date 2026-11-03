//! The app's side of sync: owns the [`SyncHandle`] and turns change sets
//! into the view models the window hands its views. Nothing here draws,
//! blocks, or touches the disk; the database lives on the sync thread.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::SharedString;
use matty_api::models::User;
use matty_api::{ApiError, Client, ServerCapabilities};
use matty_sync::{ChangeSet, Command, Connection, SidebarSection, SyncHandle, TeamWithUnreads};

use crate::channel::{ChannelModel, Post};
use crate::login::{SignInMethods, SsoProvider};
use crate::main_window::ConnectionState;
use crate::settings::SavedAccount;
use crate::sidebar::{Category, ChannelKind, ChannelRow, Presence};

/// A running sync engine and what the window last heard from it.
pub struct Live {
    sync: SyncHandle,
    /// The team the sidebar shows: the first in the user's order until
    /// they pick another.
    team: Option<String>,
    teams: Vec<TeamWithUnreads>,
    sections: HashMap<String, Arc<[SidebarSection]>>,
    presence: HashMap<String, Presence>,
    /// The signed-in user's id and username.
    me: (String, String),
    /// Everyone whose posts have been on screen.
    users: HashMap<String, User>,
    /// The channel on screen and its latest posts snapshot.
    open: Option<ChannelRow>,
    posts: Option<Arc<[matty_api::models::Post]>>,
    max_post_size: Option<usize>,
    zone: jiff::tz::TimeZone,
    /// The posts the channel view shows now, to send it only what changed.
    shown: Option<Vec<Post>>,
}

/// What a change set means for the window.
#[derive(Debug, Default, PartialEq)]
pub struct Update {
    /// The shown team's categories, to replace the sidebar's.
    pub categories: Option<Vec<Category>>,
    /// The shown team's display name, for the sidebar header.
    pub team_name: Option<SharedString>,
    pub connection: Option<ConnectionState>,
    /// What changed in the open channel's view.
    pub channel: Option<ChannelUpdate>,
    /// The session ended; the window returns to sign-in.
    pub signed_out: bool,
}

/// A change to the open channel's view.
#[derive(Debug, PartialEq)]
pub enum ChannelUpdate {
    /// Show this model, as on opening a channel; resets the scroll.
    Replace(ChannelModel),
    /// Patch the posts on screen and keep the scroll.
    Patch {
        removed: Vec<SharedString>,
        updated: Vec<Post>,
        appended: Vec<Post>,
    },
}

/// How to go from the posts on screen to `new` without a reset, or `None`
/// when only a reset will do: a post appeared between two shown ones, or
/// the order changed.
fn patch(old: &[Post], new: &[Post]) -> Option<ChannelUpdate> {
    let kept: HashMap<&str, &Post> = old.iter().map(|post| (post.id.as_ref(), post)).collect();
    let present: std::collections::HashSet<&str> =
        new.iter().map(|post| post.id.as_ref()).collect();
    let removed: Vec<SharedString> = old
        .iter()
        .filter(|post| !present.contains(post.id.as_ref()))
        .map(|post| post.id.clone())
        .collect();
    let remaining: Vec<&str> = old
        .iter()
        .map(|post| post.id.as_ref())
        .filter(|id| present.contains(id))
        .collect();
    let (head, tail) = new.split_at(remaining.len().min(new.len()));
    if head
        .iter()
        .map(|post| post.id.as_ref())
        .ne(remaining.iter().copied())
    {
        return None;
    }
    let updated = head
        .iter()
        .filter(|post| kept.get(post.id.as_ref()) != Some(post))
        .cloned()
        .collect();
    Some(ChannelUpdate::Patch {
        removed,
        updated,
        appended: tail.to_vec(),
    })
}

impl Live {
    /// Starts sync over the database at `path` and returns it with the
    /// change sets to apply.
    pub fn start(
        path: PathBuf,
        account: &SavedAccount,
    ) -> std::io::Result<(Self, async_channel::Receiver<ChangeSet>)> {
        let sync = matty_sync::spawn_at(path)?;
        let changes = sync.subscribe();
        Ok((
            Self {
                sync,
                team: None,
                teams: Vec::new(),
                sections: HashMap::new(),
                presence: HashMap::new(),
                me: (account.user_id.clone(), account.username.clone()),
                users: HashMap::new(),
                open: None,
                posts: None,
                max_post_size: None,
                zone: jiff::tz::TimeZone::system(),
                shown: None,
            },
            changes,
        ))
    }

    pub fn send(&self, command: Command) {
        self.sync.send(command);
    }

    /// Shows `row`'s channel: sync publishes its posts now and as they
    /// change. Returns the model to show until they come.
    pub fn open(&mut self, row: ChannelRow) -> ChannelModel {
        if let Some(old) = self.open.take()
            && old.id != row.id
        {
            self.send(Command::CloseChannel {
                channel_id: old.id.to_string(),
            });
        }
        self.send(Command::OpenChannel {
            channel_id: row.id.to_string(),
        });
        self.posts = None;
        self.shown = None;
        self.open = Some(row);
        self.channel_model().expect("a channel is open")
    }

    /// The open channel's view change for the latest posts: the first
    /// snapshot replaces the model, later ones patch it.
    fn channel_update(&mut self) -> Option<ChannelUpdate> {
        let model = self.channel_model()?;
        if model.loading {
            return None;
        }
        let update = self
            .shown
            .as_deref()
            .and_then(|old| patch(old, &model.posts));
        self.shown = Some(model.posts.clone());
        Some(update.unwrap_or(ChannelUpdate::Replace(model)))
    }

    fn channel_model(&self) -> Option<ChannelModel> {
        let row = self.open.as_ref()?;
        let viewer = crate::transcript::Viewer {
            user_id: &self.me.0,
            username: &self.me.1,
            zone: &self.zone,
            now: jiff::Timestamp::now(),
        };
        Some(crate::transcript::channel_model(
            row,
            self.posts.as_deref(),
            &self.users,
            &viewer,
            self.max_post_size,
        ))
    }

    /// The sidebar row for `channel_id` in the shown team.
    pub fn row(&self, channel_id: &str) -> Option<ChannelRow> {
        self.shown_categories()?
            .into_iter()
            .flat_map(|category| category.rows)
            .find(|row| row.id.as_ref() == channel_id)
    }

    /// Folds in one change set.
    pub fn apply(&mut self, change: &ChangeSet) -> Update {
        let mut update = Update::default();
        match change {
            ChangeSet::Sidebar { team_id, sections } => {
                // Unreads, which comes in the same batch, picks the team.
                self.sections.insert(team_id.clone(), sections.clone());
                if self.team.as_ref() == Some(team_id) {
                    update.categories = Some(self.categories(sections));
                }
            }
            ChangeSet::Unreads { teams } => {
                self.teams = teams.to_vec();
                let shown = self
                    .team
                    .as_ref()
                    .filter(|id| self.teams.iter().any(|team| &team.id == *id))
                    .cloned();
                let team = shown.or_else(|| self.teams.first().map(|team| team.id.clone()));
                if team != self.team {
                    self.team = team;
                    update.categories = self.shown_categories();
                }
                update.team_name = self.team_name();
            }
            ChangeSet::Presence(statuses) => {
                for status in statuses.iter() {
                    self.presence
                        .insert(status.user_id.clone(), presence(&status.status));
                }
                update.categories = self.shown_categories();
            }
            ChangeSet::Posts { channel_id, posts } => {
                if self
                    .open
                    .as_ref()
                    .is_some_and(|row| row.id.as_ref() == channel_id)
                {
                    self.posts = Some(posts.clone());
                    update.channel = self.channel_update();
                }
            }
            ChangeSet::Users(users) => {
                for user in users.iter() {
                    self.users.insert(user.id.clone(), user.clone());
                }
                if self.posts.is_some() {
                    update.channel = self.channel_update();
                }
            }
            ChangeSet::Config(capabilities) => {
                self.max_post_size = Some(capabilities.max_post_size);
            }
            ChangeSet::Connection(connection) => match connection {
                Connection::Socket(state) => update.connection = Some(connection_state(state)),
                Connection::Failed { .. } => update.connection = Some(ConnectionState::Offline),
                Connection::SignedOut => update.signed_out = true,
                _ => {}
            },
            _ => {}
        }
        update
    }

    fn shown_categories(&self) -> Option<Vec<Category>> {
        let sections = self.sections.get(self.team.as_ref()?)?;
        Some(self.categories(sections))
    }

    fn team_name(&self) -> Option<SharedString> {
        let team = self.team.as_ref()?;
        let team = self.teams.iter().find(|candidate| &candidate.id == team)?;
        Some(team.display_name.clone().into())
    }

    fn categories(&self, sections: &[SidebarSection]) -> Vec<Category> {
        sections
            .iter()
            .map(|section| {
                let rows = section
                    .channels
                    .iter()
                    .map(|channel| {
                        let kind = match channel.kind.as_str() {
                            "P" => ChannelKind::Private,
                            "D" => ChannelKind::Direct {
                                presence: channel
                                    .dm_user_id
                                    .as_ref()
                                    .and_then(|id| self.presence.get(id).copied())
                                    .unwrap_or(Presence::Offline),
                            },
                            // Mattermost names a group by its people, comma-separated.
                            "G" => ChannelKind::Group {
                                members: channel.display_name.split(", ").count(),
                            },
                            _ => ChannelKind::Public,
                        };
                        let mut row =
                            ChannelRow::new(channel.id.clone(), channel.display_name.clone(), kind);
                        if channel.unreads.messages > 0 {
                            row = row.unread();
                        }
                        let mentions = usize::try_from(channel.unreads.mentions).unwrap_or(0);
                        row = row.mentions(mentions);
                        if channel.muted {
                            row = row.muted();
                        }
                        row
                    })
                    .collect();
                let mut category =
                    Category::new(section.id.clone(), section.display_name.clone(), rows);
                category.collapsed = section.collapsed;
                category
            })
            .collect()
    }
}

pub fn presence(status: &str) -> Presence {
    match status {
        "online" => Presence::Online,
        "away" => Presence::Away,
        "dnd" => Presence::Dnd,
        _ => Presence::Offline,
    }
}

/// The status string sync sends for `presence`.
pub fn status(presence: Presence) -> &'static str {
    match presence {
        Presence::Online => "online",
        Presence::Away => "away",
        Presence::Dnd => "dnd",
        Presence::Offline => "offline",
    }
}

fn connection_state(state: &matty_api::ws::ConnectionState) -> ConnectionState {
    use matty_api::ws::ConnectionState as Ws;
    match state {
        Ws::Connecting | Ws::Connected { .. } => ConnectionState::Online,
        Ws::Reconnecting { attempt, .. } => ConnectionState::Reconnecting { attempt: *attempt },
        Ws::Unauthorized | Ws::Stopped => ConnectionState::Offline,
    }
}

/// Checks `url` and reads how its server lets people sign in.
pub async fn check_server(url: String) -> Result<ServerCapabilities, String> {
    let answer = matty_sync::io::run(async move {
        let client = Client::new(&url)?;
        client.capabilities().await
    });
    match answer.recv().await {
        Ok(Ok(capabilities)) => Ok(capabilities),
        Ok(Err(error)) => Err(server_error(&error)),
        Err(_) => Err(STOPPED.into()),
    }
}

/// A signed-in session, ready to hand to sync.
pub struct Session {
    pub client: Client,
    pub user: User,
}

/// Signs in to `url` with a login id and password.
pub async fn sign_in(url: String, login_id: String, password: String) -> Result<Session, String> {
    let answer = matty_sync::io::run(async move {
        let credentials = matty_api::Credentials {
            login_id,
            password,
            ..Default::default()
        };
        Client::new(&url)?.login(&credentials).await
    });
    match answer.recv().await {
        Ok(Ok(signed_in)) => Ok(Session {
            client: signed_in.client,
            user: signed_in.user,
        }),
        Ok(Err(error)) => Err(sign_in_error(&error)),
        Err(_) => Err(STOPPED.into()),
    }
}

/// Signs in with a personal access token.
pub async fn sign_in_with_token(url: String, token: String) -> Result<Session, String> {
    let answer =
        matty_sync::io::run(
            async move { Client::new(&url)?.login_personal_access_token(&token).await },
        );
    match answer.recv().await {
        Ok(Ok(signed_in)) => Ok(Session {
            client: signed_in.client,
            user: signed_in.user,
        }),
        Ok(Err(error)) => Err(sign_in_error(&error)),
        Err(_) => Err(STOPPED.into()),
    }
}

const STOPPED: &str = "Matty could not start its network thread. Quit and open it again.";

/// What the sign-in card offers for a server.
pub fn methods(capabilities: &ServerCapabilities) -> SignInMethods {
    let sso = &capabilities.sso;
    let providers = [
        (sso.gitlab, SsoProvider::GitLab),
        (sso.google, SsoProvider::Google),
        (sso.office365, SsoProvider::Office365),
        (sso.openid, SsoProvider::OpenId),
        (sso.saml, SsoProvider::Saml),
    ];
    SignInMethods {
        password: capabilities.email_login || capabilities.username_login,
        ldap: capabilities.ldap,
        sso: providers
            .into_iter()
            .filter_map(|(on, provider)| on.then_some(provider))
            .collect(),
    }
}

/// The account to remember after `user` signs in to `server`.
pub fn saved_account(server: &str, user: &User) -> SavedAccount {
    let display_name = crate::transcript::display_name(user);
    SavedAccount {
        server: server.to_string(),
        user_id: user.id.clone(),
        username: user.username.clone(),
        display_name,
    }
}

/// Where `account`'s database lives: one file per server and user.
pub fn database_path(account: &SavedAccount) -> PathBuf {
    url::Url::parse(&account.server)
        .ok()
        .and_then(|server| matty_sync::database_path(&server, &account.user_id))
        .unwrap_or_else(|| std::env::temp_dir().join(format!("matty-{}.sqlite", account.user_id)))
}

fn server_error(error: &ApiError) -> String {
    match error {
        ApiError::Server { status: 404, .. } => {
            "That address answered, but it is not a Mattermost server.".into()
        }
        ApiError::Server { .. } | ApiError::Decode(_) => {
            "That address answered, but not like a Mattermost server.".into()
        }
        _ => "Matty could not reach that server. Check the address and your connection.".into(),
    }
}

fn sign_in_error(error: &ApiError) -> String {
    match error {
        ApiError::MfaRequired => {
            "This account uses multi-factor authentication, which Matty does not support yet."
                .into()
        }
        ApiError::Server { status: 401, .. } => {
            "That login or password is not right. Check them and try again.".into()
        }
        ApiError::Server { message, .. } if !message.is_empty() => message.clone(),
        _ => "Matty could not reach the server. Check your connection and try again.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use matty_api::models::Status;
    use matty_sync::{SidebarChannel, Unreads};

    fn channel(id: &str, kind: &str, messages: i64, mentions: i64) -> SidebarChannel {
        SidebarChannel {
            id: id.into(),
            kind: kind.into(),
            display_name: id.into(),
            dm_user_id: (kind == "D").then(|| format!("{id}-user")),
            unreads: Unreads {
                messages,
                mentions,
                ..Unreads::default()
            },
            muted: false,
            last_post_at: 0,
        }
    }

    fn section(id: &str, channels: Vec<SidebarChannel>) -> SidebarSection {
        SidebarSection {
            id: id.into(),
            kind: "custom".into(),
            display_name: id.to_uppercase(),
            sorting: String::new(),
            collapsed: false,
            muted: false,
            channels,
        }
    }

    fn team(id: &str) -> TeamWithUnreads {
        TeamWithUnreads {
            id: id.into(),
            name: id.into(),
            display_name: format!("Team {id}"),
            has_unreads: false,
            mentions: 0,
        }
    }

    fn live() -> Live {
        let dir = tempfile::tempdir().unwrap();
        let account = SavedAccount {
            server: "http://localhost".into(),
            user_id: "me".into(),
            username: "me".into(),
            display_name: "Me".into(),
        };
        let live = Live::start(dir.path().join("account.sqlite"), &account)
            .unwrap()
            .0;
        // The engine may still be opening the file; let the folder outlive it.
        std::mem::forget(dir);
        live
    }

    #[test]
    fn the_first_team_in_the_users_order_is_shown() {
        let mut live = live();
        let t2 = ChangeSet::Sidebar {
            team_id: "t2".into(),
            sections: Arc::from(vec![section("c", vec![channel("town", "O", 0, 0)])]),
        };
        let t1 = ChangeSet::Sidebar {
            team_id: "t1".into(),
            sections: Arc::from(vec![section("c", vec![channel("general", "O", 3, 1)])]),
        };
        live.apply(&t2);
        live.apply(&t1);
        let update = live.apply(&ChangeSet::Unreads {
            teams: Arc::from(vec![team("t1"), team("t2")]),
        });
        assert_eq!(update.team_name.as_deref(), Some("Team t1"));
        let categories = update.categories.unwrap();
        let row = &categories[0].rows[0];
        assert_eq!(
            (row.id.as_ref(), row.unread, row.mentions),
            ("general", true, 1)
        );
        // Another team's sidebar changing leaves the shown one alone.
        assert_eq!(live.apply(&t2).categories, None);
    }

    #[test]
    fn rows_take_their_kind_and_states_from_the_snapshot() {
        let mut live = live();
        live.apply(&ChangeSet::Unreads {
            teams: Arc::from(vec![team("t1")]),
        });
        let mut muted = channel("quiet", "P", 2, 0);
        muted.muted = true;
        let mut group = channel("gm", "G", 0, 0);
        group.display_name = "ada, alan, grace".into();
        let update = live.apply(&ChangeSet::Sidebar {
            team_id: "t1".into(),
            sections: Arc::from(vec![section(
                "c",
                vec![muted, group, channel("dm", "D", 0, 0)],
            )]),
        });
        let rows = &update.categories.unwrap()[0].rows;
        assert_eq!(rows[0].kind, ChannelKind::Private);
        assert!(rows[0].muted && rows[0].unread);
        assert_eq!(rows[1].kind, ChannelKind::Group { members: 3 });
        assert_eq!(
            rows[2].kind,
            ChannelKind::Direct {
                presence: Presence::Offline
            }
        );

        // A status change redraws the direct message's dot.
        let update = live.apply(&ChangeSet::Presence(Arc::from(vec![Status {
            user_id: "dm-user".into(),
            status: "dnd".into(),
            ..Status::default()
        }])));
        assert_eq!(
            update.categories.unwrap()[0].rows[2].kind,
            ChannelKind::Direct {
                presence: Presence::Dnd
            }
        );
    }

    #[test]
    fn the_connection_and_sign_out_reach_the_window() {
        let mut live = live();
        let update = live.apply(&ChangeSet::Connection(Connection::Socket(
            matty_api::ws::ConnectionState::Reconnecting {
                attempt: 2,
                reason: "closed".into(),
            },
        )));
        assert_eq!(
            update.connection,
            Some(ConnectionState::Reconnecting { attempt: 2 })
        );
        assert!(
            live.apply(&ChangeSet::Connection(Connection::SignedOut))
                .signed_out
        );
    }

    #[test]
    fn the_remembered_name_prefers_the_nickname_then_the_full_name() {
        let mut user = User {
            id: "u1".into(),
            username: "ada".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            ..User::default()
        };
        assert_eq!(
            saved_account("https://x", &user).display_name,
            "Ada Lovelace"
        );
        user.nickname = "Countess".into();
        assert_eq!(saved_account("https://x", &user).display_name, "Countess");
        user.nickname.clear();
        user.first_name.clear();
        user.last_name.clear();
        assert_eq!(saved_account("https://x", &user).display_name, "ada");
    }

    fn shown(id: &str, body: &str) -> Post {
        Post {
            id: id.into(),
            author: "a".into(),
            time: 600,
            day: "Today".into(),
            body: body.into(),
            reactions: vec![],
            mentions_me: false,
            edited: false,
            continues: false,
            mine: false,
            saved: false,
            pinned: false,
            status: crate::channel::PostStatus::Sent,
            replies: None,
            files: vec![],
        }
    }

    #[test]
    fn new_posts_append_and_changes_patch_in_place() {
        let old = [shown("a", "one"), shown("b", "two"), shown("c", "three")];
        let new = [shown("a", "one"), shown("c", "3"), shown("d", "four")];
        assert_eq!(
            patch(&old, &new),
            Some(ChannelUpdate::Patch {
                removed: vec!["b".into()],
                updated: vec![shown("c", "3")],
                appended: vec![shown("d", "four")],
            })
        );
    }

    #[test]
    fn a_post_between_shown_ones_resets() {
        let old = [shown("a", "one"), shown("c", "three")];
        let new = [shown("a", "one"), shown("b", "two"), shown("c", "three")];
        assert_eq!(patch(&old, &new), None);
    }
}
