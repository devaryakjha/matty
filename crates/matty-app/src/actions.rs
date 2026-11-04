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
    ]
);

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
        crate::settings::SendKey::Enter => format_keys("enter", "enter"),
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
pub const MAIN_WINDOW: &str = "MainWindow";
pub const CHANNEL_VIEW: &str = "ChannelView";
pub const RIGHT_PANE: &str = "RightPane";
pub const COMPOSER: &str = "Composer";
pub const COMPOSER_EMPTY: &str = "Composer && empty";

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
            "Previous unread channel",
            PreviousUnreadChannel,
            main,
            "alt-shift-up",
            "alt-shift-up"
        ),
        row!(
            "Next unread channel",
            NextUnreadChannel,
            main,
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
            "Saved messages",
            SavedMessages,
            main,
            "cmd-shift-b",
            "ctrl-shift-b"
        ),
        row!("Upload files", UploadFiles, main, "cmd-u", "ctrl-u"),
        row!("Bold", FormatBold, Some(COMPOSER), "cmd-b", "ctrl-b"),
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
    ]
}

/// Installs the bindings for the platform Matty runs on.
pub fn bind(cx: &mut App) {
    let platform = Platform::current();
    cx.bind_keys(
        bindings()
            .into_iter()
            .map(|row| (row.bind)(row.keys(platform), row.context)),
    );
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
    fn every_action_has_one_row() {
        let mut names: Vec<&str> = bindings().iter().map(|row| (row.action)().name()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "an action appears in two rows");
        assert_eq!(count, 37);
    }
}
