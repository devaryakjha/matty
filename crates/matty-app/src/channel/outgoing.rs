//! The channel composer's message priority and scheduling (#220); see
//! [`crate::priority`] and [`crate::scheduled`].

use gpui_kit::{AnyElement, AppContext as _, Context, Entity, SharedString, Window};

use super::{ChannelEvent, ChannelView};
use crate::attachments::{PostFile, SendGate};
use crate::priority::{Choice, Features};
use crate::scheduled::{ScheduleDialog, ScheduleDialogEvent};

/// The schedule dialog for a channel composer, already wired to it.
pub(super) fn schedule_dialog(
    window: &mut Window,
    cx: &mut Context<ChannelView>,
) -> (Entity<ScheduleDialog>, gpui_kit::Subscription) {
    let dialog = cx.new(|cx| ScheduleDialog::new(window, cx));
    let subscription = cx.subscribe_in(
        &dialog,
        window,
        |this: &mut ChannelView, _, event: &ScheduleDialogEvent, window, cx| {
            let ScheduleDialogEvent::Confirm { at, .. } = event;
            this.schedule(*at, window, cx);
        },
    );
    (dialog, subscription)
}

impl ChannelView {
    /// The priority the next post goes with, as sync sends it.
    pub(super) fn take_priority(
        &mut self,
        cx: &Context<Self>,
    ) -> Option<matty_api::models::PostPriority> {
        let priority = self.priority.to_api(Features::read(cx));
        self.priority = Choice::default();
        priority
    }

    /// The optimistic row's label, until sync's copy replaces it.
    pub(super) fn shown_priority(
        priority: Option<&matty_api::models::PostPriority>,
    ) -> Option<crate::priority::Shown> {
        let metadata = matty_api::models::PostMetadata {
            priority: priority.cloned(),
            ..Default::default()
        };
        crate::priority::Shown::of(&metadata, "", |_| String::new())
    }

