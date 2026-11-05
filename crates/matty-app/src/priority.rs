//! Message priority and acknowledgements (#220): the label on a post, the
//! Acknowledge bar under one that asks for it, and the composer's priority
//! picker. Each part shows only when the server's gate is on
//! ([`Features`]); the views draw them from here so the channel view, the
//! thread pane, pop-outs and split panes look the same.

use std::rc::Rc;

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    AnyElement, App, Global, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
    div, prelude::FluentBuilder as _,
};
use matty_api::models::{PostMetadata, PostPriority};

use crate::style::{self, TypeStyleExt as _, metrics, scaled};
use crate::t;
use crate::ui::{
    avatar::{Avatar, AvatarSize},
    badge::{Badge, BadgeVariant},
    button::{Button, ButtonSize, ButtonVariant},
    menu::MenuItem,
    tag::{Tag, TagVariant},
    theme::UiTheme,
};

/// How long after acknowledging the server lets the user take it back, as
/// the web app offers "Remove acknowledgement".
pub const UNDO_WINDOW_MS: i64 = 5 * 60 * 1000;

/// What the server allows, from its config and license (#220). An app
/// global, so every view hides what the server has off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Features {
    /// Important and Urgent labels (`PostPriority`).
    pub labels: bool,
    /// "Request acknowledgement" and the Acknowledge bar.
    pub acks: bool,
    /// "Send persistent notifications" on urgent posts.
    pub persistent: bool,
    /// Scheduled messages.
    pub scheduled: bool,
    /// How often a persistent notification repeats, in minutes.
    pub interval_minutes: u64,
    /// How many times it repeats at most.
    pub max_count: u64,
}

impl Global for Features {}

impl Features {
    /// What `capabilities` allow.
    pub fn of(capabilities: &matty_api::ServerCapabilities) -> Self {
        Self {
            labels: capabilities.post_priority,
            acks: capabilities.post_acknowledgements,
            persistent: capabilities.persistent_notifications,
            scheduled: capabilities.scheduled_posts,
            interval_minutes: capabilities.persistent_notification_interval_minutes,
            max_count: capabilities.persistent_notification_max_count,
        }
    }

    /// Everything on, for the preview fixtures.
    pub fn all() -> Self {
        Self {
            labels: true,
            acks: true,
            persistent: true,
            scheduled: true,
            interval_minutes: 5,
            max_count: 6,
        }
    }

    /// The features in force; all off until the server's config is read.
    pub fn read(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or_default()
    }

    /// Sets them, redrawing every window when they changed.
    pub fn set(features: Self, cx: &mut App) {
        if Self::read(cx) != features {
            cx.set_global(features);
            cx.refresh_windows();
        }
    }
}

/// A post's priority label.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Level {
    #[default]
    Standard,
    Important,
    Urgent,
}

impl Level {
    pub const ALL: [Self; 3] = [Self::Standard, Self::Important, Self::Urgent];

    fn of(priority: &str) -> Self {
        match priority {
            PostPriority::IMPORTANT => Self::Important,
            PostPriority::URGENT => Self::Urgent,
            _ => Self::Standard,
        }
    }

    fn api(self) -> &'static str {
        match self {
            Self::Standard => "",
            Self::Important => PostPriority::IMPORTANT,
            Self::Urgent => PostPriority::URGENT,
        }
    }

    pub fn label(self) -> SharedString {
        match self {
            Self::Standard => t!("priority-standard"),
            Self::Important => t!("priority-important"),
            Self::Urgent => t!("priority-urgent"),
        }
    }

    /// A name for element ids, the same in every language.
    fn key(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Important => "important",
            Self::Urgent => "urgent",
        }
    }

    fn icon(self) -> LucideIcon {
        match self {
            Self::Standard => LucideIcon::CircleAlert,
            Self::Important => LucideIcon::CircleAlert,
            Self::Urgent => LucideIcon::TriangleAlert,
        }
    }
}

/// What the composer will send: a label and the two requests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    pub level: Level,
    pub ack: bool,
    pub persistent: bool,
}

impl Choice {
    /// Whether anything is chosen beyond a standard post.
    pub fn is_set(self) -> bool {
        self.level != Level::Standard || self.ack || self.persistent
    }

    /// The `metadata.priority` to send, keeping only what `features`
    /// allow; persistent notifications only on an urgent post.
    pub fn to_api(self, features: Features) -> Option<PostPriority> {
        if !features.labels {
            return None;
        }
        let priority = PostPriority {
            priority: self.level.api().to_owned(),
            requested_ack: self.ack && features.acks,
            persistent_notifications: self.persistent
                && features.persistent
                && self.level == Level::Urgent,
        };
        priority.is_set().then_some(priority)
    }

