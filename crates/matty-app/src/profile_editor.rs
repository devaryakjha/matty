//! "Edit profile" (#212): the user's own names, nickname, position and
//! username, saved with `PUT users/{id}/patch`. Only changed fields are
//! sent. The picture changes at once, apart from Save.

use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, Focusable as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Window,
    div,
};
use matty_api::ProfilePatch;

use crate::style::TypeStyleExt as _;
use crate::t;

use crate::ui::{
    avatar::{Avatar, AvatarSize},
    button::{Button, ButtonSize, ButtonVariant},
    dialog::{DialogHandle, dialog, dialog_description, dialog_popup, dialog_title},
    field::Field,
    input::{InputEvent, InputState},
    theme::UiTheme,
};

/// The editable fields, as the server has them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileFields {
    pub first_name: String,
    pub last_name: String,
    pub nickname: String,
    pub position: String,
    pub username: String,
    /// Whose profile it is, for the picture.
    pub user_id: String,
    /// True when the user uploaded a picture, rather than the generated one.
    pub custom_picture: bool,
}

impl ProfileFields {
    pub fn of(user: &matty_api::models::User) -> Self {
        Self {
            user_id: user.id.clone(),
            // Mattermost marks a reset picture with a negative time.
            custom_picture: user.last_picture_update > 0,
            first_name: user.first_name.clone(),
            last_name: user.last_name.clone(),
            nickname: user.nickname.clone(),
            position: user.position.clone(),
            username: user.username.clone(),
        }
    }

    /// What changed from `before`, each field trimmed.
    pub fn patch_from(&self, before: &Self) -> ProfilePatch {
        let changed = |now: &str, then: &str| {
            let now = now.trim();
            (now != then).then(|| now.to_string())
        };
        ProfilePatch {
            first_name: changed(&self.first_name, &before.first_name),
            last_name: changed(&self.last_name, &before.last_name),
            nickname: changed(&self.nickname, &before.nickname),
            position: changed(&self.position, &before.position),
            username: changed(&self.username, &before.username),
        }
    }
}

/// Mattermost's limits.
pub const MAX_NAME: usize = 64;
pub const MAX_NICKNAME: usize = 64;
pub const MAX_POSITION: usize = 128;

