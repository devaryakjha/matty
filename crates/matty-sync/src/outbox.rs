//! Sending posts through the outbox (#38, PLAN §6.5).

use matty_api::models::Post;
use matty_api::{ApiError, Client, NewPost};
use matty_store::outbox::{self, Outgoing};
use matty_store::sidebar::{self, NewPost as Counted};
use matty_store::{StoreError, posts};
use rusqlite::Connection;
use tokio::sync::mpsc::UnboundedSender;

use crate::engine::Dirty;
use crate::load::Loaded;

/// Queues a send and writes its optimistic row, just after the channel's
/// newest post so it shows at the bottom.
pub(crate) fn queue(
    conn: &Connection,
    outgoing: &Outgoing,
    me: &str,
    dirty: &mut Dirty,
) -> Result<(), StoreError> {
    outbox::push(conn, outgoing)?;
    let create_at = outgoing
        .created_at
        .max(posts::newest_create_at(conn, &outgoing.channel_id)? + 1);
    let row = Post {
        id: outgoing.pending_post_id.clone(),
        pending_post_id: outgoing.pending_post_id.clone(),
        channel_id: outgoing.channel_id.clone(),
        root_id: outgoing.root_id.clone(),
        user_id: me.to_owned(),
        message: outgoing.message.clone(),
        file_ids: outgoing.file_ids.clone(),
        props: outgoing.props.clone(),
        create_at,
        ..Post::default()
    };
    // Only if absent: a retry must not undo a confirmation.
    if posts::post(conn, &row.id)?.is_none() {
        posts::insert_posts(conn, std::slice::from_ref(&row))?;
    }
    touch(outgoing, dirty);
    Ok(())
}

/// The server has the post: replace the optimistic row and drop the send.
/// When the WebSocket `posted` got here first, the row is already replaced
/// and counted, and this only drops the send.
pub(crate) fn confirm(
    conn: &Connection,
    pending_post_id: &str,
    post: &Post,
    me: Option<&str>,
    dirty: &mut Dirty,
) -> Result<(), StoreError> {
    let counted = posts::post(conn, &post.id)?.is_some();
    posts::replace_pending(conn, pending_post_id, post)?;
    outbox::remove(conn, pending_post_id)?;
    if !counted {
        if !post.root_id.is_empty() {
            posts::add_replies(conn, &post.root_id, 1)?;
        }
        if let Some(me) = me {
            sidebar::count_post(
                conn,
                &post.channel_id,
                me,
                &Counted {
                    create_at: post.create_at,
                    is_root: post.root_id.is_empty(),
                    by_me: true,
                    ..Counted::default()
                },
            )?;
        }
    }
    dirty.channels.insert(post.channel_id.clone());
    dirty.outbox.insert(post.channel_id.clone());
    dirty.all_teams = true;
    dirty.unreads = true;
    if !post.root_id.is_empty() {
        dirty.threads.insert(post.root_id.clone());
    }
    Ok(())
}

/// Marks a send failed; its row and text stay for a retry.
pub(crate) fn fail(
    conn: &Connection,
    pending_post_id: &str,
    error: &str,
    dirty: &mut Dirty,
) -> Result<(), StoreError> {
    if let Some(outgoing) = outbox::get(conn, pending_post_id)? {
        outbox::fail(conn, pending_post_id, error)?;
        touch(&outgoing, dirty);
    }
    Ok(())
}

/// Drops an unsent post and its row. `false` when it is not queued (it was
/// sent meanwhile, or never existed).
pub(crate) fn discard(
    conn: &Connection,
    pending_post_id: &str,
    dirty: &mut Dirty,
) -> Result<bool, StoreError> {
    let Some(outgoing) = outbox::get(conn, pending_post_id)? else {
        return Ok(false);
    };
    outbox::remove(conn, pending_post_id)?;
    posts::remove_post(conn, pending_post_id)?;
    touch(&outgoing, dirty);
    Ok(true)
}

fn touch(outgoing: &Outgoing, dirty: &mut Dirty) {
    dirty.channels.insert(outgoing.channel_id.clone());
    dirty.outbox.insert(outgoing.channel_id.clone());
    if !outgoing.root_id.is_empty() {
        dirty.threads.insert(outgoing.root_id.clone());
    }
}

