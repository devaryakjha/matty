//! A user's profile card (PLAN G9, M2 in §3), opened from an avatar in the
//! transcript: who they are, whether they are around, and the two things
//! people do from there: message them or mention them.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{App, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div, px};

use crate::sidebar::Presence;
use crate::style::{self, Palette, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    avatar::{Avatar, AvatarSize},
    button::{Button, ButtonSize, ButtonVariant},
    popover::popover_popup,
    theme::UiTheme,
};

/// What the card knows about a user; sync fills it from the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserProfile {
    /// The server's user id, which sync needs to open a direct message.
    pub user_id: SharedString,
    pub username: SharedString,
    pub display_name: SharedString,
    /// Job title or role, when the user set one.
    pub position: Option<SharedString>,
    pub presence: Presence,
    /// The custom status text, when set.
    pub status_text: Option<SharedString>,
    /// The user's IANA time zone, such as `Europe/Berlin`, when they
    /// share one; the card shows the time there.
    pub time_zone: Option<SharedString>,
}

/// The time now in `zone`, the way the user's clock setting prints times;
/// `None` for a zone this system does not know.
pub fn local_time(zone: &str, clock: crate::settings::ClockFormat) -> Option<String> {
    let zone = jiff::tz::TimeZone::get(zone).ok()?;
    let now = jiff::Timestamp::now().to_zoned(zone);
    Some(clock.format(now.hour() as u16 * 60 + now.minute() as u16))
}

/// The zone a user's `timezone` props name: the automatic one when they
/// chose it, else the manual one; `None` when neither is set.
pub fn time_zone(props: &std::collections::HashMap<String, String>) -> Option<SharedString> {
    let automatic = props
        .get("useAutomaticTimezone")
        .is_some_and(|v| v == "true");
    let key = if automatic {
        "automaticTimezone"
    } else {
        "manualTimezone"
    };
    props
        .get(key)
        .filter(|zone| !zone.is_empty())
        .map(|zone| SharedString::from(zone.clone()))
}

impl UserProfile {
    pub fn presence_label(&self) -> &'static str {
        match self.presence {
            Presence::Online => "Online",
            Presence::Away => "Away",
            Presence::Dnd => "Do not disturb",
            Presence::Offline => "Offline",
        }
    }
}

/// A card action: the view that opens the card decides what it does.
pub type ProfileAction = Box<dyn Fn(&mut Window, &mut App)>;

/// The card's two actions, wired by the view that opens it.
pub struct ProfileActions {
    pub on_message: ProfileAction,
    pub on_mention: ProfileAction,
}

