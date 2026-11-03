//! Mattermost Markdown flavor (CommonMark, GFM tables and strikethrough,
//! autolinks, @mentions, ~channels, :emoji:, hashtags) parsed into an AST that
//! the view renders through the gpuicn rich text view (PLAN §6.8).
//!
//! Plain Rust: this crate must never depend on GPUI.

mod ast;
mod autolink;
mod html;
mod parse;

pub use ast::{Alignment, Block, Document, Inline, ListItem};
pub use html::to_html;
pub use parse::{MAX_DEPTH, parse};
