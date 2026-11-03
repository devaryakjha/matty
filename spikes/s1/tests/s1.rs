//! S1 measurements (#57), run headless with GPUI's test platform:
//! `cargo test -p spike-s1 -- --nocapture` prints the bounds log.
//!
//! The pass criteria are in the issue. Frame timing and the video need a real
//! window on the reference machine and are measured with `cargo run -p
//! spike-s1 --release` instead.

use std::{cell::Cell, cell::RefCell, rc::Rc};

use gpui_kit::{
    AppContext as _, Context, Entity, FollowMode, IntoElement, ListAlignment, ListState,
    ParentElement as _, Pixels, Render, Styled as _, TestAppContext, VisualTestContext, Window,
    div, list, px, size,
};
use gpuicn::message_list::{MessageList, MessageListState};
use spike_s1::{INITIAL_ROWS, PAGE, Transcript, render_row};

const ANCHOR: &str = "anchor";

struct Spike {
    transcript: Rc<RefCell<Transcript>>,
    anchor: Rc<Cell<Option<i64>>>,
    list: Entity<MessageListState>,
}

impl Render for Spike {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let transcript = self.transcript.clone();
        let anchor = self.anchor.clone();
        div().size_full().flex().flex_col().child(MessageList::new(
            "s1",
            &self.list,
            move |index, _, cx| {
                let row = transcript.borrow().rows[index];
                let tag = (anchor.get() == Some(row.id)).then_some(ANCHOR);
                render_row(row, tag, cx)
            },
        ))
    }
}

fn open(cx: &mut TestAppContext) -> (Entity<Spike>, &mut VisualTestContext) {
    cx.update(gpuicn::init);
    let (view, cx) = cx.add_window_view(|_, cx| Spike {
        transcript: Rc::new(RefCell::new(Transcript::new(INITIAL_ROWS))),
        anchor: Rc::new(Cell::new(None)),
        list: cx.new(|cx| MessageListState::new(INITIAL_ROWS, cx)),
    });
    cx.simulate_resize(size(px(1200.), px(800.)));
    settle(cx);
    (view, cx)
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
    }
}

/// The row id at the top of the viewport and how far into it the list is.
fn top(view: &Entity<Spike>, cx: &mut VisualTestContext) -> (i64, Pixels) {
    cx.update(|_, cx| {
        let spike = view.read(cx);
        let offset = spike.list.read(cx).logical_scroll_top();
        (
            spike.transcript.borrow().rows[offset.item_ix].id,
            offset.offset_in_item,
        )
    })
}

/// Scroll to the middle of history and tag the top row as the anchor.
fn scroll_to_middle(view: &Entity<Spike>, cx: &mut VisualTestContext) -> i64 {
    // A tall text row (no divider, no image) so a reflow would show.
    let middle = cx.update(|_, cx| {
        let transcript = view.read(cx).transcript.clone();
        let transcript = transcript.borrow();
        (INITIAL_ROWS / 2..)
            .find(|&ix| {
                let row = transcript.rows[ix];
                !row.is_divider() && !row.has_image() && row.lines() >= 8
            })
            .expect("a tall row exists")
    });
    cx.update(|_, cx| {
        let list = view.read(cx).list.clone();
        list.update(cx, |list, cx| list.scroll_to_item(middle, cx));
    });
    settle(cx);
    // Scroll part-way into the row so a lost sub-row offset would show.
    cx.simulate_event(gpui_kit::ScrollEvent {
        position: gpui_kit::point(px(600.), px(400.)),
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-37.))),
        ..Default::default()
    });
    settle(cx);
    let (id, _) = top(view, cx);
    cx.update(|_, cx| view.read(cx).anchor.set(Some(id)));
    settle(cx);
    id
}

#[gpui_kit::test]
fn prepend_keeps_the_anchor_row_in_place(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let anchor = scroll_to_middle(&view, cx);
    let before = cx.debug_bounds(ANCHOR).expect("anchor row is rendered");
    let top_before = top(&view, cx);

    cx.update(|_, cx| {
        let spike = view.read(cx);
        let added = spike.transcript.borrow_mut().prepend(PAGE);
        let list = spike.list.clone();
        list.update(cx, |list, cx| list.prepend(added, cx));
    });
    settle(cx);
    let after = cx
        .debug_bounds(ANCHOR)
        .expect("anchor row is still rendered");
    let top_after = top(&view, cx);

    println!(
        "S1 prepend: anchor #{anchor} top {:?} -> {:?} (moved {:?}); scroll top {top_before:?} -> {top_after:?}",
        before.top(),
        after.top(),
        after.top() - before.top()
    );
    assert_eq!(after.top(), before.top(), "anchor row moved on prepend");
    assert_eq!(
        top_after, top_before,
        "the same row and offset stay at the top"
    );
}

#[gpui_kit::test]
fn repeated_prepends_keep_the_anchor_row_in_place(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let anchor = scroll_to_middle(&view, cx);
    let before = cx.debug_bounds(ANCHOR).expect("anchor row is rendered");
    for page in 1..=10 {
        cx.update(|_, cx| {
            let spike = view.read(cx);
            let added = spike.transcript.borrow_mut().prepend(PAGE);
            let list = spike.list.clone();
            list.update(cx, |list, cx| list.prepend(added, cx));
        });
        settle(cx);
        let after = cx
            .debug_bounds(ANCHOR)
            .expect("anchor row is still rendered");
        println!(
            "S1 prepend page {page}: anchor #{anchor} moved {:?}",
            after.top() - before.top()
        );
        assert_eq!(after.top(), before.top());
    }
}

