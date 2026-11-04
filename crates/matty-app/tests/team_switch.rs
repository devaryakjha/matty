//! Switching teams (#89) over the preview's three teams: the rail and
//! Cmd/Ctrl+Alt+1…9 and Up/Down show the picked team's sidebar, open its
//! town square the first time and where the user last was after that, and
//! Back returns to the other team's channel.
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

/// The labels of everything drawn now.
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

fn click(cx: &mut VisualTestContext, label: &str) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let center = cx.update(|window, _| {
        use gpui_kit::test::TestQueryExt as _;
        window
            .elements()
            .into_iter()
            .find(|element| element.label().is_some_and(|l| l == label))
            .unwrap_or_else(|| panic!("nothing is labelled {label:?}"))
            .bounds()
            .center()
    });
    cx.simulate_mouse_down(center, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(center, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

/// The sidebar row marked open.
fn open_row(cx: &mut VisualTestContext) -> String {
    labels(cx)
        .into_iter()
        .find_map(|label| {
            label
                .strip_suffix(", current destination")
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

fn has(cx: &mut VisualTestContext, label: &str) -> bool {
    labels(cx).iter().any(|drawn| drawn == label)
}

fn keys(n: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("cmd-alt-{n}")
    } else {
        format!("ctrl-alt-{n}")
    }
}

#[gpui_kit::test]
fn picking_a_team_shows_its_sidebar_and_remembers_where_you_were(
    cx: &mut gpui_kit::TestAppContext,
) {
    // Its own test binary, so turning the fixtures on touches nothing else.
    matty_app::preview::set("1".into());
    cx.update(matty_app::test_support::init);
    let window = cx.add_window(|window, cx| Root(matty_app::test_support::main_window(window, cx)));
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    assert_eq!(open_row(&mut cx), "general");
    assert!(has(&mut cx, "help-desk") && !has(&mut cx, "critique"));

    // The rail: Design Guild's sidebar, at its town square.
    click(&mut cx, "Design Guild, 3 mentions");
    assert!(has(&mut cx, "critique"), "Design Guild's channels show");
    assert!(!has(&mut cx, "help-desk"), "Matty's channels are gone");
    assert!(has(&mut cx, "Lena Fischer"), "direct messages stay");
    assert_eq!(open_row(&mut cx), "town-square");
    click(&mut cx, "critique");
    assert_eq!(open_row(&mut cx), "critique");

    // The first team's shortcut goes back to its channel.
    cx.simulate_keystrokes(&keys("1"));
    cx.run_until_parked();
    assert!(has(&mut cx, "help-desk") && !has(&mut cx, "critique"));
    assert_eq!(open_row(&mut cx), "general");

    // Next team returns to where the user was in Design Guild.
    cx.simulate_keystrokes(&keys("down"));
    cx.run_until_parked();
    assert_eq!(open_row(&mut cx), "critique");
    // Previous team wraps around past the first.
    cx.simulate_keystrokes(&keys("up"));
    cx.run_until_parked();
    cx.simulate_keystrokes(&keys("up"));
    cx.run_until_parked();
    assert!(has(&mut cx, "contributors"), "Open Source's channels show");

    // Back goes to the last channel, in its own team.
    let back = if cfg!(target_os = "macos") {
        "cmd-["
    } else {
        "alt-left"
    };
    cx.simulate_keystrokes(back);
    cx.run_until_parked();
    assert_eq!(open_row(&mut cx), "general");
    assert!(has(&mut cx, "help-desk") && !has(&mut cx, "contributors"));

    let unnamed = cx.update(|window, _| gpuicn::audit::unnamed_controls(window));
    assert!(unnamed.is_empty(), "unnamed controls: {unnamed:?}");
}
