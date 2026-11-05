//! Spell checking for the composer (#224): which words of a draft to check,
//! and the platform's checker to check them with. macOS asks
//! `NSSpellChecker`, which follows the user's language settings; Linux
//! reads the Hunspell dictionary for the user's locale.
//!
//! A draft is Markdown, so code, links, mentions, channel links and emoji
//! shortcodes are never checked.

use std::ops::Range;

#[cfg(target_os = "macos")]
mod macos;

/// A spell checker: the platform's, or one built from a Hunspell
/// dictionary.
pub struct Speller {
    backend: Backend,
    /// Words the user chose to ignore this session, lowercased.
    ignored: std::collections::HashSet<String>,
}

enum Backend {
    Hunspell(Box<spellbook::Dictionary>),
    #[cfg(target_os = "macos")]
    MacOs(macos::Checker),
}

/// Why a dictionary could not be read.
#[derive(Debug)]
pub struct DictionaryError(String);

impl std::fmt::Display for DictionaryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unreadable dictionary: {}", self.0)
    }
}

impl std::error::Error for DictionaryError {}

impl Speller {
    /// The system's checker, or `None` where it has none: on Linux, when no
    /// Hunspell dictionary for the user's language is installed.
    pub fn system() -> Option<Self> {
        #[cfg(target_os = "macos")]
        {
            Some(Self::with(Backend::MacOs(macos::Checker::new())))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let languages = locale_languages(|name| std::env::var(name).ok());
            let (aff, dic) = find_dictionary(&languages, &dictionary_dirs())?;
            Self::from_hunspell(&aff, &dic).ok()
        }
    }

    /// A checker for a Hunspell dictionary's `.aff` and `.dic` text.
    pub fn from_hunspell(aff: &str, dic: &str) -> Result<Self, DictionaryError> {
        let dictionary =
            spellbook::Dictionary::new(aff, dic).map_err(|e| DictionaryError(e.to_string()))?;
        Ok(Self::with(Backend::Hunspell(Box::new(dictionary))))
    }

    fn with(backend: Backend) -> Self {
        Self {
            backend,
            ignored: Default::default(),
        }
    }

    /// Whether `word` is spelled right, or ignored.
    pub fn check(&self, word: &str) -> bool {
        if self.ignored.contains(&word.to_lowercase()) {
            return true;
        }
        match &self.backend {
            Backend::Hunspell(dictionary) => dictionary.check(word),
            #[cfg(target_os = "macos")]
            Backend::MacOs(checker) => checker.check(word),
        }
    }

    /// The byte ranges of the misspelled words in a Markdown draft.
    pub fn misspelled(&self, text: &str) -> Vec<Range<usize>> {
        words(text)
            .filter(|range| !self.check(&text[range.clone()]))
            .collect()
    }

    /// Corrections for `word`, best first, at most `limit`.
    pub fn suggestions(&self, word: &str, limit: usize) -> Vec<String> {
        let mut out = match &self.backend {
            Backend::Hunspell(dictionary) => {
                let mut out = Vec::new();
                dictionary.suggest(word, &mut out);
                out
            }
            #[cfg(target_os = "macos")]
            Backend::MacOs(checker) => checker.suggestions(word),
        };
        out.truncate(limit);
        out
    }

    /// Stops flagging `word` until the app quits.
    pub fn ignore(&mut self, word: &str) {
        self.ignored.insert(word.to_lowercase());
    }
}