#[gpui_kit::test]
fn width_change_keeps_the_same_row_at_the_top(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let anchor = scroll_to_middle(&view, cx);
    let (_, offset_before) = top(&view, cx);
    let before = cx.debug_bounds(ANCHOR).expect("anchor row is rendered");

    for width in [900., 600., 1200.] {
        cx.simulate_resize(size(px(width), px(800.)));
        settle(cx);
        let (id, offset) = top(&view, cx);
        let bounds = cx
            .debug_bounds(ANCHOR)
            .expect("anchor row is still rendered");
        println!(
            "S1 resize to {width}px: top row #{id} offset {offset:?} (was #{anchor} offset {offset_before:?}); anchor top {:?} -> {:?}, height {:?} -> {:?}",
            before.top(),
            bounds.top(),
            before.size.height,
            bounds.size.height,
        );
        assert_eq!(
            id, anchor,
            "a different row is at the top after resizing to {width}px"
        );
    }
}

#[gpui_kit::test]
fn tail_follow_sticks_to_the_bottom_and_scrolled_up_does_not_move(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    // At the bottom: an appended row keeps the list following the tail.
    cx.update(|_, cx| {
        let spike = view.read(cx);
        spike.transcript.borrow_mut().append();
        let list = spike.list.clone();
        list.update(cx, |list, cx| list.append(1, cx));
    });
    settle(cx);
    cx.update(|_, cx| {
        let list = view.read(cx).list.read(cx);
        assert!(
            list.is_following_tail(),
            "appending at the bottom stops tail-follow"
        );
        assert!(!list.is_scrolled_up());
    });

    // Scrolled up: appends leave the viewport where it is.
    let anchor = scroll_to_middle(&view, cx);
    let before = cx.debug_bounds(ANCHOR).expect("anchor row is rendered");
    for _ in 0..5 {
        cx.update(|_, cx| {
            let spike = view.read(cx);
            spike.transcript.borrow_mut().append();
            let list = spike.list.clone();
            list.update(cx, |list, cx| list.append(1, cx));
        });
        settle(cx);
    }
    let after = cx
        .debug_bounds(ANCHOR)
        .expect("anchor row is still rendered");
    println!(
        "S1 append while scrolled up: anchor #{anchor} moved {:?}",
        after.top() - before.top()
    );
    assert_eq!(after.top(), before.top());
    cx.update(|_, cx| assert!(view.read(cx).list.read(cx).is_scrolled_up()));
}

// The scrollbar thumb. `MessageScrollerState` keeps its `ListState` private,
// so this drives GPUI's `list` with the same state settings (top alignment,
// tail follow, 400px overdraw) and the same rows, and reads the numbers the
// scrollbar is drawn from: the scroll offset and the maximum offset.

struct Raw {
    transcript: Rc<RefCell<Transcript>>,
    state: ListState,
}

impl Render for Raw {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let transcript = self.transcript.clone();
        div().size_full().child(
            list(self.state.clone(), move |index, _, cx| {
                render_row(transcript.borrow().rows[index], None, cx)
            })
            .size_full(),
        )
    }
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Printed with Debug.
struct Thumb {
    /// Thumb top as a fraction of the track.
    top: f32,
    /// Thumb length as a fraction of the track.
    len: f32,
    /// Content height the scrollbar believes in.
    content: f32,
}

fn thumb(state: &ListState) -> Thumb {
    let viewport = f32::from(state.viewport_bounds().size.height);
    let max = f32::from(state.max_offset_for_scrollbar().y);
    let offset = -f32::from(state.scroll_px_offset_for_scrollbar().y);
    let content = max + viewport;
    let len = (viewport / content).min(1.);
    Thumb {
        top: if max > 0. {
            offset / max * (1. - len)
        } else {
            0.
        },
        len,
        content,
    }
}

#[gpui_kit::test]
fn scrollbar_thumb_under_prepend_and_width_change(cx: &mut TestAppContext) {
    cx.update(gpuicn::init);
    let transcript = Rc::new(RefCell::new(Transcript::new(INITIAL_ROWS)));
    let state = ListState::new(INITIAL_ROWS, ListAlignment::Top, px(400.));
    state.set_follow_mode(FollowMode::Tail);
    let raw_state = state.clone();
    let (_view, cx) = cx.add_window_view(move |_, _| Raw {
        transcript,
        state: raw_state,
    });
    cx.simulate_resize(size(px(1200.), px(800.)));
    settle(cx);
    state.scroll_to(gpui_kit::ListOffset {
        item_ix: INITIAL_ROWS / 2,
        offset_in_item: px(0.),
    });
    settle(cx);
    let start = thumb(&state);
    println!("S1 thumb at row {}: {start:?}", INITIAL_ROWS / 2);

    // Scroll by a few screens: more rows get measured and the thumb drifts.
    for _ in 0..5 {
        state.scroll_by(px(800.));
        settle(cx);
    }
    let scrolled = thumb(&state);
    println!("S1 thumb after scrolling 4000px: {scrolled:?}");

    cx.update(|_, cx| {
        let view = _view.read(cx);
        view.transcript.borrow_mut().prepend(PAGE);
    });
    state.splice(0..0, PAGE);
    settle(cx);
    let prepended = thumb(&state);
    println!("S1 thumb after prepending {PAGE}: {prepended:?}");

    cx.simulate_resize(size(px(600.), px(800.)));
    settle(cx);
    let narrow = thumb(&state);
    println!("S1 thumb after resizing to 600px: {narrow:?}");

    // Recorded, not asserted: the numbers go in the S1 result doc.
    let rows = (INITIAL_ROWS + PAGE) as f32;
    println!(
        "S1 thumb: content believed {:.0}px for {rows} rows (≈{:.1}px per row known); thumb top moved {:+.4} on prepend, {:+.4} on resize",
        narrow.content,
        narrow.content / rows,
        prepended.top - scrolled.top,
        narrow.top - prepended.top,
    );
}
