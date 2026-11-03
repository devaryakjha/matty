//! Matty's emoji catalog for gpuicn's picker (PLAN §3 M1 "Reactions"):
//! the standard set from the `emojis` crate under its shortcodes, grouped
//! the way the picker tabs show them. Custom emoji join once sync loads
//! them from the server.

use gpui_icons::LucideIcon;
use gpui_kit::SharedString;

use crate::ui::emoji_picker::{Emoji, EmojiCategory};

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
