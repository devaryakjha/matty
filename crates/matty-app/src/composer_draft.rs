//! What each composer left unsent (#114): the text and tray files per
//! channel and per thread, kept while the user is elsewhere, saved to sync
//! after a pause in typing, and put back when they return or relaunch.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::{App, Entity, Global, SharedString, Window};

use crate::attachments::{AttachmentStatus, Upload, UploadTray};
use crate::ui::composer::ComposerState;

/// How long typing pauses before the draft is saved.
pub const SAVE_PAUSE: Duration = Duration::from_millis(500);

/// Where a draft belongs: a channel, or a thread in it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DraftKey {
    pub channel_id: SharedString,
    /// The thread's root post, or empty for the channel's own composer.
    pub root_id: SharedString,
}

impl DraftKey {
    pub fn channel(channel_id: impl Into<SharedString>) -> Self {
        Self {
            channel_id: channel_id.into(),
            root_id: SharedString::default(),
        }
    }

    pub fn thread(channel_id: impl Into<SharedString>, root_id: impl Into<SharedString>) -> Self {
        Self {
            channel_id: channel_id.into(),
            root_id: root_id.into(),
        }
    }

    pub fn is_thread(&self) -> bool {
        !self.root_id.is_empty()
    }
}

/// A composer's unsent text and files in one place.
#[derive(Clone, Default)]
pub struct Kept {
    pub text: SharedString,
    /// The tray as it was, uploads still running included.
    pub uploads: Vec<Upload>,
    /// Milliseconds since the epoch of the last edit.
    pub update_at: i64,
}

impl Kept {
    fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.uploads.is_empty()
    }

    /// Tray ids of the files already on the server.
    fn ready(&self) -> Vec<SharedString> {
        ready(&self.uploads)
    }
}

fn ready(uploads: &[Upload]) -> Vec<SharedString> {
    uploads
        .iter()
        .filter(|upload| upload.is_ready())
        .map(|upload| upload.id.clone())
        .collect()
}

/// A draft for sync to store: empty text and no files delete it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftSave {
    pub key: DraftKey,
    pub text: SharedString,
    /// Tray ids of the uploaded files; the window maps them to file ids.
    pub files: Vec<SharedString>,
}

/// Every draft this run knows, shared by the composers, the sidebar and the
/// Drafts view.
#[derive(Default)]
pub struct DraftBook {
    kept: HashMap<DraftKey, Kept>,
}

impl Global for DraftBook {}

impl DraftBook {
    pub fn read(cx: &App) -> Option<&Self> {
        cx.try_global::<Self>()
    }

    pub fn get(cx: &App, key: &DraftKey) -> Option<Kept> {
        Self::read(cx).and_then(|book| book.kept.get(key).cloned())
    }

    /// Keeps `kept` for `key`; an empty one removes it.
    pub fn put(key: DraftKey, kept: Kept, cx: &mut App) {
        let book = cx.default_global::<Self>();
        if kept.is_empty() {
            book.kept.remove(&key);
        } else {
            book.kept.insert(key, kept);
        }
    }

    /// Every draft, newest first.
    pub fn all(cx: &App) -> Vec<(DraftKey, Kept)> {
        let mut all: Vec<(DraftKey, Kept)> = Self::read(cx)
            .map(|book| {
                book.kept
                    .iter()
                    .map(|(key, kept)| (key.clone(), kept.clone()))
                    .collect()
            })
            .unwrap_or_default();
        all.sort_by(|a, b| {
            b.1.update_at
                .cmp(&a.1.update_at)
                .then_with(|| a.0.channel_id.cmp(&b.0.channel_id))
        });
        all
    }

