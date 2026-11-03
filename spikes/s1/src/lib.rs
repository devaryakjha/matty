//! Spike S1 (#57): does gpuicn's `MessageList` (Kit `MessageScroller` on
//! GPUI's `list`) keep its place with 50k mixed-height rows under prepend,
//! resize and tail-follow?
//!
//! The library holds the deterministic transcript and the row renderer, shared
//! by the headless measurements in `tests/` and the interactive window in
//! `src/main.rs`. This is throwaway spike code, not Matty app code.

#![allow(missing_docs)]

use gpui_kit::{AnyElement, App, IntoElement, ParentElement as _, Styled as _, div, px};
use gpuicn::message::{Message, day_divider};
use gpuicn::theme::UiTheme;

/// Rows in the initial transcript.
pub const INITIAL_ROWS: usize = 50_000;
/// Rows inserted at the top by one history page.
pub const PAGE: usize = 200;

/// One synthetic post. `id` is stable across prepends, unlike the list index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: i64,
}

impl Row {
    fn hash(self) -> u64 {
        // SplitMix64: deterministic, well-spread variety per id.
        let mut z = (self.id as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 1–20 lines of body text.
    pub fn lines(self) -> usize {
        1 + (self.hash() % 20) as usize
    }

    /// Roughly one row in eight carries a 200px image placeholder.
    pub fn has_image(self) -> bool {
        self.hash() >> 32 & 7 == 0
    }

    /// A day divider every 500 posts.
    pub fn is_divider(self) -> bool {
        self.id.rem_euclid(500) == 0
    }

    /// Same sender within a run of four, as the caller would group them.
    pub fn is_continuation(self) -> bool {
        self.id.rem_euclid(4) != 0
    }
}

/// The caller-owned transcript: ids ascend from oldest to newest.
pub struct Transcript {
    pub rows: Vec<Row>,
}

impl Transcript {
    /// `count` rows with ids `0..count`.
    pub fn new(count: usize) -> Self {
        Self {
            rows: (0..count as i64).map(|id| Row { id }).collect(),
        }
    }

    /// Inserts `count` older rows at the front and returns how many were added.
    pub fn prepend(&mut self, count: usize) -> usize {
        let first = self.rows.first().map_or(0, |row| row.id);
        let older = (first - count as i64..first).map(|id| Row { id });
        self.rows.splice(0..0, older);
        count
    }

    /// Appends one newer row.
    pub fn append(&mut self) {
        let next = self.rows.last().map_or(0, |row| row.id + 1);
        self.rows.push(Row { id: next });
    }

    pub fn index_of(&self, id: i64) -> Option<usize> {
        let first = self.rows.first()?.id;
        let index = usize::try_from(id - first).ok()?;
        (index < self.rows.len()).then_some(index)
    }
}

const WORDS: &[&str] = &[
    "release", "the", "sidebar", "renders", "from", "cache", "while", "sync", "catches", "up",
    "and", "nobody", "notices", "a", "jump", "in", "scroll", "position", "when", "history",
    "loads", "above",
];

fn body_line(row: Row, line: usize) -> String {
    let seed = row.hash().rotate_left(line as u32 * 7);
    // Every third line is long enough to wrap below ~900px, so width changes
    // reflow rows the way real messages do.
    let len = if seed.is_multiple_of(3) {
        30 + (seed % 20) as usize
    } else {
        6 + (seed % 10) as usize
    };
    (0..len)
        .map(|word| WORDS[((seed >> (word % 8 * 8)) as usize + word) % WORDS.len()])
        .collect::<Vec<_>>()
        .join(" ")
}

/// Renders one transcript row. `tag` marks the row whose bounds a test reads.
pub fn render_row(row: Row, tag: Option<&'static str>, cx: &App) -> AnyElement {
    let element = if row.is_divider() {
        day_divider(("day", row.id as u64), format!("Day {}", row.id / 500), cx).into_any_element()
    } else {
        let theme = UiTheme::read(cx);
        let mut message = Message::new(("post", row.id as u64))
            .author(format!("user{}", row.id.rem_euclid(17)))
            .timestamp("10:42 AM")
            .continuation(row.is_continuation())
            .gutter_time("10:42");
        for line in 0..row.lines() {
            message = message.child(div().child(format!("#{} {}", row.id, body_line(row, line))));
        }
        if row.has_image() {
            message = message.child(
                div()
                    .h(px(200.))
                    .w(px(320.))
                    .rounded(theme.radius.md)
                    .bg(theme.colors.muted),
            );
        }
        message.into_any_element()
    };
    match tag {
        Some(selector) => tagged(element, selector),
        None => element,
    }
}

#[cfg(any(test, debug_assertions))]
fn tagged(element: AnyElement, selector: &'static str) -> AnyElement {
    use gpui_kit::InteractiveElement as _;
    div()
        .debug_selector(move || selector.to_string())
        .child(element)
        .into_any_element()
}

#[cfg(not(any(test, debug_assertions)))]
fn tagged(element: AnyElement, _: &'static str) -> AnyElement {
    element
}
