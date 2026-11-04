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
mod commands;
mod download;
mod engine;
mod events;
pub mod io;
mod load;
mod outbox;
mod typing;

pub use bridge::{SyncHandle, spawn, spawn_at};
pub use change::{
    Arrived, ChangeSet, CommandError, Connection, DownloadState, ImageSize, POSTS_WINDOW,
    UploadState,
};
pub use engine::Command;
/// Where an account's database lives.
pub use matty_store::default_path as database_path;
/// Deletes an account's database and media cache after sign out.
pub use matty_store::forget as forget_account;
/// Where an account's media cache lives.
pub use matty_store::media::default_dir as media_dir;
pub use matty_store::outbox::Outgoing;
/// The snapshot types change sets carry, so the app needs no store crate.
pub use matty_store::sidebar::{SidebarChannel, SidebarSection, TeamWithUnreads, Unreads};
