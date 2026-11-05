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
        metadata: matty_api::models::PostMetadata {
            priority: outgoing.priority.clone(),
            ..Default::default()
        },
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

/// Marks what a change to `outgoing` redraws: its channel, the channel's
/// outbox and its thread.
pub(crate) fn touch(outgoing: &Outgoing, dirty: &mut Dirty) {
    dirty.channels.insert(outgoing.channel_id.clone());
    dirty.outbox.insert(outgoing.channel_id.clone());
    if !outgoing.root_id.is_empty() {
        dirty.threads.insert(outgoing.root_id.clone());
    }
}

/// Sends one queued post: first its offline files, one by one, each
/// reported as it lands so a retry skips it (#194), then the post with
/// every file id.
pub(crate) async fn send(client: Client, mut outgoing: Outgoing, results: UnboundedSender<Loaded>) {
    for path in std::mem::take(&mut outgoing.local_files) {
        match upload_local(&client, &outgoing, &path).await {
            Ok(file_id) => {
                let _ = results.send(Loaded::FileUp {
                    pending_post_id: outgoing.pending_post_id.clone(),
                    path,
                    file_id: file_id.clone(),
                });
                outgoing.file_ids.push(file_id);
            }
            Err((error, transient)) => {
                let _ = results.send(Loaded::SendFailed {
                    pending_post_id: outgoing.pending_post_id,
                    transient,
                    error,
                });
                return;
            }
        }
    }
    let request = NewPost {
        channel_id: outgoing.channel_id,
        message: outgoing.message,
        root_id: outgoing.root_id,
        file_ids: outgoing.file_ids,
        props: outgoing.props,
        pending_post_id: outgoing.pending_post_id.clone(),
        priority: outgoing
            .priority
            .filter(matty_api::models::PostPriority::is_set),
    };
    let result: Result<Post, ApiError> = client.create_post(&request).await;
    let _ = results.send(match result {
        Ok(post) => Loaded::Sent {
            pending_post_id: outgoing.pending_post_id,
            post: Box::new(post),
        },
        Err(error) => Loaded::SendFailed {
            pending_post_id: outgoing.pending_post_id,
            transient: transient(&error),
            error: error.to_string(),
        },
    });
}

/// Reads and uploads one file attached while offline. A file no longer on
/// disk, or one the server refuses, fails the send for the user to see;
/// a network or server failure is transient.
async fn upload_local(
    client: &Client,
    outgoing: &Outgoing,
    path: &str,
) -> Result<String, (String, bool)> {
    let file_name = local_file_name(path);
    let read = {
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || std::fs::read(path)).await
    };
    let bytes = match read {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => {
            return Err((format!("could not read {file_name}: {error}"), false));
        }
        Err(error) => return Err((error.to_string(), true)),
    };
    let upload = matty_api::Upload {
        channel_id: outgoing.channel_id.clone(),
        client_id: format!("{}:{path}", outgoing.pending_post_id),
        file_name,
        bytes,
    };
    let progress: matty_api::Progress = std::sync::Arc::new(|_, _| {});
    match client.upload_file(upload, progress).await {
        Ok(uploaded) => match uploaded.file_infos.into_iter().next() {
            Some(file) => Ok(file.id),
            None => Err(("the server kept no file".into(), false)),
        },
        Err(error) => Err((error.to_string(), transient(&error))),
    }
}

/// The name a file attached offline is uploaded under: its file name.
pub(crate) fn local_file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "File".to_owned())
}

/// Whether a failed send should go again by itself: the server was not
/// reached, was busy, or failed on its side. A refusal (4xx) needs the user.
pub(crate) fn transient(error: &ApiError) -> bool {
    match error {
        ApiError::Network(_) | ApiError::Timeout | ApiError::RateLimited => true,
        ApiError::Server { status, .. } => *status >= 500,
        _ => false,
    }
}

/// How long to wait before trying again after `attempts` tries: doubling
/// from two seconds up to about a minute.
pub(crate) fn backoff(attempts: i64) -> std::time::Duration {
    let exponent = u32::try_from(attempts.clamp(1, 6)).unwrap_or(6);
    std::time::Duration::from_secs(2u64.pow(exponent))
}

