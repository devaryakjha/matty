//! The Drafts view (PLAN §4.3): every unsent message the user left in a
//! composer, newest first, so nothing written gets lost. It replaces the
//! channel view when the sidebar's Drafts row is chosen.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    App, Context, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::style::{self, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    button::{Button, ButtonSize, ButtonVariant},
    empty::{Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle},
    markdown::markdown,
    theme::UiTheme,
};

/// One unsent message, as the list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub channel_id: SharedString,
    pub channel_name: SharedString,
    /// The thread the draft replies in, when it is a reply.
    pub thread_root: Option<SharedString>,
    /// Markdown, as typed.
    pub body: SharedString,
    /// When it was last edited ("Today 10:12", "Yesterday 16:40").
    pub updated: SharedString,
}

/// The user's drafts, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DraftsModel {
    pub drafts: Vec<Draft>,
}

/// What the view asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftsEvent {
    /// Open the channel (and thread) the draft belongs to, draft in the composer.
    Open(Draft),
    /// Send the draft as it is; the view already removed it.
    Send(Draft),
    /// Throw the draft away; the view already removed it.
    Delete(Draft),
}

pub struct DraftsView {
    model: DraftsModel,
}

impl EventEmitter<DraftsEvent> for DraftsView {}

impl DraftsView {
    pub fn new(model: DraftsModel) -> Self {
        Self { model }
    }

    pub fn model(&self) -> &DraftsModel {
        &self.model
    }

    pub fn set_model(&mut self, model: DraftsModel, cx: &mut Context<Self>) {
        self.model = model;
        cx.notify();
    }

    fn remove(&mut self, draft: &Draft, cx: &mut Context<Self>) {
        self.model.drafts.retain(|d| d != draft);
        cx.notify();
    }

    fn header(&self, cx: &App) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let count = self.model.drafts.len();
        div()
            .flex_shrink_0()
            .h(scaled(&theme, metrics::HEADER_HEIGHT))
            .px(scaled(&theme, metrics::GUTTER))
            .flex()
            .items_center()
            .gap(theme.space(2.))
            .border_b(style::hairline())
            .border_color(theme.colors.border)
            .child(
                div()
                    .type_style(style::TITLE, &theme)
                    .text_color(theme.colors.foreground)
                    .child("Drafts"),
            )
            .child(
                div()
                    .type_style(style::META, &theme)
                    .text_color(theme.colors.muted_foreground)
                    .child(match count {
                        0 => String::new(),
                        1 => "1 unsent message".to_string(),
                        n => format!("{n} unsent messages"),
                    }),
            )
    }

    /// A card per draft: where it goes, the text as it will read, then
    /// Delete on the left and Open and Send on the right.
    fn card(
        &self,
        index: usize,
        draft: &Draft,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let channel = match draft.channel_name.as_ref() {
            name if name.contains(' ') => name.to_string(),
            name => format!("#{name}"),
        };
        let place = if draft.thread_root.is_some() {
            format!("Reply in {channel}")
        } else {
            channel
        };
        let action = |id: String, label: &'static str, variant: ButtonVariant| {
            Button::new(gpui_kit::ElementId::Name(id.into()))
                .variant(variant)
                .size(ButtonSize::Sm)
                .child(label)
        };
        let (open, send, delete) = (draft.clone(), draft.clone(), draft.clone());
        div()
            .id(gpui_kit::ElementId::Name(format!("drafts.{index}").into()))
            .mx(scaled(&theme, metrics::GUTTER))
            .p(theme.space(4.))
            .rounded(theme.radius.lg)
            .border(style::hairline())
            .border_color(theme.colors.border)
            .bg(theme.colors.card)
            .flex()
            .flex_col()
            .gap(theme.space(3.))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .gap(theme.space(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space(1.5))
                            .child(
                                lucide(if draft.thread_root.is_some() {
                                    LucideIcon::MessageSquareReply
                                } else {
                                    LucideIcon::Hash
                                })
                                .size(scaled(&theme, 14.))
                                .text_color(theme.colors.muted_foreground),
                            )
                            .child(
                                div()
                                    .type_style(style::UI_STRONG, &theme)
                                    .text_color(theme.colors.foreground)
                                    .child(place),
                            ),
                    )
                    .child(
                        div()
                            .type_style(style::META, &theme)
                            .text_color(theme.colors.muted_foreground)
                            .child(draft.updated.clone()),
                    ),
            )
            .child(
                div()
                    .type_style(style::BODY, &theme)
                    .text_color(theme.colors.foreground)
                    .child(markdown(
                        gpui_kit::ElementId::Name(format!("drafts.{index}.body").into()),
                        draft.body.clone(),
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pt(theme.space(1.))
                    .child(
                        action(
                            format!("drafts.{index}.delete"),
                            "Delete",
                            ButtonVariant::Ghost,
                        )
                        .text_color(theme.colors.destructive)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remove(&delete, cx);
                            cx.emit(DraftsEvent::Delete(delete.clone()));
                        })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space(2.))
                            .child(
                                action(
                                    format!("drafts.{index}.open"),
                                    "Open",
                                    ButtonVariant::Outline,
                                )
                                .on_click(cx.listener(
                                    move |_, _, _, cx| cx.emit(DraftsEvent::Open(open.clone())),
                                )),
                            )
                            .child(
                                action(
                                    format!("drafts.{index}.send"),
                                    "Send",
                                    ButtonVariant::Default,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.remove(&send, cx);
                                        cx.emit(DraftsEvent::Send(send.clone()));
                                    },
                                )),
                            ),
                    ),
            )
    }

    fn empty(cx: &App) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let media = div()
            .size(theme.space(10.))
            .rounded_full()
            .bg(theme.colors.muted)
            .flex()
            .items_center()
            .justify_center()
            .child(
                lucide(LucideIcon::FilePenLine)
                    .size(scaled(&theme, metrics::ICON_LARGE))
                    .text_color(theme.colors.muted_foreground),
            );
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .p(theme.space(6.))
            .child(
                Empty::new().header(
                    EmptyHeader::new()
                        .media(EmptyMedia::new().child(media))
                        .title(
                            EmptyTitle::new()
                                .type_style(style::TITLE, &theme)
                                .text_center()
                                .child("No drafts"),
                        )
                        .description(
                            EmptyDescription::new()
                                .type_style(style::META, &theme)
                                .text_center()
                                .child("Anything you type and leave unsent waits here."),
                        ),
                ),
            )
    }
}

impl Render for DraftsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let drafts = self.model.drafts.clone();
        let root = div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(theme.colors.background)
            .child(self.header(cx));
        if drafts.is_empty() {
            return root.child(Self::empty(cx));
        }
        root.child(
            div()
                .id("drafts.list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .py(theme.space(4.))
                .flex()
                .flex_col()
                .gap(theme.space(3.))
                .max_w(px(760.))
                .children(
                    drafts
                        .iter()
                        .enumerate()
                        .map(|(index, draft)| self.card(index, draft, cx)),
                ),
        )
    }
}
