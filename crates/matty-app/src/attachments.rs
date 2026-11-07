//! Files on their way into a post: what the composer tray shows for each
//! upload and the file facts shared with sync (PLAN §4.3 Writing, §4.6
//! Files).

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    App, ElementId, ExternalPaths, Image, ImageFormat, ImageSource, InteractiveElement as _,
    IntoElement as _, ParentElement as _, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div,
};

use crate::style::{self, TypeStyleExt as _, scaled};
use crate::t;
pub use crate::ui::attachment::AttachmentStatus;
use crate::ui::{
    attachment::{Attachment, attachment_group},
    spinner::Spinner,
    theme::UiTheme,
};

/// The name a pasted image gets; Mattermost uses the same.
pub const PASTED_IMAGE: &str = "Pasted image.png"; // i18n-ignore: a file name

/// Where an upload's bytes come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadSource {
    /// A file the user picked or dropped.
    Path(PathBuf),
    /// Image bytes from the clipboard.
    Image(Arc<Image>),
}

/// One file in the composer tray.
#[derive(Clone)]
pub struct Upload {
    pub id: SharedString,
    pub name: SharedString,
    pub size: u64,
    pub source: UploadSource,
    pub status: AttachmentStatus,
    /// Added while offline: it waits on this computer and uploads when
    /// the connection is back, or with its post from the outbox (#194).
    pub queued: bool,
}

impl Upload {
    /// An upload for a file on disk; the size comes from its metadata.
    pub fn from_path(id: impl Into<SharedString>, path: PathBuf) -> Self {
        let name: SharedString = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| t!("attachments-file").to_string())
            .into();
        let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        Self {
            id: id.into(),
            name,
            size,
            source: UploadSource::Path(path),
            status: AttachmentStatus::Uploading(0.),
            queued: false,
        }
    }

    /// An upload for a pasted image.
    pub fn from_image(id: impl Into<SharedString>, image: Arc<Image>) -> Self {
        let size = image.bytes.len() as u64;
        Self {
            id: id.into(),
            name: PASTED_IMAGE.into(),
            size,
            source: UploadSource::Image(image),
            status: AttachmentStatus::Uploading(0.),
            queued: false,
        }
    }

    /// A finished upload with a known size, for fixtures and received files.
    pub fn ready(mut self, size: u64) -> Self {
        self.size = size;
        self.status = AttachmentStatus::Ready;
        self
    }

    /// Sets the upload state.
    pub fn with_status(mut self, status: AttachmentStatus) -> Self {
        self.status = status;
        self
    }

    /// Marks it queued while offline, for fixtures.
    pub fn queued(mut self) -> Self {
        self.status = AttachmentStatus::Ready;
        self.queued = true;
        self
    }

    /// The image to show as a thumbnail, when the file is one.
    pub fn image(&self) -> Option<ImageSource> {
        thumbnail(&self.name, &self.source)
    }

    /// The chip's second line: "PDF · 2.4 MB", or "Queued · 2.4 MB" while
    /// it waits for the connection.
    pub fn meta(&self) -> SharedString {
        let kind = if self.queued {
            t!("attachments-queued").to_string()
        } else {
            kind(&self.name)
        };
        format!("{kind} · {}", format_size(self.size)).into()
    }

    /// The file-type icon for a card.
    pub fn icon(&self) -> LucideIcon {
        icon_for(&self.name)
    }

    /// Whether the post can go: the file is on the server, or queued to
    /// upload with it.
    pub fn is_ready(&self) -> bool {
        self.status == AttachmentStatus::Ready
    }
    /// Its tray chip: a thumbnail for an image, a card for anything else,
    /// with remove, and Retry once it failed. `prefix` keeps element ids
    /// apart between trays.
    pub fn chip(
        &self,
        prefix: &str,
        on_remove: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
        on_retry: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Attachment {
        let mut chip = Attachment::new(
            ElementId::Name(format!("{prefix}.{}", self.id).into()),
            self.name.clone(),
        )
        .meta(self.meta())
        .icon(self.icon())
        .status(self.status)
        .on_remove(on_remove);
        if let Some(image) = self.image() {
            chip = chip.image(image);
        }
        if self.status == AttachmentStatus::Failed {
            chip = chip.on_retry(on_retry);
        }
        chip
    }
}

/// Mattermost's limit on files per post.
pub const MAX_FILE_COUNT: usize = 10;

/// Mattermost's default `MaxFileSize` (100 MB), until the server says.
pub const DEFAULT_MAX_FILE_SIZE: u64 = 104_857_600;

/// What pressing Send does with the tray as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendGate {
    /// Everything is on the server: send.
    Now,
    /// Files are still uploading: send once they finish.
    Wait,
    /// Nothing to send, or a file failed: stay put.
    Blocked,
}

/// A composer's files on their way into a post (PLAN §4.3, #149): the
/// chips, the size and count rules that keep a file out before it uploads,
/// the line saying why, and a send waiting for uploads to finish.
pub struct UploadTray {
    prefix: &'static str,
    uploads: Vec<Upload>,
    seq: usize,
    max_file_size: u64,
    notice: Option<SharedString>,
    queued: bool,
}

impl UploadTray {
    /// An empty tray; `prefix` keeps its upload ids apart from other trays'.
    pub fn new(prefix: &'static str) -> Self {
        Self {
            prefix,
            uploads: Vec::new(),
            seq: 0,
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            notice: None,
            queued: false,
        }
    }

