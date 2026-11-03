//! A canonical HTML-like rendering of a [`Document`], for tests and logs.
//! It is not meant for display: Matty renders the tree itself.

use std::fmt::Write;

use crate::{Alignment, Block, Document, Inline};

/// Renders `document` as HTML-like text: one tag per node, text escaped.
pub fn to_html(document: &Document) -> String {
    let mut out = String::new();
    for block in &document.blocks {
        block_html(block, &mut out);
    }
    out
}

fn block_html(block: &Block, out: &mut String) {
    match block {
        Block::Paragraph(inlines) => {
            out.push_str("<p>");
            inlines_html(inlines, out);
            out.push_str("</p>\n");
        }
        Block::Heading { level, content } => {
            let _ = write!(out, "<h{level}>");
            inlines_html(content, out);
            let _ = writeln!(out, "</h{level}>");
        }
        Block::CodeBlock { language, code } => {
            match language {
                Some(language) => {
                    let _ = write!(out, "<pre lang=\"{}\">", escape(language));
                }
                None => out.push_str("<pre>"),
            }
            out.push_str(&escape(code));
            out.push_str("</pre>\n");
        }
        Block::BlockQuote(blocks) => {
            out.push_str("<blockquote>\n");
            for block in blocks {
                block_html(block, out);
            }
            out.push_str("</blockquote>\n");
        }
        Block::List { start, items } => {
            match start {
                Some(1) => out.push_str("<ol>\n"),
                Some(n) => {
                    let _ = writeln!(out, "<ol start=\"{n}\">");
                }
                None => out.push_str("<ul>\n"),
            }
            for item in items {
                match item.checked {
                    Some(true) => out.push_str("<li checked>"),
                    Some(false) => out.push_str("<li unchecked>"),
                    None => out.push_str("<li>"),
                }
                // A one-paragraph item prints inline, as tight lists do.
                match item.blocks.as_slice() {
                    [Block::Paragraph(inlines)] => inlines_html(inlines, out),
                    blocks => {
                        out.push('\n');
                        for block in blocks {
                            block_html(block, out);
                        }
                    }
                }
                out.push_str("</li>\n");
            }
            out.push_str(if start.is_some() {
                "</ol>\n"
            } else {
                "</ul>\n"
            });
        }
        Block::Table {
            alignments,
            header,
            rows,
        } => {
            out.push_str("<table>\n");
            row_html("th", alignments, header, out);
            for row in rows {
                row_html("td", alignments, row, out);
            }
            out.push_str("</table>\n");
        }
        Block::ThematicBreak => out.push_str("<hr>\n"),
    }
}

fn row_html(cell: &str, alignments: &[Alignment], cells: &[Vec<Inline>], out: &mut String) {
    out.push_str("<tr>");
    for (i, content) in cells.iter().enumerate() {
        match alignments.get(i).copied().unwrap_or_default() {
            Alignment::None => {
                let _ = write!(out, "<{cell}>");
            }
            Alignment::Left => {
                let _ = write!(out, "<{cell} align=\"left\">");
            }
            Alignment::Center => {
                let _ = write!(out, "<{cell} align=\"center\">");
            }
            Alignment::Right => {
                let _ = write!(out, "<{cell} align=\"right\">");
            }
        }
        inlines_html(content, out);
        let _ = write!(out, "</{cell}>");
    }
    out.push_str("</tr>\n");
}

fn inlines_html(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(text) => out.push_str(&escape(text)),
            Inline::Code(code) => {
                let _ = write!(out, "<code>{}</code>", escape(code));
            }
            Inline::Math(math) => {
                let _ = write!(out, "<math>{}</math>", escape(math));
            }
            Inline::Emphasis(children) => wrap("em", children, out),
            Inline::Strong(children) => wrap("strong", children, out),
            Inline::Strikethrough(children) => wrap("del", children, out),
            Inline::Link {
                url,
                title,
                children,
            } => {
                let _ = write!(out, "<a href=\"{}\"", escape(url));
                if !title.is_empty() {
                    let _ = write!(out, " title=\"{}\"", escape(title));
                }
                out.push('>');
                inlines_html(children, out);
                out.push_str("</a>");
            }
            Inline::Image { url, title, alt } => {
                let _ = write!(out, "<img src=\"{}\" alt=\"{}\"", escape(url), escape(alt));
                if !title.is_empty() {
                    let _ = write!(out, " title=\"{}\"", escape(title));
                }
                out.push('>');
            }
            Inline::SoftBreak => out.push('\n'),
            Inline::HardBreak => out.push_str("<br>\n"),
        }
    }
}

fn wrap(tag: &str, children: &[Inline], out: &mut String) {
    let _ = write!(out, "<{tag}>");
    inlines_html(children, out);
    let _ = write!(out, "</{tag}>");
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}
