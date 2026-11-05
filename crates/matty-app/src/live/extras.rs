//! The open channel's bookmarks and App bindings (#226), and the App
//! calls and bookmark changes this view made, so only the window that
//! asked hears how they went.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::Arc;

use gpui_kit::SharedString;
use matty_api::models::{AppBinding, AppContext, ChannelBookmark};
use matty_sync::{AppAnswer, AppCommand, ChangeSet, Command, CommandError};

use super::{Live, Update};
use crate::apps::{AppAsk, ChannelApps};
use crate::bookmarks::{BookmarkAsk, BookmarksModel};

/// What a view keeps about bookmarks and Apps.
#[derive(Default)]
pub(crate) struct Extras {
    /// The open channel's bookmarks, once sync has them; `None` hides the
    /// bar.
    bookmarks: Option<Arc<[ChannelBookmark]>>,
    /// The open channel's App bindings.
    bindings: Option<Arc<[AppBinding]>>,
    /// App calls this view made, by id.
    calls: RefCell<HashSet<String>>,
    next_call: Cell<u64>,
    /// This view changed bookmarks, so a refusal is its to tell.
    wrote: Cell<bool>,
}

impl Extras {
    /// A view's own, for a fork: nothing open, no calls.
    pub(crate) fn fresh() -> Self {
        Self::default()
    }

    /// The channel changed.
    pub(crate) fn clear(&mut self) {
        self.bookmarks = None;
        self.bindings = None;
    }
}

const BOOKMARK_ACTIONS: [&str; 4] = [
    "add bookmark",
    "edit bookmark",
    "delete bookmark",
    "move bookmark",
];

impl Live {
    /// Sends a bookmark change the user made.
    pub fn bookmark(&self, ask: &BookmarkAsk) {
        self.extras.wrote.set(true);
        self.send(ask.command());
    }

    /// Runs the App binding the user chose.
    pub fn run_app(&self, ask: &AppAsk) {
        let Some((binding, app_id)) = self
            .extras
            .bindings
            .as_deref()
            .and_then(|bindings| crate::apps::find(bindings, &ask.location))
        else {
            return;
        };
        let next = self.extras.next_call.get() + 1;
        self.extras.next_call.set(next);
        let call_id = format!("{}:{next}", self.me.0);
        self.extras.calls.borrow_mut().insert(call_id.clone());
        let context = AppContext {
            app_id,
            location: ask.location.to_string(),
            user_agent: matty_api::APPS_USER_AGENT.into(),
            team_id: self.team.clone().unwrap_or_default(),
            channel_id: ask.channel_id.to_string(),
            post_id: ask
                .post_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            root_id: ask
                .root_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
        };
        self.send(Command::App(AppCommand::Run {
            call_id,
            binding: Box::new(binding.clone()),
            context: Box::new(context),
        }));
    }

    /// Asks for the App bindings of the channel just opened.
    pub(super) fn ask_bindings(&mut self, channel_id: &str) {
        self.extras.clear();
        self.send(Command::App(AppCommand::Bindings {
            channel_id: channel_id.to_owned(),
            team_id: self.team.clone().unwrap_or_default(),
        }));
    }

    /// The bar for the open channel, where the server has bookmarks.
    pub(super) fn bookmarks_model(&self) -> Option<BookmarksModel> {
        let open = self.open.as_ref()?;
        let list = self.extras.bookmarks.as_deref()?;
        Some(crate::bookmarks::model(&open.id, list, self.can_bookmark()))
    }

    /// Whether the user may change the open channel's bookmarks: anyone
    /// but a guest, in a channel that is not archived, as Mattermost's
    /// default roles allow. The server has the last word.
    fn can_bookmark(&self) -> bool {
        let archived = self.channel.as_ref().is_some_and(|c| c.delete_at != 0);
        let guest = self
            .users
            .get(&self.me.0)
            .is_some_and(|me| me.roles.split_whitespace().any(|r| r == "system_guest"));
        !archived && !guest
    }

    /// The open channel's App bindings for the views.
    pub(super) fn channel_apps(&self) -> ChannelApps {
        match (&self.open, self.extras.bindings.as_deref()) {
            (Some(open), Some(bindings)) => crate::apps::channel_apps(&open.id, bindings),
            (Some(open), None) => ChannelApps {
                channel_id: open.id.clone(),
                ..ChannelApps::default()
            },
            _ => ChannelApps::default(),
        }
    }

    /// The open channel's details now, before or after its first posts.
    fn current_details(&self) -> super::Details {
        self.shown_details().unwrap_or_else(|| self.details(0))
    }

