//! Scheduled messages (#220): "Schedule message" from the composer's send
//! menu, the dialog that picks a day and time (or edits one), the
//! Scheduled tab of the Drafts view, and the "N scheduled" line above a
//! channel's composer. Hidden while the server does not allow scheduling
//! ([`crate::priority::Features::scheduled`]).
//!
//! The custom time comes from gpuicn's date and time picker, kept to times
//! still ahead ([`crate::when`]).

use std::sync::Arc;

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _,
};
use jiff::Zoned;
use matty_api::models::ScheduledPost;

use crate::dnd;
use crate::priority::{Choice, Features};
use crate::settings::ClockFormat;
use crate::style::{self, TypeStyleExt as _, metrics, scaled};
use crate::t;
use crate::ui::{
    button::{Button, ButtonSize, ButtonVariant},
    dialog::{DialogHandle, dialog, dialog_description, dialog_popup, dialog_title},
    input::{Textarea, TextareaState},
    markdown::markdown,
    menu::MenuItem,
    tag::{Tag, TagVariant},
    theme::UiTheme,
};

/// The user's scheduled messages, soonest first, as sync last sent them.
/// An app global, so every composer can count its channel's.
#[derive(Clone, Default)]
pub struct Scheduled(pub Arc<[ScheduledPost]>);

impl Global for Scheduled {}

impl Scheduled {
    pub fn read(cx: &App) -> Arc<[ScheduledPost]> {
        cx.try_global::<Self>()
            .map(|scheduled| scheduled.0.clone())
            .unwrap_or_default()
    }

    /// Replaces the list, redrawing every window.
    pub fn set(posts: Arc<[ScheduledPost]>, cx: &mut App) {
        cx.set_global(Self(posts));
        cx.refresh_windows();
    }

    /// How many are set for the channel's own composer (not its threads).
    pub fn count_in(channel_id: &str, cx: &App) -> usize {
        Self::read(cx)
            .iter()
            .filter(|post| post.channel_id == channel_id && post.root_id.is_empty())
            .count()
    }
}

/// A ready-made time in the composer's schedule menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub label: SharedString,
    /// When, in ms since the epoch.
    pub at: i64,
}

/// The menu's ready-made times, as the web app offers them: tomorrow at
/// 9:00, and next Monday at 9:00 unless that is tomorrow.
pub fn presets(now: &Zoned, clock: ClockFormat) -> Vec<Preset> {
    let nine = clock.format(9 * 60);
    let today = now.date();
    let mut presets = Vec::new();
    let tomorrow = today.tomorrow().ok();
    if let Some(date) = tomorrow
        && let Some(at) = dnd::local_end(date, 9 * 60, now.time_zone())
    {
        presets.push(Preset {
            id: "tomorrow",
            label: t!("schedule-preset-tomorrow", time = nine.clone()),
            at: at * 1000,
        });
    }
    let monday = (1..=7).find_map(|offset| {
        let date = today.checked_add(jiff::ToSpan::days(offset)).ok()?;
        (date.weekday() == jiff::civil::Weekday::Monday).then_some(date)
    });
    if let Some(date) = monday
        && Some(date) != tomorrow
        && let Some(at) = dnd::local_end(date, 9 * 60, now.time_zone())
    {
        presets.push(Preset {
            id: "monday",
            label: t!("schedule-preset-monday", time = nine.clone()),
            at: at * 1000,
        });
    }
    presets
}

/// A scheduled time's parts for the strings that show it: `today`,
/// `tomorrow` or `other` (with the date), and the time of day.
fn when(at_ms: i64, now: &Zoned, clock: ClockFormat) -> Option<(&'static str, String, String)> {
    let at = jiff::Timestamp::from_millisecond(at_ms).ok()?;
    let at = at.to_zoned(now.time_zone().clone());
    let time = clock.format(at.hour() as u16 * 60 + at.minute() as u16);
    let today = now.date();
    let (day, date) = if at.date() == today {
        ("today", String::new())
    } else if Some(at.date()) == today.tomorrow().ok() {
        ("tomorrow", String::new())
    } else {
        ("other", at.strftime("%A, %B %-d").to_string())
    };
    Some((day, date, time))
}

