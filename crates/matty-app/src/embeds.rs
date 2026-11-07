//! Cards under a post (#187): a link preview from the server's Open Graph
//! data, an image a message links to, and the message attachments
//! integrations and bots send. Their images come from outside the server
//! (#159): sync fetches them, and until they are here each keeps the box
//! its size reserves, so nothing moves when one arrives. With "Load remote
//! images: Ask" that box is a click-to-load placeholder.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    App, ImageSource, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Styled as _, Window, div,
};
use matty_api::models::{EmbedKind, MessageAttachment, Post, PostImage};
use serde_json::Value;

use crate::attachments::PostFile;
use crate::t;
use crate::ui::attachment::Attachment;
use crate::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::ui::embed::{ImagePlaceholder, LinkPreview, MessageEmbed, QuotedMessage};
use crate::ui::markdown::markdown;
use crate::ui::theme::{UiTheme, primitives::text};

/// One card under a post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Embed {
    /// An Open Graph preview of a link in the message.
    Link(LinkCard),
    /// A message attachment from `props.attachments`.
    Attachment(AttachmentCard),
    /// An image the message links to, shown inline.
    Image(EmbedImage),
    /// Another post quoted through its permalink, as a forward sends it.
    Permalink(QuotedPost),
}

/// A post quoted from a permalink in the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotedPost {
    pub post_id: SharedString,
    pub channel_id: SharedString,
    /// Where it was posted, such as "~Town Square"; empty for a direct or
    /// group message.
    pub channel_name: SharedString,
    pub author_id: String,
    /// The author's name; the transcript fills it in from the users it
    /// knows.
    pub author: SharedString,
    pub message: SharedString,
    /// When it was posted, in milliseconds.
    pub create_at: i64,
    /// Its local time and day label, which the transcript fills in; the
    /// day is empty until then.
    pub time: u16,
    pub day: SharedString,
    /// The files posted with it.
    pub files: Vec<PostFile>,
}

/// The quoted post of a `permalink` embed, if the server sent it.
fn quoted_post(embed: &matty_api::models::PostEmbed) -> Option<QuotedPost> {
    if embed.kind != EmbedKind::Permalink {
        return None;
    }
    let data = embed.data.as_ref()?;
    let text = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let post: Post = serde_json::from_value(data.get("post")?.clone()).ok()?;
    let channel = text("channel_display_name");
    Some(QuotedPost {
        post_id: text("post_id").into(),
        channel_id: text("channel_id").into(),
        channel_name: match text("channel_type").as_str() {
            "D" | "G" => SharedString::default(),
            _ if channel.is_empty() => SharedString::default(),
            _ => format!("~{channel}").into(),
        },
        author_id: post.user_id,
        author: t!("embed-someone"),
        message: post.message.into(),
        create_at: post.create_at,
        time: 0,
        day: SharedString::default(),
        files: post
            .metadata
            .files
            .iter()
            .filter(|file| file.delete_at == 0)
            .map(PostFile::from_info)
            .collect(),
    })
}

/// A link preview's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkCard {
    pub url: SharedString,
    pub site_name: SharedString,
    pub title: SharedString,
    pub description: SharedString,
    pub image: Option<EmbedImage>,
}

/// An image in a card: its URL, its pixel size so the card can size it,
/// and the image once sync has fetched it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbedImage {
    pub url: SharedString,
    pub width: u32,
    pub height: u32,
    pub loaded: Option<Arc<gpui_kit::Image>>,
}

impl EmbedImage {
    /// `url` with the size the server measured, or the size the page
    /// declared; `None` without either, as the card needs one.
    fn new(url: &str, images: &HashMap<String, PostImage>, declared: (i64, i64)) -> Option<Self> {
        if url.is_empty() {
            return None;
        }
        let (width, height) = images
            .get(url)
            .map_or(declared, |image| (image.width, image.height));
        let (width, height) = (u32::try_from(width).ok()?, u32::try_from(height).ok()?);
        (width > 0 && height > 0).then(|| Self {
            url: url.to_owned().into(),
            width,
            height,
            loaded: None,
        })
    }
}

/// A message attachment's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentCard {
    /// `good`, `warning`, `danger` or a hex color; empty for the default.
    pub color: SharedString,
    pub pretext: SharedString,
    pub author: SharedString,
    pub author_link: SharedString,
    pub title: SharedString,
    pub title_link: SharedString,
    /// Markdown.
    pub text: SharedString,
    pub fields: Vec<AttachmentFieldCard>,
    pub footer: SharedString,
    pub image: Option<EmbedImage>,
    /// The small round pictures beside the author and the footer, and the
    /// square on the right.
    pub author_icon: Option<EmbedImage>,
    pub footer_icon: Option<EmbedImage>,
    pub thumb: Option<EmbedImage>,
    /// Buttons and menus (#211).
    pub actions: Vec<crate::interactive::ActionCard>,
}

