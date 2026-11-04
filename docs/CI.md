# CI

`.github/workflows/ci.yml` is built so pull requests merge fast.

| Job | Pull request | master |
| --- | --- | --- |
| `linux`: fmt, clippy, unit tests, plain-Rust checks | always (skipped for docs-only changes) | always |
| `deny` | always (skipped for docs-only changes) | always |
| `integration` (live Mattermost) | only when `matty-api`, `matty-store`, `matty-sync`, `docker/`, the seed script or the lockfile change; 12.0 and 10.5 | every supported version |
| `macos`: clippy, tests | only when macOS code changes, or with the `macos` label | always, plus the release build, `matty --version`, the dev bundle and the artifact |
| `macos universal bundle` | only when `packaging/macos/`, `scripts/macos-bundle` or `scripts/release-bundle` change, or with the `macos` label | always |
| `ci` | the one check to wait on | |

Why: macOS runners are scarce (a handful at a time per account), and three
build lanes queued enough macOS jobs that runs sat for 15 minutes or more
before a step ran. Nearly all of Matty builds and tests the same on Linux.

The macOS-only code is `crates/matty-notify`, and `logging.rs`, `actions.rs`
and `menu.rs` in `crates/matty-app`. Touching those, `packaging/`, the root
`Cargo.toml` or the toolchain turns the macOS job on for the pull request.
Add the `macos` label to run it on any other pull request.

When the macOS job fails on master it opens an issue titled "macOS CI is
failing on master", or comments on the open one. Fix it before merging more.

A new push cancels the older run of the same pull request, and a new merge
to master cancels the older master run.
