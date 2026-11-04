//! Drafts between the composers, sync, the sidebar and the Drafts view
//! (#114): composers save into the [`DraftBook`] and through sync; what
//! sync stores (this device, another one, the server's list) comes back
//! into the book and the composer showing it.

use std::sync::Arc;

use gpui_kit::{Context, SharedString, Window};

use super::MainWindow;
use crate::composer_draft::{DraftBook, DraftKey, DraftSave, Kept};
use crate::drafts::{Draft, DraftsModel};

impl MainWindow {
    /// A composer's draft changed: sync stores it and the sidebar follows.
    pub(super) fn save_draft(&mut self, save: &DraftSave, cx: &mut Context<Self>) {
        if let Some(live) = &self.live {
            live.send(matty_sync::Command::SaveDraft {
                channel_id: save.key.channel_id.to_string(),
                root_id: save.key.root_id.to_string(),
                message: save.text.to_string(),
                file_ids: live.file_ids(&save.files),
            });
        }
        self.refresh_drafts(cx);
    }

    /// Sync's copy of a draft changed: the book takes it unless it already
    /// holds the same, and a composer showing it follows.
    pub(super) fn draft_stored(
        &mut self,
        key: DraftKey,
        draft: Option<Arc<matty_api::models::Draft>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        let current = DraftBook::get(cx, &key);
        let kept = match draft {
            None if current.is_none() => return,
            None => Kept::default(),
            Some(draft) => {
                let current = current.unwrap_or_default();
                let tray: Vec<SharedString> = current
                    .uploads
                    .iter()
                    .map(|upload| upload.id.clone())
                    .collect();
                let file_ids = live.file_ids(&tray);
                if current.text.as_ref() == draft.message && file_ids == draft.file_ids {
                    return;
                }
                // Files already in the tray keep their chips; the rest come
                // as finished uploads.
                let uploads = draft
                    .file_ids
                    .iter()
                    .map(|file_id| {
                        current
                            .uploads
                            .iter()
                            .find(|upload| {
                                live.file_ids(std::slice::from_ref(&upload.id))
                                    .first()
                                    .is_some_and(|id| id == file_id)
                            })
                            .cloned()
                            .unwrap_or_else(|| live.draft_upload(&draft.channel_id, file_id))
                    })
                    .collect();
                Kept {
                    text: draft.message.clone().into(),
                    uploads,
                    update_at: draft.update_at,
                }
            }
        };
        DraftBook::put(key.clone(), kept, cx);
        if key.is_thread() {
            let detail = self.threads.read(cx).detail().clone();
            for pane in [self.right_pane.clone(), detail] {
                pane.update(cx, |pane, cx| pane.refresh_draft(window, cx));
            }
        } else {
            self.channel
                .update(cx, |view, cx| view.refresh_draft(window, cx));
        }
        self.refresh_drafts(cx);
    }

    /// A draft the Drafts view deleted or sent: gone from the book, sync
    /// and any composer showing it.
    pub(super) fn drop_draft(
        &mut self,
        draft: &Draft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = match &draft.thread_root {
            Some(root) => DraftKey::thread(draft.channel_id.clone(), root.clone()),
            None => DraftKey::channel(draft.channel_id.clone()),
        };
        self.draft_stored(key.clone(), None, window, cx);
        self.save_draft(
            &DraftSave {
                key,
                text: SharedString::default(),
                files: Vec::new(),
            },
            cx,
        );
    }

    /// The Drafts view's Send: the draft goes as a post, then away.
    pub(super) fn send_draft(
        &mut self,
        draft: &Draft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = match &draft.thread_root {
            Some(root) => DraftKey::thread(draft.channel_id.clone(), root.clone()),
            None => DraftKey::channel(draft.channel_id.clone()),
        };
        let files: Vec<SharedString> = DraftBook::get(cx, &key)
            .map(|kept| {
                kept.uploads
                    .iter()
                    .map(|upload| upload.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        if let Some(live) = self.live.as_mut() {
            let file_ids = live.take_file_ids(&files);
            live.send(matty_sync::Command::SendPost {
                pending_post_id: String::new(),
                channel_id: draft.channel_id.to_string(),
                root_id: key.root_id.to_string(),
                message: draft.body.to_string(),
                file_ids,
                props: crate::embeds::send_props(true),
            });
        }
        self.drop_draft(draft, window, cx);
    }

    /// The sidebar's pencils and Drafts count, and the Drafts view's list,
    /// from the book.
    pub(super) fn refresh_drafts(&mut self, cx: &mut Context<Self>) {
        let all = DraftBook::all(cx);
        let drafted = DraftBook::channels(cx);
        let count = all.len();
        self.sidebar.update(cx, |sidebar, cx| {
            let model = sidebar.model();
            if model.drafted != drafted || model.drafts != count {
                let mut model = model.clone();
                model.drafted = drafted;
                model.drafts = count;
                sidebar.set_model(model, cx);
            }
        });
        // Preview keeps its fixture list.
        let Some(live) = &self.live else {
            return;
        };
        let clock = crate::settings::ClockFormat::current(cx);
        let zone = jiff::tz::TimeZone::system();
        let today = jiff::Zoned::now().date();
        let drafts = all
            .into_iter()
            .filter_map(|(key, kept)| {
                let channel_name = live.row(&key.channel_id)?.name;
                let (minutes, day, _) = crate::transcript::when(kept.update_at, &zone, today);
                let thread_root = key.is_thread().then_some(key.root_id);
                Some(Draft {
                    channel_id: key.channel_id,
                    channel_name,
                    thread_root,
                    body: kept.text,
                    updated: format!("{day} {}", clock.format(minutes)).into(),
                })
            })
            .collect();
        let model = DraftsModel { drafts };
        self.drafts.update(cx, |view, cx| {
            if view.model() != &model {
                view.set_model(model, cx);
            }
        });
    }
}