    /// The composer's priority button.
    pub(super) fn priority_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = cx.entity().downgrade();
        let choice = self.priority;
        crate::priority::composer_button(
            "composer.priority",
            choice,
            move |at, window, cx| {
                let pick = view.clone();
                let items = crate::priority::menu_items(
                    choice,
                    Features::read(cx),
                    std::rc::Rc::new(move |choice, _, cx| {
                        pick.update(cx, |this, cx| {
                            this.priority = choice;
                            cx.notify();
                        })
                        .ok();
                    }),
                );
                view.update(cx, |this, cx| this.open_menu(items, at, window, cx))
                    .ok();
            },
            cx,
        )
    }

    /// The chosen priority above the composer, with its remove button.
    pub(super) fn priority_tag(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = cx.entity().downgrade();
        crate::priority::choice_tag(
            "composer.priority.chosen",
            self.priority,
            move |_, cx| {
                view.update(cx, |this, cx| {
                    this.priority = Choice::default();
                    cx.notify();
                })
                .ok();
            },
            cx,
        )
    }

    /// "Schedule message" beside Send: the presets and "Custom time…".
    pub(super) fn schedule_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = cx.entity().downgrade();
        crate::scheduled::composer_button(
            "composer.schedule",
            self.can_send() && self.draft_len > 0,
            move |at, window, cx| {
                let pick = view.clone();
                let items = crate::scheduled::menu_items(
                    move |time, window, cx| {
                        pick.update(cx, |this, cx| match time {
                            Some(time) => this.schedule(time, window, cx),
                            None => this
                                .schedule_dialog
                                .update(cx, |dialog, cx| dialog.open(None, None, window, cx)),
                        })
                        .ok();
                    },
                    cx,
                );
                view.update(cx, |this, cx| this.open_menu(items, at, window, cx))
                    .ok();
            },
            cx,
        )
    }

    /// "2 scheduled messages in this channel · See all" above the composer.
    pub(super) fn scheduled_hint(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let channel_id = self.model.as_ref()?.id.clone();
        let view = cx.entity().downgrade();
        crate::scheduled::composer_hint(
            "composer.scheduled.all",
            &channel_id,
            move |_, cx| {
                view.update(cx, |_, cx| cx.emit(ChannelEvent::OpenScheduled))
                    .ok();
            },
            cx,
        )
    }

    /// Acknowledges `post`, or takes the acknowledgement back.
    pub(super) fn acknowledge(
        &mut self,
        post: SharedString,
        acknowledged: bool,
        cx: &mut Context<Self>,
    ) {
        cx.emit(ChannelEvent::Acknowledge { post, acknowledged });
    }

    /// Schedules what the composer holds for `at` (ms) and clears it, as a
    /// send does. Blank text, text over the limit, or files still
    /// uploading stay in the composer.
    pub(super) fn schedule(&mut self, at: i64, window: &mut Window, cx: &mut Context<Self>) {
        if self.refused_offline("schedule message", cx) {
            return;
        }
        let text = self.composer.read(cx).value(cx);
        let body: SharedString = text.trim().to_string().into();
        if body.is_empty() || body.chars().count() > self.max_post_size() {
            return;
        }
        if self.tray.request_send(true) != SendGate::Now {
            cx.notify();
            return;
        }
        let files: Vec<SharedString> = self
            .tray
            .take()
            .into_iter()
            .map(PostFile::from_upload)
            .map(|file| file.id)
            .collect();
        self._draft_task = None;
        if let Some(cleared) = self.draft.sent(cx) {
            cx.emit(ChannelEvent::SaveDraft(cleared));
        }
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.draft_len = 0;
        let priority = self.take_priority(cx);
        cx.emit(ChannelEvent::Schedule {
            text: body,
            files,
            at,
            priority,
        });
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::priority::Level;
    use std::{cell::RefCell, rc::Rc};

    fn channel_view(
        features: Features,
        cx: &mut gpui_kit::TestAppContext,
    ) -> (
        Entity<ChannelView>,
        &mut gpui_kit::VisualTestContext,
        Rc<RefCell<Vec<ChannelEvent>>>,
    ) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
            cx.set_global(features);
        });
        let general = crate::preview::channel("general").unwrap();
        let (view, cx) = cx.add_window_view(|window, cx| {
            ChannelView::new(Some(general.clone()), "me", window, cx)
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        cx.update(|_, cx| {
            let events = events.clone();
            cx.subscribe(&view, move |_, event: &ChannelEvent, _| match event {
                ChannelEvent::Send { .. }
                | ChannelEvent::Schedule { .. }
                | ChannelEvent::Acknowledge { .. } => events.borrow_mut().push(event.clone()),
                _ => {}
            })
            .detach();
        });
        (view, cx, events)
    }

    fn type_and(
        view: &Entity<ChannelView>,
        cx: &mut gpui_kit::VisualTestContext,
        text: &str,
        then: impl FnOnce(&mut ChannelView, &mut Window, &mut Context<ChannelView>),
    ) {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.composer
                    .update(cx, |composer, cx| composer.set_value(text, window, cx));
                view.draft_len = text.len();
                then(view, window, cx);
            })
        });
        cx.run_until_parked();
    }

    /// An urgent post asking for acknowledgements goes out with its
    /// priority, and the composer is back to Standard after.
    #[gpui_kit::test]
    fn an_urgent_post_asking_for_acknowledgement_is_sent_with_its_priority(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (view, cx, events) = channel_view(Features::all(), cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                assert!(view.priority_button(cx).is_some());
                view.priority = Choice {
                    level: Level::Urgent,
                    ack: true,
                    persistent: false,
                };
                assert!(view.priority_tag(cx).is_some(), "the choice shows");
            })
        });
        type_and(&view, cx, "the build is red", |view, window, cx| {
            let text = view.composer.read(cx).value(cx);
            view.send(text, window, cx);
        });
        let sent = events.borrow().clone();
        let [ChannelEvent::Send { priority, .. }] = sent.as_slice() else {
            panic!("one send, got {sent:?}");
        };
        let priority = priority.clone().expect("sent with a priority");
        assert_eq!(priority.priority, matty_api::models::PostPriority::URGENT);
        assert!(priority.requested_ack);
        assert!(!priority.persistent_notifications);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(view.priority, Choice::default());
                assert!(view.priority_tag(cx).is_none());
            })
        });
    }

    /// Pressing Acknowledge asks sync to acknowledge that post.
    #[gpui_kit::test]
    fn acknowledging_a_post_asks_sync(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx, events) = channel_view(Features::all(), cx);
        let shown = crate::priority::Shown {
            level: Level::Urgent,
            requested_ack: true,
            ..Default::default()
        };
        cx.update(|_, cx| {
            let bar = crate::priority::ack_bar("p1", Some(&shown), true, Rc::new(|_, _, _| {}), cx);
            assert!(bar.is_some(), "the Acknowledge bar shows");
            view.update(cx, |view, cx| view.acknowledge("p1".into(), true, cx));
        });
        assert_eq!(
            events.borrow().as_slice(),
            [ChannelEvent::Acknowledge {
                post: "p1".into(),
                acknowledged: true
            }]
        );
    }

    /// Scheduling clears the composer and asks sync to schedule; deleting
    /// it from the Scheduled tab cancels it.
    #[gpui_kit::test]
    fn a_scheduled_message_can_be_cancelled(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx, events) = channel_view(Features::all(), cx);
        let at = 1_793_869_200_000;
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.priority = Choice {
                    level: Level::Important,
                    ..Default::default()
                };
                assert!(view.schedule_button(cx).is_some());
            })
        });
        type_and(&view, cx, "standup notes", |view, window, cx| {
            view.schedule(at, window, cx);
        });
        let scheduled = events.borrow().clone();
        let [
            ChannelEvent::Schedule {
                text,
                at: when,
                priority,
                ..
            },
        ] = scheduled.as_slice()
        else {
            panic!("one schedule, got {scheduled:?}");
        };
        assert_eq!(text.as_ref(), "standup notes");
        assert_eq!(*when, at);
        assert_eq!(
            priority.as_ref().map(|p| p.priority.as_str()),
            Some(matty_api::models::PostPriority::IMPORTANT)
        );
        cx.update(|_, cx| assert_eq!(view.read(cx).composer.read(cx).value(cx).as_ref(), ""));

        // The Scheduled tab lists it; Delete takes it away and cancels it.
        let post = matty_api::models::ScheduledPost {
            id: "s1".into(),
            channel_id: "general".into(),
            message: text.to_string(),
            scheduled_at: at,
            ..Default::default()
        };
        let list =
            cx.update(|window, cx| cx.new(|cx| crate::scheduled::ScheduledView::new(window, cx)));
        let deleted = Rc::new(RefCell::new(Vec::new()));
        cx.update(|_, cx| {
            let deleted = deleted.clone();
            cx.subscribe(
                &list,
                move |_, event: &crate::scheduled::ScheduledEvent, _| {
                    deleted.borrow_mut().push(event.clone())
                },
            )
            .detach();
            list.update(cx, |list, cx| {
                list.set_rows(
                    vec![crate::scheduled::Row {
                        post,
                        place: "#general".into(),
                    }],
                    cx,
                );
                list.delete("s1", cx);
                assert!(list.rows().is_empty());
            });
        });
        assert_eq!(
            deleted.borrow().as_slice(),
            [crate::scheduled::ScheduledEvent::Delete("s1".into())]
        );
    }

    /// With the server's gates off, none of it shows and nothing is sent.
    #[gpui_kit::test]
    fn the_gates_hide_priority_acknowledgements_and_scheduling(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx, events) = channel_view(Features::default(), cx);
        let shown = crate::priority::Shown {
            level: Level::Urgent,
            requested_ack: true,
            ..Default::default()
        };
        cx.update(|_, cx| {
            cx.set_global(crate::scheduled::Scheduled(std::sync::Arc::from([
                matty_api::models::ScheduledPost {
                    id: "s1".into(),
                    channel_id: "general".into(),
                    ..Default::default()
                },
            ])));
            assert!(crate::priority::label(Some(&shown), cx).is_none());
            assert!(
                crate::priority::ack_bar("p1", Some(&shown), true, Rc::new(|_, _, _| {}), cx)
                    .is_none()
            );
            view.update(cx, |view, cx| {
                assert!(view.priority_button(cx).is_none());
                assert!(view.scheduled_hint(cx).is_none());
                // A choice left over from before the gate closed is dropped.
                view.priority = Choice {
                    level: Level::Urgent,
                    ack: true,
                    persistent: true,
                };
            });
        });
        type_and(&view, cx, "hello", |view, window, cx| {
            assert!(view.schedule_button(cx).is_none());
            let text = view.composer.read(cx).value(cx);
            view.send(text, window, cx);
        });
        assert!(matches!(
            events.borrow().as_slice(),
            [ChannelEvent::Send { priority: None, .. }]
        ));
    }
}
