//! Typed client for the Mattermost REST API v4 and WebSocket.
//!
//! Owns HTTP, authentication headers, error mapping, rate limits and the
//! WebSocket event stream. Plain Rust: this crate must never depend on GPUI
//! (PLAN §6.1), which CI checks with `scripts/check-no-gpui.sh`.