/// The popover body for `profile`.
pub fn profile_card(
    id: &SharedString,
    profile: &UserProfile,
    actions: ProfileActions,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = UiTheme::read(cx).clone();
    let palette = Palette::read(cx);
    let dot = match profile.presence {
        Presence::Online => palette.online,
        Presence::Away => palette.away,
        Presence::Dnd => palette.dnd,
        Presence::Offline => theme.colors.muted_foreground,
    };
    let initial: String = profile.display_name.chars().take(1).collect();
    let meta = |text: String| {
        div()
            .type_style(style::META, &theme)
            .text_color(theme.colors.muted_foreground)
            .child(text)
    };
    let detail = |glyph: gpui_kit::AnyElement, text: String| {
        div()
            .flex()
            .items_center()
            .gap(theme.space(2.))
            .child(
                div()
                    .w(scaled(&theme, metrics::ICON_SMALL))
                    .flex()
                    .justify_center()
                    .child(glyph),
            )
            .child(
                div()
                    .type_style(style::UI, &theme)
                    .text_color(theme.colors.foreground)
                    .child(text),
            )
    };
    let ProfileActions {
        on_message,
        on_mention,
    } = actions;
    popover_popup(
        gpui_kit::ElementId::Name(format!("profile.{id}").into()),
        format!("{}'s profile", profile.display_name),
        cx,
    )
    .p(theme.space(4.))
    .gap(theme.space(3.))
    .child(
        div()
            .flex()
            .items_center()
            .gap(theme.space(3.))
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .child(crate::avatars::with_picture(
                        Avatar::new(gpui_kit::ElementId::Name(
                            format!("profile.{id}.avatar").into(),
                        ))
                        .size(AvatarSize::Lg)
                        .fallback(initial)
                        .aria_label(profile.display_name.clone()),
                        &profile.user_id,
                        cx,
                    ))
                    .child(
                        div()
                            .absolute()
                            .bottom(px(-1.))
                            .right(px(-1.))
                            .size(scaled(&theme, 12.))
                            .rounded_full()
                            .bg(dot)
                            .border_2()
                            .border_color(theme.colors.popover),
                    ),
            )
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(theme.space(0.5))
                    .child(
                        div()
                            .type_style(style::TITLE, &theme)
                            .text_color(theme.colors.popover_foreground)
                            .truncate()
                            .child(profile.display_name.clone()),
                    )
                    .child(meta(format!("@{}", profile.username)))
                    .children(profile.position.clone().map(|position| {
                        div()
                            .type_style(style::UI, &theme)
                            .text_color(theme.colors.popover_foreground)
                            .child(position)
                    })),
            ),
    )
    .child(
        div()
            .flex()
            .flex_col()
            .gap(theme.space(1.5))
            .child(detail(
                div()
                    .size(scaled(&theme, 8.))
                    .rounded_full()
                    .bg(dot)
                    .into_any_element(),
                match &profile.status_text {
                    Some(text) => format!("{} · {text}", profile.presence_label()),
                    None => profile.presence_label().to_string(),
                },
            ))
            .children(
                profile
                    .time_zone
                    .as_ref()
                    .and_then(|zone| local_time(zone, crate::settings::ClockFormat::current(cx)))
                    .map(|time| {
                        detail(
                            lucide(LucideIcon::Clock)
                                .size(scaled(&theme, metrics::ICON_SMALL))
                                .text_color(theme.colors.muted_foreground)
                                .into_any_element(),
                            format!("{time} local time"),
                        )
                    }),
            ),
    )
    .child(
        div()
            .flex()
            .gap(theme.space(2.))
            .pt(theme.space(1.))
            .child(
                Button::new(gpui_kit::ElementId::Name(
                    format!("profile.{id}.message").into(),
                ))
                .variant(ButtonVariant::Default)
                .size(ButtonSize::Sm)
                .aria_label("Message")
                .flex_1()
                .child(
                    lucide(LucideIcon::MessageSquare)
                        .size(scaled(&theme, metrics::ICON))
                        .text_color(theme.colors.primary_foreground),
                )
                .child("Message")
                .on_click(move |_, window, cx| on_message(window, cx)),
            )
            .child(
                Button::new(gpui_kit::ElementId::Name(
                    format!("profile.{id}.mention").into(),
                ))
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::Sm)
                .aria_label("Mention")
                .flex_1()
                .child(
                    lucide(LucideIcon::AtSign)
                        .size(scaled(&theme, metrics::ICON))
                        .text_color(theme.colors.foreground),
                )
                .child("Mention")
                .on_click(move |_, window, cx| on_mention(window, cx)),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn the_zone_is_the_one_the_user_chose() {
        let props = |auto: &str, automatic: &str, manual: &str| {
            HashMap::from([
                ("useAutomaticTimezone".to_string(), auto.to_string()),
                ("automaticTimezone".to_string(), automatic.to_string()),
                ("manualTimezone".to_string(), manual.to_string()),
            ])
        };
        assert_eq!(
            time_zone(&props("true", "Europe/Berlin", "Asia/Tokyo")).as_deref(),
            Some("Europe/Berlin")
        );
        assert_eq!(
            time_zone(&props("false", "Europe/Berlin", "Asia/Tokyo")).as_deref(),
            Some("Asia/Tokyo")
        );
        assert_eq!(time_zone(&props("false", "Europe/Berlin", "")), None);
        assert_eq!(time_zone(&HashMap::new()), None);
    }

    #[test]
    fn local_time_follows_the_clock_setting() {
        use crate::settings::ClockFormat;
        let time = local_time("UTC", ClockFormat::TwentyFourHour).unwrap();
        assert_eq!(time.len(), 5, "{time}");
        let time = local_time("UTC", ClockFormat::TwelveHour).unwrap();
        assert!(time.ends_with("AM") || time.ends_with("PM"), "{time}");
        assert_eq!(local_time("Not/AZone", ClockFormat::TwentyFourHour), None);
    }
}
