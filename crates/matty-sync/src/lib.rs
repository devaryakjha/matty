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

mod apps;
mod bookmarks;
mod bridge;
mod change;
mod commands;
mod download;
mod engine;
mod events;
pub mod io;
mod load;
#[cfg(not(target_family = "wasm"))]
mod netwatch;
mod outbox;
mod presence;
mod priority;
mod typing;
#[cfg(not(target_family = "wasm"))]
mod wake;
#[cfg(target_family = "wasm")]
mod web;

pub use apps::{AppAnswer, AppCommand, DIALOG_URL as APP_DIALOG_URL};
pub use bookmarks::{BookmarkCommand, MAX_BOOKMARKS};
pub use bridge::{SyncHandle, spawn, spawn_at, spawn_demo};
pub use change::{
    Arrived, ChangeSet, CommandError, Connection, DownloadState, ImageSize, LinkAnswer, LinkQuery,
    Listing, POSTS_WINDOW, Picture, UploadState,
};
pub use engine::Command;
/// Where an account's database lives.
pub use matty_store::default_path as database_path;
/// Whose workspace the web demo's snapshot is.
pub use matty_store::demo::DemoAccount;
/// Deletes an account's database and media cache after sign out.
pub use matty_store::forget as forget_account;
/// Where an account's media cache lives.
pub use matty_store::media::default_dir as media_dir;
pub use matty_store::outbox::Outgoing;
pub use matty_store::posts::ThreadSummary;
/// The snapshot types change sets carry, so the app needs no store crate.
pub use matty_store::sidebar::{SidebarChannel, SidebarSection, TeamWithUnreads, Unreads};
/// The key that names one account's files, and where those files live
/// for a saved key (#206).
pub use matty_store::{
    account_key, is_account_key, media::dir_for_key as media_dir_for_key,
    path_for_key as database_path_for_key,
};
