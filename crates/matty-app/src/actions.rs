//! Every action from PLAN §4.4 and its key bindings.
//!
//! Each action's doc comment names where its binding comes from: (MM) is
//! Mattermost's published shortcut, (Slack) a Slack convention, (Matty) our
//! own addition. Most are declared ahead of the views that handle them; an
//! unhandled action does nothing, and the shortcut sheet (`Cmd+/`) lists them all.

use gpui_kit::{Action, App, KeyBinding};

use crate::ui::kbd::format_shortcut;

gpui_kit::actions!(
    matty,
    [
        /// Open the quick switcher. (MM)
        QuickSwitcher,
        /// Open the direct messages menu. (MM)
        DirectMessagesMenu,
        /// Browse the team's channels. (Matty)
        BrowseChannels,
        /// Create a channel. (Matty)
        CreateChannel,
        /// Open search. (MM)
        Search,
        /// Focus search when focus is not in a text field. (Slack)
        FocusSearch,
        /// Go to the previous channel in the sidebar. (MM)
        PreviousChannel,
        /// Go to the next channel in the sidebar. (MM)
        NextChannel,
        /// Go to the previous team on the rail. (MM)
        PreviousTeam,
        /// Go to the next team on the rail. (MM)
        NextTeam,
        /// Go to the previous unread channel. (MM)
        PreviousUnreadChannel,
        /// Go to the next unread channel. (MM)
        NextUnreadChannel,
        /// Go back in channel history. (MM)
        HistoryBack,
        /// Go forward in channel history. (MM)
        HistoryForward,
        /// Show only unread channels in the sidebar. (MM)
        UnreadsFilter,
        /// Show channel info in the right pane. (MM)
        ChannelInfo,
        /// Open or close the right pane. (MM)
        ToggleRightPane,
        /// Expand the right pane. (MM)
        ExpandRightPane,
        /// Show recent mentions. (MM)
        RecentMentions,
        /// Show the first unread messages of every unread channel. (Slack)
        AllUnreads,
        /// Show saved messages. (Matty)
        SavedMessages,
        /// Pick files to upload to the current channel. (MM)
        UploadFiles,
        /// Edit your last message, from an empty composer. (MM)
        EditLastMessage,
        /// Reply to the last message, from an empty composer. (MM)
        ReplyToLastMessage,
        /// React to the last message. (MM)
        ReactToLastMessage,
        /// Wrap the composer selection in bold. (MM)
        FormatBold,
        /// Wrap the composer selection in italics. (MM)
        FormatItalic,
        /// Turn the composer selection into a link. (MM)
        InsertLink,
        /// Wrap the composer selection in strikethrough. (Slack)
        FormatStrike,
        /// Show the message as it will post, above the composer. (MM)
        TogglePreview,
        /// Wrap the composer selection in inline code. (Slack)
        FormatCode,
        /// Focus the composer. (MM)
        FocusComposer,
        /// Mark the current channel read. (Matty, from Slack)
        MarkChannelRead,
        /// Mark every channel read. (Matty, from Slack)
        MarkAllRead,
        /// Show or hide the sidebar. (Slack)
        ToggleSidebar,
        /// Close the right pane. (MM)
        CloseRightPane,
        /// Zoom in. (Matty)
        ZoomIn,
        /// Zoom out. (Matty)
        ZoomOut,
        /// Reset the zoom level. (Matty)
        ResetZoom,
        /// Open preferences. (Matty)
        OpenPreferences,
        /// Show the keyboard shortcut sheet. (MM)
        ShowShortcuts,
        /// Open the current channel, or the thread in the right pane, in a
        /// window of its own. (Matty, #222)
        OpenInNewWindow,
        /// Close a channel or thread window. (Matty, #222)
        CloseWindow,
        /// Open a channel beside the current one in split view. (Matty, #222)
        OpenInSplitView,
        /// Move focus to split view's other pane. (Matty, #222)
        FocusOtherPane,
        /// Swap split view's two channels. (Matty, #222)
        SwapPanes,
        /// Close split view's focused pane; the other fills the window.
        /// (Matty, #222)
        ClosePane,
    ]
);