    /// The files in the tray, in the order they were added.
    pub fn uploads(&self) -> &[Upload] {
        &self.uploads
    }

    pub fn is_empty(&self) -> bool {
        self.uploads.is_empty()
    }

    /// The server's `MaxFileSize`.
    pub fn set_max_file_size(&mut self, bytes: u64) {
        self.max_file_size = bytes;
    }

    /// Why files were left out, or why a waiting send did not go.
    pub fn notice(&self) -> Option<&SharedString> {
        self.notice.as_ref()
    }

    /// Whether a send is waiting for uploads to finish.
    pub fn is_queued(&self) -> bool {
        self.queued
    }

    /// Adds picked, dropped or pasted files, refusing any over the size
    /// limit and any past the tenth. Returns the ones to upload.
    pub fn add(&mut self, sources: impl IntoIterator<Item = UploadSource>) -> Vec<Upload> {
        let files = sources
            .into_iter()
            .map(|source| match source {
                UploadSource::Path(path) => Upload::from_path("", path),
                UploadSource::Image(image) => Upload::from_image("", image),
            })
            .collect();
        self.admit(files)
    }

    /// [`Self::add`] for files whose name and size are known; each
    /// accepted one gets the tray's next id.
    pub fn admit(&mut self, files: Vec<Upload>) -> Vec<Upload> {
        let mut too_big: Vec<&Upload> = Vec::new();
        let mut too_many: Vec<&Upload> = Vec::new();
        let mut accepted = Vec::new();
        for file in &files {
            if file.size > self.max_file_size {
                too_big.push(file);
            } else if self.uploads.len() >= MAX_FILE_COUNT {
                too_many.push(file);
            } else {
                self.seq += 1;
                let mut upload = file.clone();
                upload.id = format!("{}-{}", self.prefix, self.seq).into();
                upload.status = AttachmentStatus::Uploading(0.);
                upload.queued = false;
                self.uploads.push(upload.clone());
                accepted.push(upload);
            }
        }
        let limit = format_size(self.max_file_size);
        let size = match too_big.as_slice() {
            [] => None,
            [file] => Some(t!(
                "attachments-too-big-one",
                name = file.name.to_string(),
                size = format_size(file.size),
                limit = limit.clone()
            )),
            files => Some(t!(
                "attachments-too-big-many",
                count = files.len(),
                limit = limit.clone()
            )),
        };
        let count = match too_many.as_slice() {
            [] => None,
            [file] => Some(t!(
                "attachments-too-many-one",
                name = file.name.to_string(),
                max = MAX_FILE_COUNT
            )),
            files => Some(t!(
                "attachments-too-many-many",
                count = files.len(),
                max = MAX_FILE_COUNT
            )),
        };
        self.notice = match (size, count) {
            (None, None) => None,
            (Some(one), None) | (None, Some(one)) => Some(one),
            (Some(size), Some(count)) => Some(format!("{size} {count}").into()),
        };
        accepted
    }

    /// Records sync's progress for a file. True when a waiting send is
    /// now due.
    pub fn set_status(&mut self, id: &str, status: AttachmentStatus) -> bool {
        let Some(upload) = self.uploads.iter_mut().find(|upload| upload.id == id) else {
            return false;
        };
        upload.status = status;
        upload.queued = false;
        self.settle()
    }

    /// Sync is offline: the file waits here and goes with the post through
    /// the outbox, or uploads when the connection is back (#194). True
    /// when a waiting send is now due.
    pub fn set_queued(&mut self, id: &str) -> bool {
        let Some(upload) = self.uploads.iter_mut().find(|upload| upload.id == id) else {
            return false;
        };
        upload.status = AttachmentStatus::Ready;
        upload.queued = true;
        self.settle()
    }

    /// Whether any file waits for the connection.
    pub fn has_queued(&self) -> bool {
        self.uploads.iter().any(|upload| upload.queued)
    }

    /// Takes a file out of the tray. True when a waiting send is now due.
    pub fn remove(&mut self, id: &str) -> bool {
        self.uploads.retain(|upload| upload.id != id);
        self.notice = None;
        self.settle()
    }

    /// What Send should do now, given whether the draft has text.
    pub fn gate(&self, has_text: bool) -> SendGate {
        if self
            .uploads
            .iter()
            .any(|upload| upload.status == AttachmentStatus::Failed)
            || (!has_text && self.uploads.is_empty())
        {
            SendGate::Blocked
        } else if self.uploads.iter().all(Upload::is_ready) {
            SendGate::Now
        } else {
            SendGate::Wait
        }
    }

    /// Send was pressed: answers like [`Self::gate`], and a send that has
    /// to wait is remembered until the uploads finish.
    pub fn request_send(&mut self, has_text: bool) -> SendGate {
        let gate = self.gate(has_text);
        if gate == SendGate::Wait {
            self.queued = true;
            self.notice = None;
        }
        gate
    }

    /// Forgets a waiting send.
    pub fn cancel_send(&mut self) {
        self.queued = false;
    }

    /// Empties the tray into a post.
    pub fn take(&mut self) -> Vec<Upload> {
        self.queued = false;
        self.notice = None;
        std::mem::take(&mut self.uploads)
    }

