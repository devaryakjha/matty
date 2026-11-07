//! Entities as `matty://` links in the Markdown source itself, for a view
//! that parses Markdown on its own (the gpuicn rich text view, PLAN §6.8).
//!
//! [`link_entities`](crate::link_entities) rewrites the AST; this rewrites
//! the text, so `@ana` becomes `[@ana](matty://user/ana)` and everything else
//! stays byte for byte as written. The same rules decide what is an entity:
//! text inside code, links, images and bare URLs is left alone.

use std::ops::Range;

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use crate::autolink::link_spans;
use crate::entities::find;
use crate::parse::options;

/// `text` with its mentions, channels and hashtags written as Markdown links
/// to their `matty://` URLs, and the emoji `is_emoji` knows as
/// `![:name:](emoji:name)` images, the form gpuicn's `CustomEmoji` draws.
pub fn link_entities_in_source(text: &str, is_emoji: &dyn Fn(&str) -> bool) -> String {
    link_entities_and_emoji_in_source(text, is_emoji, &|_| None)
}

/// [`link_entities_in_source`], with the emoji `glyph` knows (system emoji,
/// such as `:clown_face:`) written as their glyph instead of an image link.
/// `is_emoji` must know them too.
pub fn link_entities_and_emoji_in_source(
    text: &str,
    is_emoji: &dyn Fn(&str) -> bool,
    glyph: &dyn Fn(&str) -> Option<String>,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for run in text_runs(text) {
        let run_text = &text[run.clone()];
        let urls = link_spans(run_text);
        for entity in find(run_text, is_emoji) {
            if urls
                .iter()
                .any(|url| entity.start < url.end && url.start < entity.end)
            {
                continue;
            }
            let start = run.start + entity.start;
            let end = run.start + entity.end;
            // `\@ana`: the writer escaped it, so it stays text.
            if text[..start].ends_with('\\') {
                continue;
            }
            out.push_str(&text[copied..start]);
            let emoji = entity.url.strip_prefix("matty://emoji/");
            if let Some(glyph) = emoji.and_then(glyph) {
                out.push_str(&glyph);
                copied = end;
                continue;
            }
            if emoji.is_some() {
                out.push('!');
            }
            out.push('[');
            for c in text[start..end].chars() {
                if matches!(c, '\\' | '*' | '_' | '[' | ']' | '`') {
                    out.push('\\');
                }
                out.push(c);
            }
            out.push_str("](");
            match emoji {
                Some(name) => {
                    out.push_str("emoji:");
                    out.push_str(name);
                }
                None => out.push_str(&entity.url),
            }
            out.push(')');
            copied = end;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// The source ranges of plain text outside code, links and images, where
/// the source reads exactly as the text (no escapes or character
/// references), joining adjacent text events.
fn text_runs(text: &str) -> Vec<Range<usize>> {
    let mut runs = Vec::new();
    let mut current: Option<(Range<usize>, String)> = None;
    let mut inside = 0usize;
    let finish = |run: Option<(Range<usize>, String)>, runs: &mut Vec<Range<usize>>| {
        if let Some((range, decoded)) = run
            && text[range.clone()] == decoded
        {
            runs.push(range);
        }
    };
    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        match event {
            Event::Text(decoded) if inside == 0 => match &mut current {
                Some((run, joined)) if run.end == range.start => {
                    run.end = range.end;
                    joined.push_str(&decoded);
                }
                _ => {
                    finish(current.take(), &mut runs);
                    current = Some((range, decoded.into_string()));
                }
            },
            other => {
                finish(current.take(), &mut runs);
                match other {
                    Event::Start(Tag::Link { .. } | Tag::Image { .. } | Tag::CodeBlock(_)) => {
                        inside += 1;
                    }
                    Event::End(TagEnd::Link | TagEnd::Image | TagEnd::CodeBlock) => {
                        inside = inside.saturating_sub(1);
                    }
                    _ => {}
                }
            }
        }
    }
    finish(current.take(), &mut runs);
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(text: &str) -> String {
        link_entities_in_source(text, &|name| name == "tada")
    }

    #[test]
    fn entities_become_links() {
        assert_eq!(
            rewrite("hi @ana, see ~town-square and #release :tada:"),
            "hi [@ana](matty://user/ana), see [~town-square](matty://channel/town-square) \
             and [#release](matty://hashtag/release) ![:tada:](emoji:tada)"
        );
    }

    #[test]
    fn system_emoji_become_glyphs() {
        let glyph = |name: &str| match name {
            "clown_face" => Some("🤡".to_owned()),
            "+1_medium_skin_tone" => Some("👍🏽".to_owned()),
            _ => None,
        };
        let known = |name: &str| glyph(name).is_some() || name == "tada";
        assert_eq!(
            link_entities_and_emoji_in_source(
                "lol :clown_face: :+1_medium_skin_tone: :tada: :nope: `:clown_face:`",
                &known,
                &glyph
            ),
            "lol 🤡 👍🏽 ![:tada:](emoji:tada) :nope: `:clown_face:`"
        );
    }

    #[test]
    fn specials_and_markup_around_entities() {
        assert_eq!(
            rewrite("**@here** _~dev_"),
            "**[@here](matty://special/here)** _[~dev](matty://channel/dev)_"
        );
    }

    #[test]
    fn code_links_and_urls_are_left_alone() {
        for text in [
            "`@ana`",
            "```\n@ana ~dev\n```",
            "[@ana](https://example.com)",
            "see https://example.com/~dev/#section ok",
            "mail ana@example.com",
        ] {
            assert_eq!(rewrite(text), text);
        }
    }

    #[test]
    fn labels_are_escaped() {
        assert_eq!(
            rewrite("@first_last"),
            r"[@first\_last](matty://user/first_last)"
        );
    }

    #[test]
    fn escaped_text_is_kept() {
        assert_eq!(rewrite(r"\@ana \#tag"), r"\@ana \#tag");
    }

    #[test]
    fn tables_and_lists() {
        assert_eq!(
            rewrite("- @ana\n\n| a |\n| - |\n| ~dev |"),
            "- [@ana](matty://user/ana)\n\n| a |\n| - |\n| [~dev](matty://channel/dev) |"
        );
    }
}
