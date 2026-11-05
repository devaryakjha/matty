# Matty's UI strings in English, the source language (#225).
#
# Ids are grouped by area: one area per section, named after the file that
# shows them. Translations live beside this file as `locales/<tag>/matty.ftl`
# and may leave ids out: English stands in for anything missing.

## Custom emoji dialog (emoji_manager.rs)

emoji-manager-title = Custom emoji
emoji-manager-description = Add an image everyone on this server can use as an emoji.
emoji-manager-choose = Choose image…
emoji-manager-choose-another = Choose another image…
emoji-manager-choose-prompt = Choose image
emoji-manager-image-hint = PNG, JPEG or GIF, up to 512 KB.
emoji-manager-image-type = Choose a PNG, JPEG or GIF image.
emoji-manager-image-size = Choose an image of 512 KB or less.
emoji-manager-image-unreadable = Couldn't read that file.
emoji-manager-name = Name
emoji-manager-name-hint = Others use it as :{ $name }: in messages and reactions.
emoji-manager-name-empty = Others use it as :name: in messages and reactions.
emoji-manager-name-long = Use { $max } characters or fewer.
emoji-manager-name-characters = Use lowercase letters, numbers, -, + and _.
emoji-manager-name-taken = There's already an emoji called :{ $name }:.
emoji-manager-add = Add emoji
emoji-manager-listed = On this server ({ $count })
emoji-manager-delete = Delete
emoji-manager-delete-confirm = Delete for everyone
emoji-manager-delete-label = Delete :{ $name }:
emoji-manager-done = Done

## Sign-in card (login.rs)

login-ldap-username = AD/LDAP username
login-email-or-username = Email or username
login-mfa-wrong = That code is not right. Enter the current code from your authenticator app.
login-mfa-needed = This account needs a multi-factor code. Sign in again.
login-address-empty = Enter your server's address, like chat.example.com.
login-address-invalid = That does not look like a server address.
login-address-scheme = The address has to start with https://.
login-password-missing = Enter your email or username and password.
login-code-short = Enter the { $digits }-digit code from your authenticator app.
login-token-missing = Paste a personal access token.
login-add-title = Add a Mattermost server
login-connect-title = Connect to your Mattermost server
login-address-hint = Enter the address you open in the browser. Matty assumes https.
login-address = Server address
login-continue = Continue
login-connect-to = Connect to { $server }
login-recent = RECENT SERVERS
login-change-server = Change server
login-change = Change
login-sign-in-title = Sign in
login-sign-in-hint = Use the account you have on this server.
login-password = Password
login-sign-in = Sign in
login-continue-with = Continue with { $provider }
login-token-show = Use a personal access token
login-token-hide-toggle = Hide personal access token
login-token-show-toggle = Use a personal access token instead
login-token = Personal access token
login-token-hint = From Profile › Security › Personal access tokens.
login-token-sign-in = Sign in with token
login-method = Sign in with
login-method-ldap = AD/LDAP
login-mfa-title = Enter your code
login-mfa-hint = This account uses multi-factor authentication. Open your authenticator app and enter the code it shows.
login-mfa-code-for = { $digits }-digit code for { $server }
login-mfa-code = Authentication code
login-verify = Verify
login-back-to-sign-in = Back to sign in
login-back = Back
login-signing-in = Signing in…
login-waiting-for = Waiting for { $provider }…
login-browser-cancel = Cancel browser sign-in
login-cancel = Cancel
login-retry-label = Try { $provider } sign-in again
login-retry = Retry
login-browser-failed-title = Browser sign-in did not finish
login-browser-failed-hint = Try again to open a new sign-in page, or cancel to pick another way in.
login-browser-waiting-title = Waiting for your browser…
login-browser-waiting-hint = Finish signing in in the browser window Matty opened. Matty continues on its own when you are done.
login-footer = Matty is a native Mattermost client. Your password goes only to your server.