/// Why `username` is not one the server takes, or `None` when it is: 3 to
/// 22 characters, starting with a letter, then lowercase letters, digits,
/// `.`, `-` or `_`.
pub fn username_problem(username: &str) -> Option<SharedString> {
    let name = username.trim();
    let count = name.chars().count();
    if !(3..=22).contains(&count) {
        return Some(t!("profile-username-length"));
    }
    if !name.starts_with(|c: char| c.is_ascii_lowercase()) {
        return Some(t!("profile-username-start"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
    {
        return Some(t!("profile-username-characters"));
    }
    None
}

/// The user saved changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileSaved(pub ProfilePatch);

/// The user picked a new picture (its bytes and file name), or removed it
/// with `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PictureChosen(pub Option<(Vec<u8>, String)>);

/// The image types the server takes for a profile picture.
const PICTURE_TYPES: [&str; 5] = ["jpg", "jpeg", "png", "gif", "bmp"];

/// Why `path` can't be a profile picture, or `None` when it can.
pub fn picture_problem(path: &std::path::Path) -> Option<SharedString> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    (!PICTURE_TYPES.contains(&extension.as_str())).then(|| t!("profile-picture-type"))
}

struct Inputs {
    first_name: Entity<InputState>,
    last_name: Entity<InputState>,
    nickname: Entity<InputState>,
    position: Entity<InputState>,
    username: Entity<InputState>,
}

pub struct ProfileEditor {
    dialog: DialogHandle,
    inputs: Inputs,
    /// The fields when the dialog opened.
    before: ProfileFields,
    focus_first: bool,
    /// Why the last picked picture was refused.
    picture_error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ProfileSaved> for ProfileEditor {}
impl EventEmitter<PictureChosen> for ProfileEditor {}

impl ProfileEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut input = || cx.new(|cx| InputState::new(window, cx));
        let inputs = Inputs {
            first_name: input(),
            last_name: input(),
            nickname: input(),
            position: input(),
            username: input(),
        };
        let subscriptions = [
            &inputs.first_name,
            &inputs.last_name,
            &inputs.nickname,
            &inputs.position,
            &inputs.username,
        ]
        .into_iter()
        .map(|input| {
            cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::PressEnter { .. } => this.save(window, cx),
                    // The Save button and the username hint follow the text.
                    _ => cx.notify(),
                }
            })
        })
        .collect();
        Self {
            dialog: DialogHandle::new(false),
            inputs,
            before: ProfileFields::default(),
            focus_first: false,
            picture_error: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn open(&mut self, fields: ProfileFields, window: &mut Window, cx: &mut Context<Self>) {
        for (input, value) in [
            (&self.inputs.first_name, &fields.first_name),
            (&self.inputs.last_name, &fields.last_name),
            (&self.inputs.nickname, &fields.nickname),
            (&self.inputs.position, &fields.position),
            (&self.inputs.username, &fields.username),
        ] {
            input.update(cx, |input, cx| input.set_value(value.clone(), window, cx));
        }
        self.before = fields;
        self.picture_error = None;
        self.dialog.open(window, cx);
        self.focus_first = true;
        cx.notify();
    }

    /// Asks for an image file and sends it as the new picture.
    fn pick_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("profile-picture-prompt")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            if let Some(problem) = picture_problem(&path) {
                this.update(cx, |this, cx| {
                    this.picture_error = Some(problem);
                    cx.notify();
                })
                .ok();
                return;
            }
            let read = path.clone();
            let bytes = cx
                .background_executor()
                .spawn(async move { std::fs::read(read) })
                .await;
            this.update(cx, |this, cx| {
                match bytes {
                    Ok(bytes) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "picture".into());
                        this.picture_error = None;
                        this.before.custom_picture = true;
                        cx.emit(PictureChosen(Some((bytes, name))));
                    }
                    Err(_) => this.picture_error = Some(t!("profile-picture-unreadable")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn remove_picture(&mut self, cx: &mut Context<Self>) {
        self.before.custom_picture = false;
        self.picture_error = None;
        cx.emit(PictureChosen(None));
        cx.notify();
    }

    fn picture_row(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let typed = self.typed(cx);
        let name = format!("{} {}", typed.first_name.trim(), typed.last_name.trim());
        let initial: String = name
            .trim()
            .chars()
            .next()
            .or_else(|| typed.username.chars().next())
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default();
        let avatar = crate::avatars::with_picture(
            Avatar::new("profile.edit.avatar")
                .size(AvatarSize::Lg)
                .fallback(initial)
                .aria_label(t!("profile-picture")),
            &self.before.user_id,
            cx,
        );
        div()
            .flex()
            .items_center()
            .gap(theme.space(3.))
            .child(avatar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space(1.))
                    .child(
                        div()
                            .flex()
                            .gap(theme.space(2.))
                            .child(
                                Button::new("profile.edit.picture")
                                    .variant(ButtonVariant::Outline)
                                    .size(ButtonSize::Sm)
                                    .label(t!("profile-picture-upload"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.pick_picture(window, cx)
                                    })),
                            )
                            .children(self.before.custom_picture.then(|| {
                                Button::new("profile.edit.picture.remove")
                                    .variant(ButtonVariant::Ghost)
                                    .size(ButtonSize::Sm)
                                    .label(t!("profile-picture-remove"))
                                    .on_click(cx.listener(|this, _, _, cx| this.remove_picture(cx)))
                            })),
                    )
                    .child(
                        div()
                            .type_style(crate::style::META, &theme)
                            .text_color(match self.picture_error {
                                Some(_) => theme.colors.destructive,
                                None => theme.colors.muted_foreground,
                            })
                            .child(
                                self.picture_error
                                    .clone()
                                    .unwrap_or_else(|| t!("profile-picture-hint")),
                            ),
                    ),
            )
    }

    fn typed(&self, cx: &gpui_kit::App) -> ProfileFields {
        let value = |input: &Entity<InputState>| input.read(cx).value().to_string();
        ProfileFields {
            user_id: self.before.user_id.clone(),
            custom_picture: self.before.custom_picture,
            first_name: value(&self.inputs.first_name),
            last_name: value(&self.inputs.last_name),
            nickname: value(&self.inputs.nickname),
            position: value(&self.inputs.position),
            username: value(&self.inputs.username),
        }
    }

    /// The patch to send, when the typed fields are valid and change
    /// something.
    fn patch(&self, cx: &gpui_kit::App) -> Option<ProfilePatch> {
        let typed = self.typed(cx);
        let too_long = typed.first_name.trim().chars().count() > MAX_NAME
            || typed.last_name.trim().chars().count() > MAX_NAME
            || typed.nickname.trim().chars().count() > MAX_NICKNAME
            || typed.position.trim().chars().count() > MAX_POSITION;
        if too_long || username_problem(&typed.username).is_some() {
            return None;
        }
        let patch = typed.patch_from(&self.before);
        (!patch.is_empty()).then_some(patch)
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dialog.is_open() {
            return;
        }
        let Some(patch) = self.patch(cx) else {
            return;
        };
        self.dialog.close(window, cx);
        cx.emit(ProfileSaved(patch));
    }

    fn field(
        id: &'static str,
        input: &Entity<InputState>,
        label: SharedString,
        description: Option<SharedString>,
    ) -> Field {
        let field = Field::new(id, input).label(label);
        match description {
            Some(description) => field.description(description),
            None => field,
        }
    }
}