gpui_kit::actions!(
    matty,
    [
        /// Add another server and sign in to it. (Matty, #206)
        AddServer,
        /// Switch to the first saved server. (Slack)
        SwitchServer1,
        /// Switch to the second saved server. (Slack)
        SwitchServer2,
        /// Switch to the third saved server. (Slack)
        SwitchServer3,
        /// Switch to the fourth saved server. (Slack)
        SwitchServer4,
        /// Switch to the fifth saved server. (Slack)
        SwitchServer5,
        /// Switch to the sixth saved server. (Slack)
        SwitchServer6,
        /// Switch to the seventh saved server. (Slack)
        SwitchServer7,
        /// Switch to the eighth saved server. (Slack)
        SwitchServer8,
        /// Switch to the ninth saved server. (Slack)
        SwitchServer9,
    ]
);

gpui_kit::actions!(
    matty,
    [
        /// Switch to the first team on the rail. (MM)
        SwitchTeam1,
        /// Switch to the second team on the rail. (MM)
        SwitchTeam2,
        /// Switch to the third team on the rail. (MM)
        SwitchTeam3,
        /// Switch to the fourth team on the rail. (MM)
        SwitchTeam4,
        /// Switch to the fifth team on the rail. (MM)
        SwitchTeam5,
        /// Switch to the sixth team on the rail. (MM)
        SwitchTeam6,
        /// Switch to the seventh team on the rail. (MM)
        SwitchTeam7,
        /// Switch to the eighth team on the rail. (MM)
        SwitchTeam8,
        /// Switch to the ninth team on the rail. (MM)
        SwitchTeam9,
    ]
);

/// Cmd+Option+1…9 on macOS and Ctrl+Alt+1…9 elsewhere switch to the team
/// in that place on the rail, as in Mattermost (#89). Like
/// [`server_bindings`] they stay out of [`bindings`].
pub fn team_bindings(platform: Platform) -> Vec<KeyBinding> {
    const MACOS: [&str; 9] = [
        "cmd-alt-1",
        "cmd-alt-2",
        "cmd-alt-3",
        "cmd-alt-4",
        "cmd-alt-5",
        "cmd-alt-6",
        "cmd-alt-7",
        "cmd-alt-8",
        "cmd-alt-9",
    ];
    const OTHER: [&str; 9] = [
        "ctrl-alt-1",
        "ctrl-alt-2",
        "ctrl-alt-3",
        "ctrl-alt-4",
        "ctrl-alt-5",
        "ctrl-alt-6",
        "ctrl-alt-7",
        "ctrl-alt-8",
        "ctrl-alt-9",
    ];
    let keys = match platform {
        Platform::MacOs => MACOS,
        Platform::Other => OTHER,
    };
    let main = Some(MAIN_WINDOW);
    vec![
        KeyBinding::new(keys[0], SwitchTeam1, main),
        KeyBinding::new(keys[1], SwitchTeam2, main),
        KeyBinding::new(keys[2], SwitchTeam3, main),
        KeyBinding::new(keys[3], SwitchTeam4, main),
        KeyBinding::new(keys[4], SwitchTeam5, main),
        KeyBinding::new(keys[5], SwitchTeam6, main),
        KeyBinding::new(keys[6], SwitchTeam7, main),
        KeyBinding::new(keys[7], SwitchTeam8, main),
        KeyBinding::new(keys[8], SwitchTeam9, main),
    ]
}

