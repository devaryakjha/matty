//! Mattermost Markdown into a [`Document`], on top of `pulldown-cmark`.

use pulldown_cmark::{
    Alignment as CmAlignment, CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag,
    TagEnd,
};

use crate::autolink::autolink;
use crate::{Alignment, Block, Document, Inline, ListItem};

/// How deep containers nest before further ones are flattened into their
/// parent, so hostile input cannot build a tree too deep to walk or drop.
pub const MAX_DEPTH: usize = 32;

/// Parses a message, with mentions, channels and hashtags as `matty://`
/// links; no shortcode counts as an emoji. See [`parse_with`].
pub fn parse(text: &str) -> Document {
    parse_with(text, &|_| false)
}

/// Parses a message, with entities as `matty://` links
/// ([`link_entities`](crate::link_entities)). `is_emoji` says whether a
/// shortcode names a system or custom emoji.
pub fn parse_with(text: &str, is_emoji: &dyn Fn(&str) -> bool) -> Document {
    let mut builder = Builder::default();
    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        builder.event(event, &text[range]);
    }
    let mut document = builder.finish();
    crate::link_entities(&mut document, is_emoji);
    document
}

/// The Markdown extensions Mattermost turns on.
pub(crate) fn options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_MATH);
    options
}

#[derive(Debug)]
enum Kind {
    Root,
    Paragraph,
    Heading(u8),
    BlockQuote,
    CodeBlock(Option<String>),
    List(Option<u64>),
    Item,
    Table(Vec<Alignment>),
    TableHead,
    TableRow,
    TableCell,
    Emphasis,
    Strong,
    Strikethrough,
    /// `~text~`: GFM strikes it, Mattermost does not.
    SingleTilde,
    Link {
        url: String,
        title: String,
    },
    Image {
        url: String,
        title: String,
    },
    /// A container past [`MAX_DEPTH`]: its content joins the parent's.
    Flattened,
    /// Something Mattermost shows as text, such as a footnote definition.
    Transparent,
}

#[derive(Debug)]
struct Frame {
    kind: Kind,
    blocks: Vec<Block>,
    inlines: Vec<Inline>,
    /// Code, or an image's alt text.
    text: String,
    items: Vec<ListItem>,
    checked: Option<bool>,
    cells: Vec<Vec<Inline>>,
    header: Vec<Vec<Inline>>,
    rows: Vec<Vec<Vec<Inline>>>,
}

impl Frame {
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            blocks: Vec::new(),
            inlines: Vec::new(),
            text: String::new(),
            items: Vec::new(),
            checked: None,
            cells: Vec::new(),
            header: Vec::new(),
            rows: Vec::new(),
        }
    }

    /// Loose inline content (a tight list item's text) becomes a paragraph
    /// before a block joins it.
    fn flush_inlines(&mut self) {
        if !self.inlines.is_empty() {
            let inlines = finish_inlines(std::mem::take(&mut self.inlines));
            self.blocks.push(Block::Paragraph(inlines));
        }
    }

    fn push_block(&mut self, block: Block) {
        self.flush_inlines();
        self.blocks.push(block);
    }

    fn push_text(&mut self, text: &str) {
        if let Some(Inline::Text(last)) = self.inlines.last_mut() {
            last.push_str(text);
        } else {
            self.inlines.push(Inline::Text(text.to_owned()));
        }
    }
}

/// Autolinks the text runs of finished inline content.
fn finish_inlines(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out = Vec::with_capacity(inlines.len());
    for inline in inlines {
        match inline {
            Inline::Text(text) => autolink(&text, &mut out),
            other => out.push(other),
        }
    }
    out
}

struct Builder {
    stack: Vec<Frame>,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            stack: vec![Frame::new(Kind::Root)],
        }
    }
}

impl Builder {
    fn top(&mut self) -> &mut Frame {
        // The root frame is never popped.
        let last = self.stack.len() - 1;
        &mut self.stack[last]
    }

    /// Inside a link or image, where text is never autolinked again.
    fn in_link(&self) -> bool {
        self.stack
            .iter()
            .any(|f| matches!(f.kind, Kind::Link { .. } | Kind::Image { .. }))
    }

    fn in_image(&self) -> bool {
        self.stack
            .iter()
            .any(|f| matches!(f.kind, Kind::Image { .. }))
    }

