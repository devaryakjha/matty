//! Matty's emoji catalog for gpuicn's picker (PLAN §3 M1 "Reactions"):
//! the standard set from the `emojis` crate under its shortcodes, grouped
//! the way the picker tabs show them. Custom emoji join once sync loads
//! them from the server.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_icons::LucideIcon;
use gpui_kit::{App, Context, Global, Image, ImageSource, SharedString};

use crate::avatars::EmojiImages;
use crate::settings::SettingsStore;
use crate::ui::emoji_picker::{Emoji, EmojiCategory, EmojiPickerState};
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

/// Shortcodes for the composer's `:` autocomplete, as `(name, glyph)`
/// pairs: the `frequent` ones that match first, in the picker's order,
/// then those starting with `query`, then those containing it.
pub fn suggest(
    query: &str,
    limit: usize,
    frequent: &[SharedString],
) -> Vec<(SharedString, SharedString)> {
    let query = query.to_lowercase();
    if query.len() < 2 {
        return Vec::new();
    }
    let used: Vec<(SharedString, SharedString)> = frequent
        .iter()
        .filter(|name| name.contains(&query))
        .filter_map(|name| Some((name.clone(), glyph(name)?)))
        .collect();
    let mut starts = used.clone();
    let mut contains = Vec::new();
    for emoji in emojis::iter() {
        let Some(name) = emoji.shortcodes().find(|code| code.contains(&query)) else {
            continue;
        };
        if used.iter().any(|(seen, _)| seen == name) {
            continue;
        }
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
    lookup(name).map(|emoji| emoji.as_str().into())
}

/// Mattermost's tones, lightest first: `+1_medium_skin_tone` is 👍🏽.
const TONES: [(emojis::SkinTone, &str); 5] = [
    (emojis::SkinTone::Light, "_light_skin_tone"),
    (emojis::SkinTone::MediumLight, "_medium_light_skin_tone"),
    (emojis::SkinTone::Medium, "_medium_skin_tone"),
    (emojis::SkinTone::MediumDark, "_medium_dark_skin_tone"),
    (emojis::SkinTone::Dark, "_dark_skin_tone"),
];

/// The emoji a Mattermost name stands for, in a skin tone when the name
/// ends in one (the `emojis` crate names only the default tone).
pub fn lookup(name: &str) -> Option<&'static emojis::Emoji> {
    if let Some(emoji) = emojis::get_by_shortcode(name) {
        return Some(emoji);
    }
    TONES.iter().find_map(|(tone, suffix)| {
        emojis::get_by_shortcode(name.strip_suffix(suffix)?)?.with_skin_tone(*tone)
    })
}

/// The Mattermost name for `emoji`, with its skin tone's suffix.
pub fn name_of(emoji: &emojis::Emoji) -> Option<String> {
    let base = emoji
        .with_skin_tone(emojis::SkinTone::Default)
        .unwrap_or(emoji);
    let name = base.shortcode()?;
    match emoji.skin_tone() {
        None | Some(emojis::SkinTone::Default) => Some(name.to_owned()),
        Some(tone) => TONES
            .iter()
            .find(|(known, _)| *known == tone)
            .map(|(_, suffix)| format!("{name}{suffix}")),
    }
}

/// The emoji picked most, for the picker's first row and the top of the
/// `:` autocomplete (#209).
pub fn frequent(cx: &App) -> Vec<SharedString> {
    SettingsStore::global(cx)
        .read(cx)
        .get()
        .emoji_usage
        .iter()
        .take(RECENT_LIMIT)
        .map(|seen| SharedString::from(seen.name.clone()))
        .collect()
}

/// Counts a pick of `name` for [`frequent`], under its default tone.
pub fn record_pick(name: &str, cx: &mut App) {
    let name = base_name(name);
    SettingsStore::global(cx).update(cx, |store, cx| {
        store.update(cx, |settings| settings.count_emoji(&name))
    });
}

/// Keeps the tone the user chose in a picker, for every picker.
pub fn save_skin_tone(tone: Option<usize>, cx: &mut App) {
    SettingsStore::global(cx).update(cx, |store, cx| {
        store.update(cx, |settings| settings.skin_tone = tone)
    });
}

/// Brings a picker up to date as it opens: the saved skin tone and the
/// most used emoji first.
pub fn prepare_picker(picker: &mut EmojiPickerState, cx: &mut Context<EmojiPickerState>) {
    let tone = SettingsStore::global(cx).read(cx).get().skin_tone;
    picker.set_skin_tone(tone, cx);
    picker.set_recents(frequent(cx), cx);
}

/// `name` in the default tone: `+1_medium_skin_tone` counts as `+1`.
fn base_name(name: &str) -> String {
    lookup(name)
        .and_then(|emoji| emoji.with_skin_tone(emojis::SkinTone::Default))
        .and_then(emojis::Emoji::shortcode)
        .unwrap_or(name)
        .to_owned()
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
    // One tone per person only: the picker has five, lightest first.
    let tones: Vec<Emoji> = TONES
        .iter()
        .filter_map(|(tone, suffix)| {
            let toned = emoji.with_skin_tone(*tone)?;
            Some(Emoji::new(format!("{name}{suffix}"), toned.as_str()))
        })
        .collect();
    if tones.len() == TONES.len() {
        out = out.skin_tones(tones);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_count_as_their_default_and_used_emoji_suggest_first() {
        assert_eq!(base_name("+1_medium_skin_tone"), "+1");
        assert_eq!(glyph("+1_dark_skin_tone").as_deref(), Some("👍🏿"));
        assert_eq!(
            emojis::get("👍🏼").and_then(name_of).as_deref(),
            Some("+1_medium_light_skin_tone")
        );
        assert_eq!(emojis::get("👍").and_then(name_of).as_deref(), Some("+1"));
        let hand = categories()
            .into_iter()
            .flat_map(|category| category.emoji)
            .find(|emoji| emoji.name == "wave")
            .unwrap();
        assert_eq!(hand.in_tone(Some(0)).name, "wave_light_skin_tone");
        assert_eq!(base_name("joy"), "joy");
        assert_eq!(base_name("not_an_emoji"), "not_an_emoji");
        let names = |frequent: &[SharedString]| {
            suggest("smil", 3, frequent)
                .into_iter()
                .map(|(name, _)| name.to_string())
                .collect::<Vec<_>>()
        };
        let plain = names(&[]);
        assert!(!plain.contains(&"smiley_cat".to_string()));
        let ranked = names(&["tada".into(), "smiley_cat".into()]);
        assert_eq!(ranked[0], "smiley_cat");
        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked.iter().filter(|n| *n == "smiley_cat").count(), 1);
    }

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
        let names: Vec<_> = suggest("tad", 8, &[]).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names.first().map(|n| n.as_ref()), Some("tada"));
        assert!(
            suggest("t", 8, &[]).is_empty(),
            "one letter is too little to go on"
        );
        assert_eq!(glyph("not-an-emoji"), None);
    }
}
