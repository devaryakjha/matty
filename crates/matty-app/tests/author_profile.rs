//! A post's author name opens the same profile card as its avatar (#212):
//! a click pins it open, and a click elsewhere closes it.
#![allow(clippy::unwrap_used)]

use gpui_kit::{AnyView, IntoElement, Modifiers, Render, VisualTestContext};

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

fn labels(cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, _| {
        use gpui_kit::test::TestQueryExt as _;
        window
            .elements()
            .into_iter()
            .filter_map(|element| element.label().map(|label| label.to_string()))
            .collect()
    })
}

/// The center of the last element labelled `label`: the newest post's.
fn center(cx: &mut VisualTestContext, label: &str) -> gpui_kit::Point<gpui_kit::Pixels> {
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, _| {
        use gpui_kit::test::TestQueryExt as _;
        window
            .elements()
            .into_iter()
            .filter(|element| element.label().is_some_and(|l| l == label))
            .last()
            .unwrap_or_else(|| panic!("nothing is labelled {label:?}"))
            .bounds()
            .center()
    })
}

/// Whether a profile card is open: its "View profile" button shows.
fn card_open(cx: &mut VisualTestContext) -> bool {
    labels(cx).iter().any(|label| label == "View profile")
}

#[gpui_kit::test]
fn the_author_name_opens_the_profile_card(cx: &mut gpui_kit::TestAppContext) {
    // Its own test binary, so turning the fixtures on touches nothing else.
    matty_app::preview::set("1".into());
    cx.update(matty_app::test_support::init);
    let window = cx.add_window(|window, cx| Root(matty_app::test_support::main_window(window, cx)));
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    // The newest post in #general is Lena Fischer's.
    let name = "Lena Fischer";
    assert!(!card_open(&mut cx));

    let at = center(&mut cx, name);
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
    assert!(card_open(&mut cx), "a click on the name opens the card");

    // A click elsewhere closes it.
    let composer = center(&mut cx, "Message");
    cx.simulate_click(composer, Modifiers::none());
    cx.run_until_parked();
    assert!(!card_open(&mut cx), "a click elsewhere closes it");
}
