//! Files on their way into a post: what the composer tray shows for each
//! upload and the file facts shared with sync (PLAN §4.3 Writing, §4.6
//! Files).

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui_icons::LucideIcon;
use gpui_kit::{
    App, ElementId, Image, ImageFormat, ImageSource, ParentElement as _, SharedString, Styled as _,
    Window, div,
};

pub use crate::ui::attachment::AttachmentStatus;
use crate::ui::{
    attachment::{Attachment, attachment_group},
    theme::UiTheme,
};

/// The name a pasted image gets; Mattermost uses the same.
pub const PASTED_IMAGE: &str = "Pasted image.png";

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
}

impl Upload {
    /// An upload for a file on disk; the size comes from its metadata.
    pub fn from_path(id: impl Into<SharedString>, path: PathBuf) -> Self {
        let name: SharedString = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "File".to_string())
            .into();
        let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        Self {
            id: id.into(),
            name,
            size,
            source: UploadSource::Path(path),
            status: AttachmentStatus::Uploading(0.),
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

    /// The image to show as a thumbnail, when the file is one.
    pub fn image(&self) -> Option<ImageSource> {
        thumbnail(&self.name, &self.source)
    }

    /// The chip's second line: "PDF · 2.4 MB".
    pub fn meta(&self) -> SharedString {
        format!("{} · {}", kind(&self.name), format_size(self.size)).into()
    }

    /// The file-type icon for a card.
    pub fn icon(&self) -> LucideIcon {
        icon_for(&self.name)
    }

    /// Whether the file is on the server and the post can go.
    pub fn is_ready(&self) -> bool {
        self.status == AttachmentStatus::Ready
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

    /// "PDF · 2.4 MB".
    pub fn meta(&self) -> SharedString {
        format!("{} · {}", kind(&self.name), format_size(self.size)).into()
    }
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
    for file in files {
        let id = ElementId::Name(format!("{prefix}.file.{}", file.id).into());
        match file.image() {
            Some(image) => {
                let index = images.len();
                let open = on_image.clone();
                images.push(
                    Attachment::new(id, file.name.clone())
                        .image(image)
                        .dimensions(file.width, file.height)
                        .on_click(move |_, window, cx| open(index, window, cx)),
                );
            }
            None => {
                let file_id = file.id.clone();
                let open = on_file.clone();
                cards.push(
                    Attachment::new(id, file.name.clone())
                        .meta(file.meta())
                        .icon(icon_for(&file.name))
                        .on_click(move |_, window, cx| open(file_id.clone(), window, cx)),
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
        }))
        .children((!cards.is_empty()).then(|| {
            attachment_group(ElementId::Name(format!("{prefix}.cards").into()), cards, cx)
        }))
}

fn thumbnail(name: &str, source: &UploadSource) -> Option<ImageSource> {
    match source {
        UploadSource::Image(image) => Some(ImageSource::Image(image.clone())),
        UploadSource::Path(path) if is_image(name) => Some(ImageSource::from(path.clone())),
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
        "File".to_string()
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
