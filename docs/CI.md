# CI

`.github/workflows/ci.yml` runs the tiers of
[engineering-process.md](engineering-process.md) (section 5.2): pull
requests lint (T1), every merge to master runs the tests and one live
server (T2), and the slow release builds, Windows and every supported
Mattermost run once a night (T3), only when master moved since the last
good nightly. A manual run (`workflow_dispatch`) runs the nightly set.

| Job | Pull request (T1) | master and the merge queue (T2) | nightly (T3) |
| --- | --- | --- | --- |
| `linux`: fmt, clippy, plain-Rust checks | always (skipped for docs-only changes) | always | no |
| `linux` workspace tests | `tests` label | always | no |
| `macos`: clippy | always (skipped for docs-only changes) | always | always |
| `macos` workspace tests | `macos` label | always | no |
| `macos` release build, `matty --version`, dev bundle, artifact | no | no | always |
| `deny` | when a `Cargo.toml`, `Cargo.lock` or `deny.toml` changes | same | always |
| `web` build | `web` label | always | no |
| `integration` (live Mattermost) | `integration` label: 12.0, 10.5 | 12.0 | 12.0, 11.7, 10.11, 10.5 |
| `macos universal bundle` | `macos` label | when the packaging changes | always |
| `linux packages` (#216) | `linux` label | when the packaging changes | always |
| `windows`: clippy, tests (#215) | `windows` label | when Windows code changes | always |
| `windows installer` (#215) | `windows` label | when `packaging/windows/` or `scripts/windows-package` change | always |
| `ci` | the one check to wait on | | |

"The packaging changes" means `packaging/`, `scripts/*-package`,
`scripts/release-*`, `scripts/dev-bundle`, `Cargo.lock`,
`rust-toolchain.toml` or `ci.yml` itself. Every decision about what runs is
made in the `changes` job's "Plan the run" step; the jobs only read its
flags.

The `full-ci` label runs every pull request job above. Add a label when a
change is risky in a way lint cannot see, and use the narrowest one: the
sync engine or the store (`tests`, `integration`), platform code
(`macos`, `windows`), or packaging (`linux`, `macos`).

Adding a label that is not a CI label starts a run that checks nothing: its
`ci` repeats the latest other `ci` result on the commit, and it never
cancels the real run.

When the macOS or integration jobs fail on master or nightly they open an
issue ("macOS CI is failing on master", "Integration tests are failing on
master"), or comment on the open one. Fix it before merging more.

A new push cancels the older run of the same pull request. Every master run
goes ahead, but each macOS, packaging and Windows job on master runs one at
a time: a running one finishes, and only the newest waiting one is kept.
That job shows as cancelled in the older run, and `ci` still passes because
the newer run checks the same code; no other job may be cancelled.
Artifacts are kept for 7 days.

`linux` also runs a test of the Linux notification backend against a
stand-in notification service, on a private `dbus-daemon` the test starts.

## Runners

Linux and macOS jobs run on Karya's own machines when the organization
variables `KARYA_LINUX_RUNNER` and `KARYA_MACOS_RUNNER` name them (the
older repository variables `MATTY_LINUX_RUNNER` and `MATTY_MACOS_RUNNER`
still work while those are unset), and on GitHub's runners (`ubuntu-latest`,
`macos-15`) when neither is set. Setup, labels and upkeep are in
[self-hosted-runners.md](self-hosted-runners.md). Pull requests from forks
always run on GitHub's runners.

`changes`, `deny` and `ci` always stay on GitHub's Ubuntu runners: they take
seconds to a minute, and `changes` decides where the rest run. `integration`
stays there too (see self-hosted-runners.md).

There is no self-hosted Windows machine, and Windows minutes cost double,
so the two Windows jobs run nightly (02:23 UTC, with the rest of T3) and on
master only when Windows code or packaging changes. Use the `windows` label
to run them on a pull request. A release still builds
the Windows installer ([releasing.md](releasing.md)).

