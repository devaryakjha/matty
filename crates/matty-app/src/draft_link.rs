//! The link preview above a composer (#188): the draft's first web link,
//! its card once the server has read it, and the links the user removed
//! from this draft. The channel and thread composers each keep one.

use std::collections::HashSet;

use gpui_kit::{
    App, ClickEvent, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div,
};

use crate::embeds::{LinkCard, first_link};

/// How long typing pauses before the draft's link is asked about.
pub const PAUSE: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Debug, Default)]
pub struct DraftLink {
    link: Option<(SharedString, Option<LinkCard>)>,
    removed: HashSet<SharedString>,
}

impl DraftLink {
    /// Follows the draft's text; a link it has not followed yet comes back
    /// to be asked about after [`PAUSE`].
    pub fn follow(&mut self, text: &str) -> Option<SharedString> {
        let link = first_link(text)
            .map(SharedString::from)
            .filter(|link| !self.removed.contains(link));
        if self.link.as_ref().map(|(url, _)| url) == link.as_ref() {
            return None;
        }
        self.link = link.clone().map(|url| (url, None));
        link
    }

    /// Whether the draft has a link to preview at all.
    pub fn has_link(&self) -> bool {
        self.link.is_some()
    }

    /// What the server read from `url`; true when it is shown.
    pub fn answer(&mut self, url: &str, card: Option<LinkCard>) -> bool {
        match &mut self.link {
            Some((current, shown)) if current.as_ref() == url => {
                *shown = card;
                true
            }
            _ => false,
        }
    }

    /// Remove preview: this draft keeps the link without a card.
    pub fn remove(&mut self) {
        if let Some((url, _)) = self.link.take() {
            self.removed.insert(url);
        }
    }

    /// The draft `body` was sent: whether its post may show a preview, and
    /// a fresh start for the next draft.
    pub fn sent(&mut self, body: &str) -> bool {
        let unfurl = first_link(body).is_none_or(|link| !self.removed.contains(link.as_str()));
        *self = Self::default();
        unfurl
    }

    /// The card above the composer, once there is one.
    pub fn preview(
        &self,
        id: &'static str,
        cx: &App,
        on_remove: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Option<gpui_kit::AnyElement> {
        let theme = crate::ui::theme::UiTheme::read(cx).clone();
        let card = self.link.as_ref()?.1.as_ref()?;
        let mut preview = crate::ui::embed::LinkPreview::new(id, card.title.clone())
            .density(crate::channel::message_density(cx))
            .site_name(card.site_name.clone())
            .on_remove(on_remove);
        if !card.description.is_empty() {
            preview = preview.description(card.description.clone());
        }
        Some(div().pb(theme.space(2.)).child(preview).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(url: &str) -> LinkCard {
        LinkCard {
            url: url.to_owned().into(),
            site_name: "GitHub".into(),
            title: "Repo".into(),
            description: SharedString::default(),
            image: None,
        }
    }

    #[test]
    fn a_link_is_followed_once_and_a_removed_one_stays_removed() {
        let mut link = DraftLink::default();
        let url = "https://github.com/a";
        assert_eq!(
            link.follow("see https://github.com/a").as_deref(),
            Some(url)
        );
        assert_eq!(
            link.follow("see https://github.com/a now"),
            None,
            "same link"
        );
        assert!(link.answer(url, Some(card(url))));
        assert!(!link.answer("https://other.org", None), "not the draft's");
        link.remove();
        assert_eq!(
            link.follow("see https://github.com/a again"),
            None,
            "removed"
        );
        assert!(!link.sent("see https://github.com/a"));
        assert_eq!(
            link.follow("https://github.com/a").as_deref(),
            Some(url),
            "new draft"
        );
        assert!(link.sent("https://github.com/a"));
    }
}
