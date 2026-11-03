//! The Mattermost webapp's own Markdown and text formatting cases (#44),
//! imported by `scripts/import-markdown-fixtures.py` into
//! `fixtures/webapp.json`. Each case either matches, or is listed with a
//! reason in `docs/markdown-deviations.md`; a listed case that starts to
//! match fails too, so the list stays true.

use std::collections::BTreeMap;

use matty_markdown::{parse_with, to_html};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixtures {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    input: String,
    expected: String,
    emoji: Vec<String>,
}

/// Newlines between tags are layout, not content: the webapp prints none.
fn flat(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\n' && (out.ends_with('>') || chars.peek() == Some(&'<')) {
            continue;
        }
        out.push(c);
    }
    out.trim().to_owned()
}

/// The ids listed in the deviations doc, with their reasons.
fn deviations() -> BTreeMap<String, String> {
    let doc = include_str!("../../../docs/markdown-deviations.md");
    doc.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("| `")?;
            let (id, rest) = rest.split_once('`')?;
            let reason = rest.trim_start_matches(" |").trim_end_matches('|').trim();
            Some((id.to_owned(), reason.to_owned()))
        })
        .collect()
}

#[test]
fn webapp_cases_match_or_are_listed_deviations() {
    let fixtures: Fixtures = serde_json::from_str(include_str!("../fixtures/webapp.json")).unwrap();
    assert!(fixtures.cases.len() >= 100, "too few cases");
    let listed = deviations();
    let mut unlisted = Vec::new();
    let mut stale = Vec::new();
    for case in &fixtures.cases {
        let known = |name: &str| case.emoji.iter().any(|e| e == name);
        let got = flat(&to_html(&parse_with(&case.input, &known)));
        let expected = flat(&case.expected);
        match (got == expected, listed.contains_key(&case.id)) {
            (false, false) => unlisted.push(format!(
                "{}\n  input:    {:?}\n  expected: {expected}\n  got:      {got}",
                case.id, case.input
            )),
            (true, true) => stale.push(case.id.clone()),
            _ => {}
        }
    }
    let ids: Vec<&String> = fixtures.cases.iter().map(|c| &c.id).collect();
    let unknown: Vec<&String> = listed.keys().filter(|id| !ids.contains(id)).collect();
    assert!(
        unlisted.is_empty() && stale.is_empty() && unknown.is_empty(),
        "{} cases differ and are not in docs/markdown-deviations.md:\n{}\n\n\
         listed but now matching: {stale:?}\nlisted but not a case: {unknown:?}",
        unlisted.len(),
        unlisted.join("\n"),
    );
}
