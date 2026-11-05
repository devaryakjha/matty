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
