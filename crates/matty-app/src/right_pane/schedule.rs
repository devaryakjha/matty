//! "Schedule message" in the reply composer (#220), as the channel
//! composer has it; see [`crate::scheduled`]. A scheduled reply goes to the
//! thread's channel with its root, so it posts in the thread.

use gpui_kit::{AnyElement, AppContext as _, Context, Entity, SharedString, Window};

use super::{RightPaneContent, RightPaneEvent, RightPaneView};
use crate::attachments::{PostFile, SendGate};
use crate::scheduled::{ScheduleDialog, ScheduleDialogEvent};

/// The schedule dialog for the reply composer, already wired to it.
pub(super) fn schedule_dialog(
    window: &mut Window,
    cx: &mut Context<RightPaneView>,
) -> (Entity<ScheduleDialog>, gpui_kit::Subscription) {
    let dialog = cx.new(|cx| ScheduleDialog::new(window, cx));
    let subscription = cx.subscribe_in(
        &dialog,
        window,
        |this: &mut RightPaneView, _, event: &ScheduleDialogEvent, window, cx| {
            let ScheduleDialogEvent::Confirm { at, .. } = event;
            this.schedule(*at, window, cx);
        },
    );
    (dialog, subscription)
}

impl RightPaneView {
    /// "Schedule message" beside Reply: the presets and "Custom time…".
    /// `None` while the server does not allow scheduling.
    pub(super) fn schedule_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = cx.entity().downgrade();
        crate::scheduled::composer_button(
            "right-pane.schedule",
            self.draft_len > 0 && self.tray.gate(true) == SendGate::Now,
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

    /// Schedules the reply the composer holds for `at` (ms) and clears it,
    /// as replying does. Blank text or files still uploading stay in the
    /// composer.
    pub(super) fn schedule(&mut self, at: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(RightPaneContent::Thread(thread)) = self.content.as_ref() else {
            return;
        };
        let (channel, root) = (thread.channel_id.clone(), thread.root.id.clone());
        if self.refused_offline("schedule message", cx) {
            return;
        }
        let text = self.composer.read(cx).value(cx);
        let body: SharedString = text.trim().to_string().into();
        if body.is_empty() {
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
            cx.emit(RightPaneEvent::SaveDraft(cleared));
        }
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.draft_len = 0;
        self.link.sent(&body);
        self._link_task = None;
        cx.emit(RightPaneEvent::Schedule {
            channel,
            root,
            text: body,
            files,
            at,
        });
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::priority::Features;
    use std::{cell::RefCell, rc::Rc};

    fn thread_pane(
        features: Features,
        cx: &mut gpui_kit::TestAppContext,
    ) -> (
        Entity<RightPaneView>,
        &mut gpui_kit::VisualTestContext,
        Rc<RefCell<Vec<RightPaneEvent>>>,
    ) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
            cx.set_global(features);
        });
        let (view, cx) = cx.add_window_view(|window, cx| RightPaneView::new("me", window, cx));
        let posts = crate::preview::channel("general").unwrap().posts;
        let thread = crate::preview::thread("general", &posts[0].id).expect("a fixture thread");
        cx.update(|_, cx| {
            view.update(cx, |pane, cx| {
                pane.set_content(Some(RightPaneContent::Thread(Box::new(thread))), cx)
            })
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        cx.update(|_, cx| {
            let events = events.clone();
            cx.subscribe(&view, move |_, event: &RightPaneEvent, _| {
                if matches!(event, RightPaneEvent::Schedule { .. }) {
                    events.borrow_mut().push(event.clone());
                }
            })
            .detach();
        });
        cx.run_until_parked();
        (view, cx, events)
    }

    fn type_reply(
        view: &Entity<RightPaneView>,
        cx: &mut gpui_kit::VisualTestContext,
        text: &str,
    ) {
        cx.update(|window, cx| {
            view.update(cx, |pane, cx| {
                pane.composer
                    .update(cx, |composer, cx| composer.set_value(text, window, cx));
                pane.draft_len = text.chars().count();
            })
        });
        cx.run_until_parked();
    }

    /// Scheduling a reply clears the composer and asks for it in the
    /// thread's channel under its root.
    #[gpui_kit::test]
    fn a_reply_is_scheduled_under_its_root(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx, events) = thread_pane(Features::all(), cx);
        type_reply(&view, cx, "  after lunch  ");
        let at = 1_793_869_200_000;
        cx.update(|window, cx| {
            view.update(cx, |pane, cx| {
                assert!(pane.schedule_button(cx).is_some(), "the button shows");
                pane.schedule(at, window, cx);
            })
        });
        let (channel, root) = cx.update(|_, cx| {
            let Some(RightPaneContent::Thread(thread)) = view.read(cx).content.clone() else {
                panic!("the thread stays open");
            };
            (thread.channel_id, thread.root.id)
        });
        assert_eq!(
            events.borrow().as_slice(),
            [RightPaneEvent::Schedule {
                channel,
                root,
                text: "after lunch".into(),
                files: vec![],
                at,
            }]
        );
        cx.update(|_, cx| {
            let pane = view.read(cx);
            assert_eq!(pane.composer.read(cx).value(cx).as_ref(), "");
            assert_eq!(pane.draft_len, 0);
        });
    }

    /// A blank composer schedules nothing.
    #[gpui_kit::test]
    fn a_blank_reply_is_not_scheduled(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx, events) = thread_pane(Features::all(), cx);
        type_reply(&view, cx, "   ");
        cx.update(|window, cx| view.update(cx, |pane, cx| pane.schedule(1, window, cx)));
        assert!(events.borrow().is_empty());
    }

    /// With scheduling off on the server, the reply composer has no button.
    #[gpui_kit::test]
    fn the_gate_hides_scheduling_in_threads(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx, _) = thread_pane(Features::default(), cx);
        type_reply(&view, cx, "hello");
        cx.update(|_, cx| view.update(cx, |pane, cx| assert!(pane.schedule_button(cx).is_none())));
    }
}
