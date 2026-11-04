//! Whether a message mentions the user (PLAN §4.6): `@username`, the first
//! name when the user turned that on, `@channel`, `@all` and `@here` when
//! the user and the channel allow them, and the user's own keywords. The
//! transcript's mention highlight and the notification policy both use it,
//! so there is one implementation.

use std::collections::HashMap;

/// What counts as a mention of one user.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MentionKeys {
    /// The username, without the `@`.
    pub username: String,
    /// The first name, matched as a word, when the user turned it on.
    pub first_name: Option<String>,
    /// Whether `@channel`, `@all` and `@here` count.
    pub channel_wide: bool,
    /// The user's own keywords, matched as words, ignoring case. A keyword
    /// that starts with `@` matches only with it.
    pub keywords: Vec<String>,
}

impl MentionKeys {
    /// Only `@username`, as the transcript needs before preferences load.
    pub fn username(username: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            ..Self::default()
        }
    }

    /// The keys from the user's `notify_props`: `first_name`, `channel`
    /// and the comma-separated `mention_keys`.
    pub fn from_props(username: &str, first_name: &str, props: &HashMap<String, String>) -> Self {
        let on = |key: &str| props.get(key).is_some_and(|value| value == "true");
        let keywords = props
            .get("mention_keys")
            .map(|keys| {
                keys.split(',')
                    .map(str::trim)
                    .filter(|key| !key.is_empty())
                    // The username is always a key; Mattermost stores it in
                    // the list too.
                    .filter(|key| {
                        !key.eq_ignore_ascii_case(username)
                            && !key
                                .strip_prefix('@')
                                .is_some_and(|key| key.eq_ignore_ascii_case(username))
                    })
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            username: username.to_owned(),
            first_name: (on("first_name") && !first_name.is_empty()).then(|| first_name.to_owned()),
            channel_wide: props.get("channel").is_none_or(|value| value == "true"),
            keywords,
        }
    }

    /// Whether `message` mentions the user directly or by a keyword.
    pub fn mentioned(&self, message: &str) -> bool {
        (!self.username.is_empty() && has_word(message, &format!("@{}", self.username)))
            || self
                .first_name
                .as_deref()
                .is_some_and(|name| has_word(message, name))
            || self.keywords.iter().any(|key| has_word(message, key))
    }

    /// Whether `message` uses `@channel`, `@all` or `@here` and the user
    /// takes those.
    pub fn channel_mentioned(&self, message: &str) -> bool {
        self.channel_wide
            && ["@channel", "@all", "@here"]
                .iter()
                .any(|key| has_word(message, key))
    }
}

/// Characters that continue a word or a username.
fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `key` in `message` as a whole word, ignoring case. A `.` or `-` right
/// after it ends the word only when no more word follows, so `@arya.` and
/// `@arya-` at the end of a sentence count but `@arya.jha` does not.
pub fn has_word(message: &str, key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    let haystack = message.to_lowercase();
    let needle = key.to_lowercase();
    haystack.match_indices(&needle).any(|(at, _)| {
        let before = haystack[..at].chars().next_back();
        // `mail@arya` is no mention, and `@arya` is not the keyword `arya`.
        let starts = !before.is_some_and(|c| word_char(c) || c == '@');
        let mut rest = haystack[at + needle.len()..].chars();
        let ends = match rest.next() {
            None => true,
            Some(c) if word_char(c) => false,
            Some('.' | '-') => !rest.next().is_some_and(word_char),
            Some(_) => true,
        };
        starts && ends
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn at_username_is_a_word() {
        let keys = MentionKeys::username("arya");
        assert!(keys.mentioned("hey @arya!"));
        assert!(keys.mentioned("@arya"));
        assert!(keys.mentioned("thanks @Arya."));
        assert!(keys.mentioned("(@arya)"));
        assert!(!keys.mentioned("hey @arya.jha"));
        assert!(!keys.mentioned("hey @aryaj"));
        assert!(!keys.mentioned("hey @arya_j"));
        assert!(!keys.mentioned("hey arya"));
    }

    #[test]
    fn keywords_ignore_case_and_need_word_edges() {
        let keys = MentionKeys::from_props(
            "arya",
            "Arya",
            &props(&[("mention_keys", "arya,@arya,Deploy, release train ")]),
        );
        assert_eq!(keys.keywords, vec!["Deploy", "release train"]);
        assert!(keys.mentioned("the deploy failed"));
        assert!(keys.mentioned("DEPLOY!"));
        assert!(keys.mentioned("on the release train today"));
        assert!(!keys.mentioned("redeploy it"));
        assert!(!keys.mentioned("deployment"));
        // The username in the list does not match without the @.
        assert!(!keys.mentioned("arya said"));
    }

    #[test]
    fn first_name_only_when_turned_on() {
        let off = MentionKeys::from_props("arya", "Arya", &props(&[]));
        assert!(!off.mentioned("Arya, look"));
        let on = MentionKeys::from_props("arya", "Arya", &props(&[("first_name", "true")]));
        assert!(on.mentioned("arya, look"));
        assert!(!on.mentioned("aryan, look"));
        assert!(!on.mentioned("mail@arya"));
    }

    #[test]
    fn channel_wide_mentions_follow_the_preference() {
        let on = MentionKeys::from_props("arya", "", &props(&[]));
        assert!(on.channel_mentioned("@here standup"));
        assert!(on.channel_mentioned("ping @all."));
        assert!(!on.channel_mentioned("@channels"));
        let off = MentionKeys::from_props("arya", "", &props(&[("channel", "false")]));
        assert!(!off.channel_mentioned("@channel"));
    }
}