    /// Channels whose own composer has a draft, for the sidebar's pencil.
    pub fn channels(cx: &App) -> std::collections::HashSet<SharedString> {
        Self::read(cx)
            .map(|book| {
                book.kept
                    .keys()
                    .filter(|key| !key.is_thread())
                    .map(|key| key.channel_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Records an upload's progress for a draft that is not on screen.
    /// Returns the draft to save again when its files changed on the server.
    pub fn set_upload_status(
        id: &str,
        status: AttachmentStatus,
        cx: &mut App,
    ) -> Option<DraftSave> {
        let book = cx.default_global::<Self>();
        let (key, kept) = book
            .kept
            .iter_mut()
            .find(|(_, kept)| kept.uploads.iter().any(|upload| upload.id == id))?;
        let upload = kept.uploads.iter_mut().find(|upload| upload.id == id)?;
        upload.status = status;
        (status == AttachmentStatus::Ready).then(|| DraftSave {
            key: key.clone(),
            text: kept.text.clone(),
            files: kept.ready(),
        })
    }
}

/// One composer's link to the book: which draft it shows, and what was
/// last saved of it, so saves only go when something changed and a change
/// from elsewhere only lands while the user has nothing unsaved.
#[derive(Default)]
pub struct ComposerDraft {
    key: Option<DraftKey>,
    saved_text: SharedString,
    saved_files: Vec<SharedString>,
}

impl ComposerDraft {
    pub fn key(&self) -> Option<&DraftKey> {
        self.key.as_ref()
    }

    /// Whether `text` is what was last saved or restored, so a change
    /// event for it is not the user typing.
    pub fn is_saved(&self, text: &str) -> bool {
        self.saved_text.as_ref() == text
    }

    /// Whether switching to `key` leaves the composer's text as it is (empty
    /// before and after), so the switch needs no window.
    pub fn switches_quietly(
        &self,
        key: Option<&DraftKey>,
        composer: &Entity<ComposerState>,
        cx: &App,
    ) -> bool {
        composer.read(cx).value(cx).is_empty()
            && key
                .and_then(|key| DraftBook::get(cx, key))
                .is_none_or(|kept| kept.text.is_empty())
    }

    /// Points the composer at `key`: what it held goes into the book (and
    /// is returned to save when it changed) and `key`'s draft comes out.
    /// `window` may be left out only when [`Self::switches_quietly`] says
    /// so, since only the text needs it.
    pub fn switch(
        &mut self,
        key: Option<DraftKey>,
        composer: &Entity<ComposerState>,
        tray: &mut UploadTray,
        window: Option<&mut Window>,
        cx: &mut App,
    ) -> Option<DraftSave> {
        if key == self.key {
            return None;
        }
        let left = self.save(composer, tray, cx);
        if let Some(old) = self.key.take() {
            let kept = Kept {
                text: composer.read(cx).value(cx),
                uploads: tray.take(),
                update_at: DraftBook::get(cx, &old).map_or_else(now, |kept| kept.update_at),
            };
            DraftBook::put(old, kept, cx);
        } else {
            tray.take();
        }
        let kept = key
            .as_ref()
            .and_then(|key| DraftBook::get(cx, key))
            .unwrap_or_default();
        self.key = key;
        self.show(kept, composer, tray, window, cx);
        left
    }

    /// The draft as the composer holds it now, when it differs from the
    /// last save; the book follows.
    pub fn save(
        &mut self,
        composer: &Entity<ComposerState>,
        tray: &UploadTray,
        cx: &mut App,
    ) -> Option<DraftSave> {
        let key = self.key.clone()?;
        let text = composer.read(cx).value(cx);
        let files = ready(tray.uploads());
        if text == self.saved_text && files == self.saved_files {
            return None;
        }
        let kept = Kept {
            text: text.clone(),
            uploads: tray.uploads().to_vec(),
            update_at: now(),
        };
        DraftBook::put(key.clone(), kept, cx);
        self.saved_text = text.clone();
        self.saved_files = files.clone();
        let text = if text.trim().is_empty() {
            SharedString::default()
        } else {
            text
        };
        Some(DraftSave { key, text, files })
    }

    /// The message went: the draft is gone here and in sync.
    pub fn sent(&mut self, cx: &mut App) -> Option<DraftSave> {
        let key = self.key.clone()?;
        DraftBook::put(key.clone(), Kept::default(), cx);
        self.saved_text = SharedString::default();
        self.saved_files.clear();
        Some(DraftSave {
            key,
            text: SharedString::default(),
            files: Vec::new(),
        })
    }

    /// The book's draft for the shown key changed elsewhere (another
    /// device, the Drafts view): it replaces the composer unless the user
    /// has typed since the last save. True when it did.
    pub fn refresh(
        &mut self,
        composer: &Entity<ComposerState>,
        tray: &mut UploadTray,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let Some(key) = &self.key else {
            return false;
        };
        let text = composer.read(cx).value(cx);
        if text != self.saved_text || ready(tray.uploads()) != self.saved_files {
            return false;
        }
        let kept = DraftBook::get(cx, key).unwrap_or_default();
        if kept.text == text && ready(&kept.uploads) == self.saved_files {
            return false;
        }
        tray.take();
        self.show(kept, composer, tray, Some(window), cx);
        true
    }

    fn show(
        &mut self,
        kept: Kept,
        composer: &Entity<ComposerState>,
        tray: &mut UploadTray,
        window: Option<&mut Window>,
        cx: &mut App,
    ) {
        self.saved_files = kept.ready();
        self.saved_text = kept.text.clone();
        tray.extend(kept.uploads);
        // The caret lands at the end of the text. Without a window the text
        // is already right (see `switches_quietly`).
        if let Some(window) = window {
            composer.update(cx, |composer, cx| composer.set_value(kept.text, window, cx));
        }
    }
}

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}
