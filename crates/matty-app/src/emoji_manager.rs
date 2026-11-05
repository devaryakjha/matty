//! "Custom emoji…" (#209): the server's custom emoji, a form to add one
//! from an image with a preview and the name checked as it is typed, and
//! Delete on the user's own (an admin's on any). Sync sends `POST emoji`
//! and `DELETE emoji/{id}`; the list follows from its change sets.

use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, Image, ImageSource,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, img,
};

use crate::style::{self, TypeStyleExt as _, scaled};
use crate::ui::{
    button::{Button, ButtonSize, ButtonVariant},
    dialog::{DialogHandle, dialog, dialog_description, dialog_popup, dialog_title},
    field::Field,
    input::{InputEvent, InputState},
    theme::UiTheme,
};

/// What the dialog asks of sync.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmojiManagerEvent {
    /// Add `name` from an image's bytes and file name.
    Add {
        name: String,
        image: (Vec<u8>, String),
    },
    /// Remove the emoji with this id.
    Delete(String),
}

/// The server's limit on an emoji image.
const MAX_BYTES: usize = 512 * 1024;
/// The server's limit on a name.
const MAX_NAME: usize = 64;

/// Why `name` can't be a new emoji's name, or `None` when it can. `taken`
/// says whether a custom emoji already has it.
pub fn name_problem(name: &str, taken: impl Fn(&str) -> bool) -> Option<String> {
    if name.is_empty() {
        return Some("Others use it as :name: in messages and reactions.".into());
    }
    if name.chars().count() > MAX_NAME {
        return Some(format!("Use {MAX_NAME} characters or fewer."));
    }
    let allowed = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_+".contains(c);
    if !name.chars().all(allowed) {
        return Some("Use lowercase letters, numbers, -, + and _.".into());
    }
    if crate::emoji::lookup(name).is_some() || taken(name) {
        return Some(format!("There's already an emoji called :{name}:."));
    }
    None
}

/// Why `bytes` can't be an emoji image, or `None` when it can.
pub fn image_problem(bytes: &[u8]) -> Option<&'static str> {
    use gpui_kit::ImageFormat;
    let format = crate::live::image_format(bytes, "");
    if !matches!(
        format,
        Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif)
    ) {
        return Some("Choose a PNG, JPEG or GIF image.");
    }
    (bytes.len() > MAX_BYTES).then_some("Choose an image of 512 KB or less.")
}

/// One custom emoji in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Listed {
    id: String,
    name: String,
    mine: bool,
}

/// The chosen image: its bytes and file name, and the preview.
struct Chosen {
    bytes: Vec<u8>,
    file_name: String,
    preview: Arc<Image>,
}

