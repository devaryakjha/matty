//! The thread, runtime and channels between the engine and the app.

use std::io;
use std::path::PathBuf;
#[cfg(not(target_family = "wasm"))]
use std::time::Duration;

use matty_store::Store;
use tokio::sync::mpsc;

use crate::change::ChangeSet;
use crate::engine::{Command, Engine, Message};

/// How long the runtime waits for its tasks when the engine stops.
#[cfg(not(target_family = "wasm"))]
const SHUTDOWN_GRACE: Duration = Duration::from_millis(500);

/// The app's side of the sync engine. Cheap to clone and usable from any
/// thread; the engine stops when the last clone is dropped, and every
/// [`subscribe`](Self::subscribe) receiver then reports closed.
#[derive(Clone, Debug)]
pub struct SyncHandle {
    messages: mpsc::UnboundedSender<Message>,
}

impl SyncHandle {
    /// Queues a command. Never blocks; returns `false` once the engine has
    /// stopped.
    pub fn send(&self, command: Command) -> bool {
        self.messages.send(Message::Command(command)).is_ok()
    }

    /// A new stream of change sets, from now on. The receiver works on any
    /// executor (GPUI's included) and never waits on a lock the sync thread
    /// holds: the engine only pushes onto it.
    pub fn subscribe(&self) -> async_channel::Receiver<ChangeSet> {
        let (sender, receiver) = async_channel::unbounded();
        // If the engine is gone, the sender drops here and the receiver
        // reports closed, which is the truth.
        let _ = self.messages.send(Message::Subscribe(sender));
        receiver
    }
}

/// Starts the engine on its own thread, owning `store`.
pub fn spawn(store: Store) -> io::Result<SyncHandle> {
    start(move || store, None)
}

/// [`spawn`] over the database at `path`, opened on the sync thread so the
/// caller's thread never touches the disk. A database that will not open
/// is logged and replaced by an empty one in memory, so the app still runs
/// against the server, only without a cache. Images go to the media
/// cache at `media`, when given (PLAN §6.5).
pub fn spawn_at(path: PathBuf, media: Option<PathBuf>) -> io::Result<SyncHandle> {
    let media = media.map(matty_store::media::MediaCache::new);
    start(
        move || match Store::open(&path) {
            Ok(store) => store,
            Err(error) => {
                tracing::error!(path = %path.display(), %error, "could not open the cache; using memory");
                Store::open_in_memory().expect("an in-memory database opens")
            }
        },
        media,
    )
}

/// On the web the engine runs on the page's event loop: the browser has
/// no threads to give it, and its fetch and WebSocket handles stay on the
/// page anyway.
#[cfg(target_family = "wasm")]
fn start(
    open: impl FnOnce() -> Store + 'static,
    media: Option<matty_store::media::MediaCache>,
) -> io::Result<SyncHandle> {
    let (messages, inbox) = mpsc::unbounded_channel();
    matty_api::rt::run_local(async move {
        Engine::new(open()).with_media(media).run(inbox).await;
        tracing::debug!("sync engine stopped");
    });
    Ok(SyncHandle { messages })
}

#[cfg(not(target_family = "wasm"))]
fn start(
    open: impl FnOnce() -> Store + Send + 'static,
    media: Option<matty_store::media::MediaCache>,
) -> io::Result<SyncHandle> {
    let (messages, inbox) = mpsc::unbounded_channel();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_name("matty-sync")
        .build()?;
    std::thread::Builder::new()
        .name("matty-sync".into())
        .spawn({
            let messages = messages.downgrade();
            move || {
                // Lives as long as the engine; dropped on this thread.
                let _wake = crate::wake::WakeWatch::start(messages.clone());
                runtime.spawn(crate::netwatch::watch(messages));
                runtime.block_on(Engine::new(open()).with_media(media).run(inbox));
                runtime.shutdown_timeout(SHUTDOWN_GRACE);
                tracing::debug!("sync engine stopped");
            }
        })?;
    Ok(SyncHandle { messages })
}