    fn event(&mut self, event: Event<'_>, source: &str) {
        match event {
            Event::Start(tag) => self.start(tag, source),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if self.in_image() {
                    self.image_alt(&text);
                } else if let Kind::CodeBlock(_) = self.top().kind {
                    self.top().text.push_str(&text);
                } else {
                    self.top().push_text(&text);
                }
            }
            Event::Code(code) => self.inline(Inline::Code(code.into_string())),
            Event::InlineMath(math) => self.inline(Inline::Math(math.into_string())),
            Event::DisplayMath(math) => {
                let math = math.into_string();
                let alone = matches!(self.top().kind, Kind::Paragraph)
                    && self.top().inlines.is_empty()
                    && source.trim_start().starts_with("$$");
                if alone {
                    // Closed by the paragraph's end; see `end`.
                    let code = math.trim_matches('\n').to_owned() + "\n";
                    self.top().blocks.push(Block::CodeBlock {
                        language: Some("latex".into()),
                        code,
                    });
                } else {
                    self.inline(Inline::Math(math));
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                if self.in_image() {
                    self.image_alt(&html);
                } else if let Kind::CodeBlock(_) = self.top().kind {
                    self.top().text.push_str(&html);
                } else {
                    self.top().push_text(&html);
                }
            }
            Event::FootnoteReference(label) => self.top().push_text(&format!("[^{label}]")),
            Event::SoftBreak => {
                if self.in_image() {
                    self.image_alt(" ");
                } else {
                    self.inline(Inline::SoftBreak);
                }
            }
            Event::HardBreak => {
                if self.in_image() {
                    self.image_alt(" ");
                } else {
                    self.inline(Inline::HardBreak);
                }
            }
            Event::Rule => self.top().push_block(Block::ThematicBreak),
            Event::TaskListMarker(checked) => {
                if let Some(item) = self
                    .stack
                    .iter_mut()
                    .rev()
                    .find(|f| matches!(f.kind, Kind::Item))
                {
                    item.checked = Some(checked);
                }
            }
        }
    }

    fn image_alt(&mut self, text: &str) {
        if let Some(image) = self
            .stack
            .iter_mut()
            .rev()
            .find(|f| matches!(f.kind, Kind::Image { .. }))
        {
            image.text.push_str(text);
        }
    }

    fn inline(&mut self, inline: Inline) {
        if self.in_image() {
            if let Inline::Code(text) | Inline::Math(text) = &inline {
                let text = text.clone();
                self.image_alt(&text);
            }
            return;
        }
        self.top().inlines.push(inline);
    }

    fn start(&mut self, tag: Tag<'_>, source: &str) {
        let kind = match tag {
            Tag::Paragraph => Kind::Paragraph,
            Tag::Heading { level, .. } => Kind::Heading(heading_level(level)),
            Tag::BlockQuote(_) => Kind::BlockQuote,
            Tag::CodeBlock(CodeBlockKind::Fenced(info)) => {
                let language = info.split_whitespace().next().map(|word| {
                    if matches!(word, "tex" | "latex") {
                        "latex".to_owned()
                    } else {
                        word.to_owned()
                    }
                });
                Kind::CodeBlock(language)
            }
            Tag::CodeBlock(CodeBlockKind::Indented) => Kind::CodeBlock(None),
            Tag::List(start) => Kind::List(start),
            Tag::Item => Kind::Item,
            Tag::Table(alignments) => Kind::Table(alignments.into_iter().map(alignment).collect()),
            Tag::TableHead => Kind::TableHead,
            Tag::TableRow => Kind::TableRow,
            Tag::TableCell => Kind::TableCell,
            Tag::Emphasis => Kind::Emphasis,
            Tag::Strong => Kind::Strong,
            Tag::Strikethrough if !source.starts_with("~~") => Kind::SingleTilde,
            Tag::Strikethrough => Kind::Strikethrough,
            Tag::Link {
                link_type,
                dest_url,
                title,
                ..
            } => Kind::Link {
                url: link_url(link_type, &dest_url),
                title: title.into_string(),
            },
            Tag::Image {
                dest_url, title, ..
            } => Kind::Image {
                url: dest_url.into_string(),
                title: title.into_string(),
            },
            _ => Kind::Transparent,
        };
        let structural = matches!(
            kind,
            Kind::TableHead | Kind::TableRow | Kind::TableCell | Kind::Item
        );
        let kind =
            if self.stack.len() > MAX_DEPTH && !structural && !matches!(kind, Kind::CodeBlock(_)) {
                Kind::Flattened
            } else {
                kind
            };
        self.stack.push(Frame::new(kind));
    }

