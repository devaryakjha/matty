//! Custom statuses (#221): an emoji and a line of text a user sets, shown
//! next to their name until it clears. Mattermost keeps them in the user's
//! props; this turns them into what the views show, and works out when a
//! preset clears.

use gpui_kit::{App, IntoElement, ParentElement as _, SharedString, Styled as _, div, img};
use matty_api::models::CustomStatus;

use crate::t;

/// A custom status as the views show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusLine {
    /// The emoji's name, such as `calendar`; a server's custom emoji too.
    pub emoji: SharedString,
    pub text: SharedString,
    /// When it clears, in seconds since the epoch; `None` when it does not.
    pub until: Option<i64>,
    /// The preset it was set with, kept so recents can offer it again.
    pub duration: Duration,
}

impl StatusLine {
    /// The status `status` describes, or `None` when it is empty or has
    /// expired by `now`.
    pub fn from_server(status: &CustomStatus, now: jiff::Timestamp) -> Option<Self> {
        if status.emoji.is_empty() && status.text.is_empty() {
            return None;
        }
        let until = (!status.expires_at.is_empty())
            .then(|| status.expires_at.parse::<jiff::Timestamp>().ok())
            .flatten()
            // Mattermost's "never" is the zero time.
            .filter(|at| at.as_second() > 0)
            .map(|at| at.as_second());
        if until.is_some_and(|until| until <= now.as_second()) {
            return None;
        }
        Some(Self {
            emoji: if status.emoji.is_empty() {
                "speech_balloon".into()
            } else {
                status.emoji.clone().into()
            },
            text: status.text.clone().into(),
            until,
            duration: Duration::from_server(&status.duration),
        })
    }

    /// The user's status from their props, when it has not expired.
    pub fn of(user: &matty_api::models::User) -> Option<Self> {
        Self::from_server(&user.custom_status()?, jiff::Timestamp::now())
    }

    /// The emoji and text, for tooltips and accessible names.
    pub fn label(&self) -> String {
        match crate::emoji::glyph(&self.emoji) {
            Some(glyph) if self.text.is_empty() => glyph.to_string(),
            Some(glyph) => format!("{glyph} {}", self.text),
            None if self.text.is_empty() => format!(":{}:", self.emoji),
            None => format!(":{}: {}", self.emoji, self.text),
        }
    }

    /// "Until 10:30", "Until tomorrow 9:00" or "Until Fri 17:00" in `zone`,
    /// or `None` when it does not clear.
    pub fn until_label(
        &self,
        now: &jiff::Zoned,
        clock: crate::settings::ClockFormat,
    ) -> Option<SharedString> {
        let until = jiff::Timestamp::from_second(self.until?)
            .ok()?
            .to_zoned(now.time_zone().clone());
        let time = clock.format(until.hour() as u16 * 60 + until.minute() as u16);
        let days = (until.date() - now.date()).get_days();
        Some(match days {
            0 => t!("until-today", time = time),
            1 => t!("until-tomorrow", time = time),
            2..=6 => t!(
                "until-day-short",
                day = until.strftime("%a").to_string(),
                time = time
            ),
            _ => t!(
                "until-day-short",
                day = until.strftime("%b %-d").to_string(),
                time = time
            ),
        })
    }
}

/// When a status clears, as Mattermost's presets name it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Duration {
    #[default]
    DontClear,
    ThirtyMinutes,
    OneHour,
    FourHours,
    Today,
    ThisWeek,
}

impl Duration {
    pub const ALL: [Duration; 6] = [
        Self::DontClear,
        Self::ThirtyMinutes,
        Self::OneHour,
        Self::FourHours,
        Self::Today,
        Self::ThisWeek,
    ];

    pub fn from_server(name: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|duration| duration.server_name() == name)
            .unwrap_or_default()
    }

    pub fn server_name(self) -> &'static str {
        match self {
            Self::DontClear => "dont_clear",
            Self::ThirtyMinutes => "thirty_minutes",
            Self::OneHour => "one_hour",
            Self::FourHours => "four_hours",
            Self::Today => "today",
            Self::ThisWeek => "this_week",
        }
    }

    pub fn label(self) -> SharedString {
        match self {
            Self::DontClear => t!("status-never"),
            Self::ThirtyMinutes => t!("status-30m"),
            Self::OneHour => t!("status-1h"),
            Self::FourHours => t!("status-4h"),
            Self::Today => t!("status-today"),
            Self::ThisWeek => t!("status-this-week"),
        }
    }

    /// When a status set at `now` with this preset clears: the end of the
    /// day or of the week (Sunday) in the user's own zone, which follows
    /// daylight saving; `None` for Don't clear.
    pub fn ends(self, now: &jiff::Zoned) -> Option<jiff::Timestamp> {
        let after = |span: jiff::Span| now.checked_add(span).ok().map(|end| end.timestamp());
        let end_of = |date: jiff::civil::Date| {
            date.at(23, 59, 59, 999_000_000)
                .to_zoned(now.time_zone().clone())
                .ok()
                .map(|end| end.timestamp())
        };
        match self {
            Self::DontClear => None,
            Self::ThirtyMinutes => after(jiff::Span::new().minutes(30)),
            Self::OneHour => after(jiff::Span::new().hours(1)),
            Self::FourHours => after(jiff::Span::new().hours(4)),
            Self::Today => end_of(now.date()),
            Self::ThisWeek => {
                let to_sunday = 6 - now.weekday().to_monday_zero_offset();
                end_of(
                    now.date()
                        .checked_add(jiff::Span::new().days(to_sunday))
                        .ok()?,
                )
            }
        }
    }
}

