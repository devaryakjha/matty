## The channel view (channel.rs): header, transcript, message menus,
## composer and dialogs.

## Reactions

chan-reaction-you = You
chan-reaction-others = { $count ->
        [one] { $count } other
       *[other] { $count } others
    }
chan-reaction-nobody = Nobody
chan-list-and = { $rest } and { $last }
chan-list-separator = {", "}
chan-reacted-with = { $who } reacted with { $emoji }
chan-react-with = React with { $emoji }
chan-reaction-click-add = (click to add)
chan-reaction-click-remove = (click to remove)

## A post's header

chan-only-you = Only visible to you
chan-pinned = Pinned
chan-saved = Saved
chan-edited = (edited)
chan-pending-offline = Will send when back online
chan-sending = Sending…
chan-not-sent = Not sent
chan-sent-at = { $date } at { $time }
chan-unsupported = This message type isn't supported yet.
chan-open-in-browser = Open in browser

## Editing and deleting

chan-edit = Edit
chan-discard-menu = Discard…
chan-edit-hint = Enter to save · Escape to cancel
chan-cancel = Cancel
chan-save-edit = Save
chan-delete-description = { $replies ->
        [0] It is removed for everyone in { $place }. This cannot be undone.
        [one] It is removed for everyone in { $place }, along with its reply. This cannot be undone.
       *[other] It is removed for everyone in { $place }, along with its { $replies } replies. This cannot be undone.
    }
chan-discard-label = Discard message
chan-discard-title = Discard this message?
chan-discard-description = It has not been sent, so nobody else has seen it. This cannot be undone.
chan-discard = Discard
chan-delete-label = Delete message
chan-delete-title = Delete this message?
chan-delete = Delete
chan-this-conversation = this conversation
chan-the-channel = the channel
chan-this-channel = this channel

## Reminders

chan-remind-30-minutes = In 30 minutes
chan-remind-1-hour = In 1 hour
chan-remind-2-hours = In 2 hours
chan-remind-tomorrow = Tomorrow at 9:00

## A message's menus and hover toolbar

chan-copy-text = Copy text
chan-copy-markdown = Copy as Markdown
chan-dismiss = Dismiss message
chan-forward = Forward…
chan-follow-thread = Follow thread
chan-unfollow-thread = Unfollow thread
chan-mark-unread = Mark as unread
chan-remind = Remind me
chan-save = Save
chan-unsave = Unsave
chan-pin = Pin to channel
chan-unpin = Unpin from channel
chan-copy-link = Copy link
chan-delete-menu = Delete…
chan-reply-in-thread = Reply in thread
chan-thread-window = Open thread in new window
chan-add-reaction-menu = Add reaction…
chan-add-reaction = Add reaction
chan-more-actions = More actions
chan-message-actions = Message actions

## Threads under a post

chan-replies = { $count ->
        [one] { $count } reply
       *[other] { $count } replies
    }
chan-open-thread = Open thread, { $replies }
chan-last-reply = Last reply { $when }
chan-following = Following
chan-follow = Follow
chan-replying-label = Open thread: commented on { $name }'s message: { $excerpt }
chan-replying-before = Commented on
chan-replying-whose = { $name }'s
chan-replying-after = message:

## The transcript

chan-loading-newer = Loading newer messages
chan-loading-newer-text = Loading newer messages…
chan-newer-failed = Couldn't load newer messages.
chan-loading-older = Loading older messages
chan-loading-older-text = Loading older messages…
chan-older-failed = Couldn't load older messages.
chan-retry = Retry
chan-start-of = This is the start of { $name }.
chan-new-messages = New messages
chan-download = Download

## The header

chan-menu = Channel menu
chan-menu-notifications = Notification preferences…
chan-menu-header = Edit header…
chan-menu-purpose = Edit purpose…
chan-menu-rename = Rename channel…
chan-menu-add-members = Add members…
chan-menu-leave = Leave channel…
chan-menu-convert = Convert to private channel…
chan-menu-archive = Archive channel…
chan-add-members = Add members
chan-notifications = Notification preferences
chan-add-favorite = Add to favorites
chan-remove-favorite = Remove from favorites
chan-apps = Apps
chan-members-tip = Members
chan-members-count = { $count } members
chan-pinned-tip = Pinned messages
chan-pinned-count = { $count } pinned messages
chan-info = Channel info
chan-swap-panes = Swap panes
chan-close-pane = Close pane
chan-local-time = { $time } local time

## Archive, convert and leave

chan-archive-title = Archive { $name }?
chan-archive-description = Nobody can post in it any more and it leaves everyone's sidebar. Its messages and files stay on the server.
chan-archive = Archive
chan-convert-title = Make { $name } private?
chan-convert-description = Only its members will see it, and people join by invitation. Its history stays. This can't be undone from Matty.
chan-convert = Make private
chan-leave-label = Leave channel
chan-leave-title = Leave { $name }?
chan-leave-description = You stop receiving its messages and it leaves your sidebar. You can join again later.
chan-leave = Leave

## The composer

chan-placeholder-direct = Message { $name }
chan-placeholder-channel = Message #{ $name }
chan-placeholder-none = Write a message…
chan-emoji = Emoji
chan-attach = Attach files
chan-bold = Bold
chan-italic = Italic
chan-code = Code
chan-show-formatting = Show formatting
chan-hide-formatting = Hide formatting
chan-preview = Preview
chan-mention = Mention someone
chan-send = Send
chan-drop = Files go into the message you are writing.
chan-typing-one = { $name } is typing…
chan-typing-two = { $first } and { $second } are typing…
chan-typing-many = Several people are typing…
chan-mention-here = Everyone online here
chan-mention-channel = Everyone in the channel

## Empty and offline channels

chan-offline-title = You're offline
chan-offline-place-direct = your conversation with { $name }
chan-offline-description = Matty hasn't saved { $place } on this computer yet. Its messages load when you're back online.
chan-start-direct = This is the start of your conversation with { $name }
chan-welcome = Welcome to #{ $name }
chan-empty-direct = Messages here stay between the two of you.
chan-empty-channel = Nothing has been posted yet. Say hello, or set a purpose so people know what belongs here.
chan-no-channel = No channel open
chan-no-channel-detail = Pick a channel or a conversation in the sidebar.
chan-not-connected = Not connected yet
chan-not-connected-detail = Your channels appear here once Matty is connected to a Mattermost server.