    fn end(&mut self, _tag: TagEnd) {
        if self.stack.len() == 1 {
            return;
        }
        let Some(mut frame) = self.stack.pop() else {
            return;
        };
        let in_link = self.in_link();
        let finish = |inlines: Vec<Inline>| {
            if in_link {
                inlines
            } else {
                finish_inlines(inlines)
            }
        };
        let parent = self.top();
        match frame.kind {
            Kind::Root => {}
            Kind::Paragraph => {
                // A `$$` block came out as a code block in the paragraph.
                for block in std::mem::take(&mut frame.blocks) {
                    parent.push_block(block);
                }
                if !frame.inlines.is_empty() {
                    parent.push_block(Block::Paragraph(finish(frame.inlines)));
                }
            }
            Kind::Heading(level) => parent.push_block(Block::Heading {
                level,
                content: finish(frame.inlines),
            }),
            Kind::BlockQuote => {
                frame.flush_inlines();
                parent.push_block(Block::BlockQuote(frame.blocks));
            }
            Kind::CodeBlock(language) => parent.push_block(Block::CodeBlock {
                language,
                code: frame.text,
            }),
            Kind::List(start) => parent.push_block(Block::List {
                start,
                items: frame.items,
            }),
            Kind::Item => {
                frame.flush_inlines();
                let item = ListItem {
                    checked: frame.checked,
                    blocks: frame.blocks,
                };
                if matches!(parent.kind, Kind::List(_)) {
                    parent.items.push(item);
                } else {
                    for block in item.blocks {
                        parent.push_block(block);
                    }
                }
            }
            Kind::Table(alignments) => parent.push_block(Block::Table {
                alignments,
                header: frame.header,
                rows: frame.rows,
            }),
            Kind::TableHead => {
                // Header cells sit straight in the head, with no row.
                parent.header = frame.cells;
            }
            Kind::TableRow => parent.rows.push(frame.cells),
            Kind::TableCell => parent.cells.push(finish(frame.inlines)),
            Kind::Emphasis => parent.inlines.push(Inline::Emphasis(finish(frame.inlines))),
            Kind::Strong => parent.inlines.push(Inline::Strong(finish(frame.inlines))),
            Kind::Strikethrough => parent
                .inlines
                .push(Inline::Strikethrough(finish(frame.inlines))),
            Kind::SingleTilde => {
                parent.push_text("~");
                for inline in frame.inlines {
                    match inline {
                        Inline::Text(text) => parent.push_text(&text),
                        other => parent.inlines.push(other),
                    }
                }
                parent.push_text("~");
            }
            Kind::Link { url, title } => parent.inlines.push(Inline::Link {
                url,
                title,
                children: frame.inlines,
            }),
            Kind::Image { url, title } => {
                if parent_is_image(parent) {
                    parent.text.push_str(&frame.text);
                } else {
                    parent.inlines.push(Inline::Image {
                        url,
                        title,
                        alt: frame.text,
                    });
                }
            }
            Kind::Flattened | Kind::Transparent => {
                frame.flush_inlines();
                for block in frame.blocks {
                    parent.push_block(block);
                }
                parent.items.append(&mut frame.items);
                for inline in frame.inlines {
                    match inline {
                        Inline::Text(text) => parent.push_text(&text),
                        other => parent.inlines.push(other),
                    }
                }
            }
        }
    }

    fn finish(mut self) -> Document {
        // Unclosed frames cannot happen with pulldown-cmark, but close any
        // left so nothing is lost.
        while self.stack.len() > 1 {
            self.end(TagEnd::Paragraph);
        }
        let mut root = self.stack.pop().unwrap_or_else(|| Frame::new(Kind::Root));
        root.flush_inlines();
        Document {
            blocks: root.blocks,
        }
    }
}

/// A written link's target as Mattermost opens it: `<a@b.c>` mails, and a
/// bare domain such as `example.com/x` gets `http://`.
fn link_url(link_type: LinkType, url: &str) -> String {
    if link_type == LinkType::Email {
        return format!("mailto:{url}");
    }
    let has_scheme = url.find(':').is_some_and(|colon| {
        url[..colon]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            && colon > 0
    });
    let first = url.split(['/', '?', '#']).next().unwrap_or("");
    let looks_like_domain = first.contains('.')
        && !first.starts_with('.')
        && first
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-'));
    if !has_scheme && looks_like_domain {
        format!("http://{url}")
    } else {
        url.to_owned()
    }
}

fn parent_is_image(frame: &Frame) -> bool {
    matches!(frame.kind, Kind::Image { .. })
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn alignment(alignment: CmAlignment) -> Alignment {
    match alignment {
        CmAlignment::None => Alignment::None,
        CmAlignment::Left => Alignment::Left,
        CmAlignment::Center => Alignment::Center,
        CmAlignment::Right => Alignment::Right,
    }
}
