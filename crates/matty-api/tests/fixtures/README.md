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