/// How a scheduled time reads: "Today at 14:30", "Tomorrow at 09:00" or
/// "Friday, November 6 at 14:30".
pub fn when_label(at_ms: i64, now: &Zoned, clock: ClockFormat) -> SharedString {
    when(at_ms, now, clock)
        .map(|(day, date, time)| t!("schedule-when", day = day, date = date, time = time))
        .unwrap_or_default()
}

/// A scheduled card's tag: "Send tomorrow at 09:00".
fn send_label(at_ms: i64, now: &Zoned, clock: ClockFormat) -> SharedString {
    when(at_ms, now, clock)
        .map(|(day, date, time)| t!("scheduled-send-at", day = day, date = date, time = time))
        .unwrap_or_default()
}

/// What the composer's send menu offers: the presets, then "Custom time".
/// `pick` gets a preset's time, or `None` for the custom dialog.
pub fn menu_items(
    pick: impl Fn(Option<i64>, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> Vec<MenuItem> {
    let now = Zoned::now();
    let mut items = vec![MenuItem::label("schedule.heading", t!("schedule-message"))];
    for preset in presets(&now, ClockFormat::current(cx)) {
        let (pick, at) = (pick.clone(), preset.at);
        items.push(
            MenuItem::new(
                gpui_kit::ElementId::Name(format!("schedule.{}", preset.id).into()),
                preset.label,
            )
            .on_click(move |_, window, cx| pick(Some(at), window, cx)),
        );
    }
    items.push(
        MenuItem::new("schedule.custom", t!("schedule-custom-time"))
            .on_click(move |_, window, cx| pick(None, window, cx)),
    );
    items
}

/// The composer's "Schedule message" button beside Send. `None` while the
/// server does not allow scheduling.
pub fn composer_button(
    id: &'static str,
    enabled: bool,
    on_open: impl Fn(gpui_kit::Point<gpui_kit::Pixels>, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Option<AnyElement> {
    if !Features::read(cx).scheduled {
        return None;
    }
    let theme = UiTheme::read(cx);
    Some(
        style::tipped(
            gpui_kit::ElementId::Name(format!("{id}.tip").into()),
            t!("schedule-message"),
            Button::new(id)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::IconSm)
                .aria_label(t!("schedule-message"))
                .disabled(!enabled)
                .child(
                    lucide(LucideIcon::CalendarClock)
                        .size(scaled(theme, metrics::ICON))
                        .text_color(theme.colors.muted_foreground),
                )
                .on_click(move |event: &gpui_kit::ClickEvent, window, cx| {
                    on_open(event.position(), window, cx)
                }),
        )
        .into_any_element(),
    )
}

/// Above a channel's composer while it has scheduled messages: "2
/// scheduled messages in this channel", with See all.
pub fn composer_hint(
    id: &'static str,
    channel_id: &str,
    on_see_all: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Option<AnyElement> {
    if !Features::read(cx).scheduled {
        return None;
    }
    let count = Scheduled::count_in(channel_id, cx);
    if count == 0 {
        return None;
    }
    let theme = UiTheme::read(cx);
    let text = t!("scheduled-in-channel", count = count);
    Some(
        div()
            .pb(theme.space(1.5))
            .flex()
            .items_center()
            .gap(theme.space(1.5))
            .type_style(style::META, theme)
            .text_color(theme.colors.muted_foreground)
            .child(
                lucide(LucideIcon::CalendarClock)
                    .size(scaled(theme, metrics::ICON_TINY))
                    .text_color(theme.colors.muted_foreground),
            )
            .child(text)
            .child(
                div()
                    .id(id)
                    .text_color(theme.colors.primary)
                    .cursor_pointer()
                    .hover(|link| link.underline())
                    .child(t!("scheduled-see-all"))
                    .on_click(move |_, window, cx| on_see_all(window, cx)),
            )
            .into_any_element(),
    )
}

/// What the schedule dialog decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScheduleDialogEvent {
    /// Schedule for `at` (ms since the epoch), with the message changed to
    /// `message` when the dialog edits one.
    Confirm {
        at: i64,
        message: Option<SharedString>,
    },
}

/// Picks when a message posts, a time still ahead, and for an edit the
/// message too.
pub struct ScheduleDialog {
    handle: DialogHandle,
    when: Entity<crate::ui::date_time_picker::DateTimeState>,
    /// The message, when editing a scheduled one.
    message: Option<Entity<TextareaState>>,
    _subscriptions: Vec<gpui_kit::Subscription>,
}

impl EventEmitter<ScheduleDialogEvent> for ScheduleDialog {}

impl ScheduleDialog {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let when = crate::when::state(cx);
        Self {
            handle: DialogHandle::new(false),
            _subscriptions: vec![cx.subscribe(
                &when,
                |_, _, _: &crate::ui::date_time_picker::DateTimeEvent, cx| cx.notify(),
            )],
            when,
            message: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.handle.is_open()
    }

    /// Opens on `at` (ms), or the next half hour; with `message`, the
    /// dialog edits a scheduled message's text too.
    pub fn open(
        &mut self,
        at: Option<i64>,
        message: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = Zoned::now();
        let at = at
            .and_then(|at| jiff::Timestamp::from_millisecond(at).ok())
            .map(|at| at.to_zoned(now.time_zone().clone()));
        let clock = ClockFormat::current(cx);
        crate::when::reset(&self.when, at.as_ref(), &now, clock, cx);
        self.message = message.map(|text| {
            cx.new(|cx| {
                TextareaState::new(window, cx)
                    .default_value(SharedString::from(text))
                    .rows(4)
            })
        });
        self.handle.open(window, cx);
        cx.notify();
    }

    /// The pick in ms since the epoch, if it is still ahead.
    fn picked(&self, cx: &App) -> Option<i64> {
        crate::when::picked(&self.when, &Zoned::now(), cx).map(|at| at * 1000)
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.picked(cx) else {
            return;
        };
        let message = self
            .message
            .as_ref()
            .map(|state| SharedString::from(state.read(cx).value().trim().to_string()));
        if message.as_ref().is_some_and(|text| text.is_empty()) {
            return;
        }
        self.handle.close(window, cx);
        cx.emit(ScheduleDialogEvent::Confirm { at, message });
    }
}

impl Render for ScheduleDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let picked = self.picked(cx);
        let editing = self.message.is_some();
        let title = if editing {
            t!("schedule-edit-title")
        } else {
            t!("schedule-message")
        };
        let popup = dialog_popup("schedule.popup", title.clone(), cx)
            .child(dialog_title("schedule.title", cx).child(title))
            .child(dialog_description("schedule.description", cx).child(t!("schedule-description")))
            .children(self.message.as_ref().map(|state| {
                div().pt(theme.space(2.)).child(
                    Textarea::new(state).bg(theme.colors.popover.blend(theme.control().background)),
                )
            }))
            .child(
                div().pt(theme.space(2.)).child(
                    crate::ui::date_time_picker::DateTimePicker::new("schedule.when", &self.when)
                        .aria_label(t!("schedule-posts-at")),
                ),
            )
            .when(picked.is_none(), |popup| {
                popup.child(
                    div()
                        .type_style(style::META, &theme)
                        .text_color(theme.colors.destructive)
                        .child(t!("schedule-time-passed")),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        Button::new("schedule.cancel")
                            .variant(ButtonVariant::Outline)
                            .label(t!("schedule-cancel"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.handle.close(window, cx);
                            })),
                    )
                    .child(
                        Button::new("schedule.save")
                            .variant(ButtonVariant::Default)
                            .label(if editing {
                                t!("schedule-save")
                            } else {
                                t!("schedule-confirm")
                            })
                            .disabled(picked.is_none())
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                    ),
            );
        dialog("schedule", &self.handle, popup, window, cx)
    }
}

/// One scheduled message, as the list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub post: ScheduledPost,
    /// Where it posts: "#town-square", a person's name, or "Reply in …".
    pub place: SharedString,
}