/// One field of an attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentFieldCard {
    pub title: SharedString,
    /// Markdown.
    pub value: SharedString,
    pub short: bool,
}

/// The cards for `post`. Attachments win over a link preview, and only the
/// first link gets one, as in the web app: an Open Graph card, or the image
/// itself when the link is to one.
pub fn from_post(post: &Post) -> Vec<Embed> {
    let attachments = post.attachments();
    if !attachments.is_empty() {
        return attachments
            .iter()
            .map(|attachment| Embed::Attachment(attachment_card(attachment, &post.metadata.images)))
            .collect();
    }
    if let Some(quoted) = post.metadata.embeds.iter().find_map(quoted_post) {
        return vec![Embed::Permalink(quoted)];
    }
    if post.props.get(REMOVE_LINK_PREVIEW).and_then(Value::as_str) == Some("true") {
        return Vec::new();
    }
    let images = &post.metadata.images;
    post.metadata
        .embeds
        .iter()
        .find_map(|embed| match embed.kind {
            EmbedKind::OpenGraph => open_graph(embed, images),
            // The server measures a linked image into `metadata.images`;
            // without that size the box cannot be reserved, so no card.
            EmbedKind::Image => EmbedImage::new(&embed.url, images, (0, 0)).map(Embed::Image),
            _ => None,
        })
        .into_iter()
        .collect()
}

fn open_graph(
    embed: &matty_api::models::PostEmbed,
    images: &HashMap<String, PostImage>,
) -> Option<Embed> {
    let graph = embed.opengraph()?;
    let title = if graph.title.is_empty() {
        embed.url.clone()
    } else {
        graph.title
    };
    let image = graph.images.first().and_then(|image| {
        let url = if image.secure_url.is_empty() {
            &image.url
        } else {
            &image.secure_url
        };
        EmbedImage::new(url, images, (image.width, image.height))
    });
    Some(Embed::Link(LinkCard {
        url: embed.url.clone().into(),
        site_name: graph.site_name.into(),
        title: title.into(),
        description: graph.description.into(),
        image,
    }))
}

fn attachment_card(
    attachment: &MessageAttachment,
    images: &HashMap<String, PostImage>,
) -> AttachmentCard {
    let text = if attachment.text.is_empty()
        && attachment.title.is_empty()
        && attachment.fields.is_empty()
    {
        &attachment.fallback
    } else {
        &attachment.text
    };
    AttachmentCard {
        color: attachment.color.clone().into(),
        pretext: attachment.pretext.clone().into(),
        author: attachment.author_name.clone().into(),
        author_link: attachment.author_link.clone().into(),
        title: attachment.title.clone().into(),
        title_link: attachment.title_link.clone().into(),
        text: text.clone().into(),
        fields: attachment
            .fields
            .iter()
            .filter(|field| !field.title.is_empty() || !field_value(&field.value).is_empty())
            .map(|field| AttachmentFieldCard {
                title: field.title.clone().into(),
                value: field_value(&field.value).into(),
                short: field.short,
            })
            .collect(),
        footer: attachment.footer.clone().into(),
        image: EmbedImage::new(&attachment.image_url, images, (0, 0)),
        // Icons draw at a fixed size, so one the server did not measure
        // still gets a box.
        author_icon: EmbedImage::new(&attachment.author_icon, images, (ICON, ICON)),
        footer_icon: EmbedImage::new(&attachment.footer_icon, images, (ICON, ICON)),
        thumb: EmbedImage::new(&attachment.thumb_url, images, (ICON, ICON)),
        actions: attachment
            .actions
            .iter()
            .filter(|action| !action.id.is_empty())
            .map(crate::interactive::ActionCard::from_api)
            .collect(),
    }
}

/// The size an unmeasured icon is taken to have.
const ICON: i64 = 64;

impl Embed {
    /// The card's image, if it has one.
    pub fn image(&self) -> Option<&EmbedImage> {
        match self {
            Self::Link(link) => link.image.as_ref(),
            Self::Attachment(attachment) => attachment.image.as_ref(),
            Self::Permalink(_) => None,
            Self::Image(image) => Some(image),
        }
    }

    /// Every outside image the card shows: its image, and an attachment's
    /// icons and thumbnail.
    pub fn images(&self) -> Vec<&EmbedImage> {
        match self {
            Self::Attachment(attachment) => [
                &attachment.image,
                &attachment.author_icon,
                &attachment.footer_icon,
                &attachment.thumb,
            ]
            .into_iter()
            .flatten()
            .collect(),
            other => other.image().into_iter().collect(),
        }
    }

