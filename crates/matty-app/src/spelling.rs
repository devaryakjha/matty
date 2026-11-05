//! Spell checking in the composers (#224): misspelled words get gpuicn's
//! wavy underline a moment after typing stops, and right-clicking one
//! offers the checker's suggestions and "Ignore Spelling".
//!
//! The checker is `matty-spell`'s: the platform's on macOS, the Hunspell
//! dictionary for the user's language on Linux. Without one, nothing is
//! underlined.

use std::{cell::RefCell, rc::Rc, time::Duration};

use gpui_kit::{App, Entity, SharedString, Task, WeakEntity};
use matty_spell::Speller;

use crate::t;
use crate::ui::composer::ComposerState;
use crate::ui::input::InputEvent;

/// How long typing has to pause before the draft is checked.
const PAUSE: Duration = Duration::from_millis(250);

/// How many suggestions the menu offers.
const SUGGESTIONS: usize = 5;

/// Stops underlining a word until Matty quits.
#[derive(Clone, PartialEq, Debug, gpui_kit::Action)]
#[action(namespace = matty, no_json)]
pub struct IgnoreSpelling {
    pub word: SharedString,
}

/// The checker, loaded on the first check, and the composers it watches.
#[derive(Default)]
struct Spelling {
    /// `None` until loaded; then `None` inside when there is no checker.
    speller: Option<Option<Rc<RefCell<Speller>>>>,
    composers: Vec<WeakEntity<ComposerState>>,
}

impl gpui_kit::Global for Spelling {}

/// The checker, loading it the first time.
fn speller(cx: &mut App) -> Option<Rc<RefCell<Speller>>> {
    let spelling = cx.default_global::<Spelling>();
    if spelling.speller.is_none() {
        let speller = Speller::system();
        if speller.is_none() {
            tracing::info!("no spell checker for this language; drafts are not checked");
        }
        spelling.speller = Some(speller.map(|speller| Rc::new(RefCell::new(speller))));
    }
    spelling.speller.clone().flatten()
}

/// Checks `composer`'s draft as the user types.
pub(crate) fn check(composer: &Entity<ComposerState>, cx: &mut App) {
    let spelling = cx.default_global::<Spelling>();
    spelling
        .composers
        .retain(|composer| composer.upgrade().is_some());
    spelling.composers.push(composer.downgrade());
    if !cx.has_global::<IgnoreRegistered>() {
        cx.set_global(IgnoreRegistered);
        cx.on_action(|action: &IgnoreSpelling, cx| ignore(&action.word, cx));
    }

    composer.update(cx, |state, cx| {
        state.set_on_misspelling_menu(
            |menu, _, cx| {
                let Some(speller) = speller(cx) else {
                    return;
                };
                let word = menu.word().clone();
                let suggestions = speller.borrow().suggestions(&word, SUGGESTIONS);
                for suggestion in suggestions {
                    menu.suggestion(suggestion);
                }
                menu.separator();
                menu.item(t!("spelling-ignore"), Box::new(IgnoreSpelling { word }));
            },
            cx,
        );
    });

    let input = composer.read(cx).input().clone();
    let weak = composer.downgrade();
    let pending: Rc<RefCell<Option<Task<()>>>> = Rc::default();
    cx.subscribe(&input, move |_, event: &InputEvent, cx| {
        if !matches!(event, InputEvent::Change) {
            return;
        }
        let weak = weak.clone();
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(PAUSE).await;
            cx.update(|cx| {
                if let Some(composer) = weak.upgrade() {
                    underline(&composer, cx);
                }
            });
        });
        // A newer edit restarts the wait.
        pending.replace(Some(task));
    })
    .detach();
}

/// Marker that the ignore action has a handler.
struct IgnoreRegistered;

impl gpui_kit::Global for IgnoreRegistered {}

/// Underlines the draft's misspelled words.
fn underline(composer: &Entity<ComposerState>, cx: &mut App) {
    let Some(speller) = speller(cx) else {
        return;
    };
    let text = composer.read(cx).value(cx);
    let ranges = speller.borrow().misspelled(&text);
    composer.update(cx, |state, cx| state.set_misspellings(ranges, cx));
}

/// Ignores `word` and checks every draft again without it.
fn ignore(word: &str, cx: &mut App) {
    let Some(speller) = speller(cx) else {
        return;
    };
    speller.borrow_mut().ignore(word);
    let composers: Vec<_> = cx
        .default_global::<Spelling>()
        .composers
        .iter()
        .filter_map(WeakEntity::upgrade)
        .collect();
    for composer in composers {
        underline(&composer, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// matty-spell's test dictionary, so the result does not depend on the
    /// machine's.
    fn use_test_dictionary(cx: &mut App) {
        let speller = Speller::from_hunspell(
            include_str!("../../matty-spell/tests/fixtures/en_test.aff"),
            include_str!("../../matty-spell/tests/fixtures/en_test.dic"),
        )
        .expect("the test dictionary reads");
        cx.default_global::<Spelling>().speller = Some(Some(Rc::new(RefCell::new(speller))));
    }

    #[gpui_kit::test]
    fn drafts_are_underlined_once_typing_pauses(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
            use_test_dictionary(cx);
        });
        let (composer, cx) = cx.add_window_view(|window, cx| ComposerState::new(4, window, cx));
        cx.update(|_, cx| check(&composer, cx));
        let underlined = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|_, cx| {
                let composer = composer.read(cx);
                let text = composer.value(cx);
                composer
                    .misspellings()
                    .read(cx)
                    .ranges()
                    .iter()
                    .map(|range| text[range.clone()].to_string())
                    .collect::<Vec<_>>()
            })
        };
        cx.update(|window, cx| {
            use gpui_kit::Focusable as _;
            let input = composer.read(cx).input().clone();
            input.read(cx).focus_handle(cx).focus(window, cx);
        });
        cx.simulate_input("the qick brown, `qick` hello");
        // Not before the pause.
        cx.run_until_parked();
        assert!(underlined(cx).is_empty());
        cx.executor().advance_clock(PAUSE);
        cx.run_until_parked();
        assert_eq!(underlined(cx), ["qick"]);

        cx.update(|window, cx| {
            window.dispatch_action(
                Box::new(IgnoreSpelling {
                    word: "qick".into(),
                }),
                cx,
            )
        });
        cx.run_until_parked();
        assert!(
            underlined(cx).is_empty(),
            "ignored words are not underlined"
        );
    }
}