/// Cmd/Ctrl+1…9 switch to the saved server in that place on the rail
/// (PLAN §4.4, #206). They stay out of [`bindings`], whose rows the
/// shortcut sheet and the menu bar list one by one.
pub fn server_bindings(platform: Platform) -> Vec<KeyBinding> {
    const MACOS: [&str; 9] = [
        "cmd-1", "cmd-2", "cmd-3", "cmd-4", "cmd-5", "cmd-6", "cmd-7", "cmd-8", "cmd-9",
    ];
    const OTHER: [&str; 9] = [
        "ctrl-1", "ctrl-2", "ctrl-3", "ctrl-4", "ctrl-5", "ctrl-6", "ctrl-7", "ctrl-8", "ctrl-9",
    ];
    let keys = match platform {
        Platform::MacOs => MACOS,
        Platform::Other => OTHER,
    };
    let main = Some(MAIN_WINDOW);
    vec![
        KeyBinding::new(keys[0], SwitchServer1, main),
        KeyBinding::new(keys[1], SwitchServer2, main),
        KeyBinding::new(keys[2], SwitchServer3, main),
        KeyBinding::new(keys[3], SwitchServer4, main),
        KeyBinding::new(keys[4], SwitchServer5, main),
        KeyBinding::new(keys[5], SwitchServer6, main),
        KeyBinding::new(keys[6], SwitchServer7, main),
        KeyBinding::new(keys[7], SwitchServer8, main),
        KeyBinding::new(keys[8], SwitchServer9, main),
    ]
}

/// Which platform a binding table is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }

    /// The operating system gpuicn formats shortcuts for.
    pub fn os(self) -> gpui_kit::OperatingSystem {
        match self {
            Self::MacOs => gpui_kit::OperatingSystem::Mac,
            Self::Other => gpui_kit::OperatingSystem::Linux,
        }
    }
}

/// One row of the keymap: an action, its label in the shortcut sheet, the
/// key context it applies in, and its keystrokes on macOS and elsewhere.
pub struct Binding {
    pub label: &'static str,
    pub action: fn() -> Box<dyn Action>,
    bind: fn(&'static str, Option<&'static str>) -> KeyBinding,
    pub context: Option<&'static str>,
    pub macos: &'static str,
    pub other: &'static str,
}

impl Binding {
    pub fn keys(&self, platform: Platform) -> &'static str {
        match platform {
            Platform::MacOs => self.macos,
            Platform::Other => self.other,
        }
    }
}

/// The keystrokes bound to the row labelled `label`, formatted for this
/// platform (`⌘⇧I` on macOS, `Ctrl+Shift+I` elsewhere), or `None` when the
/// keymap has no such row.
pub fn shortcut_for(label: &str) -> Option<gpui_kit::SharedString> {
    let platform = Platform::current();
    bindings()
        .into_iter()
        .find(|row| row.label == label)
        .map(|row| format_shortcut(row.keys(platform), platform.os()))
}

/// `macos` or `other`, whichever this platform uses, formatted like
/// [`shortcut_for`]; for keys the composer handles outside the keymap.
pub fn format_keys(macos: &str, other: &str) -> gpui_kit::SharedString {
    let platform = Platform::current();
    let keys = match platform {
        Platform::MacOs => macos,
        Platform::Other => other,
    };
    format_shortcut(keys, platform.os())
}

/// Tooltip text for the send button: `label` and the key that sends.
pub fn send_tooltip(label: &str, send_on: crate::settings::SendKey) -> gpui_kit::SharedString {
    let keys = match send_on {
        crate::settings::SendKey::Enter | crate::settings::SendKey::CodeBlocks => {
            format_keys("enter", "enter")
        }
        crate::settings::SendKey::ModEnter => format_keys("cmd-enter", "ctrl-enter"),
    };
    format!("{label}  {keys}").into()
}

/// Tooltip text for an icon-only control (DESIGN.md §5): `label`, followed
/// by the shortcut of the keymap row labelled `binding` when there is one.
pub fn tooltip_text(label: &str, binding: &str) -> gpui_kit::SharedString {
    match shortcut_for(binding) {
        Some(keys) => format!("{label}  {keys}").into(),
        None => label.to_string().into(),
    }
}

