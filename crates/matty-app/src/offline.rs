//! Actions that need the server while Matty is offline (#163).
//!
//! Reading works from the cache, and sends and drafts wait on this
//! computer. Anything else the server has to do is refused where the user
//! asked for it, before the view changes, with one toast that says so: the
//! action stays where it was, so the user tries again once back online. A
//! command that the network dropped on its way comes back from sync as
//! [`matty_sync::CommandError::Offline`] and reads the same way.

use gpui_kit::SharedString;

/// The toast's title for every refusal.
pub const TITLE: &str = "You're offline";

/// What every refusal adds: nothing was lost, and when it can work again.
pub const AGAIN: &str = "Try again when you're back online.";

/// The toast's first sentence for a refused action, named as sync names
/// it (`CommandError::Offline::action`), or `None` for one the user did not
/// ask for: background writes such as marking a channel viewed or saving a
/// draft, and loads whose pane already says it is offline.
pub fn refusal(action: &str) -> Option<&'static str> {
    Some(match action {
        "add reaction" => "Couldn't add the reaction.",
        "remove reaction" => "Couldn't remove the reaction.",
        "edit" => "Couldn't save your edit.",
        "delete" => "Couldn't delete the message.",
        "save message" => "Couldn't save the message.",
        "unsave message" => "Couldn't remove the message from Saved.",
        "pin message" => "Couldn't pin the message.",
        "unpin message" => "Couldn't unpin the message.",
        "mark unread" => "Couldn't mark the message unread.",
        "set reminder" => "Couldn't set the reminder.",
        "follow thread" => "Couldn't follow the thread.",
        "unfollow thread" => "Couldn't unfollow the thread.",
        "open direct message" => "Couldn't open the conversation.",
        "join channel" => "Couldn't join the channel.",
        "leave channel" => "Couldn't leave the channel.",
        "archive channel" => "Couldn't archive the channel.",
        "create channel" => "Couldn't create the channel.",
        "change channel" => "Couldn't change the channel.",
        "change channel privacy" => "Couldn't make the channel private.",
        "change notifications" => "Couldn't change the channel's notifications.",
        "add members" => "Couldn't add the members.",
        "remove member" => "Couldn't remove the member.",
        "set status" => "Couldn't change your status.",
        "save your profile" => "Couldn't save your profile.",
        "change your picture" => "Couldn't change your picture.",
        "remove your picture" => "Couldn't remove your picture.",
        _ => return None,
    })
}

/// The toast for a refused action: [`TITLE`], then what did not happen
/// and [`AGAIN`].
pub fn notice(action: &str) -> Option<(SharedString, SharedString)> {
    let refused = refusal(action)?;
    Some((TITLE.into(), format!("{refused} {AGAIN}").into()))
}

/// The right pane's empty state while offline, by what it lists: a
/// listing that may hold more than this computer saved says so rather than
/// "nothing here".
pub fn pane_empty(listing: &str) -> (&'static str, &'static str) {
    match listing {
        "search" => (
            TITLE,
            "Search runs on the server. Try again when you're back online.",
        ),
        "mentions" => (
            TITLE,
            "Recent mentions load from the server when you're back online.",
        ),
        "saved" => (
            TITLE,
            "None of your saved messages are on this computer yet. They load when you're back online.",
        ),
        "pinned" => (
            TITLE,
            "None of this channel's pinned messages are on this computer yet. They load when you're back online.",
        ),
        _ => (
            TITLE,
            "Matty hasn't saved this thread on this computer yet. Its replies load when you're back online.",
        ),
    }
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
                text.starts_with("Couldn't ") && text.ends_with(AGAIN),
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
