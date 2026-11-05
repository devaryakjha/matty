//! `NSSpellChecker`, which checks in the languages the user set in System
//! Settings and learns the words they taught it.

use objc2_app_kit::NSSpellChecker;
use objc2_foundation::{NSRange, NSString};

pub(crate) struct Checker {
    /// A document tag of Matty's own, so words ignored here stay here.
    tag: isize,
}

impl Checker {
    pub(crate) fn new() -> Self {
        Self {
            tag: NSSpellChecker::uniqueSpellDocumentTag(),
        }
    }

    pub(crate) fn check(&self, word: &str) -> bool {
        let checker = NSSpellChecker::sharedSpellChecker();
        let word = NSString::from_str(word);
        // `NSNotFound` (as a location past the end) means nothing is wrong.
        let range = checker.checkSpellingOfString_startingAt(&word, 0);
        range.length == 0
    }

    pub(crate) fn suggestions(&self, word: &str) -> Vec<String> {
        let checker = NSSpellChecker::sharedSpellChecker();
        let string = NSString::from_str(word);
        let range = NSRange::new(0, string.length());
        checker
            .guessesForWordRange_inString_language_inSpellDocumentWithTag(
                range, &string, None, self.tag,
            )
            .map(|guesses| guesses.iter().map(|guess| guess.to_string()).collect())
            .unwrap_or_default()
    }
}
