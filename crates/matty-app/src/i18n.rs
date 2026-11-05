//! Localization (#225): the UI's words come from Fluent files under
//! `locales/<tag>/matty.ftl`, looked up by id with [`t!`](crate::t).
//! English (`locales/en`) is the source; another language may leave ids
//! out and English stands in for them.
//!
//! The locale is the `MATTY_LOCALE` environment variable when set, else the
//! system's. `en-XA` is a pseudo-locale: English with every letter accented
//! and each string bracketed, so text that skipped [`t!`](crate::t) stands
//! out on screen. A test also fails on hard-coded strings in the files
//! listed in [`LOCALIZED`].

use std::cell::RefCell;

use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use gpui_kit::SharedString;
use unic_langid::LanguageIdentifier;

/// The English source, built in.
const ENGLISH: &str = include_str!("../locales/en/matty.ftl");

/// The pseudo-locale's tag.
pub const PSEUDO: &str = "en-XA";

/// Files whose UI strings all go through [`t!`](crate::t). An area joins
/// the list once its strings are extracted; the test below keeps it so.
pub const LOCALIZED: &[&str] = &[
    "account.rs",
    "bookmarks.rs",
    "custom_status.rs",
    "dnd.rs",
    "emoji_manager.rs",
    "format_bar.rs",
    "login.rs",
    "profile.rs",
    "profile_editor.rs",
];

struct Strings {
    english: FluentBundle<FluentResource>,
    pseudo: bool,
}

thread_local! {
    static STRINGS: RefCell<Option<Strings>> = const { RefCell::new(None) };
}

fn bundle(tag: &str, source: &str) -> FluentBundle<FluentResource> {
    let lang: LanguageIdentifier = tag.parse().unwrap_or_default();
    let mut bundle = FluentBundle::new(vec![lang]);
    // No isolation marks around values: the fonts draw them as boxes. Bidi
    // isolation comes with right-to-left support.
    bundle.set_use_isolating(false);
    let resource = match FluentResource::try_new(source.to_owned()) {
        Ok(resource) => resource,
        Err((resource, errors)) => {
            tracing::error!(?errors, tag, "locale file has errors");
            resource
        }
    };
    if let Err(errors) = bundle.add_resource(resource) {
        tracing::error!(?errors, tag, "locale file repeats ids");
    }
    bundle
}

fn strings(pseudo: bool) -> Strings {
    Strings {
        english: bundle("en", ENGLISH),
        pseudo,
    }
}

/// The locale asked for: `MATTY_LOCALE`, else the system's.
fn requested() -> Option<String> {
    if let Ok(tag) = std::env::var("MATTY_LOCALE")
        && !tag.is_empty()
    {
        return Some(tag);
    }
    #[cfg(not(target_family = "wasm"))]
    {
        sys_locale::get_locale()
    }
    #[cfg(target_family = "wasm")]
    {
        web_sys::window().and_then(|window| window.navigator().language())
    }
}

/// The locale Matty shows for `requested`: the pseudo-locale when asked
/// for, else English, the only language so far.
pub fn negotiate(requested: Option<&str>) -> &'static str {
    match requested {
        Some(tag) if tag.eq_ignore_ascii_case(PSEUDO) => PSEUDO,
        _ => "en",
    }
}

/// Picks the locale at launch, before any window opens.
pub fn init() -> &'static str {
    let requested = requested();
    let locale = negotiate(requested.as_deref());
    tracing::info!(requested = ?requested, locale, "locale");
    STRINGS.with(|cell| *cell.borrow_mut() = Some(strings(locale == PSEUDO)));
    locale
}

/// The text for `id` with `args`, in the current locale. A missing id
/// shows as itself, so it is easy to spot.
pub fn text(id: &str, args: &[(&str, FluentValue<'_>)]) -> SharedString {
    STRINGS.with(|cell| {
        let mut cell = cell.borrow_mut();
        let strings = cell.get_or_insert_with(|| strings(false));
        let Some(pattern) = strings
            .english
            .get_message(id)
            .and_then(|message| message.value())
        else {
            tracing::warn!(id, "no string for this id");
            return SharedString::from(id.to_owned());
        };
        let args = (!args.is_empty()).then(|| {
            let mut fluent = FluentArgs::new();
            for (name, value) in args {
                fluent.set(*name, value.clone());
            }
            fluent
        });
        let mut errors = Vec::new();
        let formatted = strings
            .english
            .format_pattern(pattern, args.as_ref(), &mut errors);
        if !errors.is_empty() {
            tracing::warn!(id, ?errors, "string did not format cleanly");
        }
        if strings.pseudo {
            pseudo(&formatted).into()
        } else {
            SharedString::from(formatted.into_owned())
        }
    })
}

/// A value for [`t!`](crate::t)'s named arguments.
pub fn arg<'a>(value: impl Into<FluentValue<'a>>) -> FluentValue<'a> {
    value.into()
}