    /// [`Embed::images`], to fill in once fetched.
    pub fn images_mut(&mut self) -> Vec<&mut EmbedImage> {
        match self {
            Self::Link(link) => link.image.iter_mut().collect(),
            Self::Attachment(attachment) => [
                &mut attachment.image,
                &mut attachment.author_icon,
                &mut attachment.footer_icon,
                &mut attachment.thumb,
            ]
            .into_iter()
            .flatten()
            .collect(),
            Self::Permalink(_) => Vec::new(),
            Self::Image(image) => vec![image],
        }
    }

    /// The files of a quoted post, whose thumbnails sync fetches as it
    /// does a post's own.
    pub fn files_mut(&mut self) -> &mut [PostFile] {
        match self {
            Self::Permalink(quoted) => &mut quoted.files,
            _ => &mut [],
        }
    }

    /// The files of a quoted post.
    pub fn files(&self) -> &[PostFile] {
        match self {
            Self::Permalink(quoted) => &quoted.files,
            _ => &[],
        }
    }
}

/// A field's value: the server allows a string or a number.
fn field_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A 1×1 transparent PNG: what a card draws until sync brings an image,
/// so the card keeps the image's box, showing its muted fill, meanwhile.
const PENDING_PNG: [u8; 68] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0x00, 0x02, 0x00,
    0x00, 0x05, 0x00, 0x01, 0x7a, 0x5e, 0xab, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44,
    0xae, 0x42, 0x60, 0x82,
];

static PENDING: LazyLock<Arc<gpui_kit::Image>> =
    LazyLock::new(|| crate::attachments::png(&PENDING_PNG));

/// A card's image and its size: the fetched image, or a blank one of the
/// same size while it loads.
fn shown(image: Option<&EmbedImage>) -> Option<(ImageSource, u32, u32)> {
    let image = image?;
    let loaded = image.loaded.clone().unwrap_or_else(|| PENDING.clone());
    Some((ImageSource::Image(loaded), image.width, image.height))
}

/// A small icon once it has come; nothing meanwhile, as it has no box to
/// keep.
fn loaded(image: Option<&EmbedImage>) -> Option<ImageSource> {
    image?.loaded.clone().map(ImageSource::Image)
}

/// The click-to-load stand-in for `image` when the person is asked before
/// remote images load (#159): the image's box, from the server's
/// `metadata.images`, and its host.
fn placeholder(image: &EmbedImage, cx: &App) -> Option<ImagePlaceholder> {
    if !crate::remote_images::waiting(&image.url, cx) {
        return None;
    }
    let url = image.url.clone();
    Some(
        ImagePlaceholder::new(crate::remote_images::host(&url), move |_, _, cx| {
            crate::remote_images::load(&url, cx);
        })
        .size(image.width, image.height),
    )
}

/// The name an image link shows: the last part of its path.
pub(crate) fn image_name(url: &str) -> SharedString {
    let url = url.split(['?', '#']).next().unwrap_or_default();
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = after_scheme.split_once('/').map_or("", |(_, path)| path);
    let name = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    if name.is_empty() {
        t!("embed-image")
    } else {
        name.to_owned().into()
    }
}

/// Collapses or expands a post's previews; gets the new collapsed state.
pub type ToggleHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// Opens a linked image in the lightbox; gets the image's URL.
pub type ImageHandler = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;

/// The card for what the server read from a link.
pub fn link_card(url: &str, graph: &matty_api::models::OpenGraph) -> LinkCard {
    let title = if graph.title.is_empty() {
        url
    } else {
        &graph.title
    };
    LinkCard {
        url: url.to_owned().into(),
        site_name: graph.site_name.clone().into(),
        title: title.to_owned().into(),
        description: graph.description.clone().into(),
        image: None,
    }
}

/// The first web link in a draft, for the preview above the composer
/// (#188): a word that starts with `http://` or `https://`, without the
/// punctuation that closes a sentence or wraps the link.
pub fn first_link(text: &str) -> Option<String> {
    text.split_whitespace().find_map(|word| {
        let word = word.trim_start_matches(['(', '<', '"', '\'', '[']);
        let link = word.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '>', '"', '\'', ']']);
        let host = link
            .strip_prefix("https://")
            .or_else(|| link.strip_prefix("http://"))?;
        (!host.is_empty() && host.contains('.')).then(|| link.to_owned())
    })
}

/// The post property Mattermost clients set to hide a post's link
/// preview: the web app's Remove link preview writes `"true"`.
const REMOVE_LINK_PREVIEW: &str = "remove_link_preview";

/// A new post's properties: a link preview the user removed while
/// composing stays off the post, for every client.
pub fn send_props(unfurl: bool) -> std::collections::HashMap<String, Value> {
    let mut props = std::collections::HashMap::new();
    if !unfurl {
        props.insert(REMOVE_LINK_PREVIEW.to_owned(), Value::String("true".into()));
    }
    props
}

