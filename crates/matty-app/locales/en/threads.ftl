## Threads view (threads.rs)

threads-title = Threads
threads-filter = Show
threads-filter-all = All
threads-filter-unread = Unread
threads-filter-unread-count = Unread { $count }
threads-mark-all-read = Mark all as read
threads-more-failed = Couldn't load more threads.
threads-retry = Retry
threads-loading-more = Loading more threads
threads-loading-more-text = Loading more threads…
threads-in-channel = in { $channel }
threads-replies = { $count ->
    [one] 1 reply
   *[other] { $count } replies
}
threads-new-replies = { $count ->
    [one] 1 new reply
   *[other] { $count } new replies
}
# A thread row's accessible name.
threads-row-label = { $mentions ->
    [0] { $author }, { $replies }, in { $channel }
    [one] { $author }, { $replies }, 1 mention, in { $channel }
   *[other] { $author }, { $replies }, { $mentions } mentions, in { $channel }
}
threads-empty-unread = No unread threads
threads-empty-unread-hint = You are caught up. New replies will show here.
threads-empty = No followed threads
threads-empty-hint = Reply in a thread, or choose Follow on one, and it shows up here.
