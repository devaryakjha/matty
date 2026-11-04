//! The Threads view (PLAN §4.4, DESIGN.md §4): every thread the user
//! follows, newest activity first, with the selected one open beside the
//! list. It replaces the channel view when the sidebar's Threads row is
//! chosen and never opens the right pane.

use gpui_icons::LucideIcon;
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, prelude::FluentBuilder as _, px,
};

use crate::channel::ReplySummary;
use crate::right_pane::{RightPaneContent, RightPaneView, ThreadModel};
use crate::settings::{ClockFormat, SettingsChanged, SettingsStore};
use crate::style::{self, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    avatar::{Avatar, AvatarSize},
    button::{Button, ButtonSize, ButtonVariant},
    theme::UiTheme,
    toggle_group::{ToggleGroup, ToggleGroupItem},
};

/// One followed thread, as the list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadRow {
    pub channel_id: SharedString,
    pub thread: ThreadModel,
    pub summary: ReplySummary,
}

impl ThreadRow {
    fn id(&self) -> &SharedString {
        &self.thread.root.id
    }
}

/// The user's followed threads, newest reply first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadsModel {
    pub threads: Vec<ThreadRow>,
    pub loading: bool,
}

impl ThreadsModel {
    pub fn unread(&self) -> usize {
        self.threads.iter().filter(|row| row.summary.unread).count()
    }
}

/// What the view asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadsEvent {
    /// The user read every thread; sync marks them on the server.
    MarkAllRead,
    /// The user stopped following `root`.
    Unfollow(SharedString),
}

pub struct ThreadsView {
    model: ThreadsModel,
    selected: Option<SharedString>,
    unread_only: bool,
    /// The open thread, drawn by the same view as the right pane.
    detail: Entity<RightPaneView>,
}

impl EventEmitter<ThreadsEvent> for ThreadsView {}

impl ThreadsView {
    pub fn new(
        model: ThreadsModel,
        me: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.subscribe(
            &SettingsStore::global(cx),
            |_, _, _: &SettingsChanged, cx| cx.notify(),
        )
        .detach();
        let detail = cx.new(|cx| RightPaneView::new(me, window, cx));
        let mut this = Self {
            model: ThreadsModel::default(),
            selected: None,
            unread_only: false,
            detail,
        };
        this.set_model(model, cx);
        this
    }

    pub fn model(&self) -> &ThreadsModel {
        &self.model
    }

    /// Replaces the list and keeps the selection when its thread is still
    /// there; otherwise the first thread opens.
    pub fn set_model(&mut self, model: ThreadsModel, cx: &mut Context<Self>) {
        self.model = model;
        let keep = self
            .selected
            .as_ref()
            .filter(|id| self.model.threads.iter().any(|row| row.id() == *id))
            .cloned();
        let next = keep.or_else(|| self.model.threads.first().map(|row| row.id().clone()));
        self.select(next, cx);
    }

    fn select(&mut self, id: Option<SharedString>, cx: &mut Context<Self>) {
        self.selected = id.clone();
        let row = id.and_then(|id| self.model.threads.iter_mut().find(|row| *row.id() == id));
        let content = row.map(|row| {
            // Opening a thread reads its replies.
            row.summary.unread = false;
            RightPaneContent::Thread(row.thread.clone())
        });
        self.detail
            .update(cx, |pane, cx| pane.set_content(content, cx));
        cx.notify();
    }

    fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        for row in &mut self.model.threads {
            row.summary.unread = false;
        }
        cx.emit(ThreadsEvent::MarkAllRead);
        cx.notify();
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let unread = self.model.unread();
        let filter: SharedString = if self.unread_only { "unread" } else { "all" }.into();
        let item = |id: &'static str, value: &'static str, text: String| {
            ToggleGroupItem::new(id, value)
                .aria_label(text.clone())
                .px(theme.space(2.5))
                .child(text)
        };
        div()
            .flex_shrink_0()
            .h(scaled(&theme, metrics::HEADER_HEIGHT))
            .px(scaled(&theme, metrics::GUTTER))
            .flex()
            .items_center()
            .justify_between()
            .gap(theme.space(3.))
            .border_b(style::hairline())
            .border_color(theme.colors.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space(3.))
                    .child(
                        div()
                            .type_style(style::TITLE, &theme)
                            .text_color(theme.colors.foreground)
                            .child("Threads"),
                    )
                    .child(
                        ToggleGroup::new("threads.filter")
                            .aria_label("Show")
                            .joined(true)
                            .value([filter])
                            .item(item("threads.filter.all", "all", "All".into()))
                            .item(item(
                                "threads.filter.unread",
                                "unread",
                                if unread > 0 {
                                    format!("Unread {unread}")
                                } else {
                                    "Unread".into()
                                },
                            ))
                            .on_change({
                                let view = cx.entity().downgrade();
                                move |values: Vec<SharedString>, _, cx| {
                                    let unread_only =
                                        values.first().is_some_and(|value| value == "unread");
                                    view.update(cx, |this, cx| {
                                        this.unread_only = unread_only;
                                        cx.notify();
                                    })
                                    .ok();
                                }
                            }),
                    ),
            )
            .when(unread > 0, |this| {
                this.child(
                    Button::new("threads.mark-all-read")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Sm)
                        .label("Mark all as read")
                        .on_click(cx.listener(|this, _, _, cx| this.mark_all_read(cx))),
                )
            })
    }

    fn row(&self, row: &ThreadRow, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let clock = ClockFormat::current(cx);
        let root = &row.thread.root;
        let selected = self.selected.as_ref() == Some(row.id());
        let id = row.id().clone();
        let initial: String = root.author.chars().take(1).collect();
        let channel = match row.thread.channel_name.as_ref() {
            name if name.contains(' ') => name.to_string(),
            name => format!("#{name}"),
        };
        let replies = match (row.summary.count, row.summary.unread) {
            (1, true) => "1 new reply".to_string(),
            (n, true) => format!("{n} new replies"),
            (1, false) => "1 reply".to_string(),
            (n, false) => format!("{n} replies"),
        };
        div()
            .id(gpui_kit::ElementId::Name(format!("threads.{id}").into()))
            .role(gpui_kit::Role::Button)
            .aria_label(format!("{}, {replies}, in {channel}", root.author))
            .mx(theme.space(2.))
            .px(theme.space(3.))
            .py(theme.space(2.5))
            .rounded(theme.radius.md)
            .flex()
            .gap(theme.space(3.))
            .cursor_pointer()
            .when(selected, |this| this.bg(theme.colors.accent))
            .when(!selected, |this| {
                this.hover(|style| style.bg(theme.colors.accent.opacity(0.6)))
            })
            .on_click(cx.listener(move |this, _, _, cx| this.select(Some(id.clone()), cx)))
            .child(
                Avatar::new(gpui_kit::ElementId::Name(
                    format!("threads.{}.avatar", row.id()).into(),
                ))
                .size(AvatarSize::Default)
                .fallback(initial)
                .aria_label(root.author.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(theme.space(1.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(theme.space(1.5))
                            .child(
                                div()
                                    .type_style(style::UI_STRONG, &theme)
                                    .text_color(theme.colors.foreground)
                                    .child(root.author.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .type_style(style::META, &theme)
                                    .text_color(theme.colors.muted_foreground)
                                    .child(format!("in {channel}")),
                            )
                            .child(
                                div()
                                    .type_style(style::META, &theme)
                                    .text_color(theme.colors.muted_foreground)
                                    .child(row.summary.last_reply_label(clock)),
                            ),
                    )
                    .child(
                        div()
                            .max_h(theme.space(9.))
                            .overflow_hidden()
                            .type_style(style::UI, &theme)
                            .text_color(theme.colors.muted_foreground)
                            .child(snippet(&root.body)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space(2.))
                            .pt(theme.space(0.5))
                            .child(div().flex().items_center().children(
                                row.summary.participants.iter().take(3).enumerate().map(
                                    |(index, name)| {
                                        let initial: String = name.chars().take(1).collect();
                                        div().when(index > 0, |this| this.ml(px(-4.))).child(
                                            Avatar::new(gpui_kit::ElementId::Name(
                                                format!("threads.{}.p{index}", row.id()).into(),
                                            ))
                                            .size(AvatarSize::Sm)
                                            .fallback(initial)
                                            .aria_label(name.clone()),
                                        )
                                    },
                                ),
                            ))
                            .child(
                                div()
                                    .type_style(
                                        if row.summary.unread {
                                            style::UI_STRONG
                                        } else {
                                            style::META
                                        },
                                        &theme,
                                    )
                                    .text_color(if row.summary.unread {
                                        theme.colors.primary
                                    } else {
                                        theme.colors.muted_foreground
                                    })
                                    .child(replies),
                            ),
                    ),
            )
            .when(row.summary.unread, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .mt(theme.space(1.5))
                        .size(theme.space(2.))
                        .rounded_full()
                        .bg(theme.colors.primary),
                )
            })
    }

    fn empty(&self, cx: &App) -> impl IntoElement + use<> {
        let (title, description) = if self.unread_only {
            (
                "No unread threads",
                "You are caught up. New replies will show here.",
            )
        } else {
            (
                "No followed threads",
                "Reply in a thread, or choose Follow on one, and it shows up here.",
            )
        };
        style::centered_empty(LucideIcon::MessagesSquare, title, description, cx)
    }
}

/// The first line of a Markdown body as plain text, for a list row.
fn snippet(body: &str) -> String {
    let line = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("```"))
        .unwrap_or("");
    line.replace(['*', '`', '_', '>', '#'], "")
}

impl Render for ThreadsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let rows: Vec<ThreadRow> = self
            .model
            .threads
            .iter()
            .filter(|row| !self.unread_only || row.summary.unread)
            .cloned()
            .collect();
        let list = div()
            .w(px(360.))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .border_r(style::hairline())
            .border_color(theme.colors.border)
            .child(self.header(cx))
            .child(if rows.is_empty() {
                self.empty(cx).into_any_element()
            } else {
                div()
                    .id("threads.list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py(theme.space(2.))
                    .flex()
                    .flex_col()
                    .gap(theme.space(0.5))
                    .children(rows.iter().map(|row| self.row(row, cx)))
                    .into_any_element()
            });
        div()
            .size_full()
            .min_w_0()
            .flex()
            .bg(theme.colors.background)
            .child(list)
            .child(div().flex_1().min_w_0().h_full().child(self.detail.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::snippet;

    #[test]
    fn snippet_takes_the_first_plain_line() {
        assert_eq!(
            snippet("Two small **comments**.\n\nMore"),
            "Two small comments."
        );
        assert_eq!(snippet("```rust\nlet x = 1;\n```"), "let x = 1;");
        assert_eq!(snippet(""), "");
    }
}
