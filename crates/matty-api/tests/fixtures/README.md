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

`command*.json` and `commands_autocomplete.json` are written by hand from
Mattermost 10.5's `model.Command`, `model.AutocompleteSuggestion` (whose
fields the server spells with capitals), `model.CommandResponse` and the
error body for an unknown command, for the slash command endpoints (#210),
until the recorder covers them.

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