/// Only web links open from a card.
fn opens(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// Opens a quoted post: its channel and its id.
pub type OnJump =
    std::rc::Rc<dyn Fn(&SharedString, &SharedString, &mut gpui_kit::Window, &mut App)>;

/// An attachment's menus and what runs a button press or menu choice
/// (#211).
pub type Interactive<'a> = (
    &'a crate::interactive::Menus,
    crate::interactive::ActionHandler,
);

/// The cards under the post `id`. `collapsed` hides their images, as the
/// web app's per-post preview toggle does; `on_toggle` flips it.
/// `on_image` opens a linked image in the lightbox (#152).
#[allow(clippy::too_many_arguments)]
pub fn post_embeds(
    id: &str,
    embeds: &[Embed],
    collapsed: bool,
    on_toggle: ToggleHandler,
    on_image: ImageHandler,
    on_jump: OnJump,
    (menus, on_action): Interactive<'_>,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = crate::ui::theme::UiTheme::read(cx).clone();
    let density = crate::channel::message_density(cx);
    div()
        .flex()
        .flex_col()
        .gap(theme.space(2.))
        .pt(theme.space(1.))
        .children(embeds.iter().enumerate().map(|(index, embed)| {
            let element_id = gpui_kit::ElementId::Name(format!("post.{id}.embed.{index}").into());
            match embed {
                Embed::Link(link) => {
                    let toggle = on_toggle.clone();
                    let mut card = LinkPreview::new(element_id, link.title.clone())
                        .density(density)
                        .site_name(link.site_name.clone())
                        .collapsed(collapsed)
                        .on_toggle(move |collapsed, window, cx| toggle(*collapsed, window, cx));
                    if !link.description.is_empty() {
                        card = card.description(link.description.clone());
                    }
                    if let Some(waiting) = link.image.as_ref().and_then(|i| placeholder(i, cx)) {
                        card = card.image_placeholder(waiting);
                    } else if let Some((source, width, height)) = shown(link.image.as_ref()) {
                        card = card.image(source, width, height);
                    }
                    if opens(&link.url) {
                        let url = link.url.clone();
                        card = card.on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            cx.open_url(&url)
                        });
                    }
                    card.into_any_element()
                }
                Embed::Attachment(attachment) => {
                    let actions = crate::interactive::actions(
                        &element_id,
                        id,
                        &attachment.actions,
                        menus,
                        &on_action,
                        cx,
                    );
                    attachment_element(element_id, id, index, attachment, density, cx)
                        .actions(actions)
                        .into_any_element()
                }
                Embed::Permalink(quoted) => {
                    quoted_element(element_id, id, index, quoted, on_jump.clone(), cx)
                }
                Embed::Image(image) => image_element(
                    element_id,
                    image,
                    collapsed,
                    (on_toggle.clone(), on_image.clone()),
                    &theme,
                    cx,
                ),
            }
        }))
}

/// Opens one quoted post.
type Jump = dyn Fn(&mut Window, &mut App);

/// A quoted post as gpuicn's quoted message card: the author's avatar,
/// name and time, where it was posted, the text clamped to a few lines,
/// and its files. Jump, and a click on one of its files, open the
/// original post.
fn quoted_element(
    element_id: gpui_kit::ElementId,
    id: &str,
    index: usize,
    quoted: &QuotedPost,
    on_jump: OnJump,
    cx: &App,
) -> gpui_kit::AnyElement {
    let density = crate::channel::message_density(cx);
    let mut card = QuotedMessage::new(element_id, quoted.author.clone())
        .density(density)
        .body(markdown(
            gpui_kit::ElementId::Name(format!("post.{id}.embed.{index}.quote").into()),
            quoted.message.clone(),
            cx,
        ));
    if let Some(picture) = crate::avatars::image(&quoted.author_id, cx) {
        card = card.avatar(picture);
    }
    if !quoted.day.is_empty() {
        let time = crate::settings::ClockFormat::current(cx).format(quoted.time);
        // "Today" is the day key sync sends, not shown as is.
        let today = quoted.day == "Today"; // i18n-ignore
        card = card.timestamp(if today {
            time.into()
        } else {
            t!("embed-sent-at", date = quoted.day.to_string(), time = time)
        });
    }
    if !quoted.channel_name.is_empty() {
        card = card.channel(quoted.channel_name.clone());
    }
    let jump: Option<Rc<Jump>> = (!quoted.channel_id.is_empty()).then(|| {
        let (channel, post) = (quoted.channel_id.clone(), quoted.post_id.clone());
        Rc::new(move |window: &mut Window, cx: &mut App| on_jump(&channel, &post, window, cx))
            as Rc<Jump>
    });
    if let Some(jump) = jump.clone() {
        card = card.on_jump(move |_, window, cx| jump(window, cx));
    }
    if !quoted.files.is_empty() {
        let (on_image, on_file) = (jump.clone(), jump);
        card = card.attachments(crate::attachments::post_files(
            &format!("post.{id}.embed.{index}"),
            &quoted.files,
            move |_, window, cx| {
                if let Some(jump) = &on_image {
                    jump(window, cx);
                }
            },
            move |_, window, cx| {
                if let Some(jump) = &on_file {
                    jump(window, cx);
                }
            },
            cx,
        ));
    }
    card.into_any_element()
}