    /// Applies a change about bookmarks or Apps; `false` when it is none.
    pub(super) fn apply_extra(&mut self, change: &ChangeSet, update: &mut Update) -> bool {
        match change {
            ChangeSet::Bookmarks {
                channel_id,
                bookmarks,
            } => {
                if self.is_open(channel_id) {
                    self.extras.bookmarks = Some(bookmarks.clone());
                    update.details = Some(self.current_details());
                }
            }
            ChangeSet::AppBindings {
                channel_id,
                bindings,
            } => {
                if self.is_open(channel_id) {
                    self.extras.bindings = Some(bindings.clone());
                    update.details = Some(self.current_details());
                }
            }
            ChangeSet::AppAnswered { call_id, answer } => {
                if !self.extras.calls.borrow_mut().remove(call_id) {
                    return true;
                }
                match answer {
                    AppAnswer::Failed(text) => update.refused = Some(text.clone().into()),
                    AppAnswer::Navigate { url, .. } => update.navigate = Some(url.clone().into()),
                    // A form carries the call on; its answer comes later.
                    AppAnswer::Form => {
                        self.extras.calls.borrow_mut().insert(call_id.clone());
                    }
                    AppAnswer::Done => {}
                }
            }
            ChangeSet::Failed(CommandError::Server { action, error, .. })
                if BOOKMARK_ACTIONS.contains(action) =>
            {
                if self.extras.wrote.get() {
                    update.refused =
                        Some(SharedString::from(format!("Couldn't {action}: {error}")));
                }
            }
            ChangeSet::Failed(CommandError::Server { action, error, .. })
                if matches!(*action, "add emoji" | "delete emoji") =>
            {
                // From the Custom emoji dialog (#209): the server's reason,
                // such as a name taken meanwhile or custom emoji turned off.
                update.refused = Some(SharedString::from(format!("Couldn't {action}: {error}")));
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_failures_say_why() {
        let mut live = super::super::tests::live();
        let mut update = Update::default();
        let failed = ChangeSet::Failed(CommandError::Server {
            action: "add emoji",
            target: "party".into(),
            error: "A custom emoji with that name already exists.".into(),
        });
        assert!(live.apply_extra(&failed, &mut update));
        assert_eq!(
            update.refused.as_deref(),
            Some("Couldn't add emoji: A custom emoji with that name already exists.")
        );
    }
    use matty_api::models::AppCall;
    use serde_json::json;

    fn bookmark(id: &str, order: i64) -> ChannelBookmark {
        serde_json::from_value(json!({
            "id": id, "channel_id": "c", "display_name": id, "sort_order": order,
            "type": "link", "link_url": "https://example.com"
        }))
        .unwrap()
    }

    #[test]
    fn bookmarks_reach_the_open_channels_details() {
        let mut live = Live::with_channel_for_tests();
        let _ = live.open(live.row("c").unwrap());
        let update = live.apply(&ChangeSet::Bookmarks {
            channel_id: "c".into(),
            bookmarks: Arc::from([bookmark("a", 0)]),
        });
        let bar = update.details.unwrap().bookmarks.unwrap();
        assert_eq!(bar.bookmarks[0].id.as_ref(), "a");
        assert!(bar.can_edit);
        // Another channel's bookmarks are not this one's.
        let update = live.apply(&ChangeSet::Bookmarks {
            channel_id: "other".into(),
            bookmarks: Arc::from([]),
        });
        assert!(update.details.is_none());
    }

    #[test]
    fn only_the_view_that_asked_hears_how_a_call_went() {
        let mut live = Live::with_channel_for_tests();
        let _ = live.open(live.row("c").unwrap());
        let bindings: Arc<[AppBinding]> = Arc::from(vec![AppBinding {
            location: "/channel_header".into(),
            bindings: vec![AppBinding {
                app_id: "hello".into(),
                label: "Wave".into(),
                submit: Some(AppCall::default()),
                ..AppBinding::default()
            }],
            ..AppBinding::default()
        }]);
        let update = live.apply(&ChangeSet::AppBindings {
            channel_id: "c".into(),
            bindings,
        });
        let apps = update.details.unwrap().apps;
        assert_eq!(apps.channel_header[0].label.as_ref(), "Wave");
        live.run_app(&AppAsk {
            channel_id: "c".into(),
            location: "/channel_header/Wave".into(),
            post_id: None,
            root_id: None,
        });
        let call_id = live.extras.calls.borrow().iter().next().unwrap().clone();
        let (mut fork, _) = live.fork();
        let failed = ChangeSet::AppAnswered {
            call_id,
            answer: AppAnswer::Failed("Nope".into()),
        };
        assert_eq!(fork.apply(&failed).refused, None);
        assert_eq!(live.apply(&failed).refused.as_deref(), Some("Nope"));
        // Once only.
        assert_eq!(live.apply(&failed).refused, None);
    }
}
