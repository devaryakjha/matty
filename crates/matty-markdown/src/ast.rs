//! The document tree the view renders.

/// A parsed message: its blocks in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    /// Top-level blocks.
    pub blocks: Vec<Block>,
}

/// A block-level element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// A paragraph of inline content.
    Paragraph(Vec<Inline>),
    /// A heading, level 1 to 6.
    Heading {
        /// 1 to 6.
        level: u8,
        /// Its text.
        content: Vec<Inline>,
    },
    /// Fenced or indented code. LaTeX (```` ```latex ````, ```` ```tex ````
    /// and `$$` blocks) arrives here with the language `latex` until there
    /// is a renderer.
    CodeBlock {
        /// The info string's first word, if any.
        language: Option<String>,
        /// The code, with its final newline.
        code: String,
    },
    /// A `>` quote.
    BlockQuote(Vec<Block>),
    /// A bulleted or numbered list.
    List {
        /// The first number of a numbered list; `None` for bullets.
        start: Option<u64>,
        /// Its items.
        items: Vec<ListItem>,
    },
    /// A table with a header row.
    Table {
        /// Each column's alignment.
        alignments: Vec<Alignment>,
        /// Header cells.
        header: Vec<Vec<Inline>>,
        /// Body rows of cells.
        rows: Vec<Vec<Vec<Inline>>>,
    },
    /// A `---` rule.
    ThematicBreak,
}

/// One list item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListItem {
    /// For a task list item (`- [ ]` or `- [x]`), whether it is checked.
    pub checked: Option<bool>,
    /// Its content. A tight list's text is a paragraph here too.
    pub blocks: Vec<Block>,
}

/// A table column's alignment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Alignment {
    /// No colon in the delimiter row.
    #[default]
    None,
    /// `:--`.
    Left,
    /// `:-:`.
    Center,
    /// `--:`.
    Right,
}

/// An inline element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    /// Plain text. Raw HTML arrives here too: Mattermost shows it as text.
    Text(String),
    /// `` `code` ``.
    Code(String),
    /// Inline LaTeX (`$x^2$`), shown as code until there is a renderer.
    Math(String),
    /// `*emphasis*`.
    Emphasis(Vec<Inline>),
    /// `**strong**`.
    Strong(Vec<Inline>),
    /// `~~strikethrough~~`.
    Strikethrough(Vec<Inline>),
    /// A link: written, autolinked from a bare URL or email, or a
    /// `matty://` entity link.
    Link {
        /// Where it goes.
        url: String,
        /// Its title, or empty.
        title: String,
        /// Its label.
        children: Vec<Inline>,
    },
    /// An image.
    Image {
        /// Its source.
        url: String,
        /// Its title, or empty.
        title: String,
        /// Its alternative text.
        alt: String,
    },
    /// A line break inside a paragraph that is not a hard break.
    SoftBreak,
    /// A hard line break (two spaces or a backslash at the end of a line).
    HardBreak,
}
