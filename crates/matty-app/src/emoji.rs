//! Matty's emoji catalog for gpuicn's picker (PLAN §3 M1 "Reactions"):
//! the standard set from the `emojis` crate under its shortcodes, grouped
//! the way the picker tabs show them. Custom emoji join once sync loads
//! them from the server.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_icons::LucideIcon;
use gpui_kit::{App, Global, Image, ImageSource, SharedString};

use crate::avatars::EmojiImages;
use crate::ui::emoji_picker::{Emoji, EmojiCategory};
use crate::ui::message::Reaction;

/// How many recently picked emoji the picker remembers.
pub const RECENT_LIMIT: usize = 27;

/// The picker's categories, in tab order.
pub fn categories() -> Vec<EmojiCategory> {
    use emojis::Group;
    let groups = [
        (
            Group::SmileysAndEmotion,
            "Smileys & Emotion",
            LucideIcon::FaceSlightlySmiling,
        ),
        (Group::PeopleAndBody, "People & Body", LucideIcon::Hand),
        (
            Group::AnimalsAndNature,
            "Animals & Nature",
            LucideIcon::Leaf,
        ),
        (Group::FoodAndDrink, "Food & Drink", LucideIcon::Coffee),
        (Group::TravelAndPlaces, "Travel & Places", LucideIcon::Plane),
        (Group::Activities, "Activities", LucideIcon::Trophy),
        (Group::Objects, "Objects", LucideIcon::Lightbulb),
        (Group::Symbols, "Symbols", LucideIcon::Heart),
        (Group::Flags, "Flags", LucideIcon::Flag),
    ];
    groups
        .into_iter()
        .map(|(group, name, icon)| {
            EmojiCategory::new(name, icon, group.emojis().filter_map(convert))
        })
        .collect()
}

/// Shortcodes for the composer's `:` autocomplete: those starting with
/// `query` first, then those containing it, as `(name, glyph)` pairs.
pub fn suggest(query: &str, limit: usize) -> Vec<(SharedString, SharedString)> {
    let query = query.to_lowercase();
    if query.len() < 2 {
        return Vec::new();
    }
    let mut starts = Vec::new();
    let mut contains = Vec::new();
    for emoji in emojis::iter() {
        let Some(name) = emoji.shortcodes().find(|code| code.contains(&query)) else {
            continue;
        };
        let pair = (
            SharedString::from(name.to_string()),
            SharedString::from(emoji.as_str()),
        );
        if name.starts_with(&query) {
            starts.push(pair);
        } else {
            contains.push(pair);
        }
        if starts.len() >= limit {
            break;
        }
    }
    starts.extend(contains);
    starts.truncate(limit);
    starts
}

/// The glyph behind a shortcode, for reactions and the composer.
pub fn glyph(name: &str) -> Option<SharedString> {
    emojis::get_by_shortcode(name).map(|emoji| emoji.as_str().into())
}

/// The server's custom emoji, name to id (#153), as an app global the
/// views read while rendering. [`crate::live::Live`] fills it from sync.
#[derive(Clone, Default)]
pub struct CustomEmoji(Rc<RefCell<HashMap<String, String>>>);

impl Global for CustomEmoji {}

impl CustomEmoji {
    /// Replaces the list with `emoji`.
    pub fn set(&self, emoji: &[matty_api::models::Emoji]) {
        *self.0.borrow_mut() = emoji
            .iter()
            .map(|one| (one.name.clone(), one.id.clone()))
            .collect();
    }

    /// The id of the custom emoji called `name`.
    pub fn id(&self, name: &str) -> Option<String> {
        self.0.borrow().get(name).cloned()
    }

    /// Whether `name` is one of the server's custom emoji.
    pub fn contains(&self, name: &str) -> bool {
        self.0.borrow().contains_key(name)
    }
}

/// The image for the custom emoji called `name`, asking sync for it the
/// first time. `None` until it lands, and for names that are not custom
/// emoji, so the view shows `:name:` meanwhile.
pub fn custom_image(name: &str, cx: &App) -> Option<Arc<Image>> {
    let id = cx.try_global::<CustomEmoji>()?.id(name)?;
    cx.try_global::<EmojiImages>()?.image(&id)
}

/// [`custom_image`] as a source for gpuicn's views.
pub fn custom_source(name: &str, cx: &App) -> Option<ImageSource> {
    custom_image(name, cx).map(ImageSource::Image)
}

/// A reaction chip for `emoji` (a glyph, or `:name:` for a custom emoji),
/// with the custom emoji's image once it has one.
pub fn reaction(emoji: &SharedString, count: usize, mine: bool, cx: &App) -> Reaction {
    let chip = Reaction::new(emoji.clone(), count, mine);
    let custom = emoji
        .strip_prefix(':')
        .and_then(|name| name.strip_suffix(':'))
        .and_then(|name| custom_image(name, cx));
    match custom {
        Some(image) => chip.image(image),
        None => chip,
    }
}

fn convert(emoji: &'static emojis::Emoji) -> Option<Emoji> {
    let name = emoji.shortcode()?;
    let keywords = emoji
        .shortcodes()
        .skip(1)
        .map(str::to_string)
        .chain(std::iter::once(emoji.name().to_string()));
    let mut out = Emoji::new(name, emoji.as_str()).keywords(keywords);
    if let Some(tones) = emoji.skin_tones() {
        // The crate lists the default tone first; the picker wants only
        // the variants, lightest first.
        out = out.skin_tones(
            tones
                .skip(1)
                .filter_map(|tone| Some(Emoji::new(tone.shortcode()?, tone.as_str()))),
        );
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_the_quick_reactions() {
        let categories = categories();
        assert_eq!(categories.len(), 9);
        let names: Vec<String> = categories
            .iter()
            .flat_map(|category| category.emoji.iter().map(|emoji| emoji.name.to_string()))
            .collect();
        for name in ["+1", "heart", "tada", "eyes", "rocket", "joy"] {
            assert!(names.iter().any(|n| n == name), "missing {name}");
        }
        assert_eq!(glyph("tada").as_deref(), Some("🎉"));
        let names: Vec<_> = suggest("tad", 8).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names.first().map(|n| n.as_ref()), Some("tada"));
        assert!(
            suggest("t", 8).is_empty(),
            "one letter is too little to go on"
        );
        assert_eq!(glyph("not-an-emoji"), None);
    }
}
