//! The async runtime Matty's plain-Rust crates run on: tokio on the
//! desktop, the browser's event loop on the web.
//!
//! On the web, tokio's scheduler still runs tasks (a [`tokio::task::LocalSet`]
//! that [`run_local`] drives from the browser), but tokio's timers have no
//! driver there, so every sleep, timeout and interval in Matty goes through
//! this module instead of `tokio::time`. Futures here are `Send` on both
//! targets, so code using them compiles the same way for each.

use std::{future::Future, time::Duration};

/// A point on the monotonic clock that [`sleep_until`] and [`Interval`]
/// understand. tokio's on the desktop, so paused-time tests still work.
#[cfg(not(target_family = "wasm"))]
pub use tokio::time::Instant;
/// A point on the monotonic clock that [`sleep_until`] and [`Interval`]
/// understand: `performance.now()` on the web.
#[cfg(target_family = "wasm")]
pub use web_time::Instant;

/// The wall clock, which `std::time::SystemTime` does not provide in the
/// browser.
pub use web_time::{SystemTime, UNIX_EPOCH};

/// A boxed future that is `Send` where tasks can move between threads (the
/// desktop) and not on the web, where browser handles are tied to the page.
#[cfg(not(target_family = "wasm"))]
pub type BoxFuture<'a, T> = futures_util::future::BoxFuture<'a, T>;
/// A boxed future that is `Send` where tasks can move between threads (the
/// desktop) and not on the web, where browser handles are tied to the page.
#[cfg(target_family = "wasm")]
pub type BoxFuture<'a, T> = futures_util::future::LocalBoxFuture<'a, T>;

/// `Send` on the desktop; nothing on the web. See [`BoxFuture`].
#[cfg(not(target_family = "wasm"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_family = "wasm"))]
impl<T: Send> MaybeSend for T {}
/// `Send` on the desktop; nothing on the web. See [`BoxFuture`].
#[cfg(target_family = "wasm")]
pub trait MaybeSend {}
#[cfg(target_family = "wasm")]
impl<T> MaybeSend for T {}

/// [`timeout`] ran out before the future finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Elapsed;

/// Spawns `future` on the current runtime. On the desktop it may run on any
/// thread; on the web it runs on the page's event loop, inside [`run_local`],
/// because browser fetch futures are not `Send`.
#[cfg(not(target_family = "wasm"))]
pub fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(future)
}

/// Spawns `future` on the page's event loop. Must be called from a task
/// that [`run_local`] runs.
#[cfg(target_family = "wasm")]
pub fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    tokio::task::spawn_local(future)
}

/// Runs `future` to completion on the browser's event loop, with a tokio
/// task set around it so [`spawn`] works inside. Returns at once.
#[cfg(target_family = "wasm")]
pub fn run_local(future: impl Future<Output = ()> + 'static) {
    wasm_bindgen_futures::spawn_local(async move {
        tokio::task::LocalSet::new().run_until(future).await;
    });
}

/// Resolves after `duration`.
#[cfg(not(target_family = "wasm"))]
pub fn sleep(duration: Duration) -> impl Future<Output = ()> + Send {
    tokio::time::sleep(duration)
}

/// Resolves after `duration`, on the browser's `setTimeout`. The future
/// only holds a channel, so it is `Send` like tokio's.
#[cfg(target_family = "wasm")]
pub fn sleep(duration: Duration) -> impl Future<Output = ()> + Send {
    let (done, wait) = tokio::sync::oneshot::channel::<()>();
    web::set_timeout(duration, move || {
        let _ = done.send(());
    });
    async move {
        let _ = wait.await;
    }
}

/// Resolves at `deadline`, or at once if it has passed.
pub fn sleep_until(deadline: Instant) -> impl Future<Output = ()> + Send {
    #[cfg(not(target_family = "wasm"))]
    {
        tokio::time::sleep_until(deadline)
    }
    #[cfg(target_family = "wasm")]
    {
        sleep(deadline.saturating_duration_since(Instant::now()))
    }
}