/// What the Scheduled list asks the window to do.
#[derive(Clone, Debug, PartialEq)]
pub enum ScheduledEvent {
    /// Save a changed message or time.
    Save(ScheduledPost),
    /// Post it now.
    SendNow(String),
    /// Throw it away.
    Delete(String),
    /// Open the channel it posts in.
    Open(ScheduledPost),
}

/// The Scheduled tab of the Drafts view.
pub struct ScheduledView {
    rows: Vec<Row>,
    dialog: Entity<ScheduleDialog>,
    /// The message the dialog edits.
    editing: Option<ScheduledPost>,
    _subscription: gpui_kit::Subscription,
}

impl EventEmitter<ScheduledEvent> for ScheduledView {}

impl ScheduledView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let dialog = cx.new(|cx| ScheduleDialog::new(window, cx));
        let subscription = cx.subscribe(
            &dialog,
            |this: &mut Self, _, event: &ScheduleDialogEvent, cx| {
                let ScheduleDialogEvent::Confirm { at, message } = event;
                if let Some(mut post) = this.editing.take() {
                    post.scheduled_at = *at;
                    if let Some(message) = message {
                        post.message = message.to_string();
                    }
                    cx.emit(ScheduledEvent::Save(post));
                }
            },
        );
        Self {
            rows: Vec::new(),
            dialog,
            editing: None,
            _subscription: subscription,
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn set_rows(&mut self, rows: Vec<Row>, cx: &mut Context<Self>) {
        if self.rows != rows {
            self.rows = rows;
            cx.notify();
        }
    }

    fn edit(&mut self, post: &ScheduledPost, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = Some(post.clone());
        let message = post.message.clone();
        let at = post.scheduled_at;
        self.dialog.update(cx, |dialog, cx| {
            dialog.open(Some(at), Some(message), window, cx)
        });
    }

    /// Deletes the scheduled message `id`: gone from the list at once.
    pub fn delete(&mut self, id: &str, cx: &mut Context<Self>) {
        self.remove(id, cx);
        cx.emit(ScheduledEvent::Delete(id.to_string()));
    }

    fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        self.rows.retain(|row| row.post.id != id);
        cx.notify();
    }

    fn card(&self, index: usize, row: &Row, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let now = Zoned::now();
        let when = send_label(row.post.scheduled_at, &now, ClockFormat::current(cx));
        let choice = Choice::from_api(row.post.metadata.priority.as_ref());
        let failed = !row.post.error_code.is_empty();
        let action = |id: String, label: SharedString, variant: ButtonVariant| {
            Button::new(gpui_kit::ElementId::Name(id.into()))
                .variant(variant)
                .size(ButtonSize::Sm)
                .label(label)
        };
        let (edit, send, delete, open) = (
            row.post.clone(),
            row.post.id.clone(),
            row.post.id.clone(),
            row.post.clone(),
        );
        div()
            .id(gpui_kit::ElementId::Name(
                format!("scheduled.{index}").into(),
            ))
            .mx(scaled(&theme, metrics::GUTTER))
            .p(theme.space(4.))
            .rounded(theme.radius.lg)
            .border(style::hairline())
            .border_color(theme.colors.border)
            .bg(theme.colors.card)
            .flex()
            .flex_col()
            .gap(theme.space(3.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme.space(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space(1.5))
                            .child(
                                lucide(if row.post.root_id.is_empty() {
                                    LucideIcon::Hash
                                } else {
                                    LucideIcon::MessageSquareReply
                                })
                                .size(scaled(&theme, metrics::ICON_SMALL))
                                .text_color(theme.colors.muted_foreground),
                            )
                            .child(
                                div()
                                    .type_style(style::UI_STRONG, &theme)
                                    .text_color(theme.colors.foreground)
                                    .child(row.place.clone()),
                            )
                            .children(crate::priority::label(
                                Some(&crate::priority::Shown {
                                    level: choice.level,
                                    ..Default::default()
                                }),
                                cx,
                            )),
                    )
                    .child(
                        Tag::new(
                            gpui_kit::ElementId::Name(format!("scheduled.{index}.when").into()),
                            if failed { t!("scheduled-failed") } else { when },
                        )
                        .icon(if failed {
                            LucideIcon::CircleAlert
                        } else {
                            LucideIcon::CalendarClock
                        })
                        .variant(if failed {
                            TagVariant::Destructive
                        } else {
                            TagVariant::Secondary
                        }),
                    ),
            )
            .child(
                div()
                    .type_style(style::BODY, &theme)
                    .text_color(theme.colors.foreground)
                    .child(markdown(
                        gpui_kit::ElementId::Name(format!("scheduled.{index}.body").into()),
                        SharedString::from(row.post.message.clone()),
                        cx,
                    )),
            )
            .when(choice.ack || choice.persistent, |card| {
                card.child(
                    div()
                        .type_style(style::META, &theme)
                        .text_color(theme.colors.muted_foreground)
                        .child(
                            Choice {
                                level: crate::priority::Level::Standard,
                                ..choice
                            }
                            .summary(),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pt(theme.space(1.))
                    .child(
                        action(
                            format!("scheduled.{index}.delete"),
                            t!("scheduled-delete"),
                            ButtonVariant::Ghost,
                        )
                        .text_color(theme.colors.destructive)
                        .on_click(cx.listener(move |this, _, _, cx| this.delete(&delete, cx))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space(2.))
                            .child(
                                action(
                                    format!("scheduled.{index}.open"),
                                    t!("scheduled-open"),
                                    ButtonVariant::Ghost,
                                )
                                .on_click(cx.listener(
                                    move |_, _, _, cx| cx.emit(ScheduledEvent::Open(open.clone())),
                                )),
                            )
                            .child(
                                action(
                                    format!("scheduled.{index}.edit"),
                                    t!("scheduled-edit"),
                                    ButtonVariant::Outline,
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| this.edit(&edit, window, cx),
                                )),
                            )
                            .child(
                                action(
                                    format!("scheduled.{index}.send"),
                                    t!("scheduled-send-now"),
                                    ButtonVariant::Default,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.remove(&send, cx);
                                        cx.emit(ScheduledEvent::SendNow(send.clone()));
                                    },
                                )),
                            ),
                    ),
            )
    }
}

