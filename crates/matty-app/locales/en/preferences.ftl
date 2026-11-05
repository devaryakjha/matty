## Preferences (main_window/preferences.rs)

prefs-title = Preferences
prefs-description = Changes apply right away and are saved for next time.
prefs-done = Done
prefs-sections = Preferences sections
prefs-synced = Synced with your Mattermost account

prefs-section-general = General
prefs-section-appearance = Appearance
prefs-section-messages = Messages
prefs-section-notifications = Notifications
prefs-section-advanced = Advanced

## General

prefs-window = Window
prefs-close-to-tray = Keep running when closed
prefs-close-to-tray-detail = Closing the window leaves Matty in the tray.
prefs-launch-at-login = Open at login
prefs-launch-at-login-detail = Start Matty when you sign in.
prefs-files = Files
prefs-download-folder = Download folder
prefs-download-folder-default = Your Downloads folder
prefs-download-folder-change = Change…
prefs-download-folder-change-label = Change download folder
prefs-download-folder-choose = Choose

## Appearance

prefs-theme = Theme
prefs-theme-system = System
prefs-theme-light = Light
prefs-theme-dark = Dark
prefs-theme-from-mattermost = Your Mattermost theme decides light or dark.
prefs-colors = Colors
prefs-colors-matty = Matty
prefs-colors-mattermost = My Mattermost theme
prefs-colors-custom = Custom theme
prefs-colors-matty-hint = Matty's own colors, light or dark as chosen above.
prefs-colors-imported-hint = { $name } from your Mattermost account, for the team on screen. It sets light or dark itself.
prefs-colors-waiting-hint = Your Mattermost theme shows once Matty connects to your server.
prefs-contrast-tag = Hard to read
prefs-contrast-warning = { $count ->
        [one] One color pair in this theme falls
       *[other] { $count } color pairs in this theme fall
    } below WCAG AA contrast (4.5:1 for text, 3:1 for other UI), such as { $examples }.
prefs-contrast-pair = { $name } ({ $ratio }:1)
prefs-list-and = { $rest } and { $last }
prefs-list-separator = {", "}
prefs-text-size = Text size
prefs-text-size-option = Text size { $size }
prefs-display = Message display
prefs-display-standard = Standard
prefs-display-compact = Compact
prefs-display-standard-hint = Avatars and a name line above each message.
prefs-display-compact-hint = One line per message, with the time in the margin.
prefs-clock = Clock
prefs-clock-12 = 12-hour
prefs-clock-24 = 24-hour
prefs-names = Names
prefs-names-label = Show names as
prefs-names-nickname = Nickname
prefs-names-full = Full name
prefs-names-username = Username
prefs-names-nickname-hint = The nickname, else the full name, else the username.
prefs-names-full-hint = The full name, else the username.
prefs-names-username-hint = Usernames only.
prefs-motion = Motion
prefs-reduce-motion = Reduce motion
prefs-reduce-motion-detail = Panes and banners change without sliding.

## Messages

prefs-send = Send messages with
prefs-send-enter = Enter
prefs-send-cmd-enter = Cmd+Enter
prefs-send-ctrl-enter = Ctrl+Enter
prefs-send-code-blocks = Enter, but not in code
prefs-send-enter-hint = Shift+Enter starts a new line.
prefs-send-mod-enter-hint = Enter starts a new line.
prefs-send-code-blocks-hint = In a code block opened with ```, Enter starts a new line and { $key } sends.
prefs-link-previews = Link previews
prefs-collapse-previews = Collapse link previews
prefs-collapse-previews-detail = Previews start closed. Each message's toggle still opens them.
prefs-remote-images = Load remote images
prefs-remote-images-always = Always
prefs-remote-images-ask = Ask first
prefs-remote-images-always-hint = Images from other sites show in messages and previews, through your server's image proxy when it has one.
prefs-remote-images-ask-hint = Images from other sites wait until you click them.
prefs-threads = Threads
prefs-collapsed-threads = Collapsed reply threads
prefs-collapsed-threads-detail = Replies stay in their thread instead of the channel.

## Notifications

prefs-notifications-offline = Notification settings show once Matty is connected to your server.
prefs-status = Status
prefs-dnd = Do not disturb
prefs-desktop = Desktop notifications
prefs-desktop-label = Notify me about
prefs-desktop-all = All messages
prefs-desktop-mention = Mentions
prefs-desktop-none = Nothing
prefs-desktop-threads = Replies in threads I follow
prefs-desktop-threads-detail = Notify for every reply, not only mentions.
prefs-sound = Sound
prefs-sound-detail = Play a sound with each notification.
prefs-notification-sound = Notification sound
prefs-notification-sound-detail = Choosing one plays it.
prefs-mentions = Mentions
prefs-first-name = First name
prefs-first-name-detail = Your first name counts as a mention.
prefs-first-name-named = “{ $name }” counts as a mention.
prefs-channel-wide = @channel, @all and @here
prefs-channel-wide-detail = Messages to the whole channel count as mentions.
prefs-keywords = Keywords
prefs-keyword-placeholder = Add a keyword
prefs-keywords-detail = Words that count as mentioning you and notify. Press Enter to add; commas add several. @username always counts.
prefs-highlights = Highlights
prefs-highlight-keywords = Highlight keywords
prefs-highlight-keywords-detail = Words that highlight a message without notifying you.

## Advanced

prefs-updates = Updates
prefs-update-channel = Update channel
prefs-update-stable = Stable
prefs-update-beta = Beta
prefs-update-stable-hint = Releases only.
prefs-update-beta-hint = Betas and alphas too, before they're released.
prefs-logs = Logs
prefs-log-detail = Log detail
prefs-log-detail-hint = How much goes into the logs.
prefs-logs-detail = Attach them when you report a problem.
prefs-open-logs = Open folder
prefs-open-logs-label = Open logs folder
prefs-account = Account
prefs-sign-out = Sign out
prefs-sign-out-detail = Removes Matty's copy of your messages and files from this computer.
prefs-sign-out-button = Sign out…
