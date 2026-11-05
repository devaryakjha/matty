//! Message priority, acknowledgements and scheduled messages (#220): the
//! engine's share of them, kept apart from the engine's larger loop.

use std::collections::BTreeMap;
use std::sync::Arc;

use matty_api::models::{PostAcknowledgement, ScheduledPost};
use matty_store::{StoreError, posts};
use rusqlite::Connection;

use crate::engine::Dirty;
use crate::events::touch_posts;

/// A change to the user's scheduled messages, from a load, a write's
/// answer or a `scheduled_post_*` event.
#[derive(Debug)]
pub(crate) enum ScheduledChange {
    /// Every scheduled message, as the server listed them.
    Replace(Vec<ScheduledPost>),
    /// One was created or changed.
    Upsert(ScheduledPost),
    /// One was sent or deleted.
    Remove(String),
}

/// The user's scheduled messages across teams. They are not cached: the
/// server keeps them and lists them at each connect.
#[derive(Default)]
pub(crate) struct Scheduled {
    posts: BTreeMap<String, ScheduledPost>,
}

impl Scheduled {
    /// Applies changes in order; `true` when anything changed.
    pub(crate) fn apply(&mut self, changes: Vec<ScheduledChange>) -> bool {
        let before = self.posts.clone();
        for change in changes {
            match change {
                ScheduledChange::Replace(all) => {
                    self.posts = all
                        .into_iter()
                        .map(|post| (post.id.clone(), post))
                        .collect();
                }
                ScheduledChange::Upsert(post) => {
                    self.posts.insert(post.id.clone(), post);
                }
                ScheduledChange::Remove(id) => {
                    self.posts.remove(&id);
                }
            }
        }
        before != self.posts
    }

    /// One by id.
    pub(crate) fn get(&self, id: &str) -> Option<&ScheduledPost> {
        self.posts.get(id)
    }

    /// Whether there are none.
    pub(crate) fn is_empty(&self) -> bool {
        self.posts.is_empty()
    }

    /// Every one, soonest first.
    pub(crate) fn snapshot(&self) -> Arc<[ScheduledPost]> {
        let mut all: Vec<ScheduledPost> = self.posts.values().cloned().collect();
        all.sort_by(|a, b| a.scheduled_at.cmp(&b.scheduled_at).then(a.id.cmp(&b.id)));
        Arc::from(all)
    }
}

/// Records an acknowledgement, or takes one back, in the cached post and
/// redraws it. Nothing happens for a post the cache does not hold.
pub(crate) fn acknowledge_locally(
    conn: &Connection,
    ack: &PostAcknowledgement,
    acknowledged: bool,
    dirty: &mut Dirty,
) -> Result<(), StoreError> {
    if posts::set_acknowledgement(conn, ack, acknowledged)?
        && let Some(post) = posts::post(conn, &ack.post_id)?
    {
        touch_posts(&post, dirty);
    }
    Ok(())
}

/// What acknowledging asked, for [`CommandError`](crate::CommandError).
pub(crate) fn ack_action(acknowledged: bool) -> &'static str {
    if acknowledged {
        "acknowledge"
    } else {
        "remove acknowledgement"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduled(id: &str, at: i64) -> ScheduledPost {
        ScheduledPost {
            id: id.into(),
            scheduled_at: at,
            ..ScheduledPost::default()
        }
    }

    #[test]
    fn changes_apply_in_order_and_list_soonest_first() {
        let mut list = Scheduled::default();
        assert!(list.apply(vec![ScheduledChange::Replace(vec![
            scheduled("b", 20),
            scheduled("a", 30),
        ])]));
        assert!(list.apply(vec![ScheduledChange::Upsert(scheduled("c", 10))]));
        let ids: Vec<String> = list.snapshot().iter().map(|post| post.id.clone()).collect();
        assert_eq!(ids, ["c", "b", "a"]);
        assert!(!list.apply(vec![ScheduledChange::Upsert(scheduled("c", 10))]));
        assert!(list.apply(vec![ScheduledChange::Remove("b".into())]));
        assert!(list.get("b").is_none() && list.get("a").is_some());
        assert!(!list.apply(vec![ScheduledChange::Remove("b".into())]));
    }
}