/// Key contexts set by views.
/// Single keys on a selected message (Shift+Tab from the composer, then
/// Up and Down), for the shortcut sheet (#213). The channel and the thread
/// pane handle them in their `selection_key`.
pub const MESSAGE_KEYS: [(&str, &str); 12] = [
    ("R", "Reply in thread"),
    ("E", "Edit your message"),
    ("Delete", "Delete your message"),
    ("+", "Add reaction"),
    ("F", "Forward"),
    ("T", "Follow or unfollow thread"),
    ("U", "Mark as unread"),
    ("S", "Save or unsave"),
    ("P", "Pin or unpin"),
    ("C", "Copy text"),
    ("L", "Copy link"),
    ("Enter", "View images"),
];

pub const MAIN_WINDOW: &str = "MainWindow";
/// A reorderable list: the sidebar's, or the rail's servers (#206). With
/// focus in it, Alt+Shift+Up and Down move the focused row or category
/// instead of walking unreads.
pub const SIDEBAR_SORTABLE: &str = "SidebarSortable";
pub const CHANNEL_VIEW: &str = "ChannelView";
pub const RIGHT_PANE: &str = "RightPane";
pub const COMPOSER: &str = "Composer";
pub const COMPOSER_EMPTY: &str = "Composer && empty";
/// A channel or thread in a window of its own (#222).
pub const POP_OUT: &str = "PopOut";
/// Split view's second pane, in the main window (#222).
pub const SPLIT_PANE: &str = "SplitPane";

macro_rules! row {
    ($label:literal, $action:ident, $context:expr, $mac:literal, $other:literal) => {
        Binding {
            label: $label,
            action: || Box::new($action),
            bind: |keys, context| KeyBinding::new(keys, $action, context),
            context: $context,
            macos: $mac,
            other: $other,
        }
    };
}

