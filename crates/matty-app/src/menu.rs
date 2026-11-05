//! The native app menu (#82, PLAN §4.4): Matty, File, Edit, View, Go,
//! Window and Help. Items that have a shortcut take their label and action
//! from the keymap row in [`actions::bindings`], so the menu and the
//! shortcut always do the same thing and macOS shows the keys beside the
//! item. Linux and Windows have no app menu bar; GPUI keeps the menus there
//! without drawing them.

use gpui_kit::base::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::{Action, App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType};

use crate::actions::{self, Binding};
use crate::t;

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

/// The first menu's title, the app's name in every language.
const APP_NAME: &str = "Matty"; // i18n-ignore: a name

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
    cx.on_action(|_: &crate::actions::CloseWindow, cx| {
        with_active_window(cx, |window, _| window.remove_window())
    });
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
    let item = |action: &dyn Action| keymap_item(&keymap, action, false);
    let dialog = |action: &dyn Action| keymap_item(&keymap, action, true);
    vec![
        Menu::new(APP_NAME).items([
            dialog(&actions::OpenPreferences),
            MenuItem::separator(),
            MenuItem::action(t!("menu-add-server"), actions::AddServer),
            MenuItem::action(t!("menu-sign-out"), SignOut),
            MenuItem::separator(),
            MenuItem::os_submenu(t!("menu-services"), SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(t!("menu-hide"), Hide),
            MenuItem::action(t!("menu-hide-others"), HideOthers),
            MenuItem::action(t!("menu-show-all"), ShowAll),
            MenuItem::separator(),
            MenuItem::action(t!("menu-quit"), Quit),
        ]),
        Menu::new(t!("menu-file")).items([
            item(&actions::OpenInNewWindow),
            MenuItem::separator(),
            dialog(&actions::UploadFiles),
            MenuItem::separator(),
            dialog(&actions::CreateChannel),
            dialog(&actions::BrowseChannels),
        ]),
        Menu::new(t!("menu-edit")).items([
            MenuItem::os_action(t!("menu-undo"), Undo, OsAction::Undo),
            MenuItem::os_action(t!("menu-redo"), Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action(t!("menu-cut"), Cut, OsAction::Cut),
            MenuItem::os_action(t!("menu-copy"), Copy, OsAction::Copy),
            MenuItem::os_action(t!("menu-paste"), Paste, OsAction::Paste),
            MenuItem::os_action(t!("menu-select-all"), SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            item(&actions::Search),
        ]),
        Menu::new(t!("menu-view")).items([
            item(&actions::ToggleSidebar),
            item(&actions::ToggleRightPane),
            item(&actions::ExpandRightPane),
            item(&actions::UnreadsFilter),
            MenuItem::separator(),
            item(&actions::OpenInSplitView),
            item(&actions::FocusOtherPane),
            item(&actions::SwapPanes),
            item(&actions::ClosePane),
            MenuItem::separator(),
            item(&actions::ChannelInfo),
            item(&actions::RecentMentions),
            item(&actions::AllUnreads),
            item(&actions::SavedMessages),
            MenuItem::separator(),
            item(&actions::ZoomIn),
            item(&actions::ZoomOut),
            item(&actions::ResetZoom),
        ]),
        Menu::new(t!("menu-go")).items([
            dialog(&actions::QuickSwitcher),
            dialog(&actions::DirectMessagesMenu),
            MenuItem::separator(),
            item(&actions::PreviousChannel),
            item(&actions::NextChannel),
            item(&actions::PreviousUnreadChannel),
            item(&actions::NextUnreadChannel),
            MenuItem::separator(),
            item(&actions::PreviousTeam),
            item(&actions::NextTeam),
            MenuItem::separator(),
            item(&actions::HistoryBack),
            item(&actions::HistoryForward),
        ]),
        Menu::new(t!("menu-window")).items([
            MenuItem::action(t!("menu-minimize"), Minimize),
            MenuItem::action(t!("menu-zoom"), ZoomWindow),
            MenuItem::separator(),
            item(&actions::CloseWindow),
        ]),
        Menu::new(t!("menu-help")).items([
            item(&actions::ShowShortcuts),
            MenuItem::separator(),
            MenuItem::action(t!("menu-open-logs"), OpenLogs),
            MenuItem::action(t!("menu-report-problem"), ReportProblem),
        ]),
    ]
}

/// The item for the keymap row that dispatches `action`, named as the row
/// is; `dialog` adds the ellipsis of an item that opens a dialog. An
/// action missing from the keymap is a bug, caught by the tests.
fn keymap_item(keymap: &[Binding], action: &dyn Action, dialog: bool) -> MenuItem {
    let row = keymap
        .iter()
        .find(|row| (row.action)().name() == action.name())
        .unwrap_or_else(|| panic!("no keymap row dispatches {}", action.name())); // i18n-ignore: a panic
    MenuItem::Action {
        name: if dialog {
            t!("menu-dialog-item", name = row.name().to_string())
        } else {
            row.name()
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
