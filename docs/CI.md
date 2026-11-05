# CI

`.github/workflows/ci.yml` is built so pull requests merge fast.

Pull requests only lint. Matty is in early alpha, and waiting on the full
suite for every feature branch cost more time than the breaks it caught,
so the tests and builds run on master, where a break is fixed forward.

| Job | Pull request | master |
| --- | --- | --- |
| `linux`: fmt, clippy, plain-Rust checks | always (skipped for docs-only changes) | always, plus the workspace tests |
| `linux` workspace tests | with the `tests` label | always |
| `deny` | when a `Cargo.toml`, `Cargo.lock` or `deny.toml` changes | always |
| `web` build | with the `web` label | always |
| `integration` (live Mattermost) | with the `integration` label (12.0 and 10.5) | every supported version; a failure opens or comments on the issue "Integration tests are failing on master" |
| `macos`: clippy, tests | with the `macos` label | always, plus the release build, `matty --version`, the dev bundle and the artifact |
| `macos universal bundle` | with the `macos` label | always |
| `linux packages`: release build, tarball, `.deb`, `.rpm`, AppImage, desktop entry and AppStream validation (#216) | with the `linux` label | always, plus the artifact |
| `windows`: clippy, tests (#215) | with the `windows` label | when platform code changes, and every night; plus the release build, `matty.exe --version` and the artifact |
| `windows installer`: release build, MSI and zip, installed and uninstalled (#215) | with the `windows` label | when `packaging/windows/` or `scripts/windows-package` change, and every night; plus the artifact |
| `ci` | the one check to wait on | |

The `full-ci` label runs everything master runs on a pull request. Add a
label when a change is risky in a way lint cannot see: the sync engine, the
store, platform code (`crates/matty-notify`, and `logging.rs`, `actions.rs`
and `menu.rs` in `crates/matty-app` are macOS-specific), or packaging.

When the macOS job fails on master it opens an issue titled "macOS CI is
failing on master", or comments on the open one. Fix it before merging more.

A new push cancels the older run of the same pull request. Every master run
goes ahead, but each macOS job on master runs one at a time: a running one
finishes, and only the newest waiting one is kept. A master macOS job
dropped that way shows as cancelled, and `ci` still passes because the newer
run checks the same code. The `linux packages` job on master queues the
same way.

`linux` also runs a test of the Linux notification backend against a
stand-in notification service, on a private `dbus-daemon` the test starts.

## Runners

Linux and macOS jobs run on Arya's own machines when the repository
variables `MATTY_LINUX_RUNNER` and `MATTY_MACOS_RUNNER` name them, and on
GitHub's runners while they are unset. Setup, labels and upkeep are in
[self-hosted-runners.md](self-hosted-runners.md). Pull requests from forks
always run on GitHub's runners.

`changes`, `deny` and `ci` always stay on GitHub's Ubuntu runners: they take
seconds to a minute, and `changes` decides where the rest run. `integration`
stays there too (see self-hosted-runners.md).

There is no self-hosted Windows machine, and Windows minutes cost double,
so the two Windows jobs no longer run on every merge: master runs them when
Windows code or packaging changes, and a scheduled run at 02:23 UTC runs
both on the latest master every night (every other job skips it). Use the
`windows` label to run them on a pull request. A tag still builds the
Windows installer for the release.