/// `text` in the pseudo-locale: accented letters, a third longer, in
/// brackets, so clipping and untranslated text both show.
pub fn pseudo(text: &str) -> String {
    let accented: String = text
        .chars()
        .map(|c| match c {
            'a' => 'á',
            'e' => 'é',
            'i' => 'í',
            'o' => 'ö',
            'u' => 'ü',
            'c' => 'ç',
            'n' => 'ñ',
            'y' => 'ý',
            'A' => 'Å',
            'E' => 'É',
            'I' => 'Î',
            'O' => 'Ö',
            'U' => 'Û',
            'C' => 'Ç',
            'N' => 'Ñ',
            other => other,
        })
        .collect();
    let padding = "·".repeat(text.chars().count() / 3);
    format!("[{accented}{padding}]")
}

/// The UI text for a Fluent id, in the current locale:
/// `t!("emoji-manager-done")`, or with named values,
/// `t!("emoji-manager-listed", count = 3)`.
#[macro_export]
macro_rules! t {
    ($id:literal) => {
        $crate::i18n::text($id, &[])
    };
    ($id:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::text($id, &[$((stringify!($name), $crate::i18n::arg($value))),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_formats_values_and_pseudo_marks_everything() {
        assert_eq!(t!("emoji-manager-done"), "Done");
        assert_eq!(t!("emoji-manager-listed", count = 3), "On this server (3)");
        assert_eq!(
            t!("emoji-manager-name-taken", name = "shipit"),
            "There's already an emoji called :shipit:."
        );
        assert_eq!(text("no-such-id", &[]), "no-such-id");
        assert_eq!(pseudo("Done"), "[Döñé·]");
        assert_eq!(negotiate(Some("en-XA")), PSEUDO);
        assert_eq!(negotiate(Some("de-DE")), "en");
        assert_eq!(negotiate(None), "en");
    }

    /// Every `t!` id in the source has an English string, and the English
    /// file parses with no errors.
    #[test]
    fn every_id_has_english() {
        let resource = FluentResource::try_new(ENGLISH.to_owned())
            .unwrap_or_else(|(_, errors)| panic!("matty.ftl: {errors:?}"));
        let mut english = FluentBundle::new(vec![LanguageIdentifier::default()]);
        english.add_resource(resource).expect("no repeated ids");
        let id = regex::Regex::new(r#"(?:^|[^\w])t!\(\s*"([a-z0-9-]+)""#).unwrap();
        for (file, source) in sources() {
            for found in id.captures_iter(&source) {
                assert!(
                    english.has_message(&found[1]),
                    "{file}: no English string for {}",
                    &found[1]
                );
            }
        }
    }

    /// The files in [`LOCALIZED`] have no UI text outside `t!`: a string
    /// literal with a word in it, outside tests, logs and element ids.
    #[test]
    fn localized_files_have_no_hard_coded_strings() {
        let literal = regex::Regex::new(r#""((?:[^"\\]|\\.)*)""#).unwrap();
        let lookup = regex::Regex::new(r"(?:^|[^\w])t!\(").unwrap();
        let wordy = regex::Regex::new(r"[A-Za-z]{2,}[ ,!?…]|^[A-Z][a-z]").unwrap();
        let mut found = Vec::new();
        for (file, source) in sources() {
            if !LOCALIZED.contains(&file.as_str()) {
                continue;
            }
            let code = source.split("#[cfg(test)]").next().unwrap_or_default();
            for (number, line) in code.lines().enumerate() {
                let trimmed = line.trim_start();
                // Names, keys and log text are not UI text; a line that
                // shows a name as is says so with `i18n-ignore`.
                let skipped = trimmed.starts_with("//")
                    || trimmed.starts_with("#[")
                    || lookup.is_match(line)
                    || line.contains("tracing::")
                    || line.contains("i18n-ignore")
                    || line.contains("debug_struct(")
                    || trimmed.starts_with(".field(");
                if skipped {
                    continue;
                }
                for text in literal.captures_iter(line) {
                    if wordy.is_match(&text[1]) {
                        found.push(format!("{file}:{}: {}", number + 1, &text[1]));
                    }
                }
            }
        }
        assert!(
            found.is_empty(),
            "hard-coded UI strings:\n{}",
            found.join("\n")
        );
    }

    fn sources() -> Vec<(String, String)> {
        let mut files = Vec::new();
        let mut dirs = vec![std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src"
        ))];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let name = path.file_name().unwrap().to_string_lossy().into_owned();
                    files.push((name, std::fs::read_to_string(&path).unwrap()));
                }
            }
        }
        files
    }
}
