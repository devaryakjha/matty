//! Mattermost Markdown flavor (CommonMark, GFM tables and strikethrough,
//! autolinks, @mentions, ~channels, :emoji:, hashtags) parsed into an AST that
//! the view renders through the gpuicn rich text view (PLAN §6.8).
//!
//! Plain Rust: this crate must never depend on GPUI.
