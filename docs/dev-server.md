# Development server

Matty develops and runs its integration tests against a local Mattermost
server in Docker: `docker/compose.yml`, driven by `scripts/dev-server`.

```sh
scripts/dev-server up        # http://localhost:8065, ready when this returns
scripts/dev-server mmctl user create --email you@example.com --username you --password 'Password1!' --system-admin
scripts/dev-server logs
scripts/dev-server down      # keeps data
scripts/dev-server reset     # deletes all data and starts fresh
```

## Image and versions

| Service | Image | Why |
| --- | --- | --- |
| Mattermost | `mattermost/mattermost-enterprise-edition:10.5` | Unlicensed, it runs with Team Edition features, and a trial license turns on licensed paths (LDAP, SAML, post priority). 10.5 is a current extended-support release. |
| Database | `postgres:16-alpine` | Mattermost's recommended database. |

`MATTY_MM_IMAGE=mattermost/mattermost-team-edition` runs the Team Edition
image instead, which has no license code at all.
`MATTY_MM_VERSION` picks another tag, for example to test the oldest
supported release (PLAN §6.3) or to record fixtures per version with
`scripts/record-fixtures.py`.

## Configuration

Set in `docker/compose.yml` through `MM_*` environment variables:

| Setting | Value | Notes |
| --- | --- | --- |
| Local mode | on | `mmctl --local` works inside the container without a token. |
| Synced drafts | on | |
| Custom emoji | on | |
| Link previews | on | Internal connections to `localhost` are allowed, so the server can preview pages it serves itself (fixture recording). |
| Personal access tokens | on | For the token sign-in path. |
| Collapsed Reply Threads | `default_on` | `MATTY_CRT=always_on` (or `disabled`, `default_off`) changes it; run `reset` or restart after changing. |
| Post priority | on | Licensed feature; inert without a license. |
| Rate limiting | off | Integration tests send bursts. Rate-limit behavior is tested with a mock server instead. |
| Email | off | No verification or notification email. |
| Port | 8065 | `MATTY_MM_PORT` changes it. |

Data lives in the `matty-dev_db` and `matty-dev_data` Docker volumes.

## Seed data

```sh
scripts/dev-server up
scripts/seed.py                 # about 35 seconds at the default size
scripts/seed.py --posts 10000 --users 50
```

`scripts/seed.py` uses only the Python standard library. It creates:

- `admin` (system admin) and `tester`, the account tests sign in as (email
  and password, no MFA, no LDAP or SSO), plus `--users` more (default 20),
  each with a generated avatar. Every password is `Password1!`.
- Teams `dev` and `ops`, each with 15 channels: Town Square, Off-Topic, 10
  more public and 3 private.
- Five direct messages and a self DM for `tester`, and two group messages.
- `--posts` posts (default 2,000). `busy` gets 40% of them for scrolling;
  `markdown` starts with one post per Markdown construct (PLAN §6.8) and
  others recur elsewhere; `empty` stays empty. About a fifth are thread
  replies, and some carry reactions, uploaded PNG images, or pins.
- Five custom emoji, used in posts and reactions.
- Custom sidebar categories Projects and Social, and up to ten saved posts,
  for `tester`.

It writes credentials and ids to `docker/seed-output.json` (ignored by git)
for tests to read: users, teams, channels with their thread roots, image,
pinned and Markdown post ids, emoji and saved posts.

Running it again creates nothing new: everything is looked up before it is
created, and posts resume from the last seeded post in each channel, so a run
that failed partway can be rerun. After changing `--posts` or `--users`, run
`scripts/dev-server reset` first.

## Recording API fixtures

```sh
scripts/dev-server reset && scripts/seed.py && scripts/record-fixtures.py
```

`scripts/record-fixtures.py` signs in as `tester` and writes the server's
responses to `crates/matty-api/tests/fixtures/v<major>.<minor>/`. It adds a
few posts to `dev/random` to record embeds, reactions, edits, deletes and
thread reads, so run it against a fresh server and expect the fixtures to
contain that server's ids.

## Integration tests

The live suites of `matty-api` and `matty-sync` run against this server when
`MATTY_TEST_SERVER` is set, and return at once otherwise:

```sh
scripts/dev-server up && scripts/seed.py
MATTY_TEST_SERVER=http://localhost:8065 MATTY_TEST_PASSWORD='Password1!' \
    cargo test -p matty-api -p matty-store -p matty-sync
```

The sync engine's suite is `crates/matty-sync/tests/integration/`. Each test
signs in as `admin` (`MATTY_TEST_ADMIN` to change it) and makes its own team,
two users and a channel, so the tests run in parallel without seeing each
other's posts or read state. Only the seeded-account test uses `tester`
(`MATTY_TEST_LOGIN`), and nothing else changes that account.

CI's `integration` job does the same on Ubuntu for each server version in its
matrix, and uploads the server log when a test fails. Changes that only touch
Markdown, `docs/` or the license skip its steps.