## Presence, status and Do not disturb (account.rs, custom_status.rs, dnd.rs)

presence-online = Online
presence-away = Away
presence-dnd = Do not disturb
presence-offline = Offline
# The account button's accessible name: the user and their presence.
account-trigger = { $name }, { $presence }
account-trigger-until = { $name }, { $presence }, { $until }
account-status-item = Status: { $status }
account-set-status-item = Set a status…
account-edit-profile = Edit profile…
account-custom-emoji = Custom emoji…
account-preferences = Preferences
account-add-server = Add a server…
account-sign-out-item = Sign out…
account-sign-out = Sign out
account-sign-out-title = Sign out of { $server }?
account-sign-out-hint = This removes Matty's copy of your messages, files and drafts from this computer. You'll sign in again next time.
account-cancel = Cancel
account-done = Done
dnd-indefinite = Don't clear
dnd-30m = 30 minutes
dnd-1h = 1 hour
dnd-2h = 2 hours
dnd-tomorrow = Tomorrow
dnd-custom = Custom…
dnd-custom-title = Do not disturb until
dnd-custom-hint = Notifications stay quiet until then. Your status goes back afterwards.
dnd-custom-day = Day
dnd-custom-time = Time
day-today = Today
day-tomorrow = Tomorrow
# When Do not disturb or a status ends; { $time } is a clock time.
until-today = Until { $time }
until-tomorrow = Until tomorrow { $time }
# { $day } is a weekday or a date.
until-day = Until { $day } at { $time }
until-day-short = Until { $day } { $time }
dnd-preferences-off = Pause desktop notifications. For a set time, use the status menu.
dnd-preferences-today = Desktop notifications are paused until { $time }.
dnd-preferences-tomorrow = Desktop notifications are paused until tomorrow { $time }.
dnd-preferences-day = Desktop notifications are paused until { $day } at { $time }.
dnd-preferences-indefinite = Desktop notifications are paused until you turn this off.
status-title = Set a status
status-hint = Shown next to your name until it clears.
status-placeholder = What are you up to?
status-field = Status
# The status emoji button's accessible name.
status-emoji = Emoji: { $name }
status-recent = Recent
status-suggestions = Suggestions
# A suggested status's accessible name: its text and when it clears.
status-offer = { $text }, { $duration }
status-clear-after = Clear after
status-clear = Clear
status-never = Don't clear
status-30m = 30 minutes
status-1h = 1 hour
status-4h = 4 hours
status-today = Today
status-this-week = This week
status-meeting = In a meeting
status-lunch = Out for lunch
status-sick = Out sick
status-home = Working from home
status-vacation = On a vacation

## Profile card and editor (profile.rs, profile_editor.rs)

profile-local-time = { $time } local time
profile-message = Message
profile-mention = Mention
# The profile card's accessible name.
profile-card-label = { $name }'s profile
profile-view = View profile
# Picks a channel for the person on the card to join.
profile-add-to-channel = Add to channel
profile-position = Position
profile-time-zone = Time zone
profile-edit-title = Edit profile
profile-edit-hint = People see these on your messages and profile card.
profile-first-name = First name
profile-last-name = Last name
profile-nickname = Nickname
profile-position-hint = Your job title, such as Product designer.
profile-username = Username
profile-username-hint = Others mention you with @ and this name.
profile-username-length = Use 3 to 22 characters.
profile-username-start = Start with a lowercase letter.
profile-username-characters = Use lowercase letters, digits, '.', '-' or '_'.
profile-email = Email
profile-email-hint = Where notifications and password resets go.
profile-email-confirm = Confirm the change with your current password.
profile-email-invalid = Enter an email address, such as ada@example.com.
profile-current-password = Current password
profile-picture = Your picture
profile-picture-prompt = Choose picture
profile-picture-upload = Upload picture…
profile-picture-remove = Remove
profile-picture-hint = JPEG, PNG, GIF or BMP.
profile-picture-type = Choose a JPEG, PNG, GIF or BMP image.
profile-picture-unreadable = Couldn't read that file.
profile-cancel = Cancel
profile-save = Save
profile-save-failed = Couldn't save your profile: { $error }

