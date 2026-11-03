//! The sync engine: merges REST responses and WebSocket events into the store
//! and publishes immutable snapshots and typed change sets for the views.
//!
//! It is the only writer to `matty-store` (PLAN §6.4). Plain Rust: this crate
//! must never depend on GPUI.
//!
//! The engine runs on its own thread with a single-threaded tokio runtime
//! (option A of PLAN §6.2). The app holds a [`SyncHandle`]: it sends
//! [`Command`]s and receives [`ChangeSet`]s over channels that work on any
//! executor, so the UI never touches the runtime, the database, or a lock the
//! sync thread holds.

mod bridge;
mod change;
mod engine;
mod events;
mod load;
mod outbox;

pub use bridge::{SyncHandle, spawn};
pub use change::{ChangeSet, CommandError, Connection, POSTS_WINDOW};
pub use engine::Command;
pub use matty_store::outbox::Outgoing;