    /// The choice a scheduled message carries.
    pub fn from_api(priority: Option<&PostPriority>) -> Self {
        priority.map_or_else(Self::default, |priority| Self {
            level: Level::of(&priority.priority),
            ack: priority.requested_ack,
            persistent: priority.persistent_notifications,
        })
    }

    /// "Urgent · Acknowledgement requested", for the composer's line and
    /// the scheduled list.
    pub fn summary(self) -> String {
        let mut parts: Vec<SharedString> = Vec::new();
        if self.level != Level::Standard {
            parts.push(self.level.label());
        }
        if self.ack {
            parts.push(t!("priority-ack-requested"));
        }
        if self.persistent {
            parts.push(t!("priority-persistent"));
        }
        parts.join(" · ") // i18n-ignore: a separator
    }
}

/// Someone who acknowledged a post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ack {
    pub user_id: SharedString,
    pub name: SharedString,
}

/// A post's priority, as the views draw it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    pub level: Level,
    /// The author asked for acknowledgements.
    pub requested_ack: bool,
    /// Mentioned people get repeated notifications until they acknowledge.
    pub persistent: bool,
    /// Who acknowledged, in order.
    pub acks: Vec<Ack>,
    /// When the viewer acknowledged (ms), if they did.
    pub mine: Option<i64>,
}

impl Shown {
    /// What `metadata` says, with names from `name`; `None` for a post
    /// with no priority and no acknowledgements.
    pub fn of(metadata: &PostMetadata, me: &str, name: impl Fn(&str) -> String) -> Option<Self> {
        let priority = metadata.priority.clone().unwrap_or_default();
        if !priority.is_set() && metadata.acknowledgements.is_empty() {
            return None;
        }
        let live = metadata
            .acknowledgements
            .iter()
            .filter(|ack| ack.acknowledged_at > 0);
        Some(Self {
            level: Level::of(&priority.priority),
            requested_ack: priority.requested_ack,
            persistent: priority.persistent_notifications,
            mine: live
                .clone()
                .find(|ack| ack.user_id == me)
                .map(|ack| ack.acknowledged_at),
            acks: live
                .map(|ack| Ack {
                    user_id: ack.user_id.clone().into(),
                    name: if ack.user_id == me {
                        t!("priority-you")
                    } else {
                        name(&ack.user_id).into()
                    },
                })
                .collect(),
        })
    }

    /// Whether the viewer may still take their acknowledgement back.
    pub fn can_undo(&self, now_ms: i64) -> bool {
        self.mine.is_some_and(|at| now_ms - at < UNDO_WINDOW_MS)
    }

    /// "Acknowledged by You, Ada and 2 others", for the avatars' tooltip.
    pub fn ack_summary(&self) -> String {
        let names: Vec<String> = self.acks.iter().map(|ack| ack.name.to_string()).collect();
        match names.as_slice() {
            [] => t!("priority-no-acks"),
            [first] => t!("priority-acked-by-one", first = first.clone()),
            [first, second] => t!(
                "priority-acked-by-two",
                first = first.clone(),
                second = second.clone()
            ),
            [first, second, third] => t!(
                "priority-acked-by-three",
                first = first.clone(),
                second = second.clone(),
                third = third.clone()
            ),
            [first, second, rest @ ..] => t!(
                "priority-acked-by-more",
                first = first.clone(),
                second = second.clone(),
                count = rest.len()
            ),
        }
        .to_string()
    }
}

/// The label after a post's time: a primary "Important" badge or a
/// destructive "Urgent" one, as the web app colors them. `None` for a
/// standard post or while the server has labels off.
pub fn label(shown: Option<&Shown>, cx: &App) -> Option<AnyElement> {
    let level = shown?.level;
    if level == Level::Standard || !Features::read(cx).labels {
        return None;
    }
    let theme = UiTheme::read(cx);
    Some(
        Badge::new(level.label().to_uppercase())
            .icon(level.icon())
            .variant(match level {
                Level::Urgent => BadgeVariant::Destructive,
                _ => BadgeVariant::Default,
            })
            .h(theme.space(4.5))
            .px(theme.space(1.5))
            .text_size(theme.text(10.))
            .into_any_element(),
    )
}