## Channel bookmarks (bookmarks.rs)

bookmark-untitled = Untitled
bookmark-name-placeholder = A short name
bookmark-emoji-placeholder = Emoji name, such as book
bookmark-edit-title = Edit bookmark
bookmark-add-file-title = Add a file
bookmark-add-title = Add a bookmark
bookmark-link = Link
bookmark-link-invalid = Enter a web address, such as example.com.
bookmark-name = Name
bookmark-name-too-long = At most { $max } characters.
bookmark-name-file-hint = Shown in the bar instead of the file's name.
bookmark-name-server-file-hint = The file keeps its own name on the server.
bookmark-name-link-hint = Leave it empty to use the link's address.
bookmark-emoji = Emoji
bookmark-emoji-file-hint = Shown instead of the file's icon. Optional.
bookmark-emoji-link-hint = Shown instead of the link's icon. Optional.
bookmark-file-hint = The file uploads to the channel, and everyone in it sees the bookmark in the bar under the header.
bookmark-hint = Everyone in the channel sees it in the bar under the header.
bookmark-cancel = Cancel
bookmark-save = Save
bookmark-add = Add
bookmark-add-link-item = Add a link…
bookmark-add-file-item = Add a file…
bookmark-open-link = Open link
bookmark-open-file = Open file
bookmark-copy-link = Copy link
bookmark-edit-item = Edit…
bookmark-move-left = Move left
bookmark-move-right = Move right
bookmark-bar-label = Channel bookmarks
bookmark-delete-item = Delete…
# A file bookmark's tooltip.
bookmark-file-tip = { $name } (file)
# A bookmark's accessible name.
bookmark-link-label = { $name }, link
bookmark-file-label = { $name }, file
bookmark-delete-title = Delete bookmark
bookmark-delete-this = Delete this bookmark?
bookmark-delete-named = Delete “{ $name }”?
bookmark-delete-hint = It leaves the bar for everyone in the channel.
bookmark-delete = Delete
bookmark-actions = Bookmark actions
bookmark-menu = Bookmark menu

## Quick switcher (switcher.rs, switcher/places.rs)

switcher-placeholder = Jump to a channel, person or team…
switcher-forward-placeholder = Forward to a channel or person…
switcher-open-link = Open link
switcher-group-link = Link
switcher-search-messages = Search messages for “{ $query }”
switcher-group-messages = Messages
switcher-group-matches = Channels and people
switcher-group-unread = Unread
switcher-group-recent = Recent
switcher-group-servers = Other servers
switcher-group-teams = Teams
switcher-no-server-match = No channels on other servers match
switcher-search-servers = Search all servers for “{ $query }”
# Words that find a row when typed, lowercase.
switcher-keyword-team = team
switcher-keyword-unread = unread

## Drafts view (drafts.rs)

drafts-title = Drafts
drafts-tabs = Drafts and scheduled messages
drafts-tab-count = Drafts ({ $count })
drafts-scheduled = Scheduled
drafts-scheduled-count = Scheduled ({ $count })
drafts-unsent = { $count ->
    [one] 1 unsent message
   *[other] { $count } unsent messages
}
drafts-reply-in = Reply in { $channel }
drafts-delete = Delete
drafts-open = Open
drafts-send = Send
drafts-empty = No drafts
drafts-empty-hint = Anything you type and leave unsent waits here.

## Formatting row and preview (channel/format_bar.rs)

format-strikethrough = Strikethrough
format-link = Link
format-heading = Heading
format-quote = Quote
format-bullets = Bulleted list
format-numbers = Numbered list
format-preview = Preview