impl Render for ScheduledView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let rows = self.rows.clone();
        let root = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.dialog.clone());
        if rows.is_empty() {
            return root.child(style::centered_empty(
                LucideIcon::CalendarClock,
                t!("scheduled-empty-title"),
                t!("scheduled-empty-body"),
                cx,
            ));
        }
        root.child(
            div()
                .id("scheduled.list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .py(theme.space(4.))
                .flex()
                .flex_col()
                .gap(theme.space(3.))
                .max_w(scaled(&theme, metrics::DRAFTS_MAX_WIDTH))
                .children(
                    rows.iter()
                        .enumerate()
                        .map(|(index, row)| self.card(index, row, cx)),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zoned(text: &str) -> Zoned {
        text.parse().unwrap()
    }

    #[test]
    fn presets_land_on_tomorrow_and_next_monday_at_nine() {
        // A Wednesday.
        let now = zoned("2026-11-04T15:00:00+00:00[UTC]");
        let offered = presets(&now, ClockFormat::TwentyFourHour);
        let labels: Vec<&str> = offered.iter().map(|p| p.label.as_ref()).collect();
        assert_eq!(labels, ["Tomorrow at 09:00", "Monday at 09:00"]);
        assert_eq!(
            offered[0].at,
            zoned("2026-11-05T09:00:00+00:00[UTC]")
                .timestamp()
                .as_millisecond()
        );
        assert_eq!(
            offered[1].at,
            zoned("2026-11-09T09:00:00+00:00[UTC]")
                .timestamp()
                .as_millisecond()
        );
        // On a Sunday, Monday is tomorrow: offered once.
        let sunday = zoned("2026-11-08T15:00:00+00:00[UTC]");
        assert_eq!(presets(&sunday, ClockFormat::TwentyFourHour).len(), 1);
    }

    #[test]
    fn scheduled_times_read_relative_to_today() {
        let now = zoned("2026-11-04T15:00:00+00:00[UTC]");
        let at = |text: &str| zoned(text).timestamp().as_millisecond();
        let clock = ClockFormat::TwentyFourHour;
        assert_eq!(
            when_label(at("2026-11-04T17:30:00+00:00[UTC]"), &now, clock),
            "Today at 17:30"
        );
        assert_eq!(
            when_label(at("2026-11-05T09:00:00+00:00[UTC]"), &now, clock),
            "Tomorrow at 09:00"
        );
        assert_eq!(
            when_label(at("2026-11-09T09:00:00+00:00[UTC]"), &now, clock),
            "Monday, November 9 at 09:00"
        );
        assert_eq!(
            send_label(at("2026-11-05T09:00:00+00:00[UTC]"), &now, clock),
            "Send tomorrow at 09:00"
        );
        assert_eq!(
            send_label(at("2026-11-09T09:00:00+00:00[UTC]"), &now, clock),
            "Send on Monday, November 9 at 09:00"
        );
    }
}
