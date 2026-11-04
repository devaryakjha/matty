//! Split view (#222) over the preview's fixtures: a saved split opens again
//! beside the main channel without counting as read, focus moves between
//! the panes and marks the focused one's channel read, the second pane's
//! composer sends into its own channel, sidebar clicks open in the focused
//! pane, the panes swap, and closing the focused pane ends the split and
//! forgets it.
#![allow(clippy::unwrap_used)]

use gpui_kit::{AnyView, IntoElement, Modifiers, MouseButton, Render, VisualTestContext};
use matty_app::settings::{SettingsStore, SplitPlace};

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

fn has(cx: &mut VisualTestContext, label: &str) -> bool {
    labels(cx).iter().any(|drawn| drawn == label)
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

fn close_pane_keys() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-alt-w"
    } else {
        "ctrl-alt-w"
    }
}

#[gpui_kit::test]
fn two_channels_side_by_side_follow_focus(cx: &mut gpui_kit::TestAppContext) {
    // Its own test binary, so turning the fixtures on touches nothing else.
    matty_app::preview::set("1".into());
    cx.update(matty_app::test_support::init);
    // The preview's split, saved as a server's is, with an unread channel.
    cx.update(|cx| {
        SettingsStore::global(cx).update(cx, |store, cx| {
            store.update(cx, |settings| {
                settings.splits = vec![SplitPlace {
                    server: String::new(),
                    channel_id: "town-square".into(),
                    ratio: 0.5,
                }]
            })
        })
    });
    let mut main = None;
    let window = cx.add_window(|window, cx| {
        let view = matty_app::test_support::main_window(window, cx);
        main = Some(view.clone());
        Root(view)
    });
    let main = main.unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    let unread = |cx: &mut VisualTestContext, id: &str| {
        let main = main.clone();
        cx.update(|_, cx| matty_app::test_support::is_unread(&main, id, cx))
    };
    let split = |cx: &mut VisualTestContext| {
        let main = main.clone();
        cx.update(|_, cx| matty_app::test_support::split(&main, cx))
    };

    // The saved split is back, beside the main channel, which keeps focus:
    // the second channel shows but is not read.
    assert!(has(&mut cx, "Left pane, general, focused"));
    assert!(has(&mut cx, "Right pane, town-square"));
    assert!(
        unread(&mut cx, "town-square"),
        "an unfocused pane reads nothing"
    );

    // F6 moves focus to the second pane, which now counts as seen.
    cx.simulate_keystrokes("f6");
    cx.run_until_parked();
    assert!(has(&mut cx, "Right pane, town-square, focused"));
    assert!(has(&mut cx, "Left pane, general"));
    assert!(!unread(&mut cx, "town-square"), "the focused pane reads it");

    // Its composer has focus and sends into its own channel.
    cx.simulate_input("hello from the split");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let (channel, bodies) = split(&mut cx).unwrap();
    assert_eq!(channel, "town-square");
    assert_eq!(
        bodies.last().map(String::as_str),
        Some("hello from the split")
    );

    // A sidebar click opens in the focused pane, and reads it there.
    assert!(unread(&mut cx, "help-desk"));
    click(&mut cx, "help-desk");
    assert_eq!(split(&mut cx).unwrap().0, "help-desk");
    assert!(has(&mut cx, "Left pane, general"));
    assert!(!unread(&mut cx, "help-desk"));
    assert!(unread(&mut cx, "lena"), "other channels stay unread");

    // The left header's Swap panes trades the channels; the click put
    // focus on the left, where it stays.
    click(&mut cx, "Swap panes");
    assert_eq!(split(&mut cx).unwrap().0, "general");
    assert!(has(&mut cx, "Left pane, help-desk, focused"));
    assert!(has(&mut cx, "Right pane, general"));

    // Back on the right, closing the focused pane ends the split and
    // forgets it.
    cx.simulate_keystrokes("f6");
    cx.run_until_parked();
    assert!(has(&mut cx, "Right pane, general, focused"));
    cx.simulate_keystrokes(close_pane_keys());
    cx.run_until_parked();
    assert!(split(&mut cx).is_none());
    assert!(!labels(&mut cx).iter().any(|l| l.starts_with("Right pane")));
    let saved = cx.update(|_, cx| SettingsStore::global(cx).read(cx).get().splits.len());
    assert_eq!(saved, 0);
}
