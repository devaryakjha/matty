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
`scripts/record-fixtures.sh`.

## Configuration

Set in `docker/compose.yml` through `MM_*` environment variables:

| Setting | Value | Notes |
| --- | --- | --- |
| Local mode | on | `mmctl --local` works inside the container without a token. |
| Synced drafts | on | |
| Custom emoji | on | |
| Link previews | on | |
| Personal access tokens | on | For the token sign-in path. |
| Collapsed Reply Threads | `default_on` | `MATTY_CRT=always_on` (or `disabled`, `default_off`) changes it; run `reset` or restart after changing. |
| Post priority | on | Licensed feature; inert without a license. |
| Rate limiting | off | Integration tests send bursts. Rate-limit behavior is tested with a mock server instead. |
| Email | off | No verification or notification email. |
| Port | 8065 | `MATTY_MM_PORT` changes it. |

Data lives in the `matty-dev_db` and `matty-dev_data` Docker volumes.