/// Sends one queued post.
pub(crate) async fn send(client: Client, outgoing: Outgoing, results: UnboundedSender<Loaded>) {
    let request = NewPost {
        channel_id: outgoing.channel_id,
        message: outgoing.message,
        root_id: outgoing.root_id,
        file_ids: outgoing.file_ids,
        props: outgoing.props,
        pending_post_id: outgoing.pending_post_id.clone(),
    };
    let result: Result<Post, ApiError> = client.create_post(&request).await;
    let _ = results.send(match result {
        Ok(post) => Loaded::Sent {
            pending_post_id: outgoing.pending_post_id,
            post: Box::new(post),
        },
        Err(error) => Loaded::SendFailed {
            pending_post_id: outgoing.pending_post_id,
            error: error.to_string(),
        },
    });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use matty_api::models::{Channel, ChannelMember};
    use matty_api::ws::{Posted, WsEvent};
    use matty_store::Store;
    use proptest::prelude::*;

    use super::*;
    use crate::events;

    const ME: &str = "me";

    fn setup() -> Store {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        sidebar::upsert_channels(
            conn,
            &[Channel {
                id: "c".into(),
                team_id: "t".into(),
                kind: "O".into(),
                name: "c".into(),
                display_name: "c".into(),
                ..Channel::default()
            }],
        )
        .unwrap();
        sidebar::upsert_channel_members(
            conn,
            &[ChannelMember {
                channel_id: "c".into(),
                user_id: ME.into(),
                ..ChannelMember::default()
            }],
        )
        .unwrap();
        store
    }

    fn server_post() -> Post {
        Post {
            id: "server".into(),
            pending_post_id: "me:1".into(),
            channel_id: "c".into(),
            user_id: ME.into(),
            create_at: 100,
            update_at: 100,
            message: "hello".into(),
            ..Post::default()
        }
    }

    proptest! {
        /// However the REST response and any number of `posted` echoes
        /// interleave, one row remains, under the server's id, counted once.
        #[test]
        fn any_interleaving_leaves_exactly_one_row(echoes in 0usize..=3, rest_at in 0usize..=3) {
            let store = setup();
            let conn = store.conn();
            let mut dirty = Dirty::default();
            let outgoing = Outgoing {
                pending_post_id: "me:1".into(),
                channel_id: "c".into(),
                message: "hello".into(),
                created_at: 1,
                ..Outgoing::default()
            };
            queue(conn, &outgoing, ME, &mut dirty).unwrap();
            let rest_at = rest_at.min(echoes);
            for step in 0..=echoes {
                if step == rest_at {
                    confirm(conn, "me:1", &server_post(), Some(ME), &mut dirty).unwrap();
                }
                if step < echoes {
                    let event = WsEvent::Posted(Box::new(Posted {
                        post: server_post(),
                        ..Posted::default()
                    }));
                    events::apply(conn, event, Some(ME), &mut dirty, &mut Vec::new()).unwrap();
                }
            }
            let ids: Vec<String> = conn
                .prepare("SELECT id FROM posts WHERE channel_id = 'c'")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            prop_assert_eq!(ids, vec!["server".to_owned()]);
            prop_assert!(outbox::get(conn, "me:1").unwrap().is_none());
            let channel = sidebar::channel(conn, "c").unwrap().unwrap();
            let member = sidebar::channel_member(conn, "c", ME).unwrap().unwrap();
            prop_assert_eq!((channel.total_msg_count, member.msg_count), (1, 1));
        }
    }

    #[test]
    fn discard_removes_the_row_and_the_send() {
        let store = setup();
        let conn = store.conn();
        let mut dirty = Dirty::default();
        let outgoing = Outgoing {
            pending_post_id: "me:2".into(),
            channel_id: "c".into(),
            message: "oops".into(),
            ..Outgoing::default()
        };
        queue(conn, &outgoing, ME, &mut dirty).unwrap();
        assert!(posts::post(conn, "me:2").unwrap().is_some());
        assert!(discard(conn, "me:2", &mut dirty).unwrap());
        assert!(posts::post(conn, "me:2").unwrap().is_none());
        assert!(!discard(conn, "me:2", &mut dirty).unwrap());
    }
}
