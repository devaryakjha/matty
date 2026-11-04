//! Typed client for the Mattermost REST API v4 and WebSocket.
//!
//! Owns HTTP, authentication headers, error mapping, rate limits and the
//! WebSocket event stream. Plain Rust: this crate must never depend on GPUI
//! (PLAN §6.1), which CI checks with `scripts/check-no-gpui.sh`.
//!
//! The client is async and runs on tokio (spike S2, option A): Matty drives it
//! from one background runtime and hands results to the UI through channels.

pub mod capabilities;
mod client;
mod endpoints;
pub(crate) mod error;
pub mod models;
pub mod rt;
#[cfg(not(target_family = "wasm"))]
mod tls;
pub mod ws;

pub use capabilities::{ServerCapabilities, UnsupportedVersion, Version};
pub use client::{
    Client, ClientBuilder, Fetched, MAX_OUTSIDE_IMAGE, RateLimit, Sleeper, TokioSleeper,
};
pub use endpoints::browse::{ChannelPatch, NewChannel, SearchQuery};
pub use endpoints::files::{Progress, Upload, Uploaded};
pub use endpoints::posts::{Direction, NewPost, PostPatch, PostsQuery, ThreadQuery, ThreadsQuery};
pub use endpoints::users::{Credentials, MFA_BAD_CODE_ID, MFA_REQUIRED_ID, SignedIn};
pub use error::ApiError;
