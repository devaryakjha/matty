//! Every recorded response decodes with the model Matty reads it into (#19).
//!
//! Each `tests/fixtures/v<version>/<name>.json` is decoded with the type the
//! table below names for `<name>`, then encoded again and compared with the
//! recording:
//!
//! - a value that comes back different fails the test. That is how an enum
//!   value Matty does not know shows up: it decodes to its `Unknown`
//!   fallback instead of round-tripping;
//! - a field Matty does not read is only reported, since new server fields
//!   must never break Matty;
//! - a fixture with no entry in the table fails, so a new recording cannot
//!   go unchecked.
//!
//! Every fixture is also checked for secrets: no session tokens, and no email
//! addresses outside `example.com`.

#![allow(clippy::unwrap_used)]

use std::{collections::BTreeMap, fs, path::Path};

use matty_api::{
    capabilities::{OldFormat, Ping},
    models::{
        Channel, ChannelMember, ChannelStats, ChannelUnreadAt, ChannelViewed, Emoji, Post,
        PostList, Preference, Reaction, SearchResults, SidebarCategories, Status, Team, TeamMember,
        TeamUnread, User, UserAutocomplete, UserThread, UserThreads,
    },
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

/// Decodes `value` as `T` and encodes it again.
fn round_trip<T: DeserializeOwned + Serialize>(value: &Value) -> Result<Value, String> {
    let decoded: T = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    serde_json::to_value(decoded).map_err(|e| e.to_string())
}

type Decoder = fn(&Value) -> Result<Value, String>;

/// The model for each fixture name, as the client decodes that response.
fn decoders() -> BTreeMap<&'static str, Decoder> {
    let table: &[(&str, Decoder)] = &[
        ("autocomplete_channels", round_trip::<Vec<Channel>>),
        ("channel_member_added", round_trip::<ChannelMember>),
        ("channel_posts", round_trip::<PostList>),
        ("channel_posts_crt", round_trip::<PostList>),
        ("channel_posts_since", round_trip::<PostList>),
        ("channel_stats", round_trip::<ChannelStats>),
        ("channel_unread_at", round_trip::<ChannelUnreadAt>),
        ("channel_viewed", round_trip::<ChannelViewed>),
        ("config_client", round_trip::<OldFormat>),
        ("direct_channel", round_trip::<Channel>),
        ("emoji_page", round_trip::<Vec<Emoji>>),
        ("group_channel", round_trip::<Channel>),
        ("license_client", round_trip::<OldFormat>),
        // An error body; ApiError::from_response reads it.
        ("login_invalid_credentials", round_trip::<Value>),
        ("my_channel_members", round_trip::<Vec<ChannelMember>>),
        ("my_channels", round_trip::<Vec<Channel>>),
        ("my_team_members", round_trip::<Vec<TeamMember>>),
        ("my_teams", round_trip::<Vec<Team>>),
        ("ping", round_trip::<Ping>),
        ("pinned_posts", round_trip::<PostList>),
        ("post_attachment", round_trip::<Post>),
        ("post_created", round_trip::<Post>),
        ("post_image_link", round_trip::<Post>),
        ("post_link", round_trip::<Post>),
        ("post_opengraph", round_trip::<Post>),
        ("post_patched", round_trip::<Post>),
        ("post_reactions", round_trip::<Post>),
        ("post_thread", round_trip::<PostList>),
        ("post_with_files", round_trip::<Post>),
        ("posts_around_unread", round_trip::<PostList>),
        ("preferences", round_trip::<Vec<Preference>>),
        ("public_channels", round_trip::<Vec<Channel>>),
        ("reactions", round_trip::<Vec<Reaction>>),
        ("saved_posts", round_trip::<PostList>),
        ("search_channels", round_trip::<Vec<Channel>>),
        ("search_posts", round_trip::<SearchResults>),
        ("sidebar_categories", round_trip::<SidebarCategories>),
        // `{"status": "OK"}`, which the client discards.
        ("status_ok", round_trip::<Value>),
        ("statuses", round_trip::<Vec<Status>>),
        ("team_unreads", round_trip::<Vec<TeamUnread>>),
        ("thread_read", round_trip::<UserThread>),
        ("user_threads", round_trip::<UserThreads>),
        ("user_threads_unread", round_trip::<UserThreads>),
        ("users_autocomplete", round_trip::<UserAutocomplete>),
        (
            "users_autocomplete_in_channel",
            round_trip::<UserAutocomplete>,
        ),
        ("users_by_ids", round_trip::<Vec<User>>),
        ("users_me", round_trip::<User>),
        ("users_page", round_trip::<Vec<User>>),
    ];
    table.iter().copied().collect()
}

/// Whether `value` is what a missing or `null` field decodes to.
fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}

/// Compares a recording with its round trip: fields Matty dropped go to
/// `unread`, values that changed go to `changed`.
fn compare(
    recorded: &Value,
    again: &Value,
    at: &str,
    unread: &mut Vec<String>,
    changed: &mut Vec<String>,
) {
    match (recorded, again) {
        (Value::Object(old), Value::Object(new)) => {
            for (key, value) in old {
                let path = format!("{at}.{key}");
                match new.get(key) {
                    Some(kept) => compare(value, kept, &path, unread, changed),
                    None => unread.push(path),
                }
            }
        }
        (Value::Array(old), Value::Array(new)) if old.len() == new.len() => {
            for (index, (value, kept)) in old.iter().zip(new).enumerate() {
                compare(value, kept, &format!("{at}[{index}]"), unread, changed);
            }
        }
        // `null` decodes to the field's default.
        (Value::Null, kept) if is_empty(kept) => {}
        (old, new) if old == new => {}
        (old, new) => changed.push(format!("{at}: recorded {old}, decoded as {new}")),
    }
}