/// An image the message links to: its name and a collapse toggle above
/// it, as in the web app, and the image in a box its size reserves;
/// clicking the image opens the lightbox.
fn image_element(
    element_id: gpui_kit::ElementId,
    image: &EmbedImage,
    collapsed: bool,
    (on_toggle, on_image): (ToggleHandler, ImageHandler),
    theme: &UiTheme,
    cx: &App,
) -> gpui_kit::AnyElement {
    let name = image_name(&image.url);
    let (icon, label) = if collapsed {
        (LucideIcon::ChevronRight, t!("embed-expand-image"))
    } else {
        (LucideIcon::ChevronDown, t!("embed-collapse-image"))
    };
    let toggle = Button::new((element_id.clone(), "toggle"))
        .variant(ButtonVariant::Ghost)
        .size(ButtonSize::IconXs)
        .aria_label(label)
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_toggle(!collapsed, window, cx)
        })
        .child(
            lucide(icon)
                .size(theme.space(3.5))
                .text_color(theme.colors.muted_foreground),
        );
    let header = div()
        .flex()
        .items_center()
        .gap(theme.space(1.))
        .max_w(theme.space(120.))
        .child(
            text::xs(div(), theme)
                .min_w_0()
                .text_color(theme.colors.muted_foreground)
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(name.clone()),
        )
        .child(
            div()
                .debug_selector(|| "post-image-toggle".into())
                .child(toggle),
        );
    let waiting = placeholder(image, cx)
        .filter(|_| !collapsed)
        .map(|waiting| {
            div()
                .debug_selector(|| "post-image-placeholder".into())
                .w(theme.space(120.))
                .child(MessageEmbed::new((element_id.clone(), "image")).image_placeholder(waiting))
                .into_any_element()
        });
    let picture = shown(Some(image))
        .filter(|_| !collapsed && waiting.is_none())
        .map(|(source, width, height)| {
            let mut picture = Attachment::new((element_id.clone(), "image"), name)
                .image(source)
                .dimensions(width, height)
                // The web app's inline image cap: the card width, 350px tall.
                .max_size(theme.space(120.), theme.space(87.5));
            if opens(&image.url) {
                let url = image.url.clone();
                picture = picture.on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    on_image(url.clone(), window, cx)
                });
            }
            div()
                .debug_selector(|| "post-image".into())
                // Shrinks with a narrow column, such as beside the thread
                // pane, instead of running past it (#683).
                .max_w_full()
                .child(picture)
                .into_any_element()
        });
    div()
        .id(element_id)
        .flex()
        .flex_col()
        .items_start()
        .gap(theme.space(1.))
        .child(header)
        .children(waiting.or(picture))
        .into_any_element()
}

fn attachment_element(
    element_id: gpui_kit::ElementId,
    id: &str,
    index: usize,
    attachment: &AttachmentCard,
    density: crate::ui::embed::MessageDensity,
    cx: &App,
) -> MessageEmbed {
    let text_id =
        |part: &str| gpui_kit::ElementId::Name(format!("post.{id}.embed.{index}.{part}").into());
    let mut card = MessageEmbed::new(element_id).density(density);
    if !attachment.color.is_empty() {
        card = card.color_str(&attachment.color);
    }
    if !attachment.pretext.is_empty() {
        card = card.pretext(markdown(text_id("pretext"), attachment.pretext.clone(), cx));
    }
    if let Some(icon) = loaded(attachment.author_icon.as_ref()) {
        card = card.author_icon(icon);
    }
    if let Some(icon) = loaded(attachment.footer_icon.as_ref()) {
        card = card.footer_icon(icon);
    }
    if let Some(waiting) = attachment.thumb.as_ref().and_then(|i| placeholder(i, cx)) {
        card = card.thumbnail_placeholder(waiting);
    } else if let Some((source, _, _)) = shown(attachment.thumb.as_ref()) {
        card = card.thumbnail(source);
    }
    if !attachment.author.is_empty() {
        card = card.author(attachment.author.clone());
        if opens(&attachment.author_link) {
            let url = attachment.author_link.clone();
            card = card.on_author_click(move |_, _, cx| cx.open_url(&url));
        }
    }
    if !attachment.title.is_empty() {
        card = card.title(attachment.title.clone());
        if opens(&attachment.title_link) {
            let url = attachment.title_link.clone();
            card = card.on_title_click(move |_, _, cx| cx.open_url(&url));
        }
    }
    if !attachment.text.is_empty() {
        card = card.body(markdown(text_id("text"), attachment.text.clone(), cx));
    }
    for (field_index, field) in attachment.fields.iter().enumerate() {
        card = card.field(
            field.title.clone(),
            markdown(
                text_id(&format!("field.{field_index}")),
                field.value.clone(),
                cx,
            ),
            field.short,
        );
    }
    if let Some(waiting) = attachment.image.as_ref().and_then(|i| placeholder(i, cx)) {
        card = card.image_placeholder(waiting);
    } else if let Some((source, width, height)) = shown(attachment.image.as_ref()) {
        card = card.image(source, width, height);
    }
    if !attachment.footer.is_empty() {
        card = card.footer(attachment.footer.clone());
    }
    card
}