    /// Puts fixtures in the tray as they are, for preview.
    pub fn extend(&mut self, uploads: impl IntoIterator<Item = Upload>) {
        self.uploads.extend(uploads);
    }

    /// A waiting send goes once every file is up, and is dropped with a
    /// word when one fails.
    fn settle(&mut self) -> bool {
        if !self.queued {
            return false;
        }
        if self
            .uploads
            .iter()
            .any(|upload| upload.status == AttachmentStatus::Failed)
        {
            self.queued = false;
            self.notice = Some(t!("attachments-upload-failed"));
            return false;
        }
        if self.uploads.iter().all(Upload::is_ready) {
            self.queued = false;
            return true;
        }
        false
    }
}

/// Where a file added while offline is read from when it uploads later
/// (#194): a picked or dropped file stays where it is, and a pasted image
/// is written to a folder of its own under the system's temporary folder,
/// keeping its name.
pub fn keep_for_outbox(id: &str, name: &str, source: &UploadSource) -> std::io::Result<PathBuf> {
    match source {
        UploadSource::Path(path) => Ok(path.clone()),
        UploadSource::Image(image) => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_nanos())
                .unwrap_or_default();
            let dir = std::env::temp_dir()
                .join("matty-outbox")
                .join(format!("{id}-{stamp}"));
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(name);
            std::fs::write(&path, &image.bytes)?;
            Ok(path)
        }
    }
}

/// A file on a posted message (PLAN §4.6 Files).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostFile {
    pub id: SharedString,
    pub name: SharedString,
    pub size: u64,
    /// Where the bytes are when the file is an image, for its thumbnail
    /// and the lightbox; sync fills it as thumbnails arrive.
    pub preview: Option<UploadSource>,
    /// Pixel size from the server's file metadata, for sizing the image
    /// thumbnail before it loads; 0 when unknown or not an image.
    pub width: u32,
    pub height: u32,
}

impl PostFile {
    /// A posted file from the server's file metadata; sync fills its
    /// thumbnail later.
    pub fn from_info(file: &matty_api::models::FileInfo) -> Self {
        Self {
            id: file.id.clone().into(),
            name: file.name.clone().into(),
            size: u64::try_from(file.size).unwrap_or(0),
            preview: None,
            width: u32::try_from(file.width).unwrap_or(0),
            height: u32::try_from(file.height).unwrap_or(0),
        }
    }

    /// The posted form of a tray upload.
    pub fn from_upload(upload: Upload) -> Self {
        let preview = is_image(&upload.name).then_some(upload.source);
        Self {
            id: upload.id,
            name: upload.name,
            size: upload.size,
            preview,
            width: 0,
            height: 0,
        }
    }

    /// The image to show, when the file is one with bytes at hand.
    pub fn image(&self) -> Option<ImageSource> {
        self.preview
            .as_ref()
            .and_then(|source| thumbnail(&self.name, source))
    }

    /// How the file shows under its post: an image thumbnail, an image
    /// whose thumbnail has not arrived yet (#105), or a file card.
    pub fn shown_as(&self) -> Shown {
        match self.image() {
            Some(image) => Shown::Image(image),
            None if self.preview.is_none() && is_image(&self.name) => Shown::Loading,
            None => Shown::Card,
        }
    }

    /// "PDF · 2.4 MB".
    pub fn meta(&self) -> SharedString {
        format!("{} · {}", kind(&self.name), format_size(self.size)).into()
    }
}

/// Where a posted file's download is (#151), shown on its card.
#[derive(Clone, Debug, PartialEq)]
pub enum FileDownload {
    /// Arriving: bytes so far, and the size when known.
    Downloading { received: u64, total: Option<u64> },
    /// Saved here; clicking the card opens this copy.
    Saved(PathBuf),
    /// Did not save; clicking the card tries again.
    Failed,
}

impl FileDownload {
    /// The card's second line for this state, in place of its kind and size:
    /// "Downloading… 42%", "Downloaded · PDF · 2.4 MB" or "Download failed ·
    /// Click to retry".
    pub fn meta(&self, file: &PostFile) -> SharedString {
        match self {
            Self::Downloading {
                received,
                total: Some(total),
            } if *total > 0 => {
                let percent = (received.saturating_mul(100) / total).min(100);
                t!("attachments-downloading-percent", percent = percent)
            }
            Self::Downloading { received, .. } => {
                t!(
                    "attachments-downloading-size",
                    size = format_size(*received)
                )
            }
            Self::Saved(_) => t!("attachments-downloaded", meta = file.meta().to_string()),
            Self::Failed => t!("attachments-download-failed"),
        }
    }

    /// The card's icon: a check once saved, an alert on failure, and the
    /// file's own icon while it arrives.
    fn icon(&self, name: &str) -> LucideIcon {
        match self {
            Self::Downloading { .. } => icon_for(name),
            Self::Saved(_) => LucideIcon::CircleCheck,
            Self::Failed => LucideIcon::CircleAlert,
        }
    }
}

/// Every download this run, by file id, for the cards to show (#151).
#[derive(Default)]
pub struct Downloads(pub std::collections::HashMap<SharedString, FileDownload>);

impl gpui_kit::Global for Downloads {}

impl Downloads {
    /// Where `file_id`'s download is, if it has one.
    pub fn get<'a>(cx: &'a App, file_id: &str) -> Option<&'a FileDownload> {
        cx.try_global::<Self>()?.0.get(file_id)
    }

    /// Records where `file_id`'s download is.
    pub fn set(cx: &mut App, file_id: SharedString, state: FileDownload) {
        cx.default_global::<Self>().0.insert(file_id, state);
    }
}

