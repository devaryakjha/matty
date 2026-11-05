//! The word splitting and a Hunspell dictionary of the tests' own, so the
//! results do not depend on what the machine has installed.
#![allow(clippy::unwrap_used)]

use matty_spell::{Speller, find_dictionary, locale_languages, words};

fn speller() -> Speller {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/en_test");
    let aff = std::fs::read_to_string(format!("{dir}.aff")).unwrap();
    let dic = std::fs::read_to_string(format!("{dir}.dic")).unwrap();
    Speller::from_hunspell(&aff, &dic).unwrap()
}

fn checked(text: &str) -> Vec<&str> {
    words(text).map(|range| &text[range]).collect()
}

#[test]
fn markdown_that_is_not_prose_is_never_checked() {
    assert_eq!(checked("Hello world"), ["Hello", "world"]);
    // Apostrophes join; one letter, digits and camelCase are left alone.
    assert_eq!(checked("don't a v2 parseJson ok"), ["don't", "ok"]);
    // Mentions, channels, emoji and URLs.
    assert_eq!(
        checked("@arya see ~town-square :tada: https://example.com/x then"),
        ["see", "then"]
    );
    // Code spans, fences, link targets (the text stays) and paths.
    assert_eq!(
        checked("run `cargo tset` and [the doc](docs/plna.md) in src/mian.rs"),
        ["run", "and", "the", "doc", "in"]
    );
    assert_eq!(
        checked("before\n```rust\nlet speling = 1;\n```\nafter"),
        ["before", "after"]
    );
    // An unclosed fence runs to the end.
    assert_eq!(checked("look\n```\nwhat teh"), ["look"]);
    // Byte offsets stay right after multi-byte text.
    let text = "naïve — wrold";
    let ranges: Vec<_> = words(text).collect();
    assert_eq!(&text[ranges[1].clone()], "wrold");
}

#[test]
fn misspelled_words_are_found_and_corrected() {
    let mut speller = speller();
    let text = "The relase is out todya, `chekc` this";
    let found: Vec<&str> = speller
        .misspelled(text)
        .into_iter()
        .map(|range| &text[range])
        .collect();
    // `this` is not in the test dictionary either.
    assert_eq!(found, ["relase", "todya", "this"]);
    assert_eq!(speller.suggestions("relase", 3)[0], "release");
    assert!(speller.suggestions("wrold", 1).len() <= 1);
    // Suffixes from the affix file, and a word the user ignored.
    assert!(speller.check("meetings"));
    speller.ignore("This");
    assert!(speller.check("this"));
}

#[test]
fn the_dictionary_follows_the_locale() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    };
    assert_eq!(
        locale_languages(env(&[("LANG", "de_DE.UTF-8")])),
        ["de_DE", "de", "en_US", "en"]
    );
    // LC_ALL wins; C and POSIX say nothing.
    assert_eq!(
        locale_languages(env(&[("LC_ALL", "fr_CA"), ("LANG", "de_DE.UTF-8")])),
        ["fr_CA", "fr", "en_US", "en"]
    );
    assert_eq!(
        locale_languages(env(&[("LANG", "C.UTF-8")])),
        ["en_US", "en"]
    );

    // A bare language finds a regional dictionary.
    let dir = std::env::temp_dir().join(format!("matty-spell-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("de_AT.aff"), "SET UTF-8\n").unwrap();
    std::fs::write(dir.join("de_AT.dic"), "1\nHallo\n").unwrap();
    let found = find_dictionary(&["de".to_string()], std::slice::from_ref(&dir));
    assert_eq!(found.map(|(_, dic)| dic), Some("1\nHallo\n".to_string()));
    assert_eq!(
        find_dictionary(&["fr".to_string()], std::slice::from_ref(&dir)),
        None
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