fn version_dirs() -> Vec<std::path::PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut dirs: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('v'))
        })
        .collect();
    dirs.sort();
    dirs
}

fn json_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
}

#[test]
fn every_recorded_response_decodes_with_its_model() {
    let decoders = decoders();
    let dirs = version_dirs();
    assert!(!dirs.is_empty(), "no recorded server versions");
    let mut failures = Vec::new();
    for dir in &dirs {
        let version = dir.file_name().unwrap().to_string_lossy();
        let mut unread_fields = BTreeMap::<String, usize>::new();
        for file in json_files(dir) {
            let name = file.file_stem().unwrap().to_string_lossy().into_owned();
            let Some(decode) = decoders.get(name.as_str()) else {
                failures.push(format!(
                    "{version}/{name}: no model listed in tests/fixtures.rs"
                ));
                continue;
            };
            let recorded: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
            let again = match decode(&recorded) {
                Ok(again) => again,
                Err(error) => {
                    failures.push(format!("{version}/{name}: does not decode: {error}"));
                    continue;
                }
            };
            let (mut unread, mut changed) = (Vec::new(), Vec::new());
            compare(&recorded, &again, "", &mut unread, &mut changed);
            failures.extend(
                changed
                    .into_iter()
                    .map(|change| format!("{version}/{name}{change}")),
            );
            for path in unread {
                // Group `[3]` and map keys such as post ids into one line.
                let shape = generalize(&path);
                *unread_fields.entry(format!("{name}{shape}")).or_default() += 1;
            }
        }
        if !unread_fields.is_empty() {
            eprintln!(
                "{version}: {} fields Matty does not read (fine; listed for reference):",
                unread_fields.len()
            );
            for field in unread_fields.keys() {
                eprintln!("  {field}");
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `.posts.abc123.metadata[2].x` as `.posts.*.metadata[].x`.
fn generalize(path: &str) -> String {
    let mut out = String::new();
    for part in path.split('.').skip(1) {
        let (key, index) = part
            .split_once('[')
            .map_or((part, false), |(k, _)| (k, true));
        let id_like = key.len() == 26 && key.chars().all(|c| c.is_ascii_alphanumeric());
        out.push('.');
        out.push_str(if id_like { "*" } else { key });
        if index {
            out.push_str("[]");
        }
    }
    out
}

/// Strings anywhere in `value`, with the key they sit under.
fn strings<'a>(value: &'a Value, key: &'a str, out: &mut Vec<(&'a str, &'a str)>) {
    match value {
        Value::String(s) => out.push((key, s)),
        Value::Array(items) => items.iter().for_each(|item| strings(item, key, out)),
        Value::Object(map) => map.iter().for_each(|(k, v)| strings(v, k, out)),
        _ => {}
    }
}

#[test]
fn fixtures_hold_no_tokens_or_real_addresses() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut dirs = vec![root.clone(), root.join("ws")];
    dirs.extend(version_dirs());
    let email = regex_lite_email();
    let mut problems = Vec::new();
    for dir in dirs {
        for file in json_files(&dir) {
            let text = fs::read_to_string(&file).unwrap();
            let shown = file.strip_prefix(&root).unwrap().display().to_string();
            // The login response's session header, if a recording kept it.
            if text.contains("\"Token\":") {
                problems.push(format!("{shown}: has a `Token` field"));
            }
            let value: Value = serde_json::from_str(&text).unwrap();
            let mut found = Vec::new();
            strings(&value, "", &mut found);
            for (key, s) in found {
                let lower = key.to_ascii_lowercase();
                let secret_key = lower == "token"
                    || lower.ends_with("_token")
                    || matches!(
                        lower.as_str(),
                        "password" | "auth_data" | "mfa_secret" | "csrf"
                    );
                if secret_key && !s.is_empty() {
                    problems.push(format!("{shown}: `{key}` holds a value"));
                }
                for address in email(s) {
                    if !address.ends_with("@example.com") {
                        problems.push(format!("{shown}: address {address}"));
                    }
                }
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Finds `local@domain.tld` in a string, without a regex dependency.
fn regex_lite_email() -> impl Fn(&str) -> Vec<String> {
    |s: &str| {
        let local = |c: char| c.is_ascii_alphanumeric() || "._%+-".contains(c);
        let domain = |c: char| c.is_ascii_alphanumeric() || ".-".contains(c);
        let mut found = Vec::new();
        for (at, _) in s.match_indices('@') {
            let start = s[..at]
                .char_indices()
                .rev()
                .take_while(|&(_, c)| local(c))
                .last()
                .map_or(at, |(i, _)| i);
            let end = s[at + 1..]
                .find(|c: char| !domain(c))
                .map_or(s.len(), |i| at + 1 + i);
            let domain_part = &s[at + 1..end];
            if start < at && domain_part.contains('.') && !domain_part.ends_with('.') {
                found.push(s[start..end].to_owned());
            }
        }
        found
    }
}