/// How a post's file shows; see [`PostFile::shown_as`].
#[derive(Clone)]
pub enum Shown {
    Image(ImageSource),
    /// An image whose thumbnail sync has not fetched yet: a tile of the
    /// image's size holds its place, so the transcript does not jump when
    /// it arrives.
    Loading,
    Card,
}

/// A source that is still loading: the attachment tile shows its muted
/// fill until the post's thumbnail replaces it.
fn loading_image() -> ImageSource {
    ImageSource::Custom(Arc::new(|_, _| None))
}

type ImageHandler = dyn Fn(usize, &mut Window, &mut App);
type FileHandler = dyn Fn(SharedString, &mut Window, &mut App);

/// A post's files under its text: image thumbnails first, then cards.
/// `on_image` gets the index among the images, for a lightbox; `on_file`
/// gets a card's file id, to open or download it.
pub fn post_files(
    prefix: &str,
    files: &[PostFile],
    on_image: impl Fn(usize, &mut Window, &mut App) + 'static,
    on_file: impl Fn(SharedString, &mut Window, &mut App) + 'static,
    cx: &App,
) -> gpui_kit::Div {
    let on_image: Rc<ImageHandler> = Rc::new(on_image);
    let on_file: Rc<FileHandler> = Rc::new(on_file);
    let mut images = Vec::new();
    let mut cards = Vec::new();
    let mut loaded = 0;
    for file in files {
        let id = ElementId::Name(format!("{prefix}.file.{}", file.id).into());
        match file.shown_as() {
            // Not in the lightbox until its thumbnail arrives, so it takes
            // no lightbox index and opens nothing.
            Shown::Loading => images.push(
                Attachment::new(id, file.name.clone())
                    .image(loading_image())
                    .dimensions(file.width, file.height),
            ),
            Shown::Image(image) => {
                let index = loaded;
                loaded += 1;
                let open = on_image.clone();
                images.push(
                    Attachment::new(id, file.name.clone())
                        .image(image)
                        .dimensions(file.width, file.height)
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            open(index, window, cx)
                        }),
                );
            }
            Shown::Card => {
                let file_id = file.id.clone();
                let open = on_file.clone();
                let download = Downloads::get(cx, &file.id);
                cards.push(
                    Attachment::new(id, file.name.clone())
                        .meta(download.map_or_else(|| file.meta(), |state| state.meta(file)))
                        .icon(
                            download.map_or_else(
                                || icon_for(&file.name),
                                |state| state.icon(&file.name),
                            ),
                        )
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            open(file_id.clone(), window, cx)
                        }),
                );
            }
        }
    }
    let theme = UiTheme::read(cx);
    div()
        .flex()
        .flex_col()
        .gap(theme.space(2.))
        .children((!images.is_empty()).then(|| {
            attachment_group(
                ElementId::Name(format!("{prefix}.images").into()),
                images,
                cx,
            )
            .restrict_scroll_to_axis()
            // The strip has scrolled by now; a sideways swipe stops here
            // rather than also nudging the message list (#623).
            .on_scroll(|event, window, cx| {
                if horizontal_swipe(event.delta.pixel_delta(window.line_height())) {
                    cx.stop_propagation();
                }
            })
        }))
        .children((!cards.is_empty()).then(|| {
            attachment_group(ElementId::Name(format!("{prefix}.cards").into()), cards, cx)
        }))
}

/// Whether a scroll goes mostly sideways, as a swipe along an image strip.
fn horizontal_swipe(delta: gpui_kit::Point<gpui_kit::Pixels>) -> bool {
    delta.x.abs() > delta.y.abs()
}

/// The tray above a composer: the chips, then one `meta` line saying why a
/// file was left out or a send did not go (`destructive`, with an icon), or
/// that a send waits for the uploads (DESIGN.md §4 Composer attachments).
/// None when there is nothing to show.
pub fn upload_tray(
    prefix: &str,
    tray: &UploadTray,
    chips: Vec<Attachment>,
    cx: &App,
) -> Option<gpui_kit::Div> {
    let theme = UiTheme::read(cx).clone();
    let line = |icon: gpui_kit::AnyElement, text: SharedString, color| {
        div()
            .flex()
            .items_start()
            .gap(theme.space(2.))
            .type_style(style::META, &theme)
            .text_color(color)
            .child(div().flex_shrink_0().mt(theme.space(0.5)).child(icon))
            .child(div().flex_1().min_w_0().child(text))
    };
    let notice = tray.notice().cloned().map(|notice| {
        line(
            lucide(LucideIcon::CircleAlert)
                .size(scaled(&theme, 14.))
                .text_color(theme.colors.destructive)
                .into_any_element(),
            notice,
            theme.colors.destructive,
        )
    });
    let waiting = tray.is_queued().then(|| {
        line(
            Spinner::new(ElementId::Name(format!("{prefix}.waiting.spinner").into()))
                .label(t!("attachments-waiting"))
                .size(gpui_kit::px(14.))
                .into_any_element(),
            t!("attachments-sends-when-done"),
            theme.colors.muted_foreground,
        )
    });
    let offline = tray.has_queued().then(|| {
        line(
            lucide(LucideIcon::CloudOff)
                .size(scaled(&theme, 14.))
                .text_color(theme.colors.muted_foreground)
                .into_any_element(),
            t!("attachments-queued-offline"),
            theme.colors.muted_foreground,
        )
    });
    if chips.is_empty() && notice.is_none() {
        return None;
    }
    Some(
        div()
            .flex()
            .flex_col()
            .gap(theme.space(2.))
            .pb(theme.space(2.))
            .children((!chips.is_empty()).then(|| {
                attachment_group(
                    ElementId::Name(format!("{prefix}.uploads").into()),
                    chips,
                    cx,
                )
            }))
            .children(notice)
            .children(offline)
            .children(waiting),
    )
}

