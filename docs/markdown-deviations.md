# Markdown deviations from the webapp

`crates/matty-markdown/tests/webapp.rs` renders every case imported from the
Mattermost webapp's own tests (`crates/matty-markdown/fixtures/webapp.json`,
made by `scripts/import-markdown-fixtures.py`) and compares the result with
the webapp's HTML. A case that renders differently must be listed here with
the reason, and the test fails when a listed case starts to match, so this
table is always the full and current list.

Most rows are deliberate: Matty follows CommonMark and GitHub's extensions
where the webapp's older marked-based renderer does not, or leaves the
decision to the view that knows the team's channels and users.

| Case | Why Matty differs |
| --- | --- |
| `text_formatting_channel_links.test:11` | `~123` links as a channel name. The parser does not know which channels exist; the view shows it as text when no channel has that name. |
| `text_formatting_channel_links.test:17` | `~town-square` links as a channel name. The webapp's test has no channels loaded; the view resolves it against the team's channels. |
| `text_formatting_email.test:56` | `"email"@domain.com` reads as text and a mention of `domain.com`. Whether that user exists is the view's call, as for any mention. |
| `text_formatting_email.test:62` | `_______@domain.com` is an email address, as GitHub links it, not an empty strong emphasis. |
| `text_formatting_email.test:72` | Inline math is on, so `$@#$` is math, as Mattermost's inline LaTeX setting renders it. |
| `text_formatting_email.test:76` | `@domain.com` is a mention of `domain.com`; the view shows it as text when there is no such user. |
| `text_formatting_hashtags.test:154` | `#test@example.com` is the hashtag `#test` and text: a hashtag stops at characters a tag cannot hold. |
| `text_formatting_imgs.test:30` | Image dimensions (`=50x76`) are not supported yet, so the image stays text. |
| `text_formatting_imgs.test:36` | Image dimensions (`=50`) are not supported yet, so the image stays text. |
| `text_formatting_links.test:23` | `@example.com` is a mention of `example.com`; the view shows it as text when there is no such user. |
| `text_formatting_links.test:75` | A bare domain without `www.` or a scheme (`example.com/index`) stays text, as in GitHub, so file names and versions are not links. |
| `text_formatting_links.test:177` | `_` around words in a URL is emphasis by CommonMark rules: the URL ends before `_one_`. |
| `text_formatting_links.test:220` | `***x***` nests as emphasis around strong, as CommonMark specifies; the webapp nests it the other way. Both look the same. |
| `text_formatting_links.test:224` | `___x___` nests as emphasis around strong, as CommonMark specifies; the webapp nests it the other way. Both look the same. |
| `text_formatting_links.test:254` | A link target with spaces is not a link in CommonMark, so the text stays as written. |
| `text_formatting_links.test:276` | Unescaped quotes inside a link title end the title in CommonMark, so the text stays as written. |
| `text_formatting_links.test:279` | A link target with spaces is not a link in CommonMark, so the text stays as written. |