impl Render for ProfileEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let username = self.inputs.username.read(cx).value().to_string();
        let username_hint: SharedString =
            username_problem(&username).unwrap_or_else(|| t!("profile-username-hint"));
        let usable = self.patch(cx).is_some();
        let picture = self.picture_row(cx);
        let popup = dialog_popup("profile.edit.popup", t!("profile-edit-title"), cx)
            .child(dialog_title("profile.edit.title", cx).child(t!("profile-edit-title")))
            .child(
                dialog_description("profile.edit.description", cx).child(t!("profile-edit-hint")),
            )
            .child(
                div()
                    .pt(theme.space(2.))
                    .flex()
                    .flex_col()
                    .gap(theme.space(3.))
                    .child(picture)
                    .child(
                        div()
                            .flex()
                            .gap(theme.space(3.))
                            .child(div().flex_1().child(Self::field(
                                "profile.edit.first",
                                &self.inputs.first_name,
                                t!("profile-first-name"),
                                None,
                            )))
                            .child(div().flex_1().child(Self::field(
                                "profile.edit.last",
                                &self.inputs.last_name,
                                t!("profile-last-name"),
                                None,
                            ))),
                    )
                    .child(Self::field(
                        "profile.edit.nickname",
                        &self.inputs.nickname,
                        t!("profile-nickname"),
                        None,
                    ))
                    .child(Self::field(
                        "profile.edit.position",
                        &self.inputs.position,
                        t!("profile-position"),
                        Some(t!("profile-position-hint")),
                    ))
                    .child(Self::field(
                        "profile.edit.username",
                        &self.inputs.username,
                        t!("profile-username"),
                        Some(username_hint),
                    )),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        Button::new("profile.edit.cancel")
                            .variant(ButtonVariant::Outline)
                            .label(t!("profile-cancel"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dialog.close(window, cx);
                            })),
                    )
                    .child(
                        Button::new("profile.edit.save")
                            .variant(ButtonVariant::Default)
                            .label(t!("profile-save"))
                            .disabled(!usable)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            );
        let element = dialog("profile.edit", &self.dialog, popup, window, cx);
        if self.focus_first && self.dialog.is_open() {
            self.focus_first = false;
            let focus = self.inputs.first_name.read(cx).focus_handle(cx);
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        div().child(element)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_are_images_the_server_takes() {
        use std::path::Path;
        assert_eq!(picture_problem(Path::new("me.PNG")), None);
        assert_eq!(picture_problem(Path::new("/a/b/me.jpeg")), None);
        assert!(picture_problem(Path::new("me.svg")).is_some());
        assert!(picture_problem(Path::new("me")).is_some());
    }

    #[test]
    fn usernames_follow_the_server_rules() {
        assert_eq!(username_problem("ada.l"), None);
        assert_eq!(username_problem(" ada "), None);
        assert!(username_problem("ab").is_some());
        assert!(username_problem("1ada").is_some());
        assert!(username_problem("Ada").is_some());
        assert!(username_problem("ada lovelace").is_some());
        assert!(username_problem(&"a".repeat(23)).is_some());
    }

    #[test]
    fn only_changed_fields_are_sent() {
        let before = ProfileFields {
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            nickname: String::new(),
            position: String::new(),
            username: "ada".into(),
            ..Default::default()
        };
        let mut after = before.clone();
        assert!(after.patch_from(&before).is_empty());
        after.position = " Engineer ".into();
        after.first_name = "Ada ".into();
        assert_eq!(
            after.patch_from(&before),
            ProfilePatch {
                position: Some("Engineer".into()),
                ..Default::default()
            }
        );
    }
}
