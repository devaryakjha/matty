//! The native app menu (#82, PLAN §4.4): Matty, File, Edit, View, Go,
//! Window and Help. Items that have a shortcut take their label and action
//! from the keymap row in [`actions::bindings`], so the menu and the
//! shortcut always do the same thing and macOS shows the keys beside the
//! item. Linux and Windows have no app menu bar; GPUI keeps the menus there
//! without drawing them.

use gpui_kit::base::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::{App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType};

use crate::actions::{self, Binding};

gpui_kit::actions!(
    matty,
    [
        /// Sign out of the server and return to the sign-in card.
        SignOut,
        /// Quit Matty.
        Quit,
        /// Hide Matty.
        Hide,
        /// Hide the other apps.
        HideOthers,
        /// Show the hidden apps again.
        ShowAll,
        /// Minimize the window.
        Minimize,
        /// Zoom the window, as double-clicking the title bar does.
        ZoomWindow,
        /// Open the folder with Matty's logs.
        OpenLogs,
        /// Open a new issue on Matty's tracker.
        ReportProblem,
    ]
);

/// Where "Report a problem" goes.
const REPORT_URL: &str = "https://github.com/devaryakjha/matty/issues/new";

/// Installs the menu bar, the macOS-only shortcuts of its items and the
/// app-wide handlers of the actions no view owns.
pub fn init(cx: &mut App) {
    if cfg!(target_os = "macos") {
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("alt-cmd-h", HideOthers, None),
            KeyBinding::new("cmd-m", Minimize, None),
        ]);
    }
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Minimize, cx| with_active_window(cx, |window, _| window.minimize_window()));
    cx.on_action(|_: &ZoomWindow, cx| with_active_window(cx, |window, _| window.zoom_window()));
    cx.on_action(|_: &OpenLogs, cx| {
        if let Some(dir) = crate::logging::default_dir() {
            let _ = std::fs::create_dir_all(&dir);
            cx.open_with_system(&dir);
        }
    });
    cx.on_action(|_: &ReportProblem, cx| cx.open_url(REPORT_URL));
    // A focused text field copies its own selection; anywhere else the
    // menu's Copy takes the transcript's selection, as Cmd+C does.
    cx.on_action(|_: &Copy, cx| {
        with_active_window(cx, |window, cx| {
            crate::ui::markdown::copy_selection(window, cx);
        })
    });
    cx.set_menus(menus());
}

/// Runs `f` on the active window once the current dispatch, which holds
/// the window, has returned.
fn with_active_window(cx: &mut App, f: impl FnOnce(&mut gpui_kit::Window, &mut App) + 'static) {
    let Some(handle) = cx.active_window() else {
        return;
    };
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, window, cx| f(window, cx));
    });
}

/// The menu bar.
pub fn menus() -> Vec<Menu> {
    let keymap = actions::bindings();
    let item = |label: &str| keymap_item(&keymap, label, false);
    let dialog = |label: &str| keymap_item(&keymap, label, true);
    vec![
        Menu::new("Matty").items([
            dialog("Preferences"),
            MenuItem::separator(),
            MenuItem::action("Add server\u{2026}", actions::AddServer),
            MenuItem::action("Sign out", SignOut),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Matty", Hide),
            MenuItem::action("Hide others", HideOthers),
            MenuItem::action("Show all", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Matty", Quit),
        ]),
        Menu::new("File").items([
            dialog("Upload files"),
            MenuItem::separator(),
            dialog("Create channel"),
            dialog("Browse channels"),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::os_action("Select all", SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            item("Search"),
        ]),
        Menu::new("View").items([
            item("Toggle sidebar"),
            item("Toggle right pane"),
            item("Expand right pane"),
            item("Unreads filter"),
            MenuItem::separator(),
            item("Channel info"),
            item("Recent mentions"),
            item("Saved messages"),
            MenuItem::separator(),
            item("Zoom in"),
            item("Zoom out"),
            item("Reset zoom"),
        ]),
        Menu::new("Go").items([
            dialog("Quick switcher"),
            dialog("Direct messages"),
            MenuItem::separator(),
            item("Previous channel"),
            item("Next channel"),
            item("Previous unread channel"),
            item("Next unread channel"),
            MenuItem::separator(),
            item("Previous team"),
            item("Next team"),
            MenuItem::separator(),
            item("Back"),
            item("Forward"),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", ZoomWindow),
        ]),
        Menu::new("Help").items([
            item("Keyboard shortcuts"),
            MenuItem::separator(),
            MenuItem::action("Open logs folder", OpenLogs),
            MenuItem::action("Report a problem", ReportProblem),
        ]),
    ]
}

/// The item for the keymap row labelled `label`, with its action; `dialog`
/// adds the ellipsis of an item that opens a dialog. A label missing from
/// the keymap is a bug, caught by the tests.
fn keymap_item(keymap: &[Binding], label: &str, dialog: bool) -> MenuItem {
    let row = keymap
        .iter()
        .find(|row| row.label == label)
        .unwrap_or_else(|| panic!("no keymap row is labelled {label:?}"));
    MenuItem::Action {
        name: if dialog {
            format!("{label}\u{2026}").into()
        } else {
            label.to_owned().into()
        },
        action: (row.action)(),
        os_action: None,
        checked: false,
        disabled: false,
    }
}

/// The name of the action `item` dispatches, if it dispatches one.
#[cfg(test)]
fn action_name(item: &MenuItem) -> Option<(&str, &'static str)> {
    match item {
        MenuItem::Action { name, action, .. } => Some((name.as_ref(), action.name())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use gpui_kit::Action as _;

    use super::*;

    fn items() -> Vec<(String, &'static str)> {
        menus()
            .iter()
            .flat_map(|menu| menu.items.iter())
            .filter_map(action_name)
            .map(|(name, action)| (name.to_owned(), action))
            .collect()
    }

    #[test]
    fn every_menu_item_dispatches_its_shortcuts_action() {
        let keymap = actions::bindings();
        for (name, action) in items() {
            let label = name.trim_end_matches('\u{2026}');
            if let Some(row) = keymap.iter().find(|row| row.label == label) {
                assert_eq!(
                    action,
                    (row.action)().name(),
                    "{name:?} dispatches another action than its shortcut"
                );
            }
        }
    }

    #[test]
    fn no_action_appears_twice() {
        let items = items();
        let unique: HashSet<_> = items.iter().map(|(_, action)| *action).collect();
        assert_eq!(unique.len(), items.len());
    }

    #[test]
    fn the_menu_has_the_planned_titles_and_items() {
        let titles: Vec<_> = menus().iter().map(|menu| menu.name.to_string()).collect();
        assert_eq!(
            titles,
            ["Matty", "File", "Edit", "View", "Go", "Window", "Help"]
        );
        let actions: HashSet<_> = items().into_iter().map(|(_, action)| action).collect();
        for expected in [
            actions::OpenPreferences.name(),
            SignOut.name(),
            Quit.name(),
            actions::UploadFiles.name(),
            actions::UnreadsFilter.name(),
            actions::ShowShortcuts.name(),
            Undo.name(),
            Redo.name(),
            Cut.name(),
            Copy.name(),
            Paste.name(),
            SelectAll.name(),
        ] {
            assert!(actions.contains(expected), "the menu lacks {expected}");
        }
    }
}
