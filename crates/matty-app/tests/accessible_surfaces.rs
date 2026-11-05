//! The surfaces added since the main window's audit (#217) name every
//! control too: pop-out windows, split view, the dialogs and panes the
//! shortcuts open, the bookmarks bar, and urgent posts that ask for an
//! acknowledgement.
//!
//! The main window opens over the preview's priority fixtures, which add
//! an urgent post asking for acknowledgements to #general beside its
//! bookmarks bar. Each surface opens in turn, shows a control only it
//! draws, and gpuicn's audit lists the interactive elements of every open
//! window that have no name. What a screen reader then says on each
//! platform still takes a manual pass: `docs/journeys/m2-screen-readers.md`.
#![allow(clippy::unwrap_used)]

use gpui_kit::{Action, AnyView, AppContext as _, IntoElement, Render, VisualTestContext};
use matty_app::actions;

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

/// Audits every open window once `surface` shows, which it does when one
/// of them draws `expected`.
fn audit(cx: &mut VisualTestContext, surface: &str, expected: &str, failures: &mut Vec<String>) {
    cx.run_until_parked();
    let mut shown = false;
    for window in cx.update(|_, cx| cx.windows()) {
        let (labels, unnamed) = cx
            .update_window(window, |_, window, cx| {
                use gpui_kit::test::TestQueryExt as _;
                window.draw(cx).clear(cx);
                let labels: Vec<String> = window
                    .elements()
                    .into_iter()
                    .filter_map(|element| element.label().map(|label| label.to_string()))
                    .collect();
                let unnamed: Vec<String> = gpuicn::audit::unnamed_controls(window)
                    .iter()
                    .map(gpuicn::audit::element_path)
                    .collect();
                (labels, unnamed)
            })
            .unwrap();
        shown |= labels.iter().any(|label| label == expected);
        failures.extend(unnamed.into_iter().map(|path| format!("{surface}: {path}")));
    }
    assert!(shown, "{surface} did not open: nothing drew {expected:?}");
}

/// Clicks the first control labelled `label`.
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
    cx.simulate_click(center, gpui_kit::Modifiers::none());
    cx.run_until_parked();
}

fn close(cx: &mut VisualTestContext) {
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_new_surfaces_have_no_unnamed_controls(cx: &mut gpui_kit::TestAppContext) {
    // Its own test binary, so turning the fixtures on touches nothing else.
    matty_app::preview::set("priority".into());
    cx.update(matty_app::test_support::init);
    let window = cx.add_window(|window, cx| Root(matty_app::test_support::main_window(window, cx)));
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.update(|window, _| window.activate_window());
    // Tall enough that the urgent post draws above the latest ones.
    cx.simulate_resize(gpui_kit::size(gpui_kit::px(1200.), gpui_kit::px(3000.)));
    let mut failures = Vec::new();

    audit(&mut cx, "Bookmarks bar", "Add a bookmark", &mut failures);
    audit(&mut cx, "Acknowledge bar", "Acknowledge", &mut failures);

    // The menus and pickers on those, and on a post.
    let menus = [
        (
            "Bookmark actions",
            "Bookmark actions",
            "Add a bookmark\u{2026}",
        ),
        (
            "Priority picker",
            "Message priority",
            "Request acknowledgement",
        ),
        ("Post menu", "More actions", "Copy link"),
    ];
    for (surface, opener, expected) in menus {
        click(&mut cx, opener);
        audit(&mut cx, surface, expected, &mut failures);
        close(&mut cx);
    }

    let surfaces: [(&str, &str, Box<dyn Action>); 10] = [
        (
            "Quick switcher",
            "Search commands",
            Box::new(actions::QuickSwitcher),
        ),
        (
            "Browse channels",
            "Search channels",
            Box::new(actions::BrowseChannels),
        ),
        (
            "Create channel",
            "Name (required)",
            Box::new(actions::CreateChannel),
        ),
        (
            "Direct messages",
            "Search people",
            Box::new(actions::DirectMessagesMenu),
        ),
        ("Search", "Search messages", Box::new(actions::Search)),
        (
            "Channel info",
            "Leave channel",
            Box::new(actions::ChannelInfo),
        ),
        ("Recent mentions", "Jump", Box::new(actions::RecentMentions)),
        ("Saved messages", "Jump", Box::new(actions::SavedMessages)),
        (
            "Preferences",
            "Appearance",
            Box::new(actions::OpenPreferences),
        ),
        (
            "Keyboard shortcuts",
            "Close",
            Box::new(actions::ShowShortcuts),
        ),
    ];
    for (surface, expected, action) in surfaces {
        cx.update(|window, cx| window.dispatch_action(action, cx));
        audit(&mut cx, surface, expected, &mut failures);
        close(&mut cx);
    }

    cx.dispatch_action(actions::OpenInSplitView);
    audit(&mut cx, "Split view", "Swap panes", &mut failures);
    cx.dispatch_action(actions::ClosePane);
    cx.run_until_parked();

    let before = cx.update(|_, cx| cx.windows().len());
    cx.dispatch_action(actions::OpenInNewWindow);
    cx.run_until_parked();
    let after = cx.update(|_, cx| cx.windows().len());
    assert_eq!(after, before + 1, "the channel popped out");
    audit(&mut cx, "Pop-out window", "Message", &mut failures);

    assert!(
        failures.is_empty(),
        "{} controls have no accessible name:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
