## The right pane (right_pane.rs): threads, search, channel info and
## the saved, pinned, mentions and unreads listings.

pane-thread = Thread
pane-channel-info = Channel info
pane-files = Files
pane-pinned = Pinned messages
pane-saved = Saved messages
pane-mentions = Recent mentions
pane-unreads = Unreads
pane-search = Search
pane-profile = Profile

pane-open-in-window = Open in new window
pane-expand = Expand
pane-close = Close
pane-download = Download
pane-message-actions = Message actions
pane-jump = Jump
pane-loading = Loading…

## The thread

pane-reply-placeholder = Reply…
pane-reply = Reply
pane-attach = Attach files
pane-emoji = Emoji
pane-drop = Files go into your reply.
pane-replies = { $count ->
        [one] { $count } reply
       *[other] { $count } replies
    }
pane-following = Following
pane-follow = Follow
pane-older-offline = Older replies load when you're back online.
pane-older-failed = Couldn't load older replies.
pane-older-retry = Retry
pane-older-loading = Loading older replies
pane-older-loading-text = Loading older replies…
pane-no-thread = No thread open
pane-no-thread-detail = Hover a message and choose Reply in thread to follow it here.
pane-discard-place = the channel
pane-sent-at = { $date } at { $time }

## A message's menu and hover toolbar

pane-add-reaction = Add reaction
pane-add-reaction-menu = Add reaction…
pane-more-actions = More actions
pane-copy-text = Copy text
pane-copy-markdown = Copy as Markdown
pane-dismiss = Dismiss message
pane-forward = Forward…
pane-mark-unread = Mark as unread
pane-remind = Remind me
pane-save = Save
pane-unsave = Unsave
pane-pin = Pin to channel
pane-unpin = Unpin from channel
pane-copy-link = Copy link
pane-retry = Retry
pane-edit = Edit
pane-delete = Delete…

## Search

pane-search-placeholder = Search messages
pane-search-empty = Search messages
pane-search-empty-detail = Words, “exact phrases”, from:someone, in:channel, on:, before: or after: a date, -word to leave a word out, and depl* for words that start with depl.
pane-search-hint = from:name  in:channel  on:date  -word  “exact phrase”
pane-searching = Searching for “{ $query }”…
pane-search-count = { $count ->
        [one] { $count } result for “{ $query }”
       *[other] { $count } results for “{ $query }”
    }
pane-search-nothing = Nothing matched
pane-search-nothing-detail = Try fewer words, or narrow with from: and in:.
pane-search-results = Search results
pane-search-messages-tab = Messages ({ $count }{ $more })
pane-search-files-tab = Files ({ $count })
pane-search-suggestion = Search { $query }
pane-search-more = Show more results
pane-search-loading-more = Loading more…
pane-recent-searches = Recent searches
pane-recent-clear = Clear
pane-recent-again = Search again for { $query }
pane-today = Today
pane-yesterday = Yesterday
pane-weekday-monday = Monday
pane-weekday-tuesday = Tuesday
pane-weekday-wednesday = Wednesday
pane-weekday-thursday = Thursday
pane-weekday-friday = Friday
pane-weekday-saturday = Saturday
pane-weekday-sunday = Sunday

## Files, pinned, saved, mentions and unreads

pane-files-empty = No files yet
pane-files-empty-detail = Files shared in this channel show up here.
pane-pinned-empty = Nothing pinned yet
pane-pinned-empty-detail = Pin a message from its More menu to keep it here for everyone.
pane-saved-empty = Nothing saved yet
pane-saved-empty-detail = Save a message from its hover toolbar to find it here later.
pane-mentions-empty = No mentions yet
pane-mentions-empty-detail = Messages that mention you or your keywords show up here.
pane-unreads-empty = You're all caught up
pane-unreads-empty-detail = Unread messages from every channel in the team show up here.

## Channel info

pane-favorite = Favorite
pane-favorited = Favorited
pane-mute = Mute
pane-muted = Muted
pane-pinned-count = { $count } pinned
pane-leave = Leave
pane-leave-channel = Leave channel
pane-members = Members · { $count }
pane-add-members = Add members
pane-add = Add
pane-all-members = Show all { $count } members
pane-make-admin = Make channel admin
pane-remove-admin = Remove channel admin
pane-remove-member = Remove from channel
pane-admin = Admin
pane-leave-this-channel = this channel
pane-leave-title = Leave { $name }?
pane-leave-description = You stop receiving its messages and it leaves your sidebar. You can join again later.
pane-leave-cancel = Cancel
