//! A user's profile card (PLAN G9, M2 in §3), opened from an avatar in the
//! transcript: who they are, whether they are around, and the two things
//! people do from there: message them or mention them. "View profile"
//! opens the same in the right pane.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    App, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::sidebar::Presence;
use crate::style::{self, Palette, TypeStyleExt as _, metrics, scaled};
use crate::t;
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
    /// The custom status, when set and not expired (#221).
    pub status: Option<crate::custom_status::StatusLine>,
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
    pub fn presence_label(&self) -> SharedString {
        crate::account::presence_label(self.presence)
    }
}

/// A card action: the view that opens the card decides what it does.
pub type ProfileAction = Box<dyn Fn(&mut Window, &mut App)>;

/// The card's actions, wired by the view that opens it.
pub struct ProfileActions {
    pub on_message: ProfileAction,
    pub on_mention: ProfileAction,
    /// "View profile", which opens the profile in the right pane; `None`
    /// where the profile is already the pane.
    pub on_view: Option<ProfileAction>,
    /// "Add to channel", which picks one of the team's channels for the
    /// person to join (#212); `None` for the user's own card.
    pub on_add_to_channel: Option<ProfileAction>,
}

/// "Add to channel" as a quiet button, on the card and the pane (#212).
fn add_to_channel(id: &SharedString, on_add: ProfileAction, cx: &App) -> Button {
    let theme = UiTheme::read(cx).clone();
    Button::new(gpui_kit::ElementId::Name(
        format!("profile.{id}.add-to-channel").into(),
    ))
    .variant(ButtonVariant::Ghost)
    .size(ButtonSize::Sm)
    .aria_label(t!("profile-add-to-channel"))
    .child(
        lucide(LucideIcon::UserPlus)
            .size(scaled(&theme, metrics::ICON))
            .text_color(theme.colors.foreground),
    )
    .child(t!("profile-add-to-channel"))
    .on_click(move |_, window, cx| on_add(window, cx))
}

fn presence_color(profile: &UserProfile, theme: &UiTheme, palette: &Palette) -> gpui_kit::Rgba {
    match profile.presence {
        Presence::Online => palette.online,
        Presence::Away => palette.away,
        Presence::Dnd => palette.dnd,
        Presence::Offline => theme.colors.muted_foreground,
    }
}

/// The avatar with its presence dot, the name, the username and, when
/// `position` is set, the position. `surface` is what the dot's ring cuts
/// out of.
fn identity(
    id: &SharedString,
    profile: &UserProfile,
    (surface, foreground): (gpui_kit::Rgba, gpui_kit::Rgba),
    position: bool,
    cx: &App,
) -> gpui_kit::Div {
    let theme = UiTheme::read(cx).clone();
    let dot = presence_color(profile, &theme, &Palette::read(cx));
    let initial: String = profile.display_name.chars().take(1).collect();
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
                        .border_color(surface),
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
                        .text_color(foreground)
                        .truncate()
                        .child(profile.display_name.clone()),
                )
                .child(
                    div()
                        .type_style(style::META, &theme)
                        .text_color(theme.colors.muted_foreground)
                        .child(format!("@{}", profile.username)),
                )
                .children(
                    profile
                        .position
                        .clone()
                        .filter(|_| position)
                        .map(|position| {
                            div()
                                .type_style(style::UI, &theme)
                                .text_color(foreground)
                                .child(position)
                        }),
                ),
        )
}

/// Presence (with the custom status) and local time, one per line.
fn details(profile: &UserProfile, cx: &App) -> gpui_kit::Div {
    let theme = UiTheme::read(cx).clone();
    let dot = presence_color(profile, &theme, &Palette::read(cx));
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
            profile.presence_label().to_string(),
        ))
        .children(profile.status.as_ref().map(|status| {
            let until = status.until_label(
                &jiff::Zoned::now(),
                crate::settings::ClockFormat::current(cx),
            );
            div()
                .flex()
                .items_center()
                .gap(theme.space(2.))
                .child(
                    div()
                        .w(scaled(&theme, metrics::ICON_SMALL))
                        .flex()
                        .justify_center()
                        .child(crate::custom_status::emoji(
                            &status.emoji,
                            scaled(&theme, metrics::ICON_SMALL),
                            cx,
                        )),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .gap(theme.space(1.5))
                        .type_style(style::UI, &theme)
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.colors.foreground)
                                .child(status.text.clone()),
                        )
                        .children(until.map(|until| {
                            div()
                                .flex_shrink_0()
                                .text_color(theme.colors.muted_foreground)
                                .child(until)
                        })),
                )
        }))
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
                        t!("profile-local-time", time = time).to_string(),
                    )
                }),
        )
}

