//! Pop-out windows' side of sync (#222). A pop-out follows the same engine
//! as the main window through a [`Live::fork`]: a copy of what the window
//! knows so far, with its own change stream and nothing open. Each fork
//! opens its own channel or thread, so new posts, edits, reactions and
//! typing reach every window that shows them, over the one connection.
//!
//! The engine keeps one set of open channels and threads, so a window
//! that closes a channel must not stop it for another window still
//! showing it: [`OpenViews`] counts who holds each one open.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use matty_sync::{ChangeSet, Command};

use super::Live;

/// What a window holds open with the engine.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Held {
    Channel(String),
    Thread(String),
}

/// Which views (a [`Live`] and its forks, each with its own id) hold each
/// channel and thread open. Shared by a `Live` and every fork of it.
#[derive(Clone, Default)]
pub(crate) struct OpenViews {
    /// This view's id among those sharing `held`.
    me: u64,
    next: Rc<RefCell<u64>>,
    held: Rc<RefCell<HashMap<Held, HashSet<u64>>>>,
}

impl OpenViews {
    /// Another view over the same counts.
    fn fork(&self) -> Self {
        let mut next = self.next.borrow_mut();
        *next += 1;
        Self {
            me: *next,
            next: self.next.clone(),
            held: self.held.clone(),
        }
    }

    /// Whether `command` should reach the engine. Opening always does,
    /// which also republishes the snapshot for the window that asked; a
    /// close only once no other view holds the channel or thread open.
    pub(super) fn allows(&self, command: &Command) -> bool {
        let (held, open) = match command {
            Command::OpenChannel { channel_id } => (Held::Channel(channel_id.clone()), true),
            Command::CloseChannel { channel_id } => (Held::Channel(channel_id.clone()), false),
            Command::OpenThread { root_id } => (Held::Thread(root_id.clone()), true),
            Command::CloseThread { root_id } => (Held::Thread(root_id.clone()), false),
            _ => return true,
        };
        let mut all = self.held.borrow_mut();
        if open {
            all.entry(held).or_default().insert(self.me);
            return true;
        }
        let Some(holders) = all.get_mut(&held) else {
            return true;
        };
        holders.remove(&self.me);
        if holders.is_empty() {
            all.remove(&held);
            true
        } else {
            false
        }
    }
}

impl Live {
    /// A view over the same engine for a pop-out window: what this one
    /// knows (the sidebar, people, preferences, pictures), its own change
    /// stream from now on, and no channel, thread or listing open.
    pub fn fork(&self) -> (Self, async_channel::Receiver<ChangeSet>) {
        let changes = self.sync.subscribe();
        let fork = Self {
            sync: self.sync.clone(),
            team: self.team.clone(),
            teams: self.teams.clone(),
            server: self.server.clone(),
            sections: self.sections.clone(),
            presence: self.presence.clone(),
            me: self.me.clone(),
            prefs: self.prefs.clone(),
            notify_props: self.notify_props.clone(),
            users: self.users.clone(),
            open: None,
            posts: None,
            max_post_size: self.max_post_size,
            thread: None,
            thread_pages: (false, false),
            reply_roots: Arc::from([]),
            thread_list: None,
            threads_choice: self.threads_choice,
            threads_on: self.threads_on,
            link_previews: self.link_previews,
            names: self.names,
            offline: self.offline,
            zone: self.zone.clone(),
            shown: None,
            failed: Default::default(),
            queued: self.queued.clone(),
            channel: None,
            member: None,
            stats: None,
            members: None,
            extras: super::extras::Extras::fresh(),
            typing: Vec::new(),
            thread_typing: Vec::new(),
            ready: None,
            search: None,
            search_page: (0, false, false),
            search_files: Arc::from(Vec::new()),
            channel_files: None,
            listing: None,
            saved: self.saved.clone(),
            uploads: HashMap::new(),
            thumbnails: self.thumbnails.clone(),
            asked_thumbnails: self.asked_thumbnails.clone(),
            remote_images: self.remote_images.clone(),
            full_images: self.full_images.clone(),
            avatars: self.avatars.clone(),
            emoji_images: self.emoji_images.clone(),
            team_icons: self.team_icons.clone(),
            custom_emoji: self.custom_emoji.clone(),
            views: self.views.fork(),
            interactive: Default::default(),
        };
        (fork, changes)
    }

