# API fixtures

`v<major>.<minor>/*.json` are real responses from that Mattermost version,
recorded from the seeded dev server with

```sh
scripts/dev-server reset && scripts/seed.py && scripts/record-fixtures.py
```

(see `docs/dev-server.md`). Re-recording changes every id, so tests assert on
seeded names and on relationships between fields, not on ids.

`config_client_*.json` and `license_client_*.json` at this level are written
by hand from Mattermost's `format=old` client config and license (every value
a string) and cover the keys `ServerCapabilities` reads. #19 replaces them
with responses recorded from each supported server version.

`command*.json`, `commands_autocomplete.json` and `post_interactive.json`
at this level are written by hand from Mattermost 10.5's models, for the
slash command (#210) and interactive message (#211) tests: they cover what a
plugin offers (subcommands with icons) and what the dev server cannot send.
The recorder records the same endpoints from the dev server into `v*/`:
it runs a small integration on the host, which the server reaches as
`host.docker.internal` (see `docker/compose.yml`), to answer post actions,
dialog submissions and a `/recorder` slash command. Two things the recordings
show that the hand-written files do not: the server sends an action's
`cookie` only on ephemeral posts, never on stored ones, and it answers every
action and command run with a fresh `trigger_id`, dialog or not.

`tests/fixtures.rs` decodes every `v*/` file with the model named for it in
its table, encodes it again, and fails when a value changes, which is how an
enum value Matty does not know shows up, or when a file has no model listed.
Fields Matty does not read are only printed (`--nocapture`). It also fails on
a session token or an address outside `example.com` in any fixture; the
recorder blanks or replaces those before writing.

To record another server version without running it locally, start the
`record fixtures` workflow with the image tags
(`gh workflow run record-fixtures.yml -f versions="12.0"`) and commit the
`fixtures-v<version>` artifacts it uploads.
