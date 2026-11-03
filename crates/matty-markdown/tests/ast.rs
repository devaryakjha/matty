//! Snapshots of the tree for each block and inline type, in the canonical
//! HTML-like form (`to_html`). Review changes with `cargo insta review`, or
//! run with `INSTA_UPDATE=always` and read the diff.

use matty_markdown::{Block, Inline, parse, to_html};

/// The input, then its rendering, so a snapshot reads on its own.
fn render(input: &str) -> String {
    format!("{input}\n----\n{}", to_html(&parse(input)))
}

macro_rules! snapshot {
    ($name:ident, $input:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!(render($input));
        }
    };
}

snapshot!(
    paragraphs_and_breaks,
    "one\ntwo  \nthree\\\nfour\n\nnext paragraph"
);
snapshot!(
    headings,
    "# One\n## Two\n### Three\n#### Four\n##### Five\n###### Six\nSetext\n==="
);
snapshot!(
    fenced_code_with_language,
    "```rust\nfn main() {}\n```\n\n```\nplain <b>\n```"
);
snapshot!(indented_code, "    indented\n    code");
snapshot!(
    latex_blocks,
    "```latex\nx^2\n```\n\n```tex\ny\n```\n\n$$\n\\frac{a}{b}\n$$"
);
snapshot!(inline_latex, "the area is $\\pi r^2$ and costs $5 and $10");
snapshot!(blockquotes, "> quoted\n> > nested\n\n> again\nlazy line");
snapshot!(
    bullet_lists_nest_with_two_spaces,
    "- one\n  - two\n    - three\n- four"
);
snapshot!(numbered_lists, "3. three\n4. four\n\n1. one");
snapshot!(loose_list, "- a\n\n  second paragraph\n- b");
snapshot!(task_list, "- [ ] todo\n- [x] done");
snapshot!(
    tables_with_alignment_and_pipes,
    "| left | center | right | none |\n|:--|:-:|--:|---|\n| `a\\|b` | **2** | 3 | a \\| b |"
);
snapshot!(
    emphasis_strong_strike,
    "*em* _em_ **strong** __strong__ ~~struck~~ ***both***"
);
snapshot!(
    single_tildes_are_not_strikethrough,
    "~town-square and ~off-topic~ stay text"
);
snapshot!(inline_code, "use `cargo test` and ``a ` b``");
snapshot!(
    links_and_images,
    "[docs](https://docs.example.com \"Docs\") ![logo *x*](https://example.com/l.png) <https://angle.example.com>"
);
snapshot!(
    bare_urls_autolink,
    "go to https://example.com/path?q=1. or www.example.org, mail ana@example.com"
);
snapshot!(
    urls_in_code_and_links_stay,
    "`https://code.example.com` [https://label.example.com](https://real.example.com)"
);
snapshot!(
    raw_html_is_text,
    "<b>bold?</b> & <script>alert(1)</script>\n\n<div>\nblock\n</div>"
);
snapshot!(thematic_break, "above\n\n---\n\nbelow");
snapshot!(
    footnote_syntax_is_a_link_reference,
    "text[^1]\n\n[^1]: note"
);

#[test]
fn ten_thousand_nested_brackets_parse_quickly() {
    for input in [
        "[".repeat(10_000) + &"]".repeat(10_000),
        "> ".repeat(10_000) + "deep",
        "*".repeat(10_000) + "x" + &"*".repeat(10_000),
        "- ".repeat(5_000) + "item",
        "![".repeat(5_000) + "x" + &"](u)".repeat(5_000),
    ] {
        let started = std::time::Instant::now();
        let document = parse(&input);
        let elapsed = started.elapsed();
        // Debug builds are slower; the budget is for an optimized build.
        let budget = if cfg!(debug_assertions) { 500 } else { 50 };
        assert!(
            elapsed.as_millis() < budget,
            "{} took {elapsed:?}",
            &input[..10]
        );
        assert!(depth(&document.blocks) <= matty_markdown::MAX_DEPTH + 2);
        drop(document);
    }
}

fn depth(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            Block::BlockQuote(children) => 1 + depth(children),
            Block::List { items, .. } => {
                1 + items.iter().map(|i| depth(&i.blocks)).max().unwrap_or(0)
            }
            Block::Paragraph(inlines)
            | Block::Heading {
                content: inlines, ..
            } => inline_depth(inlines),
            _ => 1,
        })
        .max()
        .unwrap_or(0)
}

fn inline_depth(inlines: &[Inline]) -> usize {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Emphasis(c) | Inline::Strong(c) | Inline::Strikethrough(c) => {
                1 + inline_depth(c)
            }
            Inline::Link { children, .. } => 1 + inline_depth(children),
            _ => 1,
        })
        .max()
        .unwrap_or(0)
}
