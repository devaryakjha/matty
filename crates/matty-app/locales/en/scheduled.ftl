# Scheduled messages: the composer's schedule menu and dialog, and the
# Scheduled tab of the Drafts view.

schedule-message = Schedule message
schedule-preset-tomorrow = Tomorrow at { $time }
schedule-preset-monday = Monday at { $time }
schedule-custom-time = Custom time…
# $day is today, tomorrow or other; $date is set for other.
schedule-when = { $day ->
    [today] Today at { $time }
    [tomorrow] Tomorrow at { $time }
   *[other] { $date } at { $time }
}
schedule-edit-title = Edit scheduled message
schedule-description = It posts at the time you pick, in your time zone, even while Matty is closed.
schedule-posts-at = Posts at
schedule-time-passed = Pick a time that is still ahead.
schedule-cancel = Cancel
schedule-save = Save
schedule-confirm = Schedule

scheduled-in-channel = { $count ->
    [one] { $count } scheduled message in this channel
   *[other] { $count } scheduled messages in this channel
}
scheduled-see-all = See all
scheduled-send-at = { $day ->
    [today] Send today at { $time }
    [tomorrow] Send tomorrow at { $time }
   *[other] Send on { $date } at { $time }
}
scheduled-failed = Could not send
scheduled-delete = Delete
scheduled-open = Open
scheduled-edit = Edit
scheduled-send-now = Send now
scheduled-empty-title = No scheduled messages
scheduled-empty-body = Schedule a message from the arrow beside Send, and it waits here until it posts.