/// The PLAN §4.4 table. Cmd on macOS, Ctrl elsewhere, unless noted.
pub fn bindings() -> Vec<Binding> {
    let main = Some(MAIN_WINDOW);
    vec![
        row!("Quick switcher", QuickSwitcher, main, "cmd-k", "ctrl-k"),
        row!(
            "Direct messages",
            DirectMessagesMenu,
            main,
            "cmd-shift-k",
            "ctrl-shift-k"
        ),
        row!(
            "Browse channels",
            BrowseChannels,
            main,
            "cmd-shift-j",
            "ctrl-shift-j"
        ),
        row!(
            "Create channel",
            CreateChannel,
            main,
            "cmd-shift-n",
            "ctrl-shift-n"
        ),
        row!("Search", Search, main, "cmd-shift-f", "ctrl-shift-f"),
        row!(
            "Focus search",
            FocusSearch,
            Some("MainWindow && !Input"),
            "cmd-f",
            "ctrl-f"
        ),
        row!(
            "Previous channel",
            PreviousChannel,
            main,
            "alt-up",
            "alt-up"
        ),
        row!("Next channel", NextChannel, main, "alt-down", "alt-down"),
        row!(
            "Previous team",
            PreviousTeam,
            main,
            "cmd-alt-up",
            "ctrl-alt-up"
        ),
        row!("Next team", NextTeam, main, "cmd-alt-down", "ctrl-alt-down"),
        row!(
            "Previous unread channel",
            PreviousUnreadChannel,
            Some("MainWindow && !SidebarSortable"),
            "alt-shift-up",
            "alt-shift-up"
        ),
        row!(
            "Next unread channel",
            NextUnreadChannel,
            Some("MainWindow && !SidebarSortable"),
            "alt-shift-down",
            "alt-shift-down"
        ),
        row!("Back", HistoryBack, main, "cmd-[", "alt-left"),
        row!("Forward", HistoryForward, main, "cmd-]", "alt-right"),
        row!(
            "Unreads filter",
            UnreadsFilter,
            main,
            "cmd-shift-u",
            "ctrl-shift-u"
        ),
        row!(
            "Channel info",
            ChannelInfo,
            main,
            "cmd-shift-i",
            "ctrl-shift-i"
        ),
        row!(
            "Toggle right pane",
            ToggleRightPane,
            main,
            "cmd-.",
            "ctrl-."
        ),
        row!(
            "Expand right pane",
            ExpandRightPane,
            main,
            "cmd-shift-.",
            "ctrl-shift-."
        ),
        row!(
            "Recent mentions",
            RecentMentions,
            main,
            "cmd-shift-m",
            "ctrl-shift-m"
        ),
        row!(
            "All unreads",
            AllUnreads,
            main,
            "cmd-shift-a",
            "ctrl-shift-a"
        ),
        row!(
            "Saved messages",
            SavedMessages,
            main,
            "cmd-shift-b",
            "ctrl-shift-b"
        ),
        row!("Upload files", UploadFiles, main, "cmd-u", "ctrl-u"),
        row!("Bold", FormatBold, Some(COMPOSER), "cmd-b", "ctrl-b"),
        row!(
            "Preview message",
            TogglePreview,
            Some(COMPOSER),
            "cmd-alt-p",
            "ctrl-alt-p"
        ),
        row!("Italic", FormatItalic, Some(COMPOSER), "cmd-i", "ctrl-i"),
        row!(
            "Link",
            InsertLink,
            Some(COMPOSER),
            "cmd-alt-k",
            "ctrl-alt-k"
        ),
        row!(
            "Strikethrough",
            FormatStrike,
            Some(COMPOSER),
            "cmd-shift-x",
            "ctrl-shift-x"
        ),
        row!(
            "Inline code",
            FormatCode,
            Some(COMPOSER),
            "cmd-shift-c",
            "ctrl-shift-c"
        ),
        row!(
            "Edit last message",
            EditLastMessage,
            Some(COMPOSER_EMPTY),
            "up",
            "up"
        ),
        row!(
            "Reply to last message",
            ReplyToLastMessage,
            Some(COMPOSER_EMPTY),
            "shift-up",
            "shift-up"
        ),
        row!(
            "React to last message",
            ReactToLastMessage,
            main,
            "cmd-shift-\\",
            "ctrl-shift-\\"
        ),
        row!(
            "Focus composer",
            FocusComposer,
            main,
            "cmd-shift-l",
            "ctrl-shift-l"
        ),
        row!(
            "Mark channel read",
            MarkChannelRead,
            Some(CHANNEL_VIEW),
            "escape",
            "escape"
        ),
        row!(
            "Mark all read",
            MarkAllRead,
            Some(CHANNEL_VIEW),
            "shift-escape",
            "shift-escape"
        ),
        row!(
            "Toggle sidebar",
            ToggleSidebar,
            main,
            "cmd-shift-d",
            "ctrl-shift-d"
        ),
        row!(
            "Close right pane",
            CloseRightPane,
            Some(RIGHT_PANE),
            "escape",
            "escape"
        ),
        row!("Zoom in", ZoomIn, main, "cmd-=", "ctrl-="),
        row!("Zoom out", ZoomOut, main, "cmd--", "ctrl--"),
        row!("Reset zoom", ResetZoom, main, "cmd-0", "ctrl-0"),
        row!("Preferences", OpenPreferences, main, "cmd-,", "ctrl-,"),
        row!("Keyboard shortcuts", ShowShortcuts, main, "cmd-/", "ctrl-/"),
        row!(
            "Open in new window",
            OpenInNewWindow,
            main,
            "cmd-shift-o",
            "ctrl-shift-o"
        ),
        row!(
            "Close window",
            CloseWindow,
            // A pop-out closes itself; the main window closes as its red
            // button does, and the dock brings it back (#615).
            None,
            "cmd-w",
            "ctrl-w"
        ),
        row!(
            "Open in split view",
            OpenInSplitView,
            main,
            "cmd-\\",
            "ctrl-\\"
        ),
        row!("Focus other pane", FocusOtherPane, main, "f6", "f6"),
        row!("Swap panes", SwapPanes, main, "cmd-alt-\\", "ctrl-alt-\\"),
        row!("Close pane", ClosePane, main, "cmd-alt-w", "ctrl-alt-w"),
    ]
}

