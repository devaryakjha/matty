//! The server rail (#206) draws with every control named, over the
//! `MATTY_PREVIEW=servers` fixtures.
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
fn the_server_rail_shows_each_server_named(cx: &mut gpui_kit::TestAppContext) {
    // Its own test binary, so turning the fixtures on touches nothing else.
    matty_app::preview::set("servers".into());
    cx.update(matty_app::test_support::init);
    let window = cx.add_window(|window, cx| Root(matty_app::test_support::main_window(window, cx)));
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let ids: Vec<String> = cx.update(|window, _| {
        use gpui_kit::test::TestQueryExt as _;
        window
            .elements()
            .iter()
            .map(|element| format!("{element:?}"))
            .collect()
    });
    for id in ["rail.server.0", "rail.server.2", "rail.add-server"] {
        assert!(
            ids.iter().any(|drawn| drawn.contains(id)),
            "{id} did not draw"
        );
    }
    let unnamed = cx.update(|window, _| gpuicn::audit::unnamed_controls(window));
    assert!(unnamed.is_empty(), "unnamed controls: {unnamed:?}");
}
