//! Posts from incoming webhooks (#206 follow-up): they show the webhook's
//! icon (`override_icon_url`) rather than the picture of the person who
//! made the webhook, or a webhook glyph when there is no icon or it cannot
//! load yet.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{App, IntoElement as _, ParentElement as _, SharedString, Styled as _, div};

use crate::style::{metrics, scaled};
use crate::ui::{
    avatar::{Avatar, AvatarSize},
    theme::UiTheme,
};

/// How a webhook's post shows its author.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Webhook {
    /// The icon the webhook posted with, when it gave one.
    pub icon_url: Option<SharedString>,
}

impl Webhook {
    /// The webhook a post came from: `props.from_webhook` is `"true"`,
    /// with its `override_icon_url` when the server kept one.
    pub fn of(post: &matty_api::models::Post) -> Option<Self> {
        let text = |key: &str| post.props.get(key).and_then(serde_json::Value::as_str);
        if text("from_webhook") != Some("true") {
            return None;
        }
        let icon_url = text("override_icon_url")
            .map(str::trim)
            .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
            .map(|url| SharedString::from(url.to_owned()));
        Some(Self { icon_url })
    }
}

/// The avatar of a webhook's post, `id` naming it: its icon once loaded,
/// else the webhook glyph over gpuicn's empty avatar.
pub fn avatar(
    id: SharedString,
    webhook: &Webhook,
    name: &SharedString,
    cx: &App,
) -> gpui_kit::AnyElement {
    let avatar = Avatar::new(id)
        .size(AvatarSize::Default)
        .aria_label(name.clone());
    if let Some(source) = webhook
        .icon_url
        .as_deref()
        .and_then(|url| crate::remote_images::source(url, cx))
    {
        return avatar.image(source).into_any_element();
    }
    let theme = UiTheme::read(cx);
    div()
        .relative()
        .flex_shrink_0()
        .child(avatar)
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    lucide(LucideIcon::Webhook)
                        .size(scaled(theme, metrics::ICON))
                        .text_color(theme.colors.muted_foreground),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::InteractiveElement as _;
    use serde_json::json;

    fn post(props: serde_json::Value) -> matty_api::models::Post {
        serde_json::from_value(json!({"id": "p", "user_id": "creator", "props": props})).unwrap()
    }

    #[test]
    fn a_webhook_post_keeps_its_icon() {
        assert_eq!(Webhook::of(&post(json!({}))), None);
        assert_eq!(
            Webhook::of(&post(json!({"from_webhook": "true"}))),
            Some(Webhook::default())
        );
        assert_eq!(
            Webhook::of(&post(json!({
                "from_webhook": "true",
                "override_icon_url": "https://ci.example.com/bot.png"
            }))),
            Some(Webhook {
                icon_url: Some("https://ci.example.com/bot.png".into())
            })
        );
        // Not a web address: the glyph shows instead.
        assert_eq!(
            Webhook::of(&post(
                json!({"from_webhook": "true", "override_icon_url": " "})
            )),
            Some(Webhook::default())
        );
    }

    struct Host(Webhook);

    impl gpui_kit::Render for Host {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            cx: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            div().size_full().child(
                div()
                    .debug_selector(|| "webhook-avatar".into())
                    .child(avatar("w".into(), &self.0, &"CI".into(), cx)),
            )
        }
    }

    /// Without an icon to show, the glyph's avatar draws in a view.
    #[gpui_kit::test]
    fn a_webhook_avatar_draws(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let (_, cx) = cx.add_window_view(|_, _| Host(Webhook::default()));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("webhook-avatar").is_some());
    }
}
