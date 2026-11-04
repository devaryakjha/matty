//! Do Not Disturb durations (#190): when each choice in the status menu
//! ends, and how the end reads. The server keeps `dnd_end_time` in seconds
//! since the epoch and restores the previous status itself when it passes.

use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan as _, Zoned};

use crate::settings::ClockFormat;

/// One choice under Do not disturb in the status menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DndDuration {
    /// Until the user turns it off.
    Indefinite,
    /// For this many minutes from now.
    Minutes(i64),
    /// Until 09:00 tomorrow, local time, as the webapp does.
    Tomorrow,
}

/// The menu's choices, with an id suffix and a label; "Custom…" follows.
pub const DURATIONS: [(DndDuration, &str, &str); 5] = [
    (DndDuration::Indefinite, "indefinite", "Don't clear"),
    (DndDuration::Minutes(30), "30m", "30 minutes"),
    (DndDuration::Minutes(60), "1h", "1 hour"),
    (DndDuration::Minutes(120), "2h", "2 hours"),
    (DndDuration::Tomorrow, "tomorrow", "Tomorrow"),
];

/// Minutes between the custom picker's times.
pub const STEP_MINUTES: u16 = 30;

/// When `duration` ends, in seconds since the epoch, or `None` for no end.
pub fn end_time(duration: DndDuration, now: &Zoned) -> Option<i64> {
    match duration {
        DndDuration::Indefinite => None,
        // Elapsed time, so the end is right even across a DST change.
        DndDuration::Minutes(minutes) => Some(now.timestamp().as_second() + minutes * 60),
        DndDuration::Tomorrow => {
            let tomorrow = now.date().tomorrow().ok()?;
            local_end(tomorrow, 9 * 60, now.time_zone())
        }
    }
}

/// When the custom picker's day and time end, in seconds since the epoch.
/// A time a DST change skips moves forward, as clocks do.
pub fn local_end(date: Date, minutes: u16, zone: &TimeZone) -> Option<i64> {
    let time = Time::new((minutes / 60) as i8, (minutes % 60) as i8, 0, 0).ok()?;
    let zoned = date.to_datetime(time).to_zoned(zone.clone()).ok()?;
    Some(zoned.timestamp().as_second())
}

/// The custom picker's days: today and the six after it.
pub fn custom_days(now: &Zoned) -> Vec<(Date, String)> {
    let today = now.date();
    (0..7)
        .filter_map(|offset| {
            let date = today.checked_add(offset.days()).ok()?;
            let label = match offset {
                0 => "Today".to_string(),
                1 => "Tomorrow".to_string(),
                _ => date.strftime("%A, %B %-d").to_string(),
            };
            Some((date, label))
        })
        .collect()
}

/// The custom picker's times on `date`, in minutes since midnight: every
/// half hour, and only those still ahead when `date` is today.
pub fn custom_times(date: Date, now: &Zoned) -> Vec<u16> {
    let after = if date == now.date() {
        now.hour() as u16 * 60 + now.minute() as u16
    } else if date < now.date() {
        return Vec::new();
    } else {
        0
    };
    (0..24 * 60)
        .step_by(STEP_MINUTES as usize)
        .filter(|minutes| date != now.date() || *minutes > after)
        .collect()
}

/// How the end reads in the menu and the avatar's tooltip: "Until 14:30"
/// today, "Until tomorrow 09:00", or "Until Friday, November 6 at 14:30".
pub fn until_label(end: i64, now: &Zoned, clock: ClockFormat) -> Option<String> {
    let end = Timestamp::from_second(end)
        .ok()?
        .to_zoned(now.time_zone().clone());
    let time = clock.format(end.hour() as u16 * 60 + end.minute() as u16);
    let today = now.date();
    Some(if end.date() == today {
        format!("Until {time}")
    } else if Some(end.date()) == today.tomorrow().ok() {
        format!("Until tomorrow {time}")
    } else {
        format!("Until {} at {time}", end.strftime("%A, %B %-d"))
    })
}

/// Whether Do not disturb is on now: set, and its end (if any) still ahead.
/// The server restores the status at the end; until its event arrives the
/// end already counts.
pub fn is_on(dnd: bool, end: Option<i64>, now: &Zoned) -> bool {
    dnd && matty_notify::dnd_active(end, now.timestamp().as_second())
}

