//! Cards under a post (#187): a link preview from the server's Open Graph
//! data, and the message attachments integrations and bots send. Images in
//! them wait for the remote image policy (#159).

use gpui_kit::{App, IntoElement, ParentElement as _, SharedString, Styled as _, div};
use matty_api::models::{EmbedKind, MessageAttachment, Post};
use serde_json::Value;

use crate::ui::embed::{LinkPreview, MessageEmbed};
use crate::ui::markdown::markdown;

/// One card under a post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Embed {
    /// An Open Graph preview of a link in the message.
    Link(LinkCard),
    /// A message attachment from `props.attachments`.
    Attachment(AttachmentCard),
}

/// A link preview's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkCard {
    pub url: SharedString,
    pub site_name: SharedString,
    pub title: SharedString,
    pub description: SharedString,
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
/// first link gets one, as in the web app.
pub fn from_post(post: &Post) -> Vec<Embed> {
    let attachments = post.attachments();
    if !attachments.is_empty() {
        return attachments
            .iter()
            .map(|attachment| Embed::Attachment(attachment_card(attachment)))
            .collect();
    }
    post.metadata
        .embeds
        .iter()
        .filter(|embed| embed.kind == EmbedKind::OpenGraph)
        .find_map(|embed| {
            let graph = embed.opengraph()?;
            let title = if graph.title.is_empty() {
                embed.url.clone()
            } else {
                graph.title
            };
            Some(Embed::Link(LinkCard {
                url: embed.url.clone().into(),
                site_name: graph.site_name.into(),
                title: title.into(),
                description: graph.description.into(),
            }))
        })
        .into_iter()
        .collect()
}

fn attachment_card(attachment: &MessageAttachment) -> AttachmentCard {
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

/// Only web links open from a card.
fn opens(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// The cards under the post `id`.
pub fn post_embeds(id: &str, embeds: &[Embed], cx: &App) -> impl IntoElement + use<> {
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
                    let mut card = LinkPreview::new(element_id, link.title.clone())
                        .density(density)
                        .site_name(link.site_name.clone());
                    if !link.description.is_empty() {
                        card = card.description(link.description.clone());
                    }
                    if opens(&link.url) {
                        let url = link.url.clone();
                        card = card.on_click(move |_, _, cx| cx.open_url(&url));
                    }
                    card.into_any_element()
                }
                Embed::Attachment(attachment) => {
                    attachment_element(element_id, id, index, attachment, density, cx)
                }
            }
        }))
}

fn attachment_element(
    element_id: gpui_kit::ElementId,
    id: &str,
    index: usize,
    attachment: &AttachmentCard,
    density: crate::ui::embed::MessageDensity,
    cx: &App,
) -> gpui_kit::AnyElement {
    let text_id =
        |part: &str| gpui_kit::ElementId::Name(format!("post.{id}.embed.{index}.{part}").into());
    let mut card = MessageEmbed::new(element_id).density(density);
    if !attachment.color.is_empty() {
        card = card.color_str(&attachment.color);
    }
    if !attachment.pretext.is_empty() {
        card = card.pretext(markdown(text_id("pretext"), attachment.pretext.clone(), cx));
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
    if !attachment.footer.is_empty() {
        card = card.footer(attachment.footer.clone());
    }
    card.into_any_element()
}

#[cfg(test)]
mod tests {
    use matty_api::models::{PostEmbed, PostMetadata};
    use serde_json::json;

    use super::*;

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
    fn only_web_links_open() {
        assert!(opens("https://example.com"));
        assert!(!opens("javascript:alert(1)"));
        assert!(!opens("file:///etc/passwd"));
    }
}