#[cfg(test)]
mod tests {
    use matty_api::models::{PostEmbed, PostMetadata};
    use serde_json::json;

    use super::*;

    #[test]
    fn a_permalink_embed_quotes_the_post_and_where_it_was() {
        let post: Post = serde_json::from_value(serde_json::json!({
            "id": "fwd",
            "message": "https://chat.example.com/dev/pl/orig",
            "metadata": {"embeds": [{
                "type": "permalink",
                "data": {
                    "post_id": "orig",
                    "channel_id": "c1",
                    "channel_display_name": "Town Square",
                    "channel_type": "O",
                    "team_name": "dev",
                    "post": {
                        "id": "orig", "user_id": "ada", "message": "Ship it",
                        "create_at": 1_700_000_000_000_i64,
                        "metadata": {"files": [
                            {"id": "f1", "name": "plan.pdf", "size": 2048},
                            {"id": "f2", "name": "gone.png", "delete_at": 5}
                        ]}
                    }
                }
            }]}
        }))
        .unwrap();
        let embeds = from_post(&post);
        let [Embed::Permalink(quoted)] = embeds.as_slice() else {
            panic!("expected one quoted post");
        };
        assert_eq!(quoted.post_id.as_ref(), "orig");
        assert_eq!(quoted.channel_id.as_ref(), "c1");
        assert_eq!(quoted.channel_name.as_ref(), "~Town Square");
        assert_eq!(quoted.author_id, "ada");
        assert_eq!(quoted.message.as_ref(), "Ship it");
        assert_eq!(quoted.create_at, 1_700_000_000_000);
        // Its files come along, but not deleted ones.
        let names: Vec<_> = quoted.files.iter().map(|file| file.name.as_ref()).collect();
        assert_eq!(names, ["plan.pdf"]);
        assert_eq!(embeds[0].files().len(), 1);
    }

    fn post(props: Value, embeds: Vec<PostEmbed>) -> Post {
        Post {
            props: serde_json::from_value(props).unwrap(),
            metadata: PostMetadata {
                embeds,
                ..PostMetadata::default()
            },
            ..Post::default()
        }
    }

    fn graph(url: &str, title: &str) -> PostEmbed {
        PostEmbed {
            kind: EmbedKind::OpenGraph,
            url: url.into(),
            data: Some(json!({ "title": title, "site_name": "GitHub", "description": "Code" })),
        }
    }

    #[test]
    fn the_first_open_graph_link_gets_a_preview() {
        let embeds = from_post(&post(
            json!({}),
            vec![
                PostEmbed {
                    kind: EmbedKind::Image,
                    url: "https://example.com/a.png".into(),
                    data: None,
                },
                graph("https://github.com/a", "Repo A"),
                graph("https://github.com/b", "Repo B"),
            ],
        ));
        assert_eq!(
            embeds,
            [Embed::Link(LinkCard {
                url: "https://github.com/a".into(),
                site_name: "GitHub".into(),
                title: "Repo A".into(),
                description: "Code".into(),
                image: None,
            })]
        );
    }

    #[test]
    fn attachments_replace_the_link_preview() {
        let embeds = from_post(&post(
            json!({ "attachments": [{
                "color": "good",
                "title": "Build 42 passed",
                "title_link": "https://ci.example.com/42",
                "fields": [
                    { "title": "Branch", "value": "main", "short": true },
                    { "title": "Tests", "value": 318, "short": true },
                    { "title": "", "value": "" },
                ],
            }]}),
            vec![graph("https://github.com/a", "Repo A")],
        ));
        let [Embed::Attachment(card)] = embeds.as_slice() else {
            panic!("{embeds:?}");
        };
        assert_eq!(card.title.as_ref(), "Build 42 passed");
        assert_eq!(card.color.as_ref(), "good");
        let fields: Vec<_> = card
            .fields
            .iter()
            .map(|f| (f.title.as_ref(), f.value.as_ref()))
            .collect();
        assert_eq!(fields, [("Branch", "main"), ("Tests", "318")]);
    }

