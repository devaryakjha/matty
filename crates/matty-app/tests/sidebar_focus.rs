//! Clicking a sidebar channel puts focus in the composer, so Alt+Shift+Up
//! and Down keep walking unread channels after a click, and only move a row
//! when the list was reached from the keyboard (#505).
#![allow(clippy::unwrap_used)]

use gpui_kit::{AnyView, IntoElement, Modifiers, MouseButton, Render, VisualTestContext};

struct Root(AnyView);

impl Render for Root {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl IntoElement {
        self.0.clone()
    }
}

/// The sidebar row named `name`; the open one is "name, current destination".
fn row(cx: &mut VisualTestContext, name: &str) -> gpui_kit::test::ElementSnapshot {
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, _| {
        use gpui_kit::test::TestQueryExt as _;
        window
            .elements()
            .into_iter()
            .find(|element| {
                element.label().is_some_and(|label| {
                    label == name || label == format!("{name}, current destination")
                })
            })
            .unwrap()
    })
}

fn row_top(cx: &mut VisualTestContext, name: &str) -> gpui_kit::Pixels {
    row(cx, name).bounds().origin.y
}

fn is_open(cx: &mut VisualTestContext, name: &str) -> bool {
    row(cx, name)
        .label()
        .is_some_and(|label| label.ends_with("current destination"))
}

#[gpui_kit::test]
fn a_clicked_channel_focuses_the_composer_and_keeps_its_place(cx: &mut gpui_kit::TestAppContext) {
    matty_app::preview::set("1".into());
    cx.update(matty_app::test_support::init);
    let window = cx.add_window(|window, cx| Root(matty_app::test_support::main_window(window, cx)));
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();

    let (random, town_square) = (row_top(&mut cx, "random"), row_top(&mut cx, "town-square"));
    assert!(town_square < random, "the fixtures list town-square first");
    let center = row(&mut cx, "random").bounds().center();
    cx.simulate_mouse_down(center, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(center, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // Focus sits in a text input (the composer) and not in the sidebar.
    let contexts = cx.update(|window, _| format!("{:?}", window.context_stack()));
    let composer_focused = contexts.contains("Input") && !contexts.contains("SidebarSortable");
    assert!(composer_focused, "focus is not in the composer: {contexts}");

    // From the composer the keys walk unreads; the row stays where it was.
    cx.simulate_keystrokes("alt-shift-up");
    cx.run_until_parked();
    assert!(
        is_open(&mut cx, "town-square"),
        "the previous unread opened"
    );
    assert_eq!(row_top(&mut cx, "random"), random);
    assert_eq!(row_top(&mut cx, "town-square"), town_square);
}
