//! Whether a message mentions the user (PLAN §4.6): `@username`, the first
//! name when the user turned that on, `@channel`, `@all` and `@here` when
//! the user and the channel allow them, and the user's own keywords. The
//! transcript's mention highlight and the notification policy both use it,
//! so there is one implementation.
//!
//! Matching follows the Mattermost server's mention parser, which decides
//! the webapp's notifications: keys ignore case, code spans and code
//! blocks never mention anyone, and a keyword counts inside a URL or a
//! dotted, dashed or colon-joined word (`deploy` in `deploy-bot` or
//! `ci.example.com/deploy`) but not inside a longer word (`redeploy`).
//! `:deploy:` is an emoji, not the keyword.

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
    /// Keywords that highlight a post without notifying
    /// (`highlight_keys`, Mattermost 9.3 and later on a licensed server).
    pub highlight: Vec<String>,
}

/// One of the keyword lists in `notify_props` that Preferences edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeywordList {
    /// `mention_keys`: keywords that notify like a mention.
    Mention,
    /// `highlight_keys`: keywords that only highlight.
    Highlight,
}

impl KeywordList {
    /// The `notify_props` key that holds the list.
    pub fn prop(self) -> &'static str {
        match self {
            Self::Mention => "mention_keys",
            Self::Highlight => "highlight_keys",
        }
    }
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
        Self {
            username: username.to_owned(),
            first_name: (on("first_name") && !first_name.is_empty()).then(|| first_name.to_owned()),
            channel_wide: props.get("channel").is_none_or(|value| value == "true"),
            keywords: keywords(username, props, KeywordList::Mention),
            highlight: keywords(username, props, KeywordList::Highlight),
        }
    }

    /// Whether `message` mentions the user directly or by a keyword.
    pub fn mentioned(&self, message: &str) -> bool {
        let message = &without_code(message);
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
        let message = &without_code(message);
        self.channel_wide
            && ["@channel", "@all", "@here"]
                .iter()
                .any(|key| has_word(message, key))
    }

    /// Whether `message` has one of the highlight-only keywords. These
    /// mark the post but never notify.
    pub fn highlighted(&self, message: &str) -> bool {
        let message = &without_code(message);
        self.highlight.iter().any(|key| has_word(message, key))
    }
}

/// Whether `key` is the username, with or without the `@`; Mattermost
/// keeps both in `mention_keys`, and the username always mentions anyway.
fn is_username(username: &str, key: &str) -> bool {
    !username.is_empty()
        && key
            .strip_prefix('@')
            .unwrap_or(key)
            .eq_ignore_ascii_case(username)
}

