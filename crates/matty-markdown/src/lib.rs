//! Mattermost Markdown flavor (CommonMark, GFM tables and strikethrough,
//! autolinks, @mentions, ~channels, :emoji:, hashtags) parsed into an AST that
//! the view renders through the gpuicn rich text view (PLAN §6.8).
//!
//! Plain Rust: this crate must never depend on GPUI.

mod ast;
mod autolink;
mod entities;
mod html;
mod parse;
mod plain;
mod source;

pub use ast::{Alignment, Block, Document, Inline, ListItem};
pub use entities::{SPECIAL_MENTIONS, link_entities};
pub use html::to_html;
pub use parse::{MAX_DEPTH, parse, parse_with};
pub use plain::to_plain;
pub use source::link_entities_in_source;
