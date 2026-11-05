//! Actions that need the server while Matty is offline (#163).
//!
//! Reading works from the cache, and sends and drafts wait on this
//! computer. Anything else the server has to do is refused where the user
//! asked for it, before the view changes, with one toast that says so: the
//! action stays where it was, so the user tries again once back online. A
//! command that the network dropped on its way comes back from sync as
//! [`matty_sync::CommandError::Offline`] and reads the same way.

use gpui_kit::SharedString;

use crate::t;

/// The toast's first sentence for a refused action, named as sync names
/// it (`CommandError::Offline::action`), or `None` for one the user did not
/// ask for: background writes such as marking a channel viewed or saving a
/// draft, and loads whose pane already says it is offline.
pub fn refusal(action: &str) -> Option<SharedString> {
    Some(match action {
        "add reaction" => t!("offline-add-reaction"),
        "remove reaction" => t!("offline-remove-reaction"),
        "edit" => t!("offline-edit"),
        "delete" => t!("offline-delete"),
        "save message" => t!("offline-save-message"),
        "unsave message" => t!("offline-unsave-message"),
        "pin message" => t!("offline-pin-message"),
        "unpin message" => t!("offline-unpin-message"),
        "mark unread" => t!("offline-mark-unread"),
        "set reminder" => t!("offline-set-reminder"),
        "acknowledge" => t!("offline-acknowledge"),
        "remove acknowledgement" => t!("offline-remove-acknowledgement"),
        "schedule message" => t!("offline-schedule-message"),
        "change scheduled message" => t!("offline-change-scheduled-message"),
        "delete scheduled message" => t!("offline-delete-scheduled-message"),
        "run action" => t!("offline-run-action"),
        "run command" => t!("offline-run-command"),
        "follow thread" => t!("offline-follow-thread"),
        "unfollow thread" => t!("offline-unfollow-thread"),
        "open direct message" => t!("offline-open-direct-message"),
        "join channel" => t!("offline-join-channel"),
        "leave channel" => t!("offline-leave-channel"),
        "archive channel" => t!("offline-archive-channel"),
        "create channel" => t!("offline-create-channel"),
        "change channel" => t!("offline-change-channel"),
        "change channel privacy" => t!("offline-change-channel-privacy"),
        "change notifications" => t!("offline-change-notifications"),
        "add members" => t!("offline-add-members"),
        "remove member" => t!("offline-remove-member"),
        "set status" => t!("offline-set-status"),
        "save your profile" => t!("offline-save-your-profile"),
        "change your picture" => t!("offline-change-your-picture"),
        "remove your picture" => t!("offline-remove-your-picture"),
        _ => return None,
    })
}

/// The toast for a refused action: "You're offline", then what did not
/// happen and when it can work again.
pub fn notice(action: &str) -> Option<(SharedString, SharedString)> {
    let refused = refusal(action)?;
    Some((
        t!("offline-title"),
        t!("offline-notice", refused = refused.to_string()),
    ))
}

/// The right pane's empty state while offline, by what it lists: a
/// listing that may hold more than this computer saved says so rather than
/// "nothing here".
pub fn pane_empty(listing: &str) -> (SharedString, SharedString) {
    let text = match listing {
        "search" => t!("offline-empty-search"),
        "mentions" => t!("offline-empty-mentions"),
        "unreads" => t!("offline-empty-unreads"),
        "saved" => t!("offline-empty-saved"),
        "files" => t!("offline-empty-files"),
        "pinned" => t!("offline-empty-pinned"),
        _ => t!("offline-empty-thread"),
    };
    (t!("offline-title"), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_user_asked_for_is_refused_by_name() {
        let (title, text) = notice("add reaction").unwrap();
        assert_eq!(title, "You're offline");
        assert_eq!(
            text,
            "Couldn't add the reaction. Try again when you're back online."
        );
        for action in [
            "remove reaction",
            "edit",
            "delete",
            "save message",
            "unsave message",
            "pin message",
            "unpin message",
            "mark unread",
            "set reminder",
            "follow thread",
            "unfollow thread",
        ] {
            let (_, text) = notice(action).unwrap();
            assert!(
                text.starts_with("Couldn't ")
                    && text.ends_with("Try again when you're back online."),
                "{action}"
            );
        }
    }

    #[test]
    fn background_writes_and_loads_stay_quiet() {
        // Marking a channel viewed, drafts and preferences are not something
        // the user just did, and a pane that loads says so itself.
        for action in [
            "view",
            "save draft",
            "delete draft",
            "save preferences",
            "mark thread read",
            "suggest people",
            "open link",
            "search",
            "load recent mentions",
            "load saved messages",
            "send",
        ] {
            assert_eq!(notice(action), None, "{action}");
        }
    }

    #[test]
    fn panes_say_what_waits_for_the_server() {
        assert!(
            pane_empty("search")
                .1
                .starts_with("Search runs on the server")
        );
        assert!(pane_empty("thread").1.contains("hasn't saved this thread"));
        assert!(pane_empty("saved").1.contains("saved messages"));
        assert!(pane_empty("pinned").1.contains("pinned messages"));
    }
}