/// The byte ranges of the words in a Markdown draft that are worth
/// checking. Left out: code spans and fenced code, links and bare URLs,
/// `@mentions`, `~channel` links, `:emoji:` shortcodes, and words with
/// digits, inner capitals (`camelCase`) or one letter.
pub fn words(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let skipped = skipped_spans(text);
    let mut spans = Vec::new();
    let mut start: Option<usize> = None;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (index, &(at, ch)) in chars.iter().enumerate() {
        let next = chars.get(index + 1).map(|&(_, next)| next);
        let apostrophe = matches!(ch, '\'' | '\u{2019}');
        let inside = ch.is_alphanumeric()
            // An apostrophe joins letters: don't, it's.
            || (apostrophe
                && start.is_some()
                && next.is_some_and(char::is_alphabetic));
        match (inside, start) {
            (true, None) => start = Some(at),
            (false, Some(from)) => {
                spans.push(from..at);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        spans.push(from..text.len());
    }
    spans.into_iter().filter(move |range| {
        let word = &text[range.clone()];
        let before = text[..range.start].chars().next_back();
        let after = text[range.end..].chars().next();
        word.chars().count() > 1
            && !word.chars().any(|ch| ch.is_numeric())
            && !word.chars().skip(1).any(char::is_uppercase)
            && !matches!(before, Some('@' | '~' | ':' | '/' | '.' | '_' | '#'))
            && !matches!(after, Some(':' | '/' | '_' | '@'))
            && !skipped
                .iter()
                .any(|skip| skip.start <= range.start && range.end <= skip.end)
    })
}

/// Code, link targets and URLs: spans whose words are never checked.
fn skipped_spans(text: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    // Fenced code, from a ``` line to the next or the end.
    let mut offset = 0;
    let mut fence: Option<usize> = None;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            match fence.take() {
                Some(start) => spans.push(start..offset + line.len()),
                None => fence = Some(offset),
            }
        }
        offset += line.len();
    }
    if let Some(start) = fence {
        spans.push(start..text.len());
    }
    // Code spans, between matching runs of backticks.
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'`' {
            at += 1;
            continue;
        }
        let run = bytes[at..].iter().take_while(|&&b| b == b'`').count();
        let ticks = &text[at..at + run];
        match text[at + run..].find(ticks) {
            Some(close) => {
                spans.push(at..at + run + close + run);
                at += run + close + run;
            }
            None => at += run,
        }
    }
    // Link targets: the `(...)` after `](`.
    let mut from = 0;
    while let Some(found) = text[from..].find("](") {
        let start = from + found + 2;
        let end = text[start..]
            .find(')')
            .map_or(text.len(), |close| start + close);
        spans.push(start..end);
        from = end;
    }
    // Bare URLs, paths, mentions and channel links, up to the next space.
    for (start, token) in tokens(text) {
        // Mentions and channel links run to the end of the token, hyphens
        // and dots included (`~town-square`, `@ana.lee`).
        if token.contains("://")
            || token.starts_with("www.")
            // A link's target is skipped above; its text is still prose.
            || (token.contains('/') && !token.contains("]("))
            || token.starts_with(['@', '~'])
        {
            spans.push(start..start + token.len());
        }
    }
    spans
}

/// Whitespace-separated tokens with their byte offsets.
fn tokens(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (at, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(from) = start.take() {
                out.push((from, &text[from..at]));
            }
        } else if start.is_none() {
            start = Some(at);
        }
    }
    if let Some(from) = start {
        out.push((from, &text[from..]));
    }
    out
}

/// The languages to look for, best first, from the POSIX locale variables:
/// `de_DE.UTF-8` gives `de_DE`, then `de`; English is the last resort.
pub fn locale_languages(var: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let mut languages = Vec::new();
    for name in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        let Some(value) = var(name).filter(|value| !value.is_empty()) else {
            continue;
        };
        let tag = value.split(['.', '@']).next().unwrap_or_default();
        if tag == "C" || tag == "POSIX" || tag.is_empty() {
            continue;
        }
        let tag = tag.replace('-', "_");
        let base = tag.split('_').next().unwrap_or_default().to_string();
        for candidate in [tag, base] {
            if !languages.contains(&candidate) {
                languages.push(candidate);
            }
        }
        break;
    }
    for fallback in ["en_US", "en"] {
        if !languages.iter().any(|language| language == fallback) {
            languages.push(fallback.to_string());
        }
    }
    languages
}

/// Where Hunspell dictionaries live on Linux.
#[cfg(not(target_os = "macos"))]
fn dictionary_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".local/share"))
        })
        .map(|data| vec![data.join("hunspell")])
        .unwrap_or_default();
    dirs.extend(
        [
            "/usr/share/hunspell",
            "/usr/share/myspell/dicts",
            "/usr/share/myspell",
        ]
        .map(std::path::PathBuf::from),
    );
    dirs
}

/// The first dictionary for `languages` in `dirs`: its `.aff` and `.dic`
/// text. A bare language (`de`) also matches a regional one (`de_DE`).
pub fn find_dictionary(
    languages: &[String],
    dirs: &[std::path::PathBuf],
) -> Option<(String, String)> {
    let read = |path: std::path::PathBuf| -> Option<(String, String)> {
        let aff = std::fs::read_to_string(path.with_extension("aff")).ok()?;
        let dic = std::fs::read_to_string(path.with_extension("dic")).ok()?;
        Some((aff, dic))
    };
    for language in languages {
        for dir in dirs {
            if let Some(found) = read(dir.join(language)) {
                return Some(found);
            }
            if !language.contains('_') {
                let mut regional: Vec<_> = std::fs::read_dir(dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.extension().is_some_and(|ext| ext == "dic")
                            && path
                                .file_stem()
                                .and_then(|stem| stem.to_str())
                                .is_some_and(|stem| stem.starts_with(&format!("{language}_")))
                    })
                    .collect();
                regional.sort();
                if let Some(found) = regional.into_iter().find_map(read) {
                    return Some(found);
                }
            }
        }
    }
    None
}