/// What happens when the viewer presses Acknowledge (`true`) or takes it
/// back (`false`).
pub type OnAck = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// Under a post that asks for acknowledgements: the Acknowledge button
/// (Acknowledged, with a check, once pressed; it takes the acknowledgement
/// back within [`UNDO_WINDOW_MS`]) and the avatars of who acknowledged with
/// their count. `None` when the post does not ask or the server has
/// acknowledgements off.
pub fn ack_bar(
    post_id: &str,
    shown: Option<&Shown>,
    sent: bool,
    on_ack: OnAck,
    cx: &App,
) -> Option<AnyElement> {
    let shown = shown?;
    if !shown.requested_ack || !Features::read(cx).acks {
        return None;
    }
    let theme = UiTheme::read(cx).clone();
    let now = jiff::Timestamp::now().as_millisecond();
    let acked = shown.mine.is_some();
    let undo = shown.can_undo(now);
    let label = if acked {
        t!("priority-acknowledged")
    } else {
        t!("priority-acknowledge")
    };
    let tip = match (acked, undo) {
        (false, _) => t!("priority-acknowledge-tip"),
        (true, true) => t!("priority-remove-ack"),
        (true, false) => t!("priority-acknowledged"),
    };
    let button = Button::new(gpui_kit::ElementId::Name(
        format!("post.{post_id}.ack").into(),
    ))
    .variant(if acked {
        ButtonVariant::Secondary
    } else {
        ButtonVariant::Outline
    })
    .size(ButtonSize::Sm)
    .aria_label(if acked && undo {
        t!("priority-remove-ack")
    } else {
        label.clone()
    })
    .child(
        lucide(if acked {
            LucideIcon::CircleCheck
        } else {
            LucideIcon::Check
        })
        .size(scaled(&theme, metrics::ICON_SMALL))
        .text_color(if acked {
            theme.colors.primary
        } else {
            theme.colors.foreground
        }),
    )
    .label(label)
    .disabled(!sent || (acked && !undo))
    .on_click(move |_, window, cx| on_ack(!acked, window, cx));
    let count = shown.acks.len();
    let people = (count > 0).then(|| {
        let summary = shown.ack_summary();
        style::tipped(
            gpui_kit::ElementId::Name(format!("post.{post_id}.acks").into()),
            summary.clone(),
            div()
                .flex()
                .items_center()
                .gap(theme.space(1.5))
                .child(div().flex().items_center().children(
                    shown.acks.iter().take(3).enumerate().map(|(index, ack)| {
                        let initial: String = ack.name.chars().take(1).collect();
                        div()
                            .when(index > 0, |slot| slot.ml(-theme.space(1.5)))
                            .rounded_full()
                            .border(gpui_kit::px(1.5))
                            .border_color(theme.colors.background)
                            .child(crate::avatars::with_picture(
                                Avatar::new(gpui_kit::ElementId::Name(
                                    format!("post.{post_id}.acks.{index}").into(),
                                ))
                                .size(AvatarSize::Sm)
                                .fallback(initial)
                                .aria_label(ack.name.clone()),
                                &ack.user_id,
                                cx,
                            ))
                    }),
                ))
                .child(
                    div()
                        .type_style(style::META, &theme)
                        .text_color(theme.colors.muted_foreground)
                        .child(count.to_string()),
                ),
        )
    });
    Some(
        div()
            .pt(theme.space(1.5))
            .flex()
            .items_center()
            .gap(theme.space(2.))
            .child(style::tipped(
                gpui_kit::ElementId::Name(format!("post.{post_id}.ack.tip").into()),
                tip,
                button,
            ))
            .children(people)
            .into_any_element(),
    )
}

/// What happens when the user picks a priority.
pub type OnChoice = Rc<dyn Fn(Choice, &mut Window, &mut App)>;

/// The priority menu: the three labels, then the requests the server
/// allows. Picking one keeps the others.
pub fn menu_items(choice: Choice, features: Features, on_choice: OnChoice) -> Vec<MenuItem> {
    let mut items = vec![MenuItem::label("priority.heading", t!("priority-heading"))];
    for level in Level::ALL {
        let on_choice = on_choice.clone();
        items.push(
            MenuItem::new(
                gpui_kit::ElementId::Name(format!("priority.{}", level.key()).into()),
                level.label(),
            )
            .radio(choice.level == level)
            .on_click(move |_, window, cx| {
                let persistent = choice.persistent && level == Level::Urgent;
                on_choice(
                    Choice {
                        level,
                        persistent,
                        ..choice
                    },
                    window,
                    cx,
                )
            }),
        );
    }
    if features.acks {
        items.push(MenuItem::separator());
        let ack_choice = on_choice.clone();
        items.push(
            MenuItem::new("priority.ack", t!("priority-request-ack"))
                .checked(choice.ack)
                .on_click(move |_, window, cx| {
                    ack_choice(
                        Choice {
                            ack: !choice.ack,
                            ..choice
                        },
                        window,
                        cx,
                    )
                }),
        );
        if features.persistent {
            items.push(
                MenuItem::new("priority.persistent", t!("priority-send-persistent"))
                    .checked(choice.persistent)
                    .disabled(choice.level != Level::Urgent)
                    .on_click(move |_, window, cx| {
                        on_choice(
                            Choice {
                                persistent: !choice.persistent,
                                ..choice
                            },
                            window,
                            cx,
                        )
                    }),
            );
        }
    }
    items
}