/// The line under Do not disturb in Preferences → Notifications (#158):
/// whether notifications are paused, and until when.
pub fn preferences_detail(dnd: bool, end: Option<i64>, now: &Zoned, clock: ClockFormat) -> String {
    if !is_on(dnd, end, now) {
        return "Pause desktop notifications. For a set time, use the status menu.".to_owned();
    }
    match end.and_then(|end| until_label(end, now, clock)) {
        Some(until) => format!(
            "Desktop notifications are paused {}.",
            until.replacen("Until", "until", 1)
        ),
        None => "Desktop notifications are paused until you turn this off.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(stamp: &str, zone: &str) -> Zoned {
        let zone = TimeZone::get(zone).unwrap();
        stamp.parse::<Timestamp>().unwrap().to_zoned(zone)
    }

    fn utc(seconds: i64) -> String {
        Timestamp::from_second(seconds).unwrap().to_string()
    }

    #[test]
    fn indefinite_has_no_end() {
        let now = at("2026-11-04T08:00:00Z", "Europe/Berlin");
        assert_eq!(end_time(DndDuration::Indefinite, &now), None);
    }

    #[test]
    fn minutes_count_from_now() {
        let now = at("2026-11-04T08:10:20Z", "Europe/Berlin");
        let ends = |m| end_time(DndDuration::Minutes(m), &now).map(utc);
        assert_eq!(ends(30).as_deref(), Some("2026-11-04T08:40:20Z"));
        assert_eq!(ends(60).as_deref(), Some("2026-11-04T09:10:20Z"));
        assert_eq!(ends(120).as_deref(), Some("2026-11-04T10:10:20Z"));
    }

    #[test]
    fn minutes_are_elapsed_time_across_dst() {
        // 01:45 EDT; the clocks go back at 02:00, so 30 minutes later the
        // wall clock reads 01:15 EST.
        let now = at("2026-11-01T05:45:00Z", "America/New_York");
        let end = end_time(DndDuration::Minutes(30), &now).unwrap();
        assert_eq!(utc(end), "2026-11-01T06:15:00Z");
    }

    #[test]
    fn tomorrow_is_nine_local() {
        let now = at("2026-11-04T20:00:00Z", "Asia/Kolkata");
        let end = end_time(DndDuration::Tomorrow, &now).unwrap();
        // 01:30 on the 5th in Kolkata, so tomorrow is the 6th.
        assert_eq!(utc(end), "2026-11-06T03:30:00Z");
    }

    #[test]
    fn tomorrow_across_dst_ending() {
        // Saturday evening EDT; Sunday 09:00 is already EST (UTC-5).
        let now = at("2026-11-01T01:00:00Z", "America/New_York");
        let end = end_time(DndDuration::Tomorrow, &now).unwrap();
        assert_eq!(utc(end), "2026-11-01T14:00:00Z");
    }

    #[test]
    fn tomorrow_across_dst_starting() {
        // Saturday evening EST; Sunday 09:00 is already EDT (UTC-4).
        let now = at("2026-03-08T01:00:00Z", "America/New_York");
        let end = end_time(DndDuration::Tomorrow, &now).unwrap();
        assert_eq!(utc(end), "2026-03-08T13:00:00Z");
    }

    #[test]
    fn a_skipped_custom_time_moves_forward() {
        let zone = TimeZone::get("America/New_York").unwrap();
        let date = Date::new(2026, 3, 8).unwrap();
        // 02:30 does not exist that night; it reads as 03:30 EDT.
        let end = local_end(date, 2 * 60 + 30, &zone).unwrap();
        assert_eq!(utc(end), "2026-03-08T07:30:00Z");
    }

    #[test]
    fn custom_days_start_today() {
        let now = at("2026-11-04T08:00:00Z", "UTC");
        let days = custom_days(&now);
        assert_eq!(days.len(), 7);
        assert_eq!(days[0].1, "Today");
        assert_eq!(days[1].1, "Tomorrow");
        assert_eq!(days[2].1, "Friday, November 6");
    }

    #[test]
    fn custom_times_today_are_still_ahead() {
        let now = at("2026-11-04T22:40:00Z", "UTC");
        assert_eq!(custom_times(now.date(), &now), vec![23 * 60, 23 * 60 + 30]);
        let tomorrow = now.date().tomorrow().unwrap();
        assert_eq!(custom_times(tomorrow, &now).len(), 48);
    }

    #[test]
    fn until_reads_by_day() {
        let now = at("2026-11-04T08:00:00Z", "UTC");
        let label = |stamp: &str, clock| {
            let end = stamp.parse::<Timestamp>().unwrap().as_second();
            until_label(end, &now, clock).unwrap()
        };
        assert_eq!(
            label("2026-11-04T14:30:00Z", ClockFormat::TwentyFourHour),
            "Until 14:30"
        );
        assert_eq!(
            label("2026-11-04T14:30:00Z", ClockFormat::TwelveHour),
            "Until 2:30 PM"
        );
        assert_eq!(
            label("2026-11-05T09:00:00Z", ClockFormat::TwentyFourHour),
            "Until tomorrow 09:00"
        );
        assert_eq!(
            label("2026-11-06T09:00:00Z", ClockFormat::TwentyFourHour),
            "Until Friday, November 6 at 09:00"
        );
    }

    #[test]
    fn preferences_say_whether_and_until_when() {
        let now = at("2026-11-04T08:00:00Z", "UTC");
        let clock = ClockFormat::TwentyFourHour;
        let soon = "2026-11-04T14:30:00Z"
            .parse::<Timestamp>()
            .unwrap()
            .as_second();
        let past = now.timestamp().as_second() - 60;
        assert_eq!(
            preferences_detail(true, Some(soon), &now, clock),
            "Desktop notifications are paused until 14:30."
        );
        assert_eq!(
            preferences_detail(true, Some(soon + 86_400), &now, clock),
            "Desktop notifications are paused until tomorrow 14:30."
        );
        assert_eq!(
            preferences_detail(true, None, &now, clock),
            "Desktop notifications are paused until you turn this off."
        );
        let off = "Pause desktop notifications. For a set time, use the status menu.";
        assert_eq!(preferences_detail(false, None, &now, clock), off);
        // Ended, before the server's status change arrives.
        assert_eq!(preferences_detail(true, Some(past), &now, clock), off);
        assert!(is_on(true, Some(soon), &now));
        assert!(!is_on(true, Some(past), &now));
        assert!(!is_on(false, Some(soon), &now));
    }
}