/// Sends a file, reporting progress in steps of at least 5% so the views
/// redraw a few dozen times, not once per chunk.
pub(crate) async fn upload(
    client: Client,
    upload: matty_api::Upload,
    results: UnboundedSender<Loaded>,
) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::change::UploadState;

    let upload_id = upload.client_id.clone();
    let reported = Arc::new(AtomicU32::new(0));
    let progress: matty_api::Progress = {
        let results = results.clone();
        let upload_id = upload_id.clone();
        Arc::new(move |sent, total| {
            let percent =
                u32::try_from((sent * 100).checked_div(total).unwrap_or(100)).unwrap_or(100);
            let last = reported.load(Ordering::Relaxed);
            if percent >= last + 5 && percent < 100 {
                reported.store(percent, Ordering::Relaxed);
                let _ = results.send(Loaded::Upload {
                    upload_id: upload_id.clone(),
                    state: UploadState::Sending(percent as f32 / 100.0),
                });
            }
        })
    };
    let state = match client.upload_file(upload, progress).await {
        Ok(uploaded) => match uploaded.file_infos.into_iter().next() {
            Some(file) => UploadState::Done { file_id: file.id },
            None => UploadState::Failed("the server kept no file".into()),
        },
        Err(error) => UploadState::Failed(error.to_string()),
    };
    let _ = results.send(Loaded::Upload { upload_id, state });
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
        for id in ["c", "d"] {
            sidebar::upsert_channels(
                conn,
                &[Channel {
                    id: id.into(),
                    team_id: "t".into(),
                    kind: "O".into(),
                    name: id.into(),
                    display_name: id.into(),
                    ..Channel::default()
                }],
            )
            .unwrap();
            sidebar::upsert_channel_members(
                conn,
                &[ChannelMember {
                    channel_id: id.into(),
                    user_id: ME.into(),
                    ..ChannelMember::default()
                }],
            )
            .unwrap();
        }
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

    /// One step of a drain with the connection coming and going (#194).
    #[derive(Clone, Debug)]
    enum Step {
        /// The user writes a post in channel 0 or 1, with this many files
        /// attached while offline.
        Write(usize, usize),
        /// The engine starts the channel's next send, unless one is out.
        Start(usize),
        /// The send out takes its next step: its next offline file lands
        /// on the server, or, with none left, the post comes back created.
        Created(usize),
        /// The server took the file or created the post but the answer was
        /// lost on the way back: a transient failure, so it goes again.
        Lost(usize),
        /// The connection dropped before the server saw it.
        Dropped(usize),
        /// The server refused it (4xx): it waits for Retry.
        Refused(usize),
        /// The `posted` echo of the channel's newest post on the server.
        Echo(usize),
        /// The user edits the channel's nth waiting send.
        Edit(usize, usize),
        /// The user discards the channel's nth waiting send.
        Discard(usize, usize),
        /// The user retries the channel's failed sends.
        Retry(usize),
        /// The app quits and starts again: whatever was out is forgotten
        /// and only the store remains.
        Relaunch,
    }

    fn step() -> impl Strategy<Value = Step> {
        let channel = 0usize..2;
        prop_oneof![
            3 => (channel.clone(), 0usize..3).prop_map(|(c, files)| Step::Write(c, files)),
            3 => channel.clone().prop_map(Step::Start),
            2 => channel.clone().prop_map(Step::Created),
            1 => channel.clone().prop_map(Step::Lost),
            1 => channel.clone().prop_map(Step::Dropped),
            1 => channel.clone().prop_map(Step::Refused),
            1 => channel.clone().prop_map(Step::Echo),
            1 => (channel.clone(), 0usize..4).prop_map(|(c, n)| Step::Edit(c, n)),
            1 => (channel.clone(), 0usize..4).prop_map(|(c, n)| Step::Discard(c, n)),
            1 => channel.prop_map(Step::Retry),
            1 => Just(Step::Relaunch),
        ]
    }

    /// The engine's side of a drain, over the same store calls it makes,
    /// and a server that, like Mattermost, answers a repeated
    /// `pending_post_id` with the post it already made.
    #[derive(Default)]
    struct Drain {
        written: Vec<Vec<String>>,
        /// Each send's offline files, in the order they were attached.
        attached: std::collections::HashMap<String, Vec<String>>,
        out: [Option<String>; 2],
        server: Vec<Vec<Post>>,
        /// Every file the server took, by its id: the local path it came
        /// from. Uploads whose answer was lost stay here unused.
        files: std::collections::HashMap<String, String>,
        /// Sends the server refused at least once; Retry sends them after
        /// later ones, by design.
        refused: std::collections::BTreeSet<String>,
        discarded: std::collections::BTreeSet<String>,
        clock: i64,
    }

    const CHANNELS: [&str; 2] = ["c", "d"];

    impl Drain {
        fn new() -> Self {
            Self {
                written: vec![Vec::new(); 2],
                server: vec![Vec::new(); 2],
                ..Self::default()
            }
        }

        fn accept(&mut self, channel: usize, outgoing: &Outgoing) -> Post {
            if let Some(post) = self.server[channel]
                .iter()
                .find(|post| post.pending_post_id == outgoing.pending_post_id)
            {
                return post.clone();
            }
            self.clock += 1;
            let post = Post {
                id: format!("s-{}", outgoing.pending_post_id),
                pending_post_id: outgoing.pending_post_id.clone(),
                channel_id: outgoing.channel_id.clone(),
                user_id: ME.into(),
                message: outgoing.message.clone(),
                file_ids: outgoing.file_ids.clone(),
                create_at: 1_000 + self.clock,
                update_at: 1_000 + self.clock,
                ..Post::default()
            };
            self.server[channel].push(post.clone());
            post
        }

        fn run(&mut self, conn: &Connection, step: &Step) {
            let mut dirty = Dirty::default();
            let in_flight =
                |out: &[Option<String>; 2], id: &str| out.iter().flatten().any(|out| out == id);
            match *step {
                Step::Write(channel, files) => {
                    self.clock += 1;
                    let id = format!("{ME}:{}", self.clock);
                    let local_files: Vec<String> = (0..files)
                        .map(|n| format!("/home/me/{}-{n}.png", self.clock))
                        .collect();
                    let outgoing = Outgoing {
                        pending_post_id: id.clone(),
                        channel_id: CHANNELS[channel].into(),
                        message: format!("message {}", self.clock),
                        local_files: local_files.clone(),
                        created_at: self.clock,
                        ..Outgoing::default()
                    };
                    queue(conn, &outgoing, ME, &mut dirty).unwrap();
                    self.attached.insert(id.clone(), local_files);
                    self.written[channel].push(id);
                }
                Step::Start(channel) => {
                    if self.out[channel].is_none()
                        && let Some(next) = outbox::next(conn, CHANNELS[channel]).unwrap()
                    {
                        outbox::attempt(conn, &next.pending_post_id).unwrap();
                        self.out[channel] = Some(next.pending_post_id);
                    }
                }
                Step::Created(channel) => {
                    if let Some(id) = self.out[channel].take()
                        && let Some(outgoing) = outbox::get(conn, &id).unwrap()
                    {
                        if let Some(path) = outgoing.local_files.first() {
                            // The engine's `Loaded::FileUp`; the send stays out.
                            let file_id = self.upload(path);
                            assert!(outbox::file_up(conn, &id, path, &file_id).unwrap());
                            self.out[channel] = Some(id);
                        } else {
                            let post = self.accept(channel, &outgoing);
                            confirm(conn, &id, &post, Some(ME), &mut dirty).unwrap();
                        }
                    }
                }
                Step::Lost(channel) => {
                    if let Some(id) = self.out[channel].take()
                        && let Some(outgoing) = outbox::get(conn, &id).unwrap()
                    {
                        match outgoing.local_files.first() {
                            Some(path) => {
                                self.upload(path);
                            }
                            None => {
                                self.accept(channel, &outgoing);
                            }
                        }
                    }
                }
                Step::Relaunch => self.out = [None, None],
                Step::Dropped(channel) => self.out[channel] = None,
                Step::Refused(channel) => {
                    if let Some(id) = self.out[channel].take() {
                        fail(conn, &id, "refused", &mut dirty).unwrap();
                        self.refused.insert(id);
                    }
                }
                Step::Echo(channel) => {
                    if let Some(post) = self.server[channel].last() {
                        let event = WsEvent::Posted(Box::new(Posted {
                            post: post.clone(),
                            ..Posted::default()
                        }));
                        events::apply(conn, event, Some(ME), &mut dirty, &mut Vec::new()).unwrap();
                    }
                }
                Step::Edit(channel, n) => {
                    let waiting = outbox::for_channel(conn, CHANNELS[channel]).unwrap();
                    if let Some(outgoing) = waiting.get(n)
                        && !in_flight(&self.out, &outgoing.pending_post_id)
                    {
                        let message = format!("{} (edited)", outgoing.message);
                        assert!(outbox::edit(conn, &outgoing.pending_post_id, &message).unwrap());
                        let row = posts::post(conn, &outgoing.pending_post_id).unwrap();
                        assert_eq!(row.map(|row| row.message), Some(message));
                    }
                }
                Step::Discard(channel, n) => {
                    let waiting = outbox::for_channel(conn, CHANNELS[channel]).unwrap();
                    if let Some(outgoing) = waiting.get(n)
                        && !in_flight(&self.out, &outgoing.pending_post_id)
                    {
                        assert!(discard(conn, &outgoing.pending_post_id, &mut dirty).unwrap());
                        self.discarded.insert(outgoing.pending_post_id.clone());
                    }
                }
                Step::Retry(channel) => {
                    for outgoing in outbox::for_channel(conn, CHANNELS[channel]).unwrap() {
                        outbox::unfail(conn, &outgoing.pending_post_id).unwrap();
                    }
                }
            }
        }

        /// The server stores a file and gives it a new id.
        fn upload(&mut self, path: &str) -> String {
            self.clock += 1;
            let file_id = format!("file{}", self.clock);
            self.files.insert(file_id.clone(), path.to_owned());
            file_id
        }

        /// The connection stays up until every channel has sent all it can.
        fn finish(&mut self, conn: &Connection) {
            for (channel, id) in CHANNELS.iter().enumerate() {
                self.run(conn, &Step::Retry(channel));
                while self.out[channel].is_some() || outbox::next(conn, id).unwrap().is_some() {
                    self.run(conn, &Step::Start(channel));
                    self.run(conn, &Step::Created(channel));
                }
            }
        }
    }

    proptest! {
        /// However sends, offline file uploads, dropped connections,
        /// relaunches, failures, lost answers, echoes, edits and discards
        /// interleave, each channel's posts reach the server once each, in
        /// the order they were written, with every file attached (#194),
        /// and every optimistic row ends up replaced by the server's post
        /// or gone.
        #[test]
        fn a_drain_never_duplicates_or_reorders_within_a_channel(
            steps in proptest::collection::vec(step(), 0..80)
        ) {
            let store = setup();
            let conn = store.conn();
            let mut drain = Drain::new();
            for step in &steps {
                drain.run(conn, step);
                // While it runs, too: what reached the server never repeats
                // and never overtakes, refused sends aside.
                for channel in 0..2 {
                    let sent: Vec<&str> = drain.server[channel]
                        .iter()
                        .map(|post| post.pending_post_id.as_str())
                        .filter(|id| !drain.refused.contains(*id))
                        .collect();
                    let mut unique = sent.clone();
                    unique.sort_unstable();
                    unique.dedup();
                    prop_assert_eq!(unique.len(), sent.len());
                    let written: Vec<&str> = drain.written[channel]
                        .iter()
                        .map(String::as_str)
                        .filter(|id| sent.contains(id))
                        .collect();
                    prop_assert_eq!(&sent, &written);
                }
            }
            drain.finish(conn);
            for (channel, channel_id) in CHANNELS.iter().enumerate() {
                let on_server: Vec<&str> = drain.server[channel]
                    .iter()
                    .map(|post| post.pending_post_id.as_str())
                    .collect();
                // Everything not discarded arrived, and once.
                for id in &drain.written[channel] {
                    let times = on_server.iter().filter(|sent| **sent == id).count();
                    if drain.discarded.contains(id) {
                        prop_assert!(times <= 1);
                    } else {
                        prop_assert_eq!(times, 1, "{} arrived {} times", id, times);
                    }
                }
                prop_assert!(outbox::for_channel(conn, channel_id).unwrap().is_empty());
                // Each post carries its offline files once each, in the
                // order they were attached: none lost, none twice.
                for post in &drain.server[channel] {
                    let carried: Vec<&str> = post
                        .file_ids
                        .iter()
                        .map(|id| drain.files[id].as_str())
                        .collect();
                    let attached: Vec<&str> = drain.attached[&post.pending_post_id]
                        .iter()
                        .map(String::as_str)
                        .collect();
                    prop_assert_eq!(carried, attached, "{}", post.pending_post_id);
                }
            }
            // No optimistic row is left, and no post shows twice.
            let rows: Vec<(String, String)> = conn
                .prepare("SELECT id, pending_post_id FROM posts")
                .unwrap()
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            prop_assert!(rows.iter().all(|(id, pending)| id != pending));
            let mut pending: Vec<&str> = rows.iter().map(|(_, p)| p.as_str()).collect();
            pending.sort_unstable();
            let before = pending.len();
            pending.dedup();
            prop_assert_eq!(before, pending.len());
        }
    }

    #[test]
    fn an_edit_keeps_the_send_in_its_place() {
        let store = setup();
        let conn = store.conn();
        let mut dirty = Dirty::default();
        for (id, message) in [("me:1", "first"), ("me:2", "second")] {
            let outgoing = Outgoing {
                pending_post_id: id.into(),
                channel_id: "c".into(),
                message: message.into(),
                ..Outgoing::default()
            };
            queue(conn, &outgoing, ME, &mut dirty).unwrap();
        }
        assert!(outbox::edit(conn, "me:1", "first, fixed").unwrap());
        let queued: Vec<(String, String)> = outbox::for_channel(conn, "c")
            .unwrap()
            .into_iter()
            .map(|outgoing| (outgoing.pending_post_id, outgoing.message))
            .collect();
        assert_eq!(
            queued,
            [
                ("me:1".to_owned(), "first, fixed".to_owned()),
                ("me:2".to_owned(), "second".to_owned())
            ]
        );
        let row = posts::post(conn, "me:1").unwrap().unwrap();
        assert_eq!((row.message.as_str(), row.edit_at), ("first, fixed", 0));
        assert!(!outbox::edit(conn, "me:9", "nothing").unwrap());
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