/// The composer's priority button, tinted while a priority is chosen.
/// `None` while the server has labels off.
pub fn composer_button(
    id: &'static str,
    choice: Choice,
    on_open: impl Fn(gpui_kit::Point<gpui_kit::Pixels>, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Option<AnyElement> {
    if !Features::read(cx).labels {
        return None;
    }
    let theme = UiTheme::read(cx);
    let color = match choice.level {
        Level::Urgent => theme.colors.destructive,
        Level::Important => theme.colors.primary,
        Level::Standard if choice.is_set() => theme.colors.primary,
        Level::Standard => theme.colors.muted_foreground,
    };
    Some(
        style::tipped(
            gpui_kit::ElementId::Name(format!("{id}.tip").into()),
            t!("priority-heading"),
            Button::new(id)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::IconSm)
                .aria_label(t!("priority-heading"))
                .child(
                    lucide(LucideIcon::CircleAlert)
                        .size(scaled(theme, metrics::ICON))
                        .text_color(color),
                )
                .on_click(move |event: &gpui_kit::ClickEvent, window, cx| {
                    on_open(event.position(), window, cx)
                }),
        )
        .into_any_element(),
    )
}

/// Above the composer while a priority is chosen: a tag naming it, whose
/// remove button goes back to a standard post.
pub fn choice_tag(
    id: &'static str,
    choice: Choice,
    on_clear: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Option<AnyElement> {
    if !choice.is_set() || !Features::read(cx).labels {
        return None;
    }
    let theme = UiTheme::read(cx);
    Some(
        div()
            .pb(theme.space(2.))
            .flex()
            .child(
                Tag::new(id, choice.summary())
                    .icon(choice.level.icon())
                    .variant(if choice.level == Level::Urgent {
                        TagVariant::Destructive
                    } else {
                        TagVariant::Secondary
                    })
                    .on_remove(move |_, window, cx| on_clear(window, cx)),
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use matty_api::models::PostAcknowledgement;

    #[test]
    fn a_choice_sends_only_what_the_server_allows() {
        let urgent = Choice {
            level: Level::Urgent,
            ack: true,
            persistent: true,
        };
        let sent = urgent.to_api(Features::all()).unwrap();
        assert!(sent.is_urgent() && sent.requested_ack && sent.persistent_notifications);
        // Each gate off drops its part.
        let labels_only = Features {
            labels: true,
            ..Features::default()
        };
        let sent = urgent.to_api(labels_only).unwrap();
        assert!(sent.is_urgent() && !sent.requested_ack && !sent.persistent_notifications);
        assert_eq!(urgent.to_api(Features::default()), None);
        // Persistent notifications are for urgent posts only.
        let important = Choice {
            level: Level::Important,
            ..urgent
        };
        assert!(
            !important
                .to_api(Features::all())
                .unwrap()
                .persistent_notifications
        );
        assert_eq!(Choice::default().to_api(Features::all()), None);
        assert_eq!(Choice::from_api(Some(&sent)).level, Level::Urgent);
        assert_eq!(
            urgent.summary(),
            "Urgent · Acknowledgement requested · Persistent notifications"
        );
    }

    #[test]
    fn acknowledgements_read_with_names_and_the_viewers_own() {
        let ack = |user: &str, at| PostAcknowledgement {
            user_id: user.into(),
            post_id: "p".into(),
            acknowledged_at: at,
        };
        let metadata = PostMetadata {
            priority: Some(PostPriority {
                priority: "urgent".into(),
                requested_ack: true,
                persistent_notifications: false,
            }),
            acknowledgements: vec![ack("me", 1_000), ack("u2", 2_000), ack("u3", 0)],
            ..PostMetadata::default()
        };
        let shown = Shown::of(&metadata, "me", |id| format!("name of {id}")).unwrap();
        assert_eq!(shown.level, Level::Urgent);
        assert_eq!(shown.mine, Some(1_000));
        assert_eq!(shown.acks.len(), 2, "a removed one is left out");
        assert_eq!(shown.ack_summary(), "Acknowledged by You and name of u2");
        let named = |name: &str| Ack {
            user_id: name.into(),
            name: name.into(),
        };
        let many = Shown {
            acks: ["Ada", "Bo", "Cy", "Di"].map(named).to_vec(),
            ..shown.clone()
        };
        assert_eq!(many.ack_summary(), "Acknowledged by Ada, Bo and 2 others");
        let three = Shown {
            acks: ["Ada", "Bo", "Cy"].map(named).to_vec(),
            ..shown.clone()
        };
        assert_eq!(three.ack_summary(), "Acknowledged by Ada, Bo and Cy");
        assert!(shown.can_undo(1_000 + UNDO_WINDOW_MS - 1));
        assert!(!shown.can_undo(1_000 + UNDO_WINDOW_MS));
        assert_eq!(
            Shown::of(&PostMetadata::default(), "me", |_| String::new()),
            None
        );
    }
}