/// What a drop region shows over itself while files are dragged across it
/// (#150): gpuicn's empty state, dashed in `ring` on the `accent` tint,
/// saying where the files go. It is in the tree only while something is
/// dragged, and shows only while files are over the region; `forced`
/// shows it regardless, for preview.
pub fn drop_overlay(
    id: &str,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    forced: bool,
    cx: &App,
) -> Option<gpui_kit::AnyElement> {
    if !forced && !cx.has_active_drag() {
        return None;
    }
    let theme = UiTheme::read(cx);
    let card = style::empty_state(LucideIcon::Upload, title, description, cx)
        .size_full()
        .border_2()
        .border_color(theme.colors.ring)
        .bg(theme.colors.accent);
    let overlay = div()
        .id(ElementId::Name(id.to_owned().into()))
        .debug_selector(|| "drop-overlay".into())
        .absolute()
        .inset_0()
        .p(theme.space(3.))
        .bg(theme.colors.background.opacity(0.85))
        .child(card);
    Some(if forced {
        overlay.into_any_element()
    } else {
        overlay
            .opacity(0.)
            .drag_over::<ExternalPaths>(|style, _, _, _| style.opacity(1.))
            .into_any_element()
    })
}

/// "Drop to upload to #general", or to a conversation by its name.
pub fn drop_title(channel: &str) -> String {
    if channel.contains(' ') {
        t!(
            "attachments-drop-conversation",
            channel = channel.to_string()
        )
        .to_string()
    } else {
        t!("attachments-drop-channel", channel = channel.to_string()).to_string()
    }
}

/// Whether `MATTY_PREVIEW=drop` asks for the drop overlay, for screenshots.
pub fn preview_drop() -> bool {
    std::env::var("MATTY_PREVIEW").is_ok_and(|value| value == "drop")
}

fn thumbnail(name: &str, source: &UploadSource) -> Option<ImageSource> {
    match source {
        UploadSource::Image(image) => Some(ImageSource::Image(image.clone())),
        // A draft's file restored from the server has no path here.
        UploadSource::Path(path) if is_image(name) && !path.as_os_str().is_empty() => {
            Some(ImageSource::from(path.clone()))
        }
        UploadSource::Path(_) => None,
    }
}

/// A PNG held in memory, for pasted images and fixtures.
pub fn png(bytes: &[u8]) -> Arc<Image> {
    Arc::new(Image::from_bytes(ImageFormat::Png, bytes.to_vec()))
}

fn extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Whether the name ends in an image extension the app can decode.
pub fn is_image(name: &str) -> bool {
    matches!(
        extension(name).as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg"
    )
}

/// The file kind as users say it: the extension in capitals, or "File".
pub fn kind(name: &str) -> String {
    let ext = extension(name);
    if ext.is_empty() {
        t!("attachments-file").to_string()
    } else {
        ext.to_ascii_uppercase()
    }
}

/// The icon for a file name.
pub fn icon_for(name: &str) -> LucideIcon {
    match extension(name).as_str() {
        ext if is_image(&format!("x.{ext}")) => LucideIcon::FileImage,
        "pdf" | "doc" | "docx" | "txt" | "md" | "rtf" | "pages" => LucideIcon::FileText,
        "xls" | "xlsx" | "csv" | "numbers" => LucideIcon::FileSpreadsheet,
        "zip" | "tar" | "gz" | "7z" | "rar" | "dmg" => LucideIcon::FileArchive,
        "rs" | "js" | "ts" | "py" | "go" | "json" | "toml" | "yaml" | "yml" | "sh" | "html"
        | "css" => LucideIcon::FileCode,
        "mp4" | "mov" | "webm" | "mkv" => LucideIcon::FileVideoCamera,
        "mp3" | "wav" | "m4a" | "ogg" | "flac" => LucideIcon::FileMusic,
        _ => LucideIcon::File,
    }
}