/// What a user picked in the status dialog, as the server takes it, with
/// its end worked out from `now`.
pub fn to_server(emoji: &str, text: &str, duration: Duration, now: &jiff::Zoned) -> CustomStatus {
    CustomStatus {
        emoji: emoji.to_string(),
        text: text.to_string(),
        duration: duration.server_name().to_string(),
        expires_at: duration
            .ends(now)
            .map(|end| end.to_string())
            .unwrap_or_default(),
    }
}

/// The statuses the dialog offers before the user types, as Mattermost's
/// web app does: emoji, text and preset.
pub fn suggestions() -> [(&'static str, SharedString, Duration); 5] {
    [
        ("calendar", t!("status-meeting"), Duration::OneHour),
        ("hamburger", t!("status-lunch"), Duration::ThirtyMinutes),
        ("sneezing_face", t!("status-sick"), Duration::Today),
        ("house", t!("status-home"), Duration::Today),
        ("palm_tree", t!("status-vacation"), Duration::ThisWeek),
    ]
}

/// The status emoji at `size`: its glyph, a server's custom emoji image,
/// or `:name:` until that image arrives.
pub fn emoji(name: &str, size: gpui_kit::Pixels, cx: &App) -> gpui_kit::AnyElement {
    if let Some(glyph) = crate::emoji::glyph(name) {
        return div()
            .flex_shrink_0()
            .text_size(size)
            .line_height(size)
            .child(glyph)
            .into_any_element();
    }
    match crate::emoji::custom_source(name, cx) {
        Some(source) => img(source).flex_shrink_0().size(size).into_any_element(),
        None => div()
            .flex_shrink_0()
            .text_size(size * 0.75)
            .child(format!(":{name}:"))
            .into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zoned(at: &str) -> jiff::Zoned {
        at.parse().unwrap()
    }

    #[test]
    fn statuses_hide_once_they_expire() {
        let status = CustomStatus {
            emoji: "calendar".into(),
            text: "In a meeting".into(),
            duration: "one_hour".into(),
            expires_at: "2026-11-05T10:00:00Z".into(),
        };
        let before: jiff::Timestamp = "2026-11-05T09:59:00Z".parse().unwrap();
        let after: jiff::Timestamp = "2026-11-05T10:00:00Z".parse().unwrap();
        let line = StatusLine::from_server(&status, before).unwrap();
        assert_eq!(line.duration, Duration::OneHour);
        assert_eq!(
            line.label(),
            format!("{} In a meeting", crate::emoji::glyph("calendar").unwrap())
        );
        assert_eq!(StatusLine::from_server(&status, after), None);
        // No end, the zero time and an empty status.
        for expires_at in ["", "0001-01-01T00:00:00Z"] {
            let forever = CustomStatus {
                expires_at: expires_at.into(),
                ..status.clone()
            };
            assert_eq!(
                StatusLine::from_server(&forever, after).unwrap().until,
                None
            );
        }
        assert_eq!(
            StatusLine::from_server(&CustomStatus::default(), before),
            None
        );
    }

    #[test]
    fn presets_end_in_the_users_own_zone_across_daylight_saving() {
        // Berlin leaves summer time on Sunday 2026-10-25.
        let now = zoned("2026-10-23T15:20:00+02:00[Europe/Berlin]");
        let end = |duration: Duration| duration.ends(&now).map(|end| end.to_string());
        assert_eq!(end(Duration::DontClear), None);
        assert_eq!(
            end(Duration::ThirtyMinutes).unwrap(),
            "2026-10-23T13:50:00Z"
        );
        assert_eq!(end(Duration::FourHours).unwrap(), "2026-10-23T17:20:00Z");
        // The end of Friday, still at +02:00.
        assert_eq!(end(Duration::Today).unwrap(), "2026-10-23T21:59:59.999Z");
        // The end of Sunday, by then at +01:00.
        assert_eq!(end(Duration::ThisWeek).unwrap(), "2026-10-25T22:59:59.999Z");
        // On a Sunday, this week ends tonight.
        let sunday = zoned("2026-10-25T09:00:00+01:00[Europe/Berlin]");
        assert_eq!(
            Duration::ThisWeek.ends(&sunday).unwrap().to_string(),
            "2026-10-25T22:59:59.999Z"
        );
        let sent = to_server("calendar", "In a meeting", Duration::OneHour, &now);
        assert_eq!(sent.duration, "one_hour");
        assert_eq!(sent.expires_at, "2026-10-23T14:20:00Z");
    }

    #[test]
    fn the_end_reads_relative_to_today() {
        use crate::settings::ClockFormat::TwentyFourHour;
        let now = zoned("2026-11-05T09:00:00+01:00[Europe/Berlin]");
        let line = |until: &str| StatusLine {
            emoji: "calendar".into(),
            text: "".into(),
            until: Some(until.parse::<jiff::Timestamp>().unwrap().as_second()),
            duration: Duration::OneHour,
        };
        let label = |until: &str| line(until).until_label(&now, TwentyFourHour).unwrap();
        assert_eq!(label("2026-11-05T09:30:00Z"), "Until 10:30");
        assert_eq!(label("2026-11-06T08:00:00Z"), "Until tomorrow 09:00");
        assert_eq!(label("2026-11-08T16:00:00Z"), "Until Sun 17:00");
        assert_eq!(label("2026-11-20T16:00:00Z"), "Until Nov 20 17:00");
    }
}
