//! gpuicn's date and time picker set up for "a time still ahead": the
//! custom end of Do not disturb (#190) and the custom time of a scheduled
//! message (#220). Times are the user's local clock.

use gpui_kit::{App, AppContext as _, Entity};
use jiff::{Zoned, civil::Date};

use crate::settings::ClockFormat;
use crate::ui::{
    calendar::CalendarDate,
    date_time_picker::{CalendarTime, DateTimeState, HourCycle},
};

/// A picker's state, to keep for the dialog's life.
pub fn state(cx: &mut App) -> Entity<DateTimeState> {
    cx.new(DateTimeState::new)
}

/// Readies `state` as the dialog opens: today as the calendar's today,
/// nothing before now, the user's clock, and `at` or else the next half
/// hour picked.
pub fn reset(
    state: &Entity<DateTimeState>,
    at: Option<&Zoned>,
    now: &Zoned,
    clock: ClockFormat,
    cx: &mut App,
) {
    let today = calendar_date(now.date());
    let (date, time) = match at.filter(|at| *at > now) {
        Some(at) => (at.date(), (at.hour() as u16) * 60 + at.minute() as u16),
        None => next_half_hour(now),
    };
    let time = calendar_time(time);
    let earliest = calendar_time(now.hour() as u16 * 60 + now.minute() as u16);
    state.update(cx, |state, cx| {
        state
            .calendar()
            .clone()
            .update(cx, |calendar, cx| calendar.set_today(today, cx));
        state.set_hour_cycle(
            match clock {
                ClockFormat::TwelveHour => HourCycle::H12,
                ClockFormat::TwentyFourHour => HourCycle::H23,
            },
            cx,
        );
        state.set_limits(Some((today, earliest)), None, cx);
        state.set_value(Some(calendar_date(date)), time, cx);
    });
}

/// The picked time in seconds since the epoch, if it is still ahead of
/// `now`.
pub fn picked(state: &Entity<DateTimeState>, now: &Zoned, cx: &App) -> Option<i64> {
    let state = state.read(cx);
    let date = state.date(cx)?;
    let time = state.time();
    let date = Date::new(date.year() as i16, date.month() as i8, date.day() as i8).ok()?;
    let minutes = time.hour() as u16 * 60 + time.minute() as u16;
    crate::dnd::local_end(date, minutes, now.time_zone())
        .filter(|at| *at > now.timestamp().as_second())
}

/// The first half hour after `now`, which may be tomorrow's first.
fn next_half_hour(now: &Zoned) -> (Date, u16) {
    let minutes = now.hour() as u16 * 60 + now.minute() as u16;
    let next = (minutes / crate::dnd::STEP_MINUTES + 1) * crate::dnd::STEP_MINUTES;
    if next < 24 * 60 {
        (now.date(), next)
    } else {
        (now.date().tomorrow().unwrap_or(now.date()), 0)
    }
}

fn calendar_date(date: Date) -> CalendarDate {
    CalendarDate::new(date.year() as i32, date.month() as u8, date.day() as u8)
        .unwrap_or_else(|| CalendarDate::from_days(0))
}

fn calendar_time(minutes: u16) -> CalendarTime {
    CalendarTime::new((minutes / 60 % 24) as u8, (minutes % 60) as u8)
        .unwrap_or_else(|| CalendarTime::new(0, 0).expect("midnight is a time"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Zoned {
        format!("{text}[UTC]").parse().unwrap()
    }

    #[test]
    fn the_default_is_the_next_half_hour() {
        let next = |now: &str| next_half_hour(&at(now));
        assert_eq!(
            next("2026-11-05T09:10:00"),
            (at("2026-11-05T00:00").date(), 9 * 60 + 30)
        );
        assert_eq!(
            next("2026-11-05T09:30:00"),
            (at("2026-11-05T00:00").date(), 10 * 60)
        );
        assert_eq!(
            next("2026-11-05T23:45:00"),
            (at("2026-11-06T00:00").date(), 0)
        );
    }

    #[gpui_kit::test]
    fn only_times_still_ahead_are_picked(cx: &mut gpui_kit::TestAppContext) {
        let now = at("2026-11-05T09:10:00");
        let state = cx.update(state);
        cx.update(|cx| reset(&state, None, &now, ClockFormat::TwentyFourHour, cx));
        let first = cx.update(|cx| picked(&state, &now, cx));
        assert_eq!(
            first,
            Some(at("2026-11-05T09:30:00").timestamp().as_second())
        );
        // A time already gone: kept, so the dialog can say so, but not picked.
        let gone = at("2026-11-05T08:00:00");
        cx.update(|cx| {
            state.update(cx, |state, cx| {
                state.set_limits(None, None, cx);
                state.set_value(Some(calendar_date(gone.date())), calendar_time(8 * 60), cx);
            })
        });
        assert_eq!(cx.update(|cx| picked(&state, &now, cx)), None);
        // An earlier pick that is still ahead comes back.
        let later = at("2026-11-07T14:00:00");
        cx.update(|cx| reset(&state, Some(&later), &now, ClockFormat::TwelveHour, cx));
        assert_eq!(
            cx.update(|cx| picked(&state, &now, cx)),
            Some(later.timestamp().as_second())
        );
    }
}
