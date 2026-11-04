//! The quick switcher across teams and servers (#90, #206), over the
//! `MATTY_PREVIEW=servers` fixtures: three teams on the shown server and
//! two other saved servers. Cmd/Ctrl+K lists other teams' channels, named
//! by team, and the teams; picking one switches team first. "Search all
//! servers" lists the other servers' channels; picking one switches server.
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

fn draw(cx: &mut VisualTestContext) -> Vec<gpui_kit::test::ElementSnapshot> {
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, _| {
        use gpui_kit::test::TestQueryExt as _;
        window.elements()
    })
}

/// The labels of everything drawn now.
fn labels(cx: &mut VisualTestContext) -> Vec<String> {
    draw(cx)
        .into_iter()
        .filter_map(|element| element.label().map(str::to_owned))
        .collect()
}

fn has(cx: &mut VisualTestContext, label: &str) -> bool {
    labels(cx).iter().any(|drawn| drawn == label)
}

/// The switcher's rows, top to bottom, as far as they are drawn.
fn options(cx: &mut VisualTestContext) -> Vec<String> {
    draw(cx)
        .into_iter()
        .filter(|element| element.role() == Some(gpui_kit::Role::ListBoxOption))
        .filter_map(|element| element.label().map(str::to_owned))
        .collect()
}

fn click(cx: &mut VisualTestContext, label: &str) {
    let center = draw(cx)
        .into_iter()
        .find(|element| element.label().is_some_and(|l| l == label))
        .unwrap_or_else(|| panic!("nothing is labelled {label:?}"))
        .bounds()
        .center();
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

fn switcher(cx: &mut VisualTestContext, query: &str) {
    let keys = if cfg!(target_os = "macos") {
        "cmd-k"
    } else {
        "ctrl-k"
    };
    cx.simulate_keystrokes(keys);
    cx.run_until_parked();
    // The field takes focus on the frame after the dialog mounts, which a
    // test window never asks for; a click puts it there.
    click(cx, "Search commands");
    if !query.is_empty() {
        cx.simulate_input(query);
        cx.run_until_parked();
    }
}

#[gpui_kit::test]
fn the_switcher_reaches_every_team_and_other_servers(cx: &mut gpui_kit::TestAppContext) {
    // Its own test binary, so turning the fixtures on touches nothing else.
    matty_app::preview::set("servers".into());
    cx.update(matty_app::test_support::init);
    let window = cx.add_window(|window, cx| Root(matty_app::test_support::main_window(window, cx)));
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    // Typing reaches a field only in the active window.
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    assert_eq!(open_row(&mut cx), "general");

    // With no query, unread rows lead, other teams' among them.
    switcher(&mut cx, "");
    let rows = options(&mut cx);
    assert_eq!(rows.first().map(String::as_str), Some("design"), "{rows:?}");
    assert!(rows.iter().any(|row| row == "critique"), "{rows:?}");

    // Another team's channel: Enter switches team, then opens it.
    cx.simulate_input("critiq");
    cx.run_until_parked();
    let rows = options(&mut cx);
    assert_eq!(
        rows.first().map(String::as_str),
        Some("critique"),
        "{rows:?}"
    );
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(open_row(&mut cx), "critique");
    assert!(has(&mut cx, "motion"), "Design Guild's sidebar shows");

    // A team by name: picking it switches to it.
    switcher(&mut cx, "open source");
    let rows = options(&mut cx);
    assert!(rows.iter().any(|row| row == "Open Source"), "{rows:?}");
    click(&mut cx, "Open Source");
    assert!(has(&mut cx, "contributors"), "Open Source's sidebar shows");
    assert_eq!(open_row(&mut cx), "town-square");

    // Matty's channels are still a keystroke away, from Open Source.
    switcher(&mut cx, "help-d");
    assert_eq!(
        options(&mut cx).first().map(String::as_str),
        Some("help-desk")
    );
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    // Another server's channel: offered behind "Search all servers".
    switcher(&mut cx, "deploys");
    let all = "Search all servers for \u{201c}deploys\u{201d}";
    let rows = options(&mut cx);
    assert!(!rows.iter().any(|row| row == "deploys"), "{rows:?}");
    assert!(rows.iter().any(|row| row == all), "{rows:?}");
    click(&mut cx, all);
    let rows = options(&mut cx);
    assert_eq!(
        rows.first().map(String::as_str),
        Some("deploys"),
        "{rows:?}"
    );
    let unnamed = cx.update(|window, _| gpuicn::audit::unnamed_controls(window));
    assert!(unnamed.is_empty(), "unnamed controls: {unnamed:?}");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let composer = draw(&mut cx)
        .into_iter()
        .any(|element| element.placeholder() == Some("Message #deploys"));
    assert!(composer, "deploys is open");
}