pub struct EmojiManager {
    dialog: DialogHandle,
    name: Entity<InputState>,
    chosen: Option<Chosen>,
    /// Why the last chosen file was refused.
    image_error: Option<SharedString>,
    /// The user's id, and whether they may delete anyone's emoji.
    me: String,
    admin: bool,
    /// The emoji whose Delete was pressed once, waiting for a second press.
    confirming: Option<String>,
    focus_name: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<EmojiManagerEvent> for EmojiManager {}

impl EmojiManager {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("party-parrot"));
        let subscriptions =
            vec![
                cx.subscribe_in(
                    &name,
                    window,
                    |this, _, event: &InputEvent, window, cx| match event {
                        InputEvent::PressEnter { .. } => this.add(window, cx),
                        _ => cx.notify(),
                    },
                ),
            ];
        Self {
            dialog: DialogHandle::new(false),
            name,
            chosen: None,
            image_error: None,
            me: String::new(),
            admin: false,
            confirming: None,
            focus_name: false,
            _subscriptions: subscriptions,
        }
    }

    /// Opens the dialog for the user `me`; `admin` may delete any emoji.
    pub fn open(&mut self, me: String, admin: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.me = me;
        self.admin = admin;
        self.reset(window, cx);
        self.dialog.open(window, cx);
        self.focus_name = true;
        cx.notify();
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.name
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.chosen = None;
        self.image_error = None;
        self.confirming = None;
    }

    fn typed(&self, cx: &App) -> String {
        self.name.read(cx).value().trim().to_string()
    }

    fn listed(&self, cx: &App) -> Vec<Listed> {
        let Some(custom) = cx.try_global::<crate::emoji::CustomEmoji>() else {
            return Vec::new();
        };
        custom
            .names()
            .into_iter()
            .filter_map(|name| {
                let id = custom.id(&name)?;
                let mine = custom.creator(&name).as_deref() == Some(self.me.as_str());
                Some(Listed { id, name, mine })
            })
            .collect()
    }

    fn taken(cx: &App) -> impl Fn(&str) -> bool + '_ {
        let custom = cx.try_global::<crate::emoji::CustomEmoji>();
        move |name| custom.is_some_and(|custom| custom.contains(name))
    }

    /// Asks for an image file and shows it.
    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose image".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let read = path.clone();
            let bytes = cx
                .background_executor()
                .spawn(async move { std::fs::read(read) })
                .await;
            this.update_in(cx, |this, window, cx| {
                let file_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "emoji".into());
                let stem = path
                    .file_stem()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                match bytes {
                    Ok(bytes) => this.chose(bytes, file_name, &stem, window, cx),
                    Err(_) => this.image_error = Some("Couldn't read that file.".into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Takes `bytes` as the image, and the file's name as the emoji's when
    /// none is typed yet.
    fn chose(
        &mut self,
        bytes: Vec<u8>,
        file_name: String,
        stem: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(problem) = image_problem(&bytes) {
            self.image_error = Some(problem.into());
            self.chosen = None;
            return;
        }
        let Some(format) = crate::live::image_format(&bytes, "") else {
            return;
        };
        self.image_error = None;
        let preview = Arc::new(Image::from_bytes(format, bytes.clone()));
        self.chosen = Some(Chosen {
            bytes,
            file_name,
            preview,
        });
        if self.typed(cx).is_empty() {
            let suggested: String = stem
                .chars()
                .map(|c| if c == ' ' || c == '.' { '-' } else { c })
                .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_+".contains(*c))
                .take(MAX_NAME)
                .collect();
            self.name
                .update(cx, |input, cx| input.set_value(suggested, window, cx));
        }
    }

    fn ready(&self, cx: &App) -> bool {
        self.chosen.is_some() && name_problem(&self.typed(cx), Self::taken(cx)).is_none()
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dialog.is_open() || !self.ready(cx) {
            return;
        }
        let Some(chosen) = self.chosen.take() else {
            return;
        };
        cx.emit(EmojiManagerEvent::Add {
            name: self.typed(cx),
            image: (chosen.bytes, chosen.file_name),
        });
        self.reset(window, cx);
        cx.notify();
    }

    fn delete(&mut self, id: String, cx: &mut Context<Self>) {
        if self.confirming.as_deref() == Some(id.as_str()) {
            self.confirming = None;
            cx.emit(EmojiManagerEvent::Delete(id));
        } else {
            self.confirming = Some(id);
        }
        cx.notify();
    }

    fn add_form(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let typed = self.typed(cx);
        let problem = name_problem(&typed, Self::taken(cx));
        let invalid = !typed.is_empty() && problem.is_some();
        let hint: SharedString = problem
            .unwrap_or_else(|| format!("Others use it as :{typed}: in messages and reactions."))
            .into();
        let size = scaled(&theme, 40.);
        let preview = match &self.chosen {
            Some(chosen) => div()
                .size(size)
                .flex()
                .items_center()
                .justify_center()
                .child(img(ImageSource::Image(chosen.preview.clone())).size(size)),
            None => div()
                .size(size)
                .rounded(theme.radius.md)
                .bg(theme.colors.muted),
        };
        let field = Field::new("emoji.manage.name", &self.name).label("Name");
        let field = if invalid {
            field.error(hint)
        } else {
            field.description(hint)
        };
        div()
            .flex()
            .flex_col()
            .gap(theme.space(3.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space(3.))
                    .child(preview)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.space(1.))
                            .child(
                                Button::new("emoji.manage.choose")
                                    .variant(ButtonVariant::Outline)
                                    .size(ButtonSize::Sm)
                                    .label(if self.chosen.is_some() {
                                        "Choose another image…"
                                    } else {
                                        "Choose image…"
                                    })
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.choose(window, cx)),
                                    ),
                            )
                            .child(
                                div()
                                    .type_style(style::META, &theme)
                                    .text_color(match self.image_error {
                                        Some(_) => theme.colors.destructive,
                                        None => theme.colors.muted_foreground,
                                    })
                                    .child(self.image_error.clone().unwrap_or_else(|| {
                                        "PNG, JPEG or GIF, up to 512 KB.".into()
                                    })),
                            ),
                    ),
            )
            .child(field)
            .child(
                div().flex().justify_end().child(
                    Button::new("emoji.manage.add")
                        .variant(ButtonVariant::Default)
                        .label("Add emoji")
                        .disabled(!self.ready(cx))
                        .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                ),
            )
    }

    fn list(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let listed = self.listed(cx);
        let icon = scaled(&theme, 20.);
        let heading = div()
            .type_style(style::META, &theme)
            .text_color(theme.colors.muted_foreground)
            .child(format!("On this server ({})", listed.len()));
        let rows = listed.into_iter().map(|emoji| {
            let confirming = self.confirming.as_deref() == Some(emoji.id.as_str());
            let image = match crate::emoji::custom_source(&emoji.name, cx) {
                Some(source) => img(source).size(icon).into_any_element(),
                None => div().size(icon).into_any_element(),
            };
            let id = emoji.id.clone();
            div()
                .id(SharedString::from(format!("emoji.manage.row.{}", emoji.id)))
                .flex()
                .items_center()
                .gap(theme.space(2.))
                .h(scaled(&theme, 32.))
                .child(image)
                .child(div().flex_1().truncate().child(format!(":{}:", emoji.name)))
                .children((emoji.mine || self.admin).then(|| {
                    Button::new(SharedString::from(format!(
                        "emoji.manage.delete.{}",
                        emoji.id
                    )))
                    .variant(if confirming {
                        ButtonVariant::Destructive
                    } else {
                        ButtonVariant::Ghost
                    })
                    .size(ButtonSize::Sm)
                    .label(if confirming {
                        "Delete for everyone"
                    } else {
                        "Delete"
                    })
                    .aria_label(format!("Delete :{}:", emoji.name))
                    .on_click(cx.listener(move |this, _, _, cx| this.delete(id.clone(), cx)))
                }))
        });
        div()
            .flex()
            .flex_col()
            .gap(theme.space(1.))
            .child(heading)
            .child(
                div()
                    .id("emoji.manage.list")
                    .max_h(scaled(&theme, 220.))
                    .overflow_y_scroll()
                    .children(rows),
            )
    }
}