    #[test]
    fn a_bare_attachment_shows_its_fallback() {
        let embeds = from_post(&post(
            json!({ "attachments": [{ "fallback": "Deploy finished" }] }),
            vec![],
        ));
        let [Embed::Attachment(card)] = embeds.as_slice() else {
            panic!("{embeds:?}");
        };
        assert_eq!(card.text.as_ref(), "Deploy finished");
    }

    #[test]
    fn the_first_web_link_in_a_draft_is_previewed() {
        assert_eq!(
            first_link("see (https://github.com/karyalabs/matty). Thanks").as_deref(),
            Some("https://github.com/karyalabs/matty")
        );
        assert_eq!(
            first_link("<http://example.com/a?b=1> and https://x.org").as_deref(),
            Some("http://example.com/a?b=1")
        );
        assert_eq!(first_link("no link, https:// alone, or ftp://a.b"), None);
        assert_eq!(first_link("https://localhost"), None);
    }

    #[test]
    fn a_removed_preview_stays_off_the_post() {
        assert!(send_props(true).is_empty());
        let props = serde_json::to_value(send_props(false)).unwrap();
        let removed = from_post(&post(props, vec![graph("https://github.com/a", "Repo A")]));
        assert_eq!(removed, []);
    }

    fn image_post(url: &str, measured: Option<(i64, i64)>) -> Post {
        let mut post = post(
            json!({}),
            vec![PostEmbed {
                kind: EmbedKind::Image,
                url: url.into(),
                data: None,
            }],
        );
        if let Some((width, height)) = measured {
            post.metadata.images.insert(
                url.into(),
                PostImage {
                    width,
                    height,
                    ..PostImage::default()
                },
            );
        }
        post
    }

    #[test]
    fn a_linked_image_shows_inline_at_its_measured_size() {
        let url = "https://example.com/shots/a.png";
        let embeds = from_post(&image_post(url, Some((640, 400))));
        assert_eq!(
            embeds,
            [Embed::Image(EmbedImage {
                url: url.into(),
                width: 640,
                height: 400,
                loaded: None,
            })]
        );
        assert_eq!(embeds[0].image().map(|image| image.url.as_ref()), Some(url));
    }

    #[test]
    fn an_unmeasured_or_removed_image_gets_no_card() {
        let url = "https://example.com/a.png";
        assert_eq!(from_post(&image_post(url, None)), []);
        assert_eq!(from_post(&image_post(url, Some((0, 400)))), []);
        let mut removed = image_post(url, Some((640, 400)));
        removed.props =
            serde_json::from_value(serde_json::to_value(send_props(false)).unwrap()).unwrap();
        assert_eq!(from_post(&removed), []);
    }

    #[test]
    fn an_image_is_named_by_its_path() {
        assert_eq!(
            image_name("https://example.com/shots/a%20b.png?w=2#x").as_ref(),
            "a%20b.png"
        );
        assert_eq!(image_name("https://example.com/").as_ref(), "Image");
        assert_eq!(image_name("https://example.com").as_ref(), "Image");
    }

    #[test]
    fn a_pending_image_keeps_its_size() {
        let mut image = EmbedImage {
            url: "https://example.com/a.png".into(),
            width: 640,
            height: 400,
            loaded: None,
        };
        let (_, width, height) = shown(Some(&image)).expect("a pending image still draws");
        assert_eq!((width, height), (640, 400));
        image.loaded = Some(crate::attachments::png(&PENDING_PNG));
        assert_eq!(
            shown(Some(&image)).map(|(_, w, h)| (w, h)),
            Some((640, 400))
        );
    }

    struct Probe {
        embeds: Vec<Embed>,
        collapsed: bool,
        toggled: Rc<std::cell::Cell<Option<bool>>>,
        opened: Rc<std::cell::RefCell<Vec<SharedString>>>,
    }

    impl gpui_kit::Render for Probe {
        fn render(
            &mut self,
            _window: &mut Window,
            cx: &mut gpui_kit::Context<Self>,
        ) -> impl IntoElement {
            let toggled = self.toggled.clone();
            let opened = self.opened.clone();
            div().w(gpui_kit::px(800.)).child(post_embeds(
                "p1",
                &self.embeds,
                self.collapsed,
                Rc::new(move |collapsed, _, _| toggled.set(Some(collapsed))),
                Rc::new(move |url, _, _| opened.borrow_mut().push(url)),
                Rc::new(|_, _, _, _| {}),
                (&crate::interactive::Menus::default(), Rc::new(|_, _, _| {})),
                cx,
            ))
        }
    }