/// Bytes as "312 B", "2.4 KB", "2.4 MB" or "1.2 GB"; one decimal under ten.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.;
    let mut unit = UNITS[0];
    for next in &UNITS[1..] {
        if value < 1024. {
            break;
        }
        value /= 1024.;
        unit = next;
    }
    if value < 10. {
        format!("{value:.1} {unit}")
    } else {
        format!("{} {unit}", value.round())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #194: a file queued offline counts as ready, so Send goes now and
    /// a waiting send is due; once it uploads it is no longer queued.
    #[test]
    fn a_file_queued_offline_lets_the_post_go() {
        let mut tray = UploadTray::new("t");
        let added = tray.admit(vec![file("plan.pdf", 2_516_582)]);
        assert_eq!(tray.request_send(false), SendGate::Wait);
        assert!(tray.set_queued(&added[0].id));
        assert!(tray.has_queued());
        assert_eq!(tray.uploads()[0].meta().as_ref(), "Queued · 2.4 MB");
        assert_eq!(tray.gate(false), SendGate::Now);
        tray.set_status(&added[0].id, AttachmentStatus::Uploading(0.));
        assert!(!tray.has_queued());
        assert_eq!(tray.gate(false), SendGate::Wait);
    }

    type Seen = std::rc::Rc<std::cell::Cell<Option<gpui_kit::Bounds<gpui_kit::Pixels>>>>;

    /// A post's files in an 800px column, recording their laid-out bounds.
    struct FilesView {
        files: Vec<PostFile>,
        seen: Seen,
    }

    impl gpui_kit::Render for FilesView {
        fn render(
            &mut self,
            _: &mut Window,
            cx: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            let seen = self.seen.clone();
            div()
                .w(gpui_kit::px(800.))
                .on_children_prepainted(move |children, _, _| seen.set(children.first().copied()))
                .child(post_files(
                    "test",
                    &self.files,
                    |_, _, _| {},
                    |_, _, _| {},
                    cx,
                ))
        }
    }

    /// The laid-out size of a post's files.
    fn files_size(
        files: Vec<PostFile>,
        cx: &mut gpui_kit::TestAppContext,
    ) -> gpui_kit::Size<gpui_kit::Pixels> {
        let seen = Seen::default();
        let recorded = seen.clone();
        let (_, cx) = cx.add_window_view(move |_, _| FilesView {
            files,
            seen: recorded,
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        seen.get().expect("the files were laid out").size
    }

    #[gpui_kit::test]
    fn an_image_holds_its_size_until_its_thumbnail_arrives(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let file = |preview: Option<UploadSource>, width, height| PostFile {
            id: "f1".into(),
            name: "mockup.png".into(),
            size: 120_000,
            preview,
            width,
            height,
        };
        // The thumbnail is still on its way, then it lands.
        let waiting = files_size(vec![file(None, 1200, 800)], cx);
        let arrived = files_size(
            vec![file(Some(UploadSource::Image(png(&[0; 8]))), 1200, 800)],
            cx,
        );
        assert_eq!(waiting, arrived, "the transcript does not jump");
        assert!(waiting.height > gpui_kit::px(100.), "{waiting:?}");
        // Without a size from the server, a fixed tile holds the place.
        let unsized_waiting = files_size(vec![file(None, 0, 0)], cx);
        let unsized_arrived = files_size(
            vec![file(Some(UploadSource::Image(png(&[0; 8]))), 0, 0)],
            cx,
        );
        assert_eq!(unsized_waiting, unsized_arrived);
        // A file that is not an image stays a card.
        let pdf = PostFile {
            name: "plan.pdf".into(),
            ..file(None, 0, 0)
        };
        assert!(matches!(pdf.shown_as(), Shown::Card));
        assert!(matches!(file(None, 0, 0).shown_as(), Shown::Loading));
    }

    #[test]
    fn only_a_mostly_sideways_scroll_stays_in_the_strip() {
        use gpui_kit::{point, px};
        assert!(horizontal_swipe(point(px(-12.), px(3.))));
        assert!(!horizontal_swipe(point(px(2.), px(-9.))));
        assert!(!horizontal_swipe(point(px(0.), px(0.))));
    }

    #[test]
    fn sizes_read_like_a_finder_window() {
        assert_eq!(format_size(312), "312 B");
        assert_eq!(format_size(2_500), "2.4 KB");
        assert_eq!(format_size(2_516_582), "2.4 MB");
        assert_eq!(format_size(52_428_800), "50 MB");
        assert_eq!(format_size(1_288_490_189), "1.2 GB");
    }

    #[test]
    fn kinds_and_icons_follow_the_extension() {
        assert_eq!(kind("deck.PDF"), "PDF");
        assert_eq!(kind("Makefile"), "File");
        assert!(is_image("shot.png"));
        assert!(!is_image("notes.md"));
        assert_eq!(icon_for("report.xlsx"), LucideIcon::FileSpreadsheet);
        assert_eq!(icon_for("main.rs"), LucideIcon::FileCode);
        assert_eq!(icon_for("thing"), LucideIcon::File);
    }

    #[test]
    fn posted_files_keep_image_bytes_only_for_images() {
        let image = PostFile::from_upload(Upload::from_image("u1", png(b"png")));
        assert!(image.image().is_some());
        let doc = PostFile::from_upload(Upload::from_path("u2", "notes.pdf".into()));
        assert!(doc.preview.is_none());
        assert_eq!(doc.meta().as_ref(), "PDF · 0 B");
    }

    fn notes() -> PostFile {
        PostFile {
            id: "f1".into(),
            name: "notes.pdf".into(),
            size: 2_516_582,
            preview: None,
            width: 0,
            height: 0,
        }
    }

    #[test]
    fn a_card_says_where_its_download_is() {
        let notes = notes();
        let arriving = FileDownload::Downloading {
            received: 1_056_965,
            total: Some(2_516_582),
        };
        assert_eq!(arriving.meta(&notes).as_ref(), "Downloading… 42%");
        let no_size = FileDownload::Downloading {
            received: 3 * 1024 * 1024,
            total: None,
        };
        assert_eq!(no_size.meta(&notes).as_ref(), "Downloading… 3.0 MB");
        let saved = FileDownload::Saved("/tmp/notes.pdf".into());
        assert_eq!(saved.meta(&notes).as_ref(), "Downloaded · PDF · 2.4 MB");
        assert_eq!(
            FileDownload::Failed.meta(&notes).as_ref(),
            "Download failed · Click to retry"
        );
        assert_eq!(arriving.icon(&notes.name), LucideIcon::FileText);
        assert_eq!(saved.icon(&notes.name), LucideIcon::CircleCheck);
        assert_eq!(
            FileDownload::Failed.icon(&notes.name),
            LucideIcon::CircleAlert
        );
    }

    struct CardsProbe {
        opened: Rc<std::cell::RefCell<Vec<SharedString>>>,
    }

    impl gpui_kit::Render for CardsProbe {
        fn render(
            &mut self,
            _: &mut Window,
            cx: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            let opened = self.opened.clone();
            div().size(gpui_kit::px(400.)).child(post_files(
                "probe",
                &[notes()],
                |_, _, _| {},
                move |file, _, _| opened.borrow_mut().push(file),
                cx,
            ))
        }
    }

    #[gpui_kit::test]
    fn a_file_card_asks_for_its_file_in_every_download_state(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{Modifiers, point, px};
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let opened = Rc::new(std::cell::RefCell::new(Vec::new()));
        let window = cx.add_window({
            let opened = opened.clone();
            move |_, _| CardsProbe { opened }
        });
        let mut cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let states = [
            None,
            Some(FileDownload::Downloading {
                received: 10,
                total: Some(100),
            }),
            Some(FileDownload::Failed),
            Some(FileDownload::Saved("/tmp/notes.pdf".into())),
        ];
        for (clicks, state) in states.into_iter().enumerate() {
            cx.update(|window, cx| {
                if let Some(state) = state {
                    Downloads::set(cx, "f1".into(), state);
                }
                window.refresh();
            });
            cx.run_until_parked();
            // The card is the first thing in the window.
            cx.simulate_click(point(px(40.), px(20.)), Modifiers::none());
            assert_eq!(opened.borrow().len(), clicks + 1);
        }
        assert!(opened.borrow().iter().all(|id| id.as_ref() == "f1"));
    }

    fn file(name: &str, size: u64) -> Upload {
        Upload {
            id: "".into(),
            name: name.to_owned().into(),
            size,
            source: UploadSource::Path(name.into()),
            status: AttachmentStatus::Uploading(0.),
            queued: false,
        }
    }

    fn tray_with(count: usize) -> UploadTray {
        let mut tray = UploadTray::new("upload");
        let files = (0..count).map(|i| file(&format!("f{i}.txt"), 10)).collect();
        assert_eq!(tray.admit(files).len(), count);
        tray
    }

    #[test]
    fn a_file_over_the_limit_stays_out_and_says_why() {
        let mut tray = UploadTray::new("upload");
        tray.set_max_file_size(52_428_800);
        let accepted = tray.admit(vec![file("demo.mov", 120 * 1024 * 1024), file("a.txt", 5)]);
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].name.as_ref(), "a.txt");
        assert_eq!(accepted[0].id.as_ref(), "upload-1");
        assert_eq!(
            tray.notice().map(|n| n.as_ref()),
            Some("demo.mov is 120 MB. Files must be under 50 MB.")
        );
        // Exactly the limit is fine.
        assert_eq!(tray.admit(vec![file("b.bin", 52_428_800)]).len(), 1);
        assert_eq!(tray.notice(), None, "a clean add clears the line");
        tray.admit(vec![file("c.iso", u64::MAX), file("d.iso", u64::MAX)]);
        assert_eq!(
            tray.notice().map(|n| n.as_ref()),
            Some("2 files were not added. Files must be under 50 MB.")
        );
    }

    #[test]
    fn the_eleventh_file_stays_out() {
        let mut tray = tray_with(9);
        let accepted = tray.admit(vec![file("ten.txt", 1), file("eleven.txt", 1)]);
        assert_eq!(accepted.len(), 1);
        assert_eq!(tray.uploads().len(), MAX_FILE_COUNT);
        assert_eq!(
            tray.notice().map(|n| n.as_ref()),
            Some("eleven.txt was not added: a message can carry 10 files.")
        );
        tray.admit(vec![
            file("x", 1),
            file("huge", 200 * 1024 * 1024),
            file("y", 1),
        ]);
        assert_eq!(
            tray.notice().map(|n| n.as_ref()),
            Some(
                "huge is 200 MB. Files must be under 100 MB. \
                 2 files were not added: a message can carry 10 files."
            )
        );
        // Removing one makes room again and clears the line.
        let first = tray.uploads()[0].id.clone();
        tray.remove(&first);
        assert_eq!(tray.notice(), None);
        assert_eq!(tray.admit(vec![file("again.txt", 1)]).len(), 1);
    }

    #[test]
    fn send_waits_for_uploads_then_goes() {
        let mut tray = tray_with(2);
        let ids: Vec<SharedString> = tray.uploads().iter().map(|u| u.id.clone()).collect();
        assert_eq!(tray.request_send(true), SendGate::Wait);
        assert!(tray.is_queued());
        assert!(!tray.set_status(&ids[0], AttachmentStatus::Uploading(0.5)));
        assert!(!tray.set_status(&ids[0], AttachmentStatus::Ready));
        assert!(tray.is_queued(), "still waiting on the second file");
        assert!(tray.set_status(&ids[1], AttachmentStatus::Ready), "now due");
        assert!(!tray.is_queued());
        assert_eq!(tray.request_send(true), SendGate::Now);
        assert_eq!(tray.take().len(), 2);
        assert!(tray.is_empty());
    }

    #[test]
    fn a_failed_upload_drops_the_waiting_send() {
        let mut tray = tray_with(2);
        let ids: Vec<SharedString> = tray.uploads().iter().map(|u| u.id.clone()).collect();
        assert_eq!(tray.request_send(false), SendGate::Wait);
        assert!(!tray.set_status(&ids[0], AttachmentStatus::Failed));
        assert!(!tray.is_queued());
        assert!(tray.notice().is_some_and(|n| n.starts_with("Not sent")));
        assert!(
            !tray.set_status(&ids[1], AttachmentStatus::Ready),
            "nothing goes by itself"
        );
        assert_eq!(tray.gate(true), SendGate::Blocked);
        // Removing the failed file leaves a ready tray.
        assert!(!tray.remove(&ids[0]), "no send was waiting");
        assert_eq!(tray.gate(false), SendGate::Now);
    }

    #[test]
    fn removing_the_last_uploading_file_sends_the_waiting_message() {
        let mut tray = tray_with(2);
        let ids: Vec<SharedString> = tray.uploads().iter().map(|u| u.id.clone()).collect();
        tray.set_status(&ids[0], AttachmentStatus::Ready);
        assert_eq!(tray.request_send(true), SendGate::Wait);
        assert!(tray.remove(&ids[1]));
    }

    #[test]
    fn an_empty_tray_gates_on_the_text() {
        let mut tray = UploadTray::new("upload");
        assert_eq!(tray.gate(false), SendGate::Blocked);
        assert_eq!(tray.gate(true), SendGate::Now);
        assert_eq!(tray.request_send(false), SendGate::Blocked);
        assert!(!tray.is_queued());
        assert_eq!(drop_title("general"), "Drop to upload to #general");
        assert_eq!(drop_title("Priya Nair"), "Drop to upload to Priya Nair");
    }

    struct DropProbe {
        dropped: Rc<std::cell::RefCell<Vec<PathBuf>>>,
    }

    impl gpui_kit::Render for DropProbe {
        fn render(
            &mut self,
            _: &mut Window,
            cx: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            let dropped = self.dropped.clone();
            div()
                .relative()
                .size(gpui_kit::px(400.))
                .on_drop(move |paths: &ExternalPaths, _, _| {
                    dropped.borrow_mut().extend(paths.paths().iter().cloned())
                })
                .children(drop_overlay(
                    "probe.drop",
                    "Drop to upload to #general",
                    "Files go into the message you are writing.",
                    false,
                    cx,
                ))
        }
    }

    #[gpui_kit::test]
    fn the_overlay_shows_while_files_are_dragged_and_takes_the_drop(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{FileDropEvent, InputEvent as _, point, px};
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let dropped = Rc::new(std::cell::RefCell::new(Vec::new()));
        let window = cx.add_window({
            let dropped = dropped.clone();
            move |_, _| DropProbe { dropped }
        });
        let mut cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let draw = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| window.draw(cx).clear(cx))
        };
        let paths = || ExternalPaths(vec![PathBuf::from("/tmp/notes.pdf")].into());
        let at = point(px(200.), px(200.));
        draw(&mut cx);
        assert!(
            cx.debug_bounds("drop-overlay").is_none(),
            "nothing until a drag"
        );

        cx.update(|window, cx| {
            window.dispatch_event(
                FileDropEvent::Entered {
                    position: at,
                    paths: paths(),
                }
                .to_platform_input(),
                cx,
            )
        });
        draw(&mut cx);
        let overlay = cx
            .debug_bounds("drop-overlay")
            .expect("shown on drag-enter");
        assert_eq!(overlay.size.width, px(400.), "covers the whole region");

        cx.update(|window, cx| {
            window.dispatch_event(FileDropEvent::Exited.to_platform_input(), cx)
        });
        draw(&mut cx);
        assert!(cx.debug_bounds("drop-overlay").is_none(), "gone on leave");

        cx.update(|window, cx| {
            window.dispatch_event(
                FileDropEvent::Entered {
                    position: at,
                    paths: paths(),
                }
                .to_platform_input(),
                cx,
            );
        });
        draw(&mut cx);
        cx.update(|window, cx| {
            window.dispatch_event(
                FileDropEvent::Submit { position: at }.to_platform_input(),
                cx,
            )
        });
        draw(&mut cx);
        assert_eq!(
            dropped.borrow().as_slice(),
            [PathBuf::from("/tmp/notes.pdf")]
        );
        assert!(
            cx.debug_bounds("drop-overlay").is_none(),
            "gone after the drop"
        );
    }

    #[test]
    fn pasted_images_are_named_and_thumbnailed() {
        let upload = Upload::from_image("u1", png(b"not really png"));
        assert_eq!(upload.name.as_ref(), PASTED_IMAGE);
        assert!(upload.image().is_some());
        assert_eq!(upload.meta().as_ref(), "PNG · 14 B");
        assert!(!upload.is_ready());
        assert!(upload.ready(14).is_ready());
    }
}
