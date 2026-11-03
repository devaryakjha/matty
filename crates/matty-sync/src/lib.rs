//! The sync engine: merges REST responses and WebSocket events into the store
//! and publishes immutable snapshots and typed change sets for the views.
//!
//! It is the only writer to `matty-store` (PLAN §6.4). Plain Rust: this crate
//! must never depend on GPUI.