/// Installs the bindings for the platform Matty runs on.
pub fn bind(cx: &mut App) {
    let platform = Platform::current();
    cx.bind_keys(
        bindings()
            .into_iter()
            .map(|row| (row.bind)(row.keys(platform), row.context))
            .chain(server_bindings(platform))
            .chain(team_bindings(platform)),
    );
    // On Linux and Windows a text field takes Alt+Shift+Up/Down to add a
    // cursor, which would swallow the unread jump from the composer. The
    // jump wins there, as in Mattermost; bound after the field's own keys,
    // these take precedence at the same depth.
    // A text field takes Cmd+Alt+Up/Down on macOS and Ctrl+Alt+Up/Down on
    // Windows to add a cursor, which would swallow the team keys from the
    // composer. Switching team wins, as in Mattermost.
    let (previous_team, next_team) = if platform == Platform::MacOs {
        ("cmd-alt-up", "cmd-alt-down")
    } else {
        ("ctrl-alt-up", "ctrl-alt-down")
    };
    cx.bind_keys([
        KeyBinding::new(previous_team, PreviousTeam, Some("Composer > Input")),
        KeyBinding::new(next_team, NextTeam, Some("Composer > Input")),
    ]);
    if platform != Platform::MacOs {
        cx.bind_keys([
            KeyBinding::new(
                "alt-shift-up",
                PreviousUnreadChannel,
                Some("Composer > Input"),
            ),
            KeyBinding::new(
                "alt-shift-down",
                NextUnreadChannel,
                Some("Composer > Input"),
            ),
        ]);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn no_two_bindings_collide_per_platform() {
        for platform in [Platform::MacOs, Platform::Other] {
            let mut seen: HashMap<(&str, Option<&str>), &str> = HashMap::new();
            for row in bindings() {
                if let Some(other) = seen.insert((row.keys(platform), row.context), row.label) {
                    panic!(
                        "{platform:?}: `{}` in {:?} is bound to both {other} and {}",
                        row.keys(platform),
                        row.context,
                        row.label
                    );
                }
            }
        }
    }

    #[test]
    fn every_keystroke_parses() {
        for platform in [Platform::MacOs, Platform::Other] {
            for row in bindings() {
                for key in row.keys(platform).split(' ') {
                    gpui_kit::Keystroke::parse(key)
                        .unwrap_or_else(|e| panic!("{}: `{key}` does not parse: {e:?}", row.label));
                }
            }
        }
    }

    #[test]
    fn server_switches_take_their_place_and_no_other_shortcut() {
        for platform in [Platform::MacOs, Platform::Other] {
            let taken: Vec<_> = bindings()
                .into_iter()
                .filter(|row| row.context == Some(MAIN_WINDOW))
                .map(|row| row.keys(platform))
                .collect();
            for n in 1..=9 {
                let keys = match platform {
                    Platform::MacOs => format!("cmd-{n}"),
                    Platform::Other => format!("ctrl-{n}"),
                };
                assert!(!taken.contains(&keys.as_str()), "{keys} is taken");
            }
            assert_eq!(server_bindings(platform).len(), 9);
        }
    }

    #[test]
    fn team_switches_take_their_place_and_no_other_shortcut() {
        for platform in [Platform::MacOs, Platform::Other] {
            let taken: Vec<_> = bindings()
                .into_iter()
                .map(|row| row.keys(platform))
                .collect();
            for n in 1..=9 {
                let keys = match platform {
                    Platform::MacOs => format!("cmd-alt-{n}"),
                    Platform::Other => format!("ctrl-alt-{n}"),
                };
                assert!(!taken.iter().any(|taken| *taken == keys), "{keys} is taken");
            }
            assert_eq!(team_bindings(platform).len(), 9);
        }
    }

    #[test]
    fn every_action_has_one_row() {
        let mut names: Vec<&str> = bindings().iter().map(|row| (row.action)().name()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "an action appears in two rows");
        assert_eq!(count, 47);
    }
}
