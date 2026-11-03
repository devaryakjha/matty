//! Network work that comes before a session exists, such as checking a
//! server and signing in, on a tokio runtime of its own. The app awaits
//! the answer on its own executor and never needs tokio.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

use tokio::sync::mpsc;

type Task = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Runs `future` on the I/O thread, started on first use. The receiver
/// yields its output once; it reports closed instead if the thread could
/// not start.
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

fn tasks() -> Option<&'static mpsc::UnboundedSender<Task>> {
    static TASKS: OnceLock<Option<mpsc::UnboundedSender<Task>>> = OnceLock::new();
    TASKS.get_or_init(start).as_ref()
}

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
                    tokio::spawn(task);
                }
            });
        })
        .inspect_err(|error| tracing::error!(%error, "could not start the I/O thread"))
        .ok()?;
    Some(sender)
}
