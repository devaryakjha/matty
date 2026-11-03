//! Mentions, channels, emoji and hashtags as `matty://` links (#43).

use matty_markdown::{parse_with, to_html};

/// Renders `input` with `:+1:`, `:tada:` and `:custom_emoji:` known.
fn html(input: &str) -> String {
    let known = |name: &str| matches!(name, "+1" | "tada" | "custom_emoji");
    to_html(&parse_with(input, &known))
}

#[track_caller]
fn check(input: &str, expected: &str) {
    assert_eq!(html(input), format!("<p>{expected}</p>\n"), "{input}");
}

#[test]
fn mentions_leave_out_trailing_punctuation() {
    check("@ana, hi", r#"<a href="matty://user/ana">@ana</a>, hi"#);
    check(
        "thanks @Ana.",
        r#"thanks <a href="matty://user/ana">@Ana</a>."#,
    );
    check(
        "@first.last_name-x ok",
        r#"<a href="matty://user/first.last_name-x">@first.last_name-x</a> ok"#,
    );
    check("(@ana)", r#"(<a href="matty://user/ana">@ana</a>)"#);
}

#[test]
fn special_mentions_have_their_own_links() {
    check(
        "@here @channel @all",
        r#"<a href="matty://special/here">@here</a> <a href="matty://special/channel">@channel</a> <a href="matty://special/all">@all</a>"#,
    );
}

#[test]
fn emails_are_links_not_mentions() {
    check(
        "email@host.com",
        r#"<a href="mailto:email@host.com">email@host.com</a>"#,
    );
    check("ana@localhost and a@", "ana@localhost and a@");
}

#[test]
fn channels_leave_out_the_period() {
    check(
        "join ~town-square.",
        r#"join <a href="matty://channel/town-square">~town-square</a>."#,
    );
    check("~ alone and a~b", "~ alone and a~b");
}

#[test]
fn emoji_link_only_when_the_resolver_knows_them() {
    check(
        ":+1: :custom_emoji: :unknown: :)",
        r#"<a href="matty://emoji/+1">:+1:</a> <a href="matty://emoji/custom_emoji">:custom_emoji:</a> :unknown: :)"#,
    );
    check(
        "time 10:30:tada:",
        r#"time 10:30<a href="matty://emoji/tada">:tada:</a>"#,
    );
}

#[test]
fn hashtags_need_three_characters() {
    check(
        "#release #ab #v2.0. ok",
        r#"<a href="matty://hashtag/release">#release</a> #ab <a href="matty://hashtag/v2.0">#v2.0</a>. ok"#,
    );
    check("C# and a#tag and #123", "C# and a#tag and #123");
}

#[test]
fn entities_in_code_and_links_are_left_alone() {
    check(
        "`@ana ~town :tada: #tag` [@ana](https://example.com) https://example.com/@ana",
        r#"<code>@ana ~town :tada: #tag</code> <a href="https://example.com">@ana</a> <a href="https://example.com/@ana">https://example.com/@ana</a>"#,
    );
    assert_eq!(
        html("```\n@ana ~town :tada: #tag\n```"),
        "<pre>@ana ~town :tada: #tag\n</pre>\n"
    );
}

#[test]
fn entities_inside_emphasis_lists_quotes_and_tables() {
    assert_eq!(
        html("> **@ana** see ~dev\n\n- :tada: #shipit\n\n| who |\n|---|\n| @bo |"),
        "<blockquote>\n<p><strong><a href=\"matty://user/ana\">@ana</a></strong> see <a href=\"matty://channel/dev\">~dev</a></p>\n</blockquote>\n\
         <ul>\n<li><a href=\"matty://emoji/tada\">:tada:</a> <a href=\"matty://hashtag/shipit\">#shipit</a></li>\n</ul>\n\
         <table>\n<tr><th>who</th></tr>\n<tr><td><a href=\"matty://user/bo\">@bo</a></td></tr>\n</table>\n"
    );
}

#[test]
fn headings_are_not_hashtags() {
    assert_eq!(
        html("# Title @ana"),
        "<h1>Title <a href=\"matty://user/ana\">@ana</a></h1>\n"
    );
}