/// Runs `future` for at most `duration`.
pub async fn timeout<F: Future>(duration: Duration, future: F) -> Result<F::Output, Elapsed> {
    #[cfg(not(target_family = "wasm"))]
    {
        tokio::time::timeout(duration, future)
            .await
            .map_err(|_| Elapsed)
    }
    #[cfg(target_family = "wasm")]
    {
        use futures_util::future::{Either, select};
        let future = std::pin::pin!(future);
        let timer = std::pin::pin!(sleep(duration));
        match select(future, timer).await {
            Either::Left((output, _)) => Ok(output),
            Either::Right(((), _)) => Err(Elapsed),
        }
    }
}

/// Ticks every `period`. A tick that comes late moves the ones after it
/// back, rather than firing several at once (tokio's
/// `MissedTickBehavior::Delay`).
#[derive(Debug)]
pub struct Interval {
    #[cfg(not(target_family = "wasm"))]
    inner: tokio::time::Interval,
    #[cfg(target_family = "wasm")]
    next: Instant,
    #[cfg(target_family = "wasm")]
    period: Duration,
}

impl Interval {
    /// Waits for the next tick and returns when it was due.
    pub async fn tick(&mut self) -> Instant {
        #[cfg(not(target_family = "wasm"))]
        {
            self.inner.tick().await
        }
        #[cfg(target_family = "wasm")]
        {
            let due = self.next;
            sleep_until(due).await;
            self.next = Instant::now() + self.period;
            due
        }
    }
}

/// An [`Interval`] whose first tick is at once.
pub fn interval(period: Duration) -> Interval {
    interval_at(Instant::now(), period)
}

/// An [`Interval`] whose first tick is at `start`.
pub fn interval_at(start: Instant, period: Duration) -> Interval {
    #[cfg(not(target_family = "wasm"))]
    {
        let mut inner = tokio::time::interval_at(start, period);
        inner.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Interval { inner }
    }
    #[cfg(target_family = "wasm")]
    {
        Interval {
            next: start,
            period,
        }
    }
}

#[cfg(target_family = "wasm")]
mod web {
    use std::time::Duration;

    use wasm_bindgen::{JsCast, JsValue, closure::Closure};

    /// Calls `callback` once after `duration`, on whichever global (window
    /// or worker) this code runs in.
    pub(super) fn set_timeout(duration: Duration, callback: impl FnOnce() + 'static) {
        let callback = Closure::once_into_js(callback);
        let millis = i32::try_from(duration.as_millis()).unwrap_or(i32::MAX);
        let global = js_sys::global();
        let set_timeout = js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout"))
            .ok()
            .and_then(|value| value.dyn_into::<js_sys::Function>().ok());
        if let Some(set_timeout) = set_timeout {
            let _ = set_timeout.call2(&global, &callback, &JsValue::from(millis));
        }
    }
}

#[cfg(not(target_family = "wasm"))]
thread_local! {
    static UI_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Marks the calling thread as the one that draws the UI, so that
/// [`assert_off_ui_thread`] catches blocking work there in debug builds.
/// Call it once, from the app's main thread.
pub fn mark_ui_thread() {
    #[cfg(not(target_family = "wasm"))]
    UI_THREAD.with(|ui| ui.set(true));
}

/// Fails a debug build when `what` (fetching or reading image bytes, say)
/// runs on the thread [`mark_ui_thread`] marked (#153). Image bytes are
/// fetched and read from the media cache on sync's threads, and GPUI
/// decodes them on its background executor. On the web everything shares
/// the page's one thread, so there is nothing to check.
#[track_caller]
pub fn assert_off_ui_thread(what: &str) {
    #[cfg(not(target_family = "wasm"))]
    debug_assert!(
        !UI_THREAD.with(std::cell::Cell::get),
        "{what} ran on the UI thread"
    );
    #[cfg(target_family = "wasm")]
    let _ = what;
}

#[cfg(all(test, not(target_family = "wasm")))]
mod ui_thread_tests {
    #[test]
    fn other_threads_pass_the_check() {
        super::assert_off_ui_thread("a fetch");
        std::thread::spawn(|| {
            super::mark_ui_thread();
            let caught = std::panic::catch_unwind(|| super::assert_off_ui_thread("a fetch"));
            assert_eq!(caught.is_err(), cfg!(debug_assertions));
        })
        .join()
        .unwrap();
        // Marking one thread leaves the others alone.
        super::assert_off_ui_thread("a fetch");
    }
}
