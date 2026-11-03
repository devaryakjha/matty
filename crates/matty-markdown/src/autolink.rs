//! Bare URLs and email addresses as links, after GitHub's extended autolink
//! rules, which Mattermost follows.

use crate::Inline;

/// Splits `text` into text and links for the URLs and emails in it.
pub(crate) fn autolink(text: &str, out: &mut Vec<Inline>) {
    let mut rest = text;
    let mut plain_start = 0;
    let mut offset = 0;
    while let Some(found) = next_link(rest) {
        let start = offset + found.start;
        let end = offset + found.end;
        if start > plain_start {
            push_text(out, &text[plain_start..start]);
        }
        let label = &text[start..end];
        out.push(Inline::Link {
            url: found.url,
            title: String::new(),
            children: vec![Inline::Text(label.to_owned())],
        });
        plain_start = end;
        offset = end;
        rest = &text[end..];
    }
    if plain_start < text.len() {
        push_text(out, &text[plain_start..]);
    }
}

fn push_text(out: &mut Vec<Inline>, text: &str) {
    if let Some(Inline::Text(last)) = out.last_mut() {
        last.push_str(text);
    } else {
        out.push(Inline::Text(text.to_owned()));
    }
}

struct Found {
    start: usize,
    end: usize,
    url: String,
}

/// The first link in `text`: its byte range and target.
fn next_link(text: &str) -> Option<Found> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let boundary = i == 0 || is_boundary(text[..i].chars().next_back()?);
        if boundary && let Some(found) = url_at(text, i) {
            return Some(found);
        }
        if bytes[i] == b'@'
            && let Some(found) = email_around(text, i)
        {
            return Some(found);
        }
        i += text[i..].chars().next()?.len_utf8();
    }
    None
}

/// Characters a link may follow.
fn is_boundary(c: char) -> bool {
    c.is_whitespace() || matches!(c, '*' | '_' | '~' | '(' | '"' | '\'')
}

fn url_at(text: &str, start: usize) -> Option<Found> {
    let rest = &text[start..];
    let lower = |prefix: &str| {
        rest.len() >= prefix.len() && rest[..prefix.len()].eq_ignore_ascii_case(prefix)
    };
    let (scheme_len, www) = if lower("https://") {
        (8, false)
    } else if lower("http://") {
        (7, false)
    } else if lower("ftp://") {
        (6, false)
    } else if lower("www.") {
        (0, true)
    } else {
        return None;
    };
    let run = rest
        .find(|c: char| c.is_whitespace() || c == '<')
        .unwrap_or(rest.len());
    let len = trim_trailing(&rest[..run]);
    // A domain, with at least one character after the scheme (or `www.`).
    let host_start = if www { 4 } else { scheme_len };
    if len <= host_start {
        return None;
    }
    let host_end = rest[host_start..len]
        .find(['/', '?', '#'])
        .map_or(len, |at| host_start + at);
    let host = &rest[host_start..host_end];
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '@'))
    {
        return None;
    }
    if www && !host.contains('.') {
        return None;
    }
    let shown = &rest[..len];
    let url = if www {
        format!("http://{shown}")
    } else {
        shown.to_owned()
    };
    Some(Found {
        start,
        end: start + len,
        url,
    })
}

/// How much of a URL candidate is the URL: trailing punctuation and an
/// unbalanced closing parenthesis are not part of it.
fn trim_trailing(candidate: &str) -> usize {
    let mut end = candidate.len();
    loop {
        let Some(last) = candidate[..end].chars().next_back() else {
            return end;
        };
        if matches!(
            last,
            '?' | '!' | '.' | ',' | ':' | '*' | '_' | '~' | '\'' | '"'
        ) {
            end -= last.len_utf8();
        } else if last == ')' {
            let open = candidate[..end].matches('(').count();
            let close = candidate[..end].matches(')').count();
            if close > open {
                end -= 1;
            } else {
                return end;
            }
        } else if last == ';' {
            // An entity reference such as `&amp;` at the end is not part of it.
            let head = &candidate[..end - 1];
            match head.rfind('&') {
                Some(amp)
                    if head[amp + 1..].chars().all(|c| c.is_ascii_alphanumeric())
                        && amp + 1 < head.len() =>
                {
                    end = amp;
                }
                _ => return end,
            }
        } else {
            return end;
        }
    }
}

fn email_around(text: &str, at: usize) -> Option<Found> {
    let local_ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+');
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| local_ok(c))
        .last()
        .map(|(i, _)| i)?;
    if start > 0 && !is_boundary(text[..start].chars().next_back()?) {
        return None;
    }
    let domain_ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_');
    let domain_len = text[at + 1..]
        .find(|c: char| !domain_ok(c))
        .unwrap_or(text.len() - at - 1);
    let mut end = at + 1 + domain_len;
    // A trailing dot ends the sentence, not the address.
    while text[..end].ends_with('.') {
        end -= 1;
    }
    let domain = &text[at + 1..end];
    if !domain.contains('.') || domain.ends_with(['-', '_']) || domain.starts_with('.') {
        return None;
    }
    let address = &text[start..end];
    Some(Found {
        start,
        end,
        url: format!("mailto:{address}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(text: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        autolink(text, &mut out);
        out.into_iter()
            .filter_map(|inline| match inline {
                Inline::Link { url, children, .. } => match children.as_slice() {
                    [Inline::Text(label)] => Some((url, label.clone())),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    fn one(url: &str, label: &str) -> Vec<(String, String)> {
        vec![(url.to_owned(), label.to_owned())]
    }

    #[test]
    fn bare_urls_become_links_without_trailing_punctuation() {
        assert_eq!(
            links("see https://example.com/a?b=1."),
            one("https://example.com/a?b=1", "https://example.com/a?b=1")
        );
        assert_eq!(
            links("(at www.example.com)"),
            one("http://www.example.com", "www.example.com")
        );
        assert_eq!(
            links("https://en.wikipedia.org/wiki/Rust_(programming_language)"),
            one(
                "https://en.wikipedia.org/wiki/Rust_(programming_language)",
                "https://en.wikipedia.org/wiki/Rust_(programming_language)"
            )
        );
        assert_eq!(links("ftp://files.example.com/x, ok").len(), 1);
        assert!(links("nothttps://example.com").is_empty());
        assert!(links("http:// nothing").is_empty());
        assert!(links("www.nodot").is_empty());
    }

    #[test]
    fn emails_become_mailto_links() {
        assert_eq!(
            links("mail ana@example.com."),
            one("mailto:ana@example.com", "ana@example.com")
        );
        assert!(links("@ana and ana@localhost").is_empty());
    }

    #[test]
    fn text_around_links_is_kept() {
        let mut out = Vec::new();
        autolink("a https://x.io b", &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], Inline::Text("a ".into()));
        assert_eq!(out[2], Inline::Text(" b".into()));
    }
}
