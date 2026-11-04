//! A plain-text rendering of a [`Document`], for "Copy text": the words
//! without the markup, one blank line between blocks.

use crate::{Block, Document, Inline, ListItem};

/// Renders `document` as plain text: formatting dropped, links reduced to
/// their labels, code kept as written, list items one per line.
pub fn to_plain(document: &Document) -> String {
    let mut out = String::new();
    for block in &document.blocks {
        block_plain(block, &mut out);
    }
    let trimmed = out.trim_end();
    out.truncate(trimmed.len());
    out
}

fn block_plain(block: &Block, out: &mut String) {
    match block {
        Block::Paragraph(inlines)
        | Block::Heading {
            content: inlines, ..
        } => {
            inlines_plain(inlines, out);
            out.push_str("\n\n");
        }
        Block::CodeBlock { code, .. } => {
            out.push_str(code);
            if !code.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        Block::BlockQuote(blocks) => {
            for block in blocks {
                block_plain(block, out);
            }
        }
        Block::List { start, items } => {
            for (index, item) in items.iter().enumerate() {
                match start {
                    Some(first) => {
                        out.push_str(&format!("{}. ", first + index as u64));
                    }
                    None => out.push_str("- "),
                }
                item_plain(item, out);
            }
            out.push('\n');
        }
        Block::Table { header, rows, .. } => {
            for cells in std::iter::once(header).chain(rows) {
                for (index, cell) in cells.iter().enumerate() {
                    if index > 0 {
                        out.push_str(" | ");
                    }
                    inlines_plain(cell, out);
                }
                out.push('\n');
            }
            out.push('\n');
        }
        Block::ThematicBreak => out.push_str("---\n\n"),
    }
}

fn item_plain(item: &ListItem, out: &mut String) {
    match item.checked {
        Some(true) => out.push_str("[x] "),
        Some(false) => out.push_str("[ ] "),
        None => {}
    }
    match item.blocks.as_slice() {
        [Block::Paragraph(inlines)] => {
            inlines_plain(inlines, out);
            out.push('\n');
        }
        blocks => {
            let mut inner = String::new();
            for block in blocks {
                block_plain(block, &mut inner);
            }
            out.push_str(inner.trim_end());
            out.push('\n');
        }
    }
}

fn inlines_plain(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(text) | Inline::Code(text) | Inline::Math(text) => out.push_str(text),
            Inline::Emphasis(children)
            | Inline::Strong(children)
            | Inline::Strikethrough(children) => inlines_plain(children, out),
            Inline::Link { url, children, .. } => {
                if children.is_empty() {
                    out.push_str(url);
                } else {
                    inlines_plain(children, out);
                }
            }
            Inline::Image { url, alt, .. } => out.push_str(if alt.is_empty() { url } else { alt }),
            Inline::SoftBreak | Inline::HardBreak => out.push('\n'),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::to_plain;
    use crate::parse;

    #[test]
    fn drops_markup_and_keeps_structure() {
        let text = "Hello **world**, see [the doc](https://x.test/d).\n\n- one\n- [x] two\n\n```rust\nlet a = 1;\n```\n\n> quoted *text*";
        assert_eq!(
            to_plain(&parse(text)),
            "Hello world, see the doc.\n\n- one\n- [x] two\n\nlet a = 1;\n\nquoted text"
        );
    }

    #[test]
    fn numbered_lists_count_from_their_start() {
        assert_eq!(to_plain(&parse("3. c\n4. d")), "3. c\n4. d");
    }
}