    /// The pop-out closed: its channel and thread stop, unless another
    /// window still shows them. The team's Threads list is the main
    /// window's, so it stays.
    pub fn close_view(&mut self) {
        if let Some(row) = self.open.take() {
            self.send(Command::CloseChannel {
                channel_id: row.id.to_string(),
            });
        }
        self.close_thread();
    }

    /// The shown team's channels, for `~` in a pop-out's composer.
    pub fn directory(&self) -> Vec<crate::sidebar::ChannelRow> {
        self.shown_categories()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|category| category.rows)
            .collect()
    }

    /// The channel this view shows, if any.
    pub fn open_channel_id(&self) -> Option<&str> {
        self.open.as_ref().map(|row| row.id.as_ref())
    }
}

#[cfg(test)]
impl Live {
    /// A live with team `t` and channel `c` in its sidebar, for tests
    /// elsewhere in the crate.
    pub(crate) fn with_channel_for_tests() -> Self {
        tests::with_sidebar()
    }
}

#[cfg(test)]
mod tests {
    use super::super::ChannelUpdate;
    use super::super::tests::{channel, live, section, team};
    use super::*;

    fn post(id: &str, channel_id: &str, message: &str) -> matty_api::models::Post {
        matty_api::models::Post {
            id: id.into(),
            channel_id: channel_id.into(),
            user_id: "someone".into(),
            message: message.into(),
            create_at: 1_700_000_000_000,
            ..Default::default()
        }
    }

    /// A live with team `t` and channel `c` in its sidebar.
    pub(super) fn with_sidebar() -> Live {
        let mut live = live();
        let _ = live.apply(&ChangeSet::Sidebar {
            team_id: "t".into(),
            sections: Arc::from([section("s", vec![channel("c", "O", 0, 0)])]),
        });
        let _ = live.apply(&ChangeSet::Unreads {
            teams: Arc::from([team("t")]),
        });
        live
    }

    fn appended(update: Option<ChannelUpdate>) -> Vec<String> {
        match update {
            Some(ChannelUpdate::Replace(model)) => {
                model.posts.iter().map(|p| p.body.to_string()).collect()
            }
            Some(ChannelUpdate::Patch { appended, .. }) => {
                appended.iter().map(|p| p.body.to_string()).collect()
            }
            None => Vec::new(),
        }
    }

    #[test]
    fn a_fork_knows_the_sidebar_and_both_views_get_new_posts() {
        let mut main = with_sidebar();
        let (mut popped, _changes) = main.fork();
        let row = popped.row("c").expect("the fork knows the sidebar");
        let _ = main.open(row.clone());
        let _ = popped.open(row);
        assert_eq!(popped.open_channel_id(), Some("c"));

        let first = ChangeSet::Posts {
            channel_id: "c".into(),
            posts: Arc::from([post("p1", "c", "hello")]),
        };
        assert_eq!(appended(main.apply(&first).channel), ["hello"]);
        assert_eq!(appended(popped.apply(&first).channel), ["hello"]);

        let second = ChangeSet::Posts {
            channel_id: "c".into(),
            posts: Arc::from([post("p1", "c", "hello"), post("p2", "c", "again")]),
        };
        assert_eq!(appended(main.apply(&second).channel), ["again"]);
        assert_eq!(appended(popped.apply(&second).channel), ["again"]);
    }

    #[test]
    fn a_channel_closes_only_when_no_window_holds_it() {
        let main = OpenViews::default();
        let popped = main.fork();
        let open = Command::OpenChannel {
            channel_id: "c".into(),
        };
        let close = Command::CloseChannel {
            channel_id: "c".into(),
        };
        assert!(main.allows(&open));
        assert!(popped.allows(&open));
        assert!(!main.allows(&close), "the pop-out still shows it");
        assert!(popped.allows(&close), "nobody shows it now");
        // Opening the same channel again counts once per view.
        assert!(main.allows(&open));
        assert!(main.allows(&open));
        assert!(main.allows(&close));
        // Other commands always pass.
        assert!(popped.allows(&Command::ShowCache));
    }

    #[test]
    fn closing_a_view_keeps_what_the_main_window_shows() {
        let mut main = with_sidebar();
        let (mut popped, _changes) = main.fork();
        let row = main.row("c").unwrap();
        let _ = main.open(row.clone());
        let _ = popped.open(row);
        popped.close_view();
        assert_eq!(popped.open_channel_id(), None);
        // The main window's close is the last one, so it reaches the engine.
        assert!(main.views.allows(&Command::CloseChannel {
            channel_id: "c".into()
        }));
    }
}
