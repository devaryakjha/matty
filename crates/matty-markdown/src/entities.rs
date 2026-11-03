//! Mentions, channels, emoji and hashtags as `matty://` links, so the rich
//! text view needs no inline parser of its own (PLAN §2, G2).
//!
//! The links carry the entity as written as their label:
//!
//! | Written | Link |
//! | --- | --- |
//! | `@ana` | `matty://user/ana` |
//! | `@here`, `@channel`, `@all` | `matty://special/here` and so on |
//! | `~town-square` | `matty://channel/town-square` |
//! | `:tada:` (when the resolver knows it) | `matty://emoji/tada` |
//! | `#release` (three characters or more) | `matty://hashtag/release` |
//!
//! Text inside code, links and images is left alone.

use crate::{Block, Document, Inline};

/// The words Mattermost treats as mentions of a group of people.
pub const SPECIAL_MENTIONS: [&str; 3] = ["here", "channel", "all"];

/// Hashtags shorter than this stay text, as in Mattermost.
const MIN_HASHTAG: usize = 3;

/// Rewrites the entities in `document`'s text. `is_emoji` says whether a
/// shortcode names a system or custom emoji, so `:)`-like text and unknown
/// names stay text.
pub fn link_entities(document: &mut Document, is_emoji: &dyn Fn(&str) -> bool) {
    for block in &mut document.blocks {
        block_entities(block, is_emoji);
    }
}

fn block_entities(block: &mut Block, is_emoji: &dyn Fn(&str) -> bool) {
    match block {
        Block::Paragraph(inlines)
        | Block::Heading {
            content: inlines, ..
        } => {
            inline_entities(inlines, is_emoji);
        }
        Block::BlockQuote(blocks) => {
            for block in blocks {
                block_entities(block, is_emoji);
            }
        }
        Block::List { items, .. } => {
            for item in items {
                for block in &mut item.blocks {
                    block_entities(block, is_emoji);
                }
            }
        }
        Block::Table { header, rows, .. } => {
            for cell in header.iter_mut().chain(rows.iter_mut().flatten()) {
                inline_entities(cell, is_emoji);
            }
        }
        Block::CodeBlock { .. } | Block::ThematicBreak => {}
    }
}

fn inline_entities(inlines: &mut Vec<Inline>, is_emoji: &dyn Fn(&str) -> bool) {
    let mut out = Vec::with_capacity(inlines.len());
    for inline in inlines.drain(..) {
        match inline {
            Inline::Text(text) => split(&text, is_emoji, &mut out),
            Inline::Emphasis(mut children) => {
                inline_entities(&mut children, is_emoji);
                out.push(Inline::Emphasis(children));
            }
            Inline::Strong(mut children) => {
                inline_entities(&mut children, is_emoji);
                out.push(Inline::Strong(children));
            }
            Inline::Strikethrough(mut children) => {
                inline_entities(&mut children, is_emoji);
                out.push(Inline::Strikethrough(children));
            }
            // Code, math, links and images keep their text as it is.
            other => out.push(other),
        }
    }
    *inlines = out;
}

/// An entity found in text: where it is, and where its link goes.
struct Entity {
    start: usize,
    end: usize,
    url: String,
}

fn split(text: &str, is_emoji: &dyn Fn(&str) -> bool, out: &mut Vec<Inline>) {
    let mut plain = 0;
    let mut at = 0;
    while at < text.len() {
        let Some(c) = text[at..].chars().next() else {
            break;
        };
        let entity = match c {
            '@' => mention(text, at),
            '~' => channel(text, at),
            ':' => emoji(text, at, is_emoji),
            '#' => hashtag(text, at),
            _ => None,
        };
        match entity {
            Some(entity) => {
                if entity.start > plain {
                    out.push(Inline::Text(text[plain..entity.start].to_owned()));
                }
                out.push(Inline::Link {
                    url: entity.url,
                    title: String::new(),
                    children: vec![Inline::Text(text[entity.start..entity.end].to_owned())],
                });
                plain = entity.end;
                at = entity.end;
            }
            None => at += c.len_utf8(),
        }
    }
    if plain < text.len() {
        out.push(Inline::Text(text[plain..].to_owned()));
    }
}

/// Whether an entity may start after `before`: at the start, or after a
/// character that cannot be part of a word, a name or an address.
fn starts_word(text: &str, at: usize) -> bool {
    text[..at].chars().next_back().is_none_or(|c| {
        !(c.is_alphanumeric() || matches!(c, '_' | '.' | '-' | '@' | '~' | ':' | '#' | '/'))
    })
}

/// The end of the run of `allowed` characters from `start`.
fn run_end(text: &str, start: usize, allowed: impl Fn(char) -> bool) -> usize {
    text[start..]
        .char_indices()
        .find(|&(_, c)| !allowed(c))
        .map_or(text.len(), |(i, _)| start + i)
}

/// Drops trailing characters that end a sentence more often than a name.
fn trim_end(text: &str, start: usize, mut end: usize, trailing: &[char]) -> usize {
    while end > start && text[start..end].ends_with(trailing) {
        end -= 1;
    }
    end
}

fn mention(text: &str, at: usize) -> Option<Entity> {
    if !starts_word(text, at) {
        return None;
    }
    let name_start = at + 1;
    let end = run_end(text, name_start, |c| {
        c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')
    });
    let end = trim_end(text, name_start, end, &['.', '-', '_']);
    let name = &text[name_start..end];
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    let url = if SPECIAL_MENTIONS.contains(&lower.as_str()) {
        format!("matty://special/{lower}")
    } else {
        format!("matty://user/{lower}")
    };
    Some(Entity {
        start: at,
        end,
        url,
    })
}

fn channel(text: &str, at: usize) -> Option<Entity> {
    if !starts_word(text, at) {
        return None;
    }
    let name_start = at + 1;
    let end = run_end(text, name_start, |c| {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '_')
    });
    let end = trim_end(text, name_start, end, &['-', '_']);
    let name = &text[name_start..end];
    if name.is_empty() {
        return None;
    }
    Some(Entity {
        start: at,
        end,
        url: format!("matty://channel/{}", name.to_ascii_lowercase()),
    })
}

fn emoji(text: &str, at: usize, is_emoji: &dyn Fn(&str) -> bool) -> Option<Entity> {
    let name_start = at + 1;
    let end = run_end(text, name_start, |c| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+')
    });
    if end == name_start || !text[end..].starts_with(':') {
        return None;
    }
    let name = &text[name_start..end];
    if !is_emoji(name) {
        return None;
    }
    Some(Entity {
        start: at,
        end: end + 1,
        url: format!("matty://emoji/{name}"),
    })
}

fn hashtag(text: &str, at: usize) -> Option<Entity> {
    // Only after whitespace or at the start, so `a#b` and `C#` stay text.
    if text[..at]
        .chars()
        .next_back()
        .is_some_and(|c| !c.is_whitespace() && c != '(')
    {
        return None;
    }
    let tag_start = at + 1;
    let end = run_end(text, tag_start, |c| {
        c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
    });
    let end = trim_end(text, tag_start, end, &['.', '-', '_']);
    let tag = &text[tag_start..end];
    if tag.chars().count() < MIN_HASHTAG || !tag.starts_with(char::is_alphabetic) {
        return None;
    }
    Some(Entity {
        start: at,
        end,
        url: format!("matty://hashtag/{tag}"),
    })
}
