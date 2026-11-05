//! Message priority, acknowledgements, persistent notifications and
//! scheduled messages in the main window (#220).

use gpui_kit::{Context, SharedString};

use super::MainWindow;

impl MainWindow {
    /// The user acknowledged a post, or took it back; sync shows it.
    pub(super) fn acknowledge(
        &mut self,
        post: &SharedString,
        acknowledged: bool,
        cx: &mut Context<Self>,
    ) {
        let action = if acknowledged {
            "acknowledge"
        } else {
            "remove acknowledgement"
        };
        if self.refused_offline(action, cx) {
            return;
        }
        if acknowledged {
            self.persistent.stop(post.as_ref());
        }
        if let Some(live) = &self.live {
            live.send(matty_sync::Command::Acknowledge {
                post_id: post.to_string(),
                acknowledged,
            });
        }
    }
}

/// How often repeating notifications are checked.
const PERSISTENT_TICK: std::time::Duration = std::time::Duration::from_secs(20);

impl MainWindow {
    /// An urgent post that asks for persistent notifications just
    /// notified the user about a mention: it repeats at the server's
    /// interval until they read or acknowledge it (#220).
    pub(super) fn repeat_if_persistent(
        &mut self,
        arrival: &crate::live::Arrival,
        notification: matty_notify::Notification,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        let features = crate::priority::Features::read(cx);
        if !arrival.persistent || !notification.mention || !features.persistent {
            return;
        }
        self.persistent.start(
            notification,
            jiff::Timestamp::now().as_millisecond(),
            features.interval_minutes,
            features.max_count,
        );
        if self._persistent_task.is_none() {
            self._persistent_task = Some(cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(PERSISTENT_TICK).await;
                    let more = this
                        .update_in(cx, |this, window, cx| this.repeat_due(window, cx))
                        .unwrap_or(false);
                    if !more {
                        break;
                    }
                }
            }));
        }
    }

    /// Shows the repeats that are due, dropping the posts the user has read
    /// meanwhile: their channel is on screen, or no longer unread. `false`
    /// once nothing repeats.
    fn repeat_due(&mut self, window: &mut gpui_kit::Window, cx: &mut Context<Self>) -> bool {
        let shown = (window.is_window_active() && !self.threads_open && !self.drafts_open)
            .then(|| self.selected_channel.clone())
            .flatten();
        if let Some(channel) = shown {
            self.persistent.stop_channel(&channel);
        }
        let due = self.persistent.due(jiff::Timestamp::now().as_millisecond());
        for notification in due {
            let sidebar = self.sidebar.read(cx);
            let read = sidebar.has_row(&notification.channel_id)
                && !sidebar.is_unread(&notification.channel_id);
            if read {
                self.persistent.stop_channel(&notification.channel_id);
            } else {
                self.notifier.show(&notification);
            }
        }
        if self.persistent.is_empty() {
            self._persistent_task = None;
            return false;
        }
        true
    }
}

impl MainWindow {
    /// Gives the Drafts view its Scheduled tab and listens to it.
    pub(super) fn add_scheduled_tab(
        drafts: &gpui_kit::Entity<crate::drafts::DraftsView>,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::AppContext as _;
        let view = cx.new(|cx| crate::scheduled::ScheduledView::new(window, cx));
        if crate::preview::enabled() {
            view.update(cx, |view, cx| {
                view.set_rows(crate::preview::scheduled_rows(), cx)
            });
        }
        cx.subscribe_in(&view, window, Self::scheduled_event)
            .detach();
        drafts.update(cx, |drafts, cx| drafts.set_scheduled(view, cx));
    }

    fn scheduled_event(
        &mut self,
        _: &gpui_kit::Entity<crate::scheduled::ScheduledView>,
        event: &crate::scheduled::ScheduledEvent,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        use crate::scheduled::ScheduledEvent;
        let (action, command) = match event {
            ScheduledEvent::Open(post) => {
                self.open_channel(post.channel_id.clone().into(), window, cx);
                if !post.root_id.is_empty() {
                    self.open_thread(post.root_id.clone().into(), cx);
                }
                return;
            }
            ScheduledEvent::Save(post) => (
                "change scheduled message",
                matty_sync::Command::Schedule(Box::new(post.clone())),
            ),
            ScheduledEvent::SendNow(id) => (
                "send",
                matty_sync::Command::SendScheduledNow { id: id.clone() },
            ),
            ScheduledEvent::Delete(id) => (
                "delete scheduled message",
                matty_sync::Command::DeleteScheduled { id: id.clone() },
            ),
        };
        if action != "send" && self.refused_offline(action, cx) {
            return;
        }
        if let Some(live) = &self.live {
            live.send(command);
        }
    }

    /// The composer scheduled a message (#220).
    pub(super) fn schedule_message(
        &mut self,
        channel_id: String,
        root_id: String,
        (text, files, at): (&SharedString, &[SharedString], i64),
        priority: Option<matty_api::models::PostPriority>,
        cx: &mut Context<Self>,
    ) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        let (file_ids, _) = live.take_files(files);
        live.send(matty_sync::Command::Schedule(Box::new(
            matty_api::models::ScheduledPost {
                channel_id,
                root_id,
                message: text.to_string(),
                file_ids,
                scheduled_at: at,
                metadata: matty_api::models::ScheduledMetadata { priority },
                ..Default::default()
            },
        )));
        let when = crate::scheduled::when_label(
            at,
            &jiff::Zoned::now(),
            crate::settings::ClockFormat::current(cx),
        );
        self.toasts.update(cx, |toasts, cx| {
            toasts.push(
                "scheduled",
                crate::ui::toast::Notice::new(format!(
                    "Message scheduled for {}",
                    when.to_lowercase()
                ))
                .timeout(std::time::Duration::from_secs(4)),
                cx,
            );
        });
    }

    /// "See all" above a composer: the Scheduled tab of Drafts.
    pub(super) fn show_scheduled(&mut self, cx: &mut Context<Self>) {
        self.show_drafts(cx);
        self.drafts
            .update(cx, |drafts, cx| drafts.show_scheduled(true, cx));
    }

    /// Sync's scheduled messages changed: every composer's count and the
    /// Scheduled tab follow.
    pub(super) fn scheduled_changed(
        &mut self,
        posts: std::sync::Arc<[matty_api::models::ScheduledPost]>,
        cx: &mut Context<Self>,
    ) {
        crate::scheduled::Scheduled::set(posts, cx);
        self.refresh_scheduled(cx);
    }

    /// The Scheduled tab's list: the shown team's and conversations', by
    /// where they post.
    pub(super) fn refresh_scheduled(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.drafts.read(cx).scheduled().cloned() else {
            return;
        };
        // The preview keeps its fixture list.
        let Some(live) = &self.live else {
            return;
        };
        let rows = crate::scheduled::Scheduled::read(cx)
            .iter()
            .filter_map(|post| {
                let row = live.row(&post.channel_id)?;
                let channel = match row.kind {
                    crate::sidebar::ChannelKind::Public | crate::sidebar::ChannelKind::Private => {
                        format!("#{}", row.name)
                    }
                    _ => row.name.to_string(),
                };
                Some(crate::scheduled::Row {
                    post: post.clone(),
                    place: if post.root_id.is_empty() {
                        channel.into()
                    } else {
                        format!("Reply in {channel}").into()
                    },
                })
            })
            .collect();
        view.update(cx, |view, cx| view.set_rows(rows, cx));
    }
}
