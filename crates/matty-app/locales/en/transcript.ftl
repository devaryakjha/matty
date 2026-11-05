## System messages and day labels in the transcript (transcript.rs)

sys-you = You
sys-someone = Someone
sys-system = System
sys-names-two = { $first } and { $second }
sys-names-three = { $first }, { $second } and { $third }
sys-names-more = { $first }, { $second } and { $others } others

# $who names one or more people; $count is how many.
sys-joined-channel = { $who } joined the channel.
sys-joined-channel-guest = { $who } joined the channel as a guest.
sys-left-channel = { $who } left the channel.
sys-added-to-channel = { $who } added to the channel by { $by }.
sys-added-guest-to-channel = { $who } added to the channel as a guest by { $by }.
sys-removed-from-channel = { $count ->
        [one] { $who } was removed from the channel.
       *[other] { $who } were removed from the channel.
    }
sys-joined-team = { $who } joined the team.
sys-left-team = { $who } left the team.
sys-added-to-team = { $who } added to the team by { $by }.
sys-removed-from-team = { $count ->
        [one] { $who } was removed from the team.
       *[other] { $who } were removed from the team.
    }

sys-header-removed = { $actor } removed the channel header (was: { $old })
sys-header-set = { $actor } updated the channel header to: { $new }
sys-header-changed = { $actor } updated the channel header from: { $old } to: { $new }
sys-purpose-removed = { $actor } removed the channel purpose (was: { $old })
sys-purpose-set = { $actor } updated the channel purpose to: { $new }
sys-purpose-changed = { $actor } updated the channel purpose from: { $old } to: { $new }
sys-name-removed = { $actor } removed the channel display name (was: { $old })
sys-name-set = { $actor } updated the channel display name to: { $new }
sys-name-changed = { $actor } updated the channel display name from: { $old } to: { $new }
sys-archived = { $actor } archived the channel
sys-unarchived = { $actor } unarchived the channel

## Day labels

day-today = Today
day-yesterday = Yesterday
day-this-year = { $weekday }, { $month } { $day }
day-other-year = { $month } { $day }, { $year }
day-full = { $weekday }, { $month } { $day }, { $year }
day-monday = Monday
day-tuesday = Tuesday
day-wednesday = Wednesday
day-thursday = Thursday
day-friday = Friday
day-saturday = Saturday
day-sunday = Sunday
day-month-1 = January
day-month-2 = February
day-month-3 = March
day-month-4 = April
day-month-5 = May
day-month-6 = June
day-month-7 = July
day-month-8 = August
day-month-9 = September
day-month-10 = October
day-month-11 = November
day-month-12 = December
