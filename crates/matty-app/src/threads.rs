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
    spinner::Spinner,
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
    /// The server follows more threads than `threads` lists; reaching the
    /// end of the list pages them in (#122).
    pub more: bool,
    /// The last page failed; the list's last row offers Retry.
    pub failed: bool,
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
    /// The thread under `root` opened beside the list; `unread` when it
    /// had replies the user had not read, which opening reads.
    Open { root: SharedString, unread: bool },
    /// The list reached its last row while the server follows more
    /// threads; sync pages them in (#122).
    LoadMore,
}

/// How close to the end of the list, in pixels, the next page is asked for.
const LOAD_MORE_WITHIN: f32 = 240.;

pub struct ThreadsView {
    model: ThreadsModel,
    selected: Option<SharedString>,
    unread_only: bool,
    /// The open thread, drawn by the same view as the right pane.
    detail: Entity<RightPaneView>,
    /// Where the list is scrolled, to ask for more threads near its end.
    scroll: gpui_kit::ScrollHandle,
    /// A page of threads is on its way (#122).
    loading_more: bool,
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
            scroll: gpui_kit::ScrollHandle::new(),
            loading_more: false,
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
        self.loading_more = false;
        let keep = self
            .selected
            .as_ref()
            .filter(|id| self.model.threads.iter().any(|row| row.id() == *id))
            .cloned();
        let next = keep.or_else(|| self.model.threads.first().map(|row| row.id().clone()));
        self.select(next, cx);
    }

    fn select(&mut self, id: Option<SharedString>, cx: &mut Context<Self>) {
        let changed = self.selected != id;
        self.selected = id.clone();
        let row = id.and_then(|id| self.model.threads.iter_mut().find(|row| *row.id() == id));
        let Some(row) = row else {
            self.detail
                .update(cx, |pane, cx| pane.set_content(None, cx));
            cx.notify();
            return;
        };
        // Opening a thread reads its replies.
        let unread = std::mem::take(&mut row.summary.unread);
        let root = row.id().clone();
        if changed {
            let content = Some(RightPaneContent::Thread(row.thread.clone()));
            self.detail
                .update(cx, |pane, cx| pane.set_content(content, cx));
        }
        if changed || unread {
            cx.emit(ThreadsEvent::Open { root, unread });
        }
        cx.notify();
    }

    /// The open thread's root, for the window to open it in sync again
    /// when the view comes back.
    pub fn selected(&self) -> Option<&SharedString> {
        self.selected.as_ref()
    }

    /// The pane the open thread is drawn in, for the window to handle its
    /// replies and actions as it does the right pane's.
    pub fn detail(&self) -> &Entity<RightPaneView> {
        &self.detail
    }

    /// Sync's latest of the open thread, with its replies.
    pub fn set_thread(&mut self, thread: ThreadModel, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&thread.root.id) {
            self.detail
                .update(cx, |pane, cx| pane.set_thread(thread, cx));
        }
    }

    /// Asks for the next page when the list's end is in reach and nothing
    /// stops it (#122). Before the list is laid out its scroll is unknown,
    /// so the check waits a frame.
    fn request_more(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.model.more || self.model.failed || self.loading_more {
            return;
        }
        if self.scroll.bounds().size.height <= px(0.) {
            window.request_animation_frame();
            return;
        }
        let left = self.scroll.max_offset().y + self.scroll.offset().y;
        if left > px(LOAD_MORE_WITHIN) {
            return;
        }
        self.loading_more = true;
        cx.emit(ThreadsEvent::LoadMore);
    }

    /// The list's last row while more threads are on the server: the next
    /// page's progress, or its failure with Retry.
    fn more_row(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        if !self.model.more {
            return None;
        }
        let theme = UiTheme::read(cx).clone();
        let row = div()
            .flex()
            .items_center()
            .justify_center()
            .gap(theme.space(2.))
            .px(theme.space(3.))
            .py(theme.space(3.))
            .text_size(theme.text(12.))
            .text_color(theme.colors.muted_foreground)
            .debug_selector(|| "threads.more".into());
        Some(if self.model.failed {
            row.child("Couldn't load more threads.")
                .child(
                    div().debug_selector(|| "threads.more.retry".into()).child(
                        Button::new("threads.more.retry")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Sm)
                            .label("Retry")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.failed = false;
                                this.loading_more = true;
                                cx.emit(ThreadsEvent::LoadMore);
                                cx.notify();
                            })),
                    ),
                )
                .into_any_element()
        } else {
            row.child(
                Spinner::new("threads.more.spinner")
                    .label("Loading more threads")
                    .size(px(14.)),
            )
            .child("Loading more threads…")
            .into_any_element()
        })
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
            .child(crate::avatars::with_picture(
                Avatar::new(gpui_kit::ElementId::Name(
                    format!("threads.{}.avatar", row.id()).into(),
                ))
                .size(AvatarSize::Default)
                .fallback(initial)
                .aria_label(root.author.clone()),
                &root.author_id,
                cx,
            ))
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
                                        let user_id = row
                                            .summary
                                            .participant_ids
                                            .get(index)
                                            .map(SharedString::as_ref)
                                            .unwrap_or_default();
                                        div().when(index > 0, |this| this.ml(px(-4.))).child(
                                            crate::avatars::with_picture(
                                                Avatar::new(gpui_kit::ElementId::Name(
                                                    format!("threads.{}.p{index}", row.id()).into(),
                                                ))
                                                .size(AvatarSize::Sm)
                                                .fallback(initial)
                                                .aria_label(name.clone()),
                                                user_id,
                                                cx,
                                            ),
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.request_more(window, cx);
        let theme = UiTheme::read(cx).clone();
        let rows: Vec<ThreadRow> = self
            .model
            .threads
            .iter()
            .filter(|row| !self.unread_only || row.summary.unread)
            .cloned()
            .collect();
        let list = div()
            .w(scaled(&theme, metrics::THREADS_LIST_WIDTH))
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
                    .track_scroll(&self.scroll)
                    .py(theme.space(2.))
                    .flex()
                    .flex_col()
                    .gap(theme.space(0.5))
                    .children(rows.iter().map(|row| self.row(row, cx)))
                    .children(self.more_row(cx))
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
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// #122: a list that ends short of the server's threads asks for the
    /// next page once, shows its progress, and after a failure waits for
    /// Retry.
    #[gpui_kit::test]
    fn the_end_of_the_list_asks_for_more_threads(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let model = |more, failed| ThreadsModel {
            more,
            failed,
            ..crate::preview::threads()
        };
        let window =
            cx.add_window(|window, cx| ThreadsView::new(model(true, false), "me", window, cx));
        let view = window.root(cx).unwrap();
        let events = Rc::new(RefCell::new(0));
        cx.update(|cx| {
            let events = events.clone();
            cx.subscribe(&view, move |_, event: &ThreadsEvent, _| {
                if matches!(event, ThreadsEvent::LoadMore) {
                    *events.borrow_mut() += 1;
                }
            })
            .detach();
        });
        let mut cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let draw = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.run_until_parked();
        };
        for _ in 0..3 {
            draw(&mut cx);
        }
        assert_eq!(*events.borrow(), 1, "asked once");
        assert!(cx.debug_bounds("threads.more").is_some());

        cx.update(|_, cx| view.update(cx, |view, cx| view.set_model(model(true, true), cx)));
        draw(&mut cx);
        assert_eq!(*events.borrow(), 1, "a failed page waits for Retry");
        let retry = cx.debug_bounds("threads.more.retry").expect("Retry shows");
        cx.simulate_click(retry.center(), Default::default());
        draw(&mut cx);
        assert_eq!(*events.borrow(), 2);

        cx.update(|_, cx| view.update(cx, |view, cx| view.set_model(model(false, false), cx)));
        draw(&mut cx);
        assert_eq!(*events.borrow(), 2, "every thread is listed");
        assert!(cx.debug_bounds("threads.more").is_none());
    }

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