    fn probe(
        cx: &mut gpui_kit::TestAppContext,
        embeds: Vec<Embed>,
        collapsed: bool,
    ) -> (
        gpui_kit::Entity<Probe>,
        Rc<std::cell::Cell<Option<bool>>>,
        gpui_kit::VisualTestContext,
    ) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let toggled = Rc::new(std::cell::Cell::new(None));
        let window = cx.add_window({
            let toggled = toggled.clone();
            move |_, _| Probe {
                embeds,
                collapsed,
                toggled,
                opened: Rc::default(),
            }
        });
        let entity = window.root(cx).unwrap();
        (
            entity,
            toggled,
            gpui_kit::VisualTestContext::from_window(window.into(), cx),
        )
    }

    #[gpui_kit::test]
    fn an_image_reserves_its_box_before_it_loads(cx: &mut gpui_kit::TestAppContext) {
        let pending = Embed::Image(EmbedImage {
            url: "https://example.com/a.png".into(),
            width: 1280,
            height: 800,
            loaded: None,
        });
        let (probe, _, mut cx) = probe(cx, vec![pending], false);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let before = cx.debug_bounds("post-image").expect("drawn while loading");
        // 1280 × 800 scaled down into the 480 × 350 cap keeps 8:5.
        assert_eq!(before.size.width, gpui_kit::px(480.));
        assert_eq!(before.size.height, gpui_kit::px(300.));

        cx.update(|_, cx| {
            probe.update(cx, |probe, cx| {
                if let Some(image) = probe.embeds[0].images_mut().into_iter().next() {
                    image.loaded = Some(crate::attachments::png(&PENDING_PNG));
                }
                cx.notify();
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let after = cx.debug_bounds("post-image").unwrap();
        assert_eq!(after, before, "nothing moves when the image arrives");
    }

    #[gpui_kit::test]
    fn the_toggle_collapses_a_posts_image(cx: &mut gpui_kit::TestAppContext) {
        let image = Embed::Image(EmbedImage {
            url: "https://example.com/a.png".into(),
            width: 640,
            height: 400,
            loaded: None,
        });
        let (_, toggled, mut cx) = probe(cx, vec![image], true);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("post-image").is_none(), "collapsed");
        let toggle = cx.debug_bounds("post-image-toggle").unwrap();
        cx.simulate_click(toggle.center(), Default::default());
        assert_eq!(
            toggled.get(),
            Some(false),
            "expanding reports collapsed = false"
        );
    }

    /// #152: clicking a linked image hands its URL to the lightbox instead
    /// of leaving for the browser.
    #[gpui_kit::test]
    fn clicking_an_image_opens_it_in_the_lightbox(cx: &mut gpui_kit::TestAppContext) {
        let image = Embed::Image(EmbedImage {
            url: "https://example.com/shots/a.png".into(),
            width: 640,
            height: 400,
            loaded: Some(crate::attachments::png(&PENDING_PNG)),
        });
        let (probe, _, mut cx) = probe(cx, vec![image], false);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let picture = cx.debug_bounds("post-image").unwrap();
        cx.simulate_click(picture.center(), Default::default());
        let opened = cx.update(|_, cx| probe.read(cx).opened.borrow().clone());
        assert_eq!(
            opened,
            [SharedString::from("https://example.com/shots/a.png")]
        );
    }

    /// #159: under "Load remote images: Ask" a linked image is a
    /// placeholder sized from the server's metadata, and a click loads it
    /// instead of opening it.
    #[gpui_kit::test]
    fn with_ask_on_a_click_loads_the_image(cx: &mut gpui_kit::TestAppContext) {
        let fetched = Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let log = fetched.clone();
        let images = crate::remote_images::RemoteImages::new(
            Rc::new(move |url: &str| log.borrow_mut().push(url.to_owned())),
            true,
        );
        cx.update(|cx| cx.set_global(images.clone()));
        let image = Embed::Image(EmbedImage {
            url: "https://example.com/shots/a.png".into(),
            width: 640,
            height: 400,
            loaded: None,
        });
        let (probe, _, mut cx) = probe(cx, vec![image], false);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("post-image").is_none());
        let placeholder = cx.debug_bounds("post-image-placeholder").unwrap();
        assert!(placeholder.size.height > placeholder.size.width * 0.5);
        cx.simulate_click(placeholder.center(), Default::default());
        assert_eq!(*fetched.borrow(), ["https://example.com/shots/a.png"]);
        assert!(cx.update(|_, cx| probe.read(cx).opened.borrow().is_empty()));
        // Loading: the image's box takes the placeholder's place.
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("post-image-placeholder").is_none());
        assert!(cx.debug_bounds("post-image").is_some());
    }

    #[test]
    fn only_web_links_open() {
        assert!(opens("https://example.com"));
        assert!(!opens("javascript:alert(1)"));
        assert!(!opens("file:///etc/passwd"));
    }
}
