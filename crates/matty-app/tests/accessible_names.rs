//! Every control Matty draws has an accessible name (PLAN §8, #120).
//!
//! The main window opens over the preview fixtures, as in the screenshots,
//! and gpuicn's audit lists the interactive elements of the drawn frame
//! that have no name. This covers controls `scripts/check-accessible-names.py`
//! cannot see in the source, such as ones whose label comes from data.
#![allow(clippy::unwrap_used)]

use gpui_kit::{AnyView, IntoElement, Render, VisualTestContext};

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

#[gpui_kit::test]
fn the_main_window_has_no_unnamed_controls(cx: &mut gpui_kit::TestAppContext) {
    // Its own test binary, so turning the fixtures on touches nothing else.
    matty_app::preview::set("1".into());
    cx.update(matty_app::test_support::init);
    let window = cx.add_window(|window, cx| Root(matty_app::test_support::main_window(window, cx)));
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // The fixtures drew: the sidebar, channel and composer are all there.
    let drawn = cx.update(|window, _| {
        use gpui_kit::test::TestQueryExt as _;
        window.elements().len()
    });
    assert!(drawn > 100, "only {drawn} elements drew");
    let unnamed = cx.update(|window, _| gpuicn::audit::unnamed_controls(window));
    let described: Vec<String> = unnamed
        .iter()
        .map(|control| format!("{control:?}"))
        .collect();
    assert!(
        described.is_empty(),
        "{} controls have no accessible name:\n{}",
        described.len(),
        described.join("\n")
    );
}