impl Render for EmojiManager {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let form = self.add_form(cx);
        let list = self.list(cx);
        let popup = dialog_popup("emoji.manage.popup", "Custom emoji", cx)
            .child(dialog_title("emoji.manage.title", cx).child("Custom emoji"))
            .child(
                dialog_description("emoji.manage.description", cx)
                    .child("Add an image everyone on this server can use as an emoji."),
            )
            .child(
                div()
                    .pt(theme.space(2.))
                    .flex()
                    .flex_col()
                    .gap(theme.space(4.))
                    .child(form)
                    .child(list),
            )
            .child(
                div().flex().justify_end().pt(theme.space(2.)).child(
                    Button::new("emoji.manage.done")
                        .variant(ButtonVariant::Outline)
                        .label("Done")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.dialog.close(window, cx);
                        })),
                ),
            );
        let element = dialog("emoji.manage", &self.dialog, popup, window, cx);
        if self.focus_name && self.dialog.is_open() {
            self.focus_name = false;
            let focus = self.name.read(cx).focus_handle(cx);
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        div().child(element)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_servers_rules() {
        let taken = |name: &str| name == "shipit";
        assert_eq!(name_problem("party-parrot_2+", taken), None);
        assert!(name_problem("", taken).is_some());
        assert_eq!(
            name_problem("Party", taken).as_deref(),
            Some("Use lowercase letters, numbers, -, + and _.")
        );
        assert_eq!(
            name_problem("shipit", taken).as_deref(),
            Some("There's already an emoji called :shipit:.")
        );
        // System emoji names are taken too, tones included.
        assert!(name_problem("tada", taken).is_some());
        assert!(name_problem("+1_dark_skin_tone", taken).is_some());
        assert!(name_problem(&"a".repeat(65), taken).is_some());
    }

    #[test]
    fn images_are_png_jpeg_or_gif_and_small() {
        let png = [0x89, b'P', b'N', b'G', 0, 0];
        assert_eq!(image_problem(&png), None);
        assert_eq!(
            image_problem(b"<svg/>"),
            Some("Choose a PNG, JPEG or GIF image.")
        );
        let mut big = png.to_vec();
        big.resize(MAX_BYTES + 1, 0);
        assert_eq!(
            image_problem(&big),
            Some("Choose an image of 512 KB or less.")
        );
    }
}
