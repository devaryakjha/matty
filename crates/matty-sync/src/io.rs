//! Network work that comes before a session exists, such as checking a
//! server and signing in, on a tokio runtime of its own. The app awaits
//! the answer on its own executor and never needs tokio.

use std::future::Future;
#[cfg(not(target_family = "wasm"))]
use std::pin::Pin;
#[cfg(not(target_family = "wasm"))]
use std::sync::OnceLock;

#[cfg(not(target_family = "wasm"))]
use tokio::sync::mpsc;

#[cfg(not(target_family = "wasm"))]
type Task = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Runs `future` on the I/O thread, started on first use. The receiver
/// yields its output once; it reports closed instead if the thread could
/// not start.
#[cfg(target_family = "wasm")]
pub fn run<F>(future: F) -> async_channel::Receiver<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    // The page's event loop is the I/O "thread" on the web.
    let (sender, receiver) = async_channel::bounded(1);
    matty_api::rt::run_local(async move {
        let _ = sender.send(future.await).await;
    });
    receiver
}

/// See the web `run`.
#[cfg(not(target_family = "wasm"))]
pub fn run<F>(future: F) -> async_channel::Receiver<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let (sender, receiver) = async_channel::bounded(1);
    let task: Task = Box::pin(async move {
        let _ = sender.send(future.await).await;
    });
    if let Some(tasks) = tasks() {
        let _ = tasks.send(task);
    }
    receiver
}

#[cfg(not(target_family = "wasm"))]
fn tasks() -> Option<&'static mpsc::UnboundedSender<Task>> {
    static TASKS: OnceLock<Option<mpsc::UnboundedSender<Task>>> = OnceLock::new();
    TASKS.get_or_init(start).as_ref()
}

#[cfg(not(target_family = "wasm"))]
fn start() -> Option<mpsc::UnboundedSender<Task>> {
    let (sender, mut tasks) = mpsc::unbounded_channel::<Task>();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_name("matty-io")
        .build()
        .inspect_err(|error| tracing::error!(%error, "could not start the I/O runtime"))
        .ok()?;
    std::thread::Builder::new()
        .name("matty-io".into())
        .spawn(move || {
            runtime.block_on(async move {
                while let Some(task) = tasks.recv().await {
                    matty_api::rt::spawn(task);
                }
            });
        })
        .inspect_err(|error| tracing::error!(%error, "could not start the I/O thread"))
        .ok()?;
    Some(sender)
}