/// The user's own keywords in one list of `notify_props`, as Preferences
/// shows them: without the username entries Mattermost keeps there.
pub fn keywords(username: &str, props: &HashMap<String, String>, list: KeywordList) -> Vec<String> {
    props
        .get(list.prop())
        .map(|keys| {
            keys.split(',')
                .map(str::trim)
                .filter(|key| !key.is_empty() && !is_username(username, key))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// `props` with the keywords typed in Preferences added to `list`. Typing
/// may hold several, separated by commas; repeated (ignoring case), empty
/// and username keywords drop out, and the entries already there keep
/// their order, so the username stays first as the webapp keeps it.
pub fn add_keywords(
    username: &str,
    props: &HashMap<String, String>,
    list: KeywordList,
    typed: &str,
) -> HashMap<String, String> {
    edit_list(props, list, |keys| {
        for key in typed.split(',').map(str::trim) {
            if !key.is_empty()
                && !is_username(username, key)
                && !keys.iter().any(|k| k.eq_ignore_ascii_case(key))
            {
                keys.push(key.to_owned());
            }
        }
    })
}

/// `props` without `keyword` in `list`, for a keyword's remove button.
pub fn remove_keyword(
    props: &HashMap<String, String>,
    list: KeywordList,
    keyword: &str,
) -> HashMap<String, String> {
    edit_list(props, list, |keys| {
        keys.retain(|key| !key.eq_ignore_ascii_case(keyword));
    })
}

fn edit_list(
    props: &HashMap<String, String>,
    list: KeywordList,
    edit: impl FnOnce(&mut Vec<String>),
) -> HashMap<String, String> {
    let mut keys: Vec<String> = props
        .get(list.prop())
        .map(|keys| {
            keys.split(',')
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    edit(&mut keys);
    let mut props = props.clone();
    props.insert(list.prop().into(), keys.join(","));
    props
}

/// `message` without its code blocks and code spans, which the server
/// leaves out when it looks for mentions.
pub fn without_code(message: &str) -> std::borrow::Cow<'_, str> {
    if !message.contains(['`', '~']) {
        return message.into();
    }
    // Fenced blocks: from a line opening with ``` or ~~~ to the line that
    // closes it, or to the end.
    let mut kept = String::with_capacity(message.len());
    let mut fence: Option<char> = None;
    for line in message.split_inclusive('\n') {
        let trimmed = line.trim();
        match fence {
            Some(mark) => {
                if trimmed.len() >= 3 && trimmed.chars().all(|c| c == mark) {
                    fence = None;
                }
            }
            None if trimmed.starts_with("```") => fence = Some('`'),
            None if trimmed.starts_with("~~~") => fence = Some('~'),
            None => kept.push_str(line),
        }
    }
    // Code spans: a run of backticks, at least one character, and the
    // next run of backticks. An unclosed backtick is plain text.
    let mut out = String::with_capacity(kept.len());
    let mut rest = kept.as_str();
    while let Some(open) = rest.find('`') {
        out.push_str(&rest[..open]);
        let ticks = &rest[open..];
        let body = ticks.trim_start_matches('`');
        let close = body
            .char_indices()
            .nth(1)
            .and_then(|(at, _)| body[at..].find('`').map(|close| at + close));
        match close {
            Some(close) => {
                out.push(' ');
                rest = body[close..].trim_start_matches('`');
            }
            None => {
                out.push_str(&ticks[..ticks.len() - body.len()]);
                rest = body;
            }
        }
    }
    out.push_str(rest);
    out.into()
}

/// Characters that continue a word or a username.
fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `key` in `message` as a whole word, ignoring case, the way the server's
/// mention parser reads words. Trailing `.`, `-`, `_` and `:` trim off, so
/// `@arya.` and `deploy:` at the end of a sentence count. A plain keyword
/// is also split at `.`, `-` and `:`, so `deploy-bot` and
/// `ci.example.com/deploy` have `deploy`, but `@arya.jha` is someone else.
pub fn has_word(message: &str, key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    let haystack = message.to_lowercase();
    let needle = key.to_lowercase();
    let at_key = needle.starts_with('@');
    haystack.match_indices(&needle).any(|(at, _)| {
        let head = &haystack[..at];
        let before = head.chars().next_back();
        // `mail@arya` is no mention, and `@arya` is not the keyword `arya`.
        let starts = !before.is_some_and(|c| word_char(c) || c == '@');
        let after = &haystack[at + needle.len()..];
        let tail = after.trim_start_matches(['.', '-', '_', ':']);
        let ends = match tail.chars().next() {
            None => true,
            Some(c) if !word_char(c) => true,
            Some(_) => !at_key && after.starts_with(['.', '-', ':']),
        };
        // `:deploy:` is an emoji.
        let emoji = before == Some(':')
            && !head[..head.len() - 1]
                .chars()
                .next_back()
                .is_some_and(word_char)
            && after.starts_with(':')
            && !after[1..].chars().next().is_some_and(word_char);
        starts && ends && !emoji
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
    fn keywords_round_trip_through_preferences() {
        let stored = props(&[("desktop", "all"), ("mention_keys", "arya,@arya,deploy")]);
        assert_eq!(
            keywords("arya", &stored, KeywordList::Mention),
            vec!["deploy"]
        );
        let saved = add_keywords(
            "arya",
            &stored,
            KeywordList::Mention,
            " deploy, On-call ,, on-call, @Arya ",
        );
        assert_eq!(saved["mention_keys"], "arya,@arya,deploy,On-call");
        assert_eq!(saved["desktop"], "all");
        let removed = remove_keyword(&saved, KeywordList::Mention, "on-call");
        assert_eq!(removed["mention_keys"], "arya,@arya,deploy");
        let cleared = remove_keyword(&removed, KeywordList::Mention, "deploy");
        assert_eq!(cleared["mention_keys"], "arya,@arya");
    }

    #[test]
    fn highlight_keywords_highlight_without_mentioning() {
        let stored = props(&[("mention_keys", "arya,@arya"), ("highlight_keys", "")]);
        assert!(keywords("arya", &stored, KeywordList::Highlight).is_empty());
        let saved = add_keywords("arya", &stored, KeywordList::Highlight, "Incident");
        assert_eq!(saved["highlight_keys"], "Incident");
        assert_eq!(saved["mention_keys"], "arya,@arya");
        let keys = MentionKeys::from_props("arya", "", &saved);
        assert!(keys.highlighted("new incident opened"));
        assert!(!keys.mentioned("new incident opened"));
        assert!(!keys.highlighted("incidents"));
    }

    #[test]
    fn keywords_split_like_the_server() {
        let keys = MentionKeys::from_props("arya", "", &props(&[("mention_keys", "deploy")]));
        // Inside URLs and dotted, dashed or colon-joined words.
        assert!(keys.mentioned("see https://ci.example.com/deploy"));
        assert!(keys.mentioned("https://deploy.example.com/status"));
        assert!(keys.mentioned("ping deploy-bot"));
        assert!(keys.mentioned("stage:deploy:prod"));
        assert!(keys.mentioned("deploy: done"));
        assert!(keys.mentioned("deploy_"));
        // Not inside a longer word or a query parameter name.
        assert!(!keys.mentioned("https://example.com/redeploy"));
        assert!(!keys.mentioned("?deploy_id=4"));
        assert!(!keys.mentioned("deploy_bot"));
        // An emoji is not the keyword.
        assert!(!keys.mentioned("shipped :deploy:"));
    }

    #[test]
    fn code_never_mentions() {
        let keys = MentionKeys::from_props(
            "arya",
            "",
            &props(&[("mention_keys", "deploy"), ("channel", "true")]),
        );
        assert!(!keys.mentioned("run `make deploy` first"));
        assert!(!keys.mentioned("``deploy``"));
        assert!(!keys.mentioned("```sh\n./deploy @arya\n```"));
        assert!(!keys.mentioned("~~~\ndeploy\n~~~"));
        assert!(!keys.channel_mentioned("`@channel` pings everyone"));
        assert!(keys.mentioned("```\ncode\n```\nthen deploy"));
        assert!(keys.mentioned("`code` and deploy"));
        // An unclosed backtick is text.
        assert!(keys.mentioned("a ` then deploy"));
        assert!(keys.mentioned("`deploy"));
    }

    #[test]
    fn keys_ignore_case_like_mattermost() {
        let keys = MentionKeys::from_props(
            "Arya",
            "Ärya",
            &props(&[("mention_keys", "Release,ÜBER"), ("first_name", "true")]),
        );
        assert!(keys.mentioned("@ARYA hi"));
        assert!(keys.mentioned("RELEASE today"));
        assert!(keys.mentioned("über alles"));
        assert!(keys.mentioned("ärya?"));
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
