# Contributing to Matty

The plan of record is [docs/PLAN.md](docs/PLAN.md). Work is tracked as GitHub
issues: one roadmap issue per milestone, epics under it, and tasks under each
epic. Pick a task, link it from your pull request with "Closes #N".

## Pull requests

- Keep each pull request to one task.
- Fill in the pull request template.
- **Before and after screenshots are required whenever a change is visible.**
  Show the same view before and after at the same window size, add dark mode
  when color or theming changes, and attach a short video or GIF for motion
  and interactions. When a change has no visible effect (API, sync, store, CI,
  docs), write "Not applicable" with the reason in the Screenshots section.
- Reusable UI comes from [gpuicn](https://github.com/devaryakjha/gpuicn). Do
  not add a local substitute for a gpuicn component; note the gap on the
  matching issue instead.

## Database schema changes

The local cache schema lives in `crates/matty-store/migrations`. A schema
change always adds a new numbered migration; never edit one that has shipped.
Each migration also adds an upgrade fixture: run
`scripts/make-db-fixture.sh` to write
`crates/matty-store/fixtures/db/v<N>.sqlite` for the new version, and commit
it. `crates/matty-store/tests/upgrade.rs` opens every fixture, migrates it to
the latest version and checks the data, and fails when a version has no
fixture. Fixtures of shipped versions never change. If the new version adds a
table, extend the example in `crates/matty-store/examples/db_fixture.rs` so
the fixture fills it, and the test so it checks it.