/// Message and Mention, side by side.
fn buttons(
    id: &SharedString,
    on_message: ProfileAction,
    on_mention: ProfileAction,
    cx: &App,
) -> gpui_kit::Div {
    let theme = UiTheme::read(cx).clone();
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
            .aria_label(t!("profile-message"))
            .flex_1()
            .child(
                lucide(LucideIcon::MessageSquare)
                    .size(scaled(&theme, metrics::ICON))
                    .text_color(theme.colors.primary_foreground),
            )
            .child(t!("profile-message"))
            .on_click(move |_, window, cx| on_message(window, cx)),
        )
        .child(
            Button::new(gpui_kit::ElementId::Name(
                format!("profile.{id}.mention").into(),
            ))
            .variant(ButtonVariant::Outline)
            .size(ButtonSize::Sm)
            .aria_label(t!("profile-mention"))
            .flex_1()
            .child(
                lucide(LucideIcon::AtSign)
                    .size(scaled(&theme, metrics::ICON))
                    .text_color(theme.colors.foreground),
            )
            .child(t!("profile-mention"))
            .on_click(move |_, window, cx| on_mention(window, cx)),
        )
}

/// The popover body for `profile`.
pub fn profile_card(
    id: &SharedString,
    profile: &UserProfile,
    actions: ProfileActions,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = UiTheme::read(cx).clone();
    let ProfileActions {
        on_message,
        on_mention,
        on_view,
        on_add_to_channel,
    } = actions;
    popover_popup(
        gpui_kit::ElementId::Name(format!("profile.{id}").into()),
        t!(
            "profile-card-label",
            name = profile.display_name.to_string()
        ),
        cx,
    )
    .p(theme.space(4.))
    .gap(theme.space(3.))
    .child(identity(
        id,
        profile,
        (theme.colors.popover, theme.colors.popover_foreground),
        true,
        cx,
    ))
    .child(details(profile, cx))
    .child(buttons(id, on_message, on_mention, cx))
    .when(on_view.is_some() || on_add_to_channel.is_some(), |card| {
        card.child(
            div()
                .flex()
                .gap(theme.space(2.))
                .children(on_view.map(|on_view| {
                    Button::new(gpui_kit::ElementId::Name(
                        format!("profile.{id}.view").into(),
                    ))
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Sm)
                    .aria_label(t!("profile-view"))
                    .child(t!("profile-view"))
                    .on_click(move |_, window, cx| on_view(window, cx))
                }))
                .children(on_add_to_channel.map(|on_add| add_to_channel(id, on_add, cx))),
        )
    })
}

/// The profile as the right pane shows it (#212): the card's contents with
/// room to breathe, and the position and time zone as labelled fields.
pub fn profile_pane(
    profile: &UserProfile,
    on_message: ProfileAction,
    on_mention: ProfileAction,
    on_add_to_channel: Option<ProfileAction>,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = UiTheme::read(cx).clone();
    let id: SharedString = format!("pane.{}", profile.user_id).into();
    let field = |label: SharedString, value: SharedString| {
        div()
            .flex()
            .flex_col()
            .gap(theme.space(0.5))
            .child(
                div()
                    .type_style(style::META, &theme)
                    .text_color(theme.colors.muted_foreground)
                    .child(label),
            )
            .child(
                div()
                    .type_style(style::UI, &theme)
                    .text_color(theme.colors.foreground)
                    .child(value),
            )
    };
    div()
        .id(gpui_kit::ElementId::Name(id.clone()))
        .size_full()
        .overflow_y_scroll()
        .p(theme.space(4.))
        .flex()
        .flex_col()
        .gap(theme.space(4.))
        .child(identity(
            &id,
            profile,
            (theme.colors.background, theme.colors.foreground),
            false,
            cx,
        ))
        .child(buttons(&id, on_message, on_mention, cx))
        .children(
            on_add_to_channel.map(|on_add| div().flex().child(add_to_channel(&id, on_add, cx))),
        )
        .child(details(profile, cx))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(theme.space(3.))
                .pt(theme.space(3.))
                .border_t(style::hairline())
                .border_color(theme.colors.border)
                .children(
                    profile
                        .position
                        .clone()
                        .map(|position| field(t!("profile-position"), position)),
                )
                .children(
                    profile
                        .time_zone
                        .clone()
                        .map(|zone| field(t!("profile-time-zone"), zone.replace('_', " ").into())),
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
