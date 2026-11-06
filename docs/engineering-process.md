# Engineering process: the Karya Labs framework

Status: **process of record, decided by Arya on 2026-11-06** (all
recommendations in section 6 accepted). It covers every Karya Labs
repository as one project: `karyalabs/matty`, `karyalabs/gpuicn`,
`karyalabs/gpui-icons`, `karyalabs/homebrew-matty` and `karyalabs/.github`.
The repositories build on each other (gpui-icons feeds gpuicn, gpuicn feeds
Matty, Matty feeds the Homebrew tap), so the workflows treat them as one
pipeline (section 7). `docs/CI.md` describes only the mechanics of Matty's
workflows; each other repository's CI doc links here.

Sections 1 to 4 are the audit this framework came from. Data comes from the
workflow files at `a2f9007` (matty), `59c4d58` (gpuicn) and `fb9d0d1`
(gpui-icons), and from the Actions runs of 2026-11-05, before the move to
the organization; run links still resolve through GitHub's redirect.

## 1. What runs today

### matty

| Workflow | Trigger | Jobs | Where |
| --- | --- | --- | --- |
| `ci.yml` | PR (opened, synchronize, reopened, ready_for_review, **labeled**), push to master, nightly 02:23 UTC | `changes`, `linux`, `web`, `deny`, `macos`, `macos universal bundle`, `windows`, `linux packages`, `windows installer`, `integration` ×2 or ×4, `ci` | OVH box (3 runners): linux, web, linux packages. Mac mini (1 runner): macos, universal. GitHub: changes, deny, integration, windows, ci |
| `release.yml` | push of a `v*` tag (Arya only, tag ruleset) | `macos universal` (sign, notarize), `linux x86_64`, `windows x86_64`, `GitHub Release`, `Homebrew cask bump` (stable only) | Mac mini, OVH box, GitHub (Windows, publish, Homebrew) |
| `record-fixtures.yml` | manual | records API fixtures per Mattermost version, pushes `fixtures/v<ver>` | GitHub |

What each event runs in `ci.yml`:

| Job | PR | master push | nightly |
| --- | --- | --- | --- |
| `linux` fmt, clippy, plain-Rust checks | yes (skipped for docs-only) | yes | no |
| `linux` workspace tests | `tests` label | yes | no |
| `deny` | when deps change | yes | no |
| `web` (wasm release build) | `web` label | yes | no |
| `macos` clippy + tests | `macos` label | yes, plus release build, dev bundle, artifact | no |
| `macos universal bundle` (2-arch release build) | `macos` label | yes | no |
| `linux packages` (release build, deb/rpm/AppImage) | `linux` label | yes | no |
| `windows`, `windows installer` | `windows` label | when Windows paths (including `ci.yml` itself) change | yes |
| `integration` (live Mattermost) | `integration` label: 12.0, 10.5 | 12.0, 11.7, 10.11, 10.5 | no |

### gpuicn

| Workflow | Trigger | What |
| --- | --- | --- |
| `ci.yml` | PR, and called by `release.yml` on every push to main | `rust` (macOS: fmt, clippy, **full tests**), `linux` (MSRV tests, registry and version checks, `cargo package`), `wasm` (wasm check, website build and lint) |
| `release.yml` | push to main, manual | runs CI; if the version was bumped: `verify.yml`, tag, crates.io (Trusted Publishing), `release-build.yml`, GitHub release, `cloudflare.yml` deploy of ui.imajha.com |
| `verify.yml` | release only, manual | registry install with both CLIs (macOS), new-app setup (Linux), wasm size and screenshot sheet |
| `release-build.yml` | release only, manual | signed and notarized macOS showcase, Linux x86_64 and arm64 showcase |
| `cloudflare.yml` | release only, manual | site deploy |

Everything runs on GitHub's runners.

### gpui-icons

`ci.yml` (fmt, clippy, tests, MSRV check) on PRs and from `release.yml`,
which on every push to main tags and publishes to crates.io when the version
was bumped. GitHub's runners only.

## 2. Measurements (2026-11-05)

| Run | Wall clock | Critical path |
| --- | --- | --- |
| matty PR, lint only, warm OVH runner | 40 s to 3 min | `linux` clippy |
| matty master push, warm self-hosted | about 8 min ([run 42018458358](https://github.com/devaryakjha/matty/actions/runs/42018458358)) | Mac mini: `macos` 3 min, then `macos universal bundle` 2.5 min, one after the other on its single runner |
| matty master push touching a Windows path | about 9 min more on GitHub's Windows runner (`windows` 9 min) | `windows` |
| matty `integration`, each of 4 versions | about 2.5 min on GitHub | seed 30 s, tests 30 s, build 40 s |
| matty release (beta.4, beta.5) | 12 to 14 min | macOS sign and notarize |
| gpuicn release (0.9.6) | 21 min | verify, macOS showcase |

Master health over the last 39 finished master runs of the day: **23 green,
6 red, 10 cancelled.**

- Red: #644, #650, #651, #652 (the Mac mini bring-up, fixed by #654), #637
  (macOS-only dead code that clippy flags only on macOS, which PRs no longer
  run), #635.
- Cancelled: the macOS jobs on master keep only the newest waiting run, so a
  burst of merges drops the older ones. `ci` passes, but the run shows as
  cancelled, and those commits never got a macOS check of their own.

Releases: matty had 5 tags in 3 hours (beta.1 to beta.5); beta.1 and beta.2
failed on release-script bugs that only a real tag could reveal. gpuicn
shipped 10 releases in 2 days (0.7.0 to 0.9.7). 0.9.7 merged at 07:02 and
published at 11:34, because its release run was cancelled and then re-run
by hand.

## 3. Efficiency findings

Ordered by how much they cost.

1. **Master runs everything on every merge.** Each merge builds the app in
   release mode three times (macOS arm64, macOS universal with two more
   slices, Linux packages), with `codegen-units = 1` and LTO, plus the wasm
   release build and four Mattermost servers. Packaging breaks only when
   packaging, dependencies or the toolchain change, yet it runs for a
   translated string. The single Mac mini runs `macos` and the universal
   bundle back to back, so it caps master at one verified commit about every
   6 minutes, and that is why master macOS runs are dropped during bursts.
2. **Master is fixed forward, and it was red about one run in six.** Branch
   protection is off, nothing is required, and a break is found only after
   merging. Parallel lanes then merge on top of a red master, and "merge
   latest master and build before merging" is a manual rule that threads
   forget.
3. **macOS-only lint is not on PRs.** #637 broke master with code that only
   macOS clippy sees. A warm clippy on the Mac mini takes about 10 seconds,
   and it is lint, so it fits the "PRs only lint" rule.
4. **The `labeled` trigger reruns the whole PR run for any label**, not just
   the CI labels. The runs show the same commit checked twice in a row
   (#655, #656, #653).
5. **`ci.yml` is in the Windows path filter**, so every workflow edit costs
   two Windows jobs (about 9 and 10 minutes) on master.
6. **Screenshots live in the repository.** `docs/screenshots` holds 444
   folders and 92 MB on master, and history keeps every PNG a lane added and
   later removed: merge commits keep the intermediate commits, and history
   shows 337 deleted images. Every clone, and every checkout on GitHub's
   runners (changes, deny, four integration jobs), downloads them.
7. **Configuration is copied, not shared.** The Rust version `1.99.1`
   appears 18 times across the three repos' workflows (10 in matty) next
   to `rust-toolchain.toml`. The web nightly appears in 4 workflow steps
   and `scripts/web`. The apt
   package list, the appimagetool download and the keychain import appear
   in both `ci.yml` and `release.yml`. A pin bump that misses one copy
   fails later and somewhere else.
8. **`deny` runs on every master push**, but new advisories arrive by date,
   not by commit. A nightly run catches them even on quiet days, and PRs
   still check it when dependencies change.
9. **CI artifacts are kept for the default 90 days.** Every master run
   uploads a macOS binary, a universal zip, the Linux packages and the web
   build, and nobody downloads them after a day.
10. **The `ci` gate's cancel rule is too loose.** On master it accepts a
    cancelled `linux` or `integration` job whenever any of the queued
    platform jobs was cancelled.
11. **Cost.** All three repositories are public, so GitHub's standard
    runners are free. The $300 of hosted usage mentioned earlier must come
    from something else (larger runners, the time the repos were private,
    or another account item). It is worth one look at the billing page
    before spending effort on minutes.

What is already good and stays: PR runs cancel older runs, self-hosted
runners keep `target/` warm (clippy 1 s, tests 2 min), fork PRs never reach
the machines, the master failure issues, integration on GitHub's runners,
and gpuicn's idempotent "re-run failed jobs is always safe" release.

## 4. Release findings

### matty: what Arya does by hand

1. Decide a commit and push `git tag v0.1.0-beta.N <sha>`.
2. Watch the run, and when it fails, wait for a fix on master and tag again.
3. Deploy the web build from a Mac with `scripts/web deploy`, which signs in
   to Cloudflare through a browser.
4. One-time: the Developer ID certificate, the notary API key, the update
   signing key and its public variable, the Homebrew tap token (not set yet,
   so stable releases will not bump the cask), the runner variables, both
   runner machines and their upkeep.

### Problems

- **A tag is not tied to a green commit.** The release builds and publishes
  whatever the tag points at; nothing checks that master CI passed there,
  and the release itself runs no tests.
- **The pipeline cannot be tried without releasing.** There is no dry run,
  so beta.1, beta.2 and the first beta.3 attempt were the test.
- **Versioning is loose.** `Cargo.toml` says 0.1.0 for every beta; the tag
  suffix carries the real version, the changelog is GitHub's generated list
  of PR titles (merge trains make those misleading), and there is no
  cadence, so betas went out minutes apart.
- **The web deploy is manual and machine-bound,** while gpuicn already
  deploys its site from CI with a Cloudflare token in a `release`
  environment.
- **gpuicn releases and Matty are coupled by hand.** Each gpuicn release
  needs a Matty pin bump PR; 10 releases meant 10 bumps, and a stalled
  release (0.9.7) blocked Matty for 4.5 hours.

### Inconsistent across the three repositories

| | matty | gpuicn | gpui-icons |
| --- | --- | --- | --- |
| Default branch | `master` | `main` | `main` |
| Release trigger | Arya pushes a tag | merging a version bump | merging a version bump |
| PR checks | lint only | full macOS tests + Linux + wasm | full tests |
| Runners | self-hosted + GitHub | GitHub | GitHub |
| Pre-release dry run | none | `verify.yml`, `release-build.yml` by hand | none |

## 5. Proposed framework

### 5.1 Principles

1. Every check has exactly one home tier, chosen by what can break it and
   how fast it is.
2. Master is always releasable. A red master stops merging until it is
   green.
3. Nothing reaches master that has not passed the merge tier on the exact
   tree that lands.
4. Arya makes go/no-go decisions; Claude does everything else, including
   merging green PRs.
5. One framework for every Karya Labs repository, with the same tier
   names, the same required check name (`ci`) and the same labels.
6. Every setting lives in one place (toolchain file, a reusable workflow or
   composite action in `karyalabs/.github`, an organization variable), never
   copied.
7. Upstream repositories never break downstream ones silently: a change to
   gpui-icons or gpuicn is built against its consumers before it is
   released, and a release opens the consumer's pin bump by itself.

### 5.2 Tiers: what runs when

| Tier | When | Budget | matty | gpuicn / gpui-icons |
| --- | --- | --- | --- | --- |
| **T0 local** | before every push | 1 min | `scripts/check`: fmt, clippy on touched crates, touched tests | same script name |
| **T1 PR** | every push to a PR | 3 min | fmt, Linux clippy, **macOS clippy**, plain-Rust checks, deny when deps change | fmt, clippy, registry/version checks; full tests move to T2 |
| **T2 merge** | the tree about to land (merge queue or train), then master | 8 min | Linux tests, macOS tests, web build, integration on 12.0 | tests (macOS, MSRV), wasm check |
| **T3 nightly** | 02:23 UTC, only if master moved | 45 min | universal bundle, Linux packages, Windows (both jobs), integration on all four versions, deny, dev bundle; artifacts kept 7 days | `verify.yml`, release builds |
| **T4 release** | release approved by Arya | 30 min | T3 again on the release commit, then sign, notarize, publish, update manifest, Homebrew, web deploy | unchanged pipeline |

Rules that carry over unchanged: the opt-in labels pull a T2 or T3 job into a
PR, the narrowest label only, never `full-ci` to be safe. T2 and T3 failures
open the "failing on master" issue. Packaging jobs also run in T2 when their
own files change (`packaging/`, `scripts/*-package`, `scripts/release-*`,
`Cargo.lock`, toolchain, workflows).

Effect on matty master: the Mac mini does one 2–3 minute job per merge
instead of about 6 minutes in two jobs, three release builds per merge
become zero, and four integration servers become one.

### 5.3 Branches and pull requests

- `master` (matty) and `main` (gpuicn, gpui-icons) are protected: the `ci`
  check is required, history is linear, and only squash merges are allowed.
  One PR is one commit on master, so `git revert` undoes one change and the
  screenshots a PR added and removed never reach history.
- Branches are deleted on merge (repository setting).
- Merging goes through a **merge queue** that runs T2 on the combined tree
  and batches PRs. It replaces both the manual merge trains and the "merge
  latest master and build" rule. The repositories are in the `karyalabs`
  organization now (D4), so the queue is available; until Arya turns it on,
  merge trains stay, and T2 runs on the train PR.
- Stacked PRs stay as they are; a stack enters the queue together.
- Every PR links its issue and follows the template. Screenshots go in the PR
  body, hosted in the `karyalabs/assets` repository (D6), never in `docs/`.

### 5.4 Releases and versioning

- **SemVer, with `Cargo.toml` as the single source of the version.** Betas
  are `0.1.0-beta.N` in `Cargo.toml` too, so `matty --version` and the tag
  always agree.
- **A release is a release PR** in every repository that publishes: Claude
  opens it with the version bump and a hand-written `CHANGELOG.md` entry;
  merging it is the go. The workflow then tags the commit, runs T4 and
  publishes, exactly as gpuicn already does. Pushing a tag by hand stops
  being a step (D5).
- **Dry run:** `release.yml` gets a manual run that builds, signs and
  notarizes without publishing, and T3 runs it every week, so a broken
  pipeline shows up before release day.
- **Cadence:** one beta a week from green master (Claude opens the release
  PR every Monday, say), plus a hotfix beta when a release-blocking bug is
  fixed. Stable at milestone exits (PLAN §12).
- **Channels:** beta and stable as PLAN §10 already says; the update manifest
  is published for both.
- **gpuicn coordination:** gpuicn releases when Matty needs something that
  is merged, at most once a day, never in the middle of a Matty release.
  The release opens Matty's pin bump PR by itself (section 7.4).

### 5.5 Who does what

| Task | Arya | Claude |
| --- | --- | --- |
| Pick priorities, approve releases (merge the release PR) | ✓ | |
| Secrets, certificates, repository settings, runner machines | ✓ | writes the exact steps |
| Open, review, fix and merge PRs once `ci` is green | | ✓ |
| Fix a red master before anything else | | ✓ |
| Open the weekly release PR and changelog | | ✓ |
| gpuicn and gpui-icons releases | | ✓ (opens and merges the release PR; the pin bump PR follows by itself) |

### 5.6 Arya's manual steps after the change

Per release: merge the release PR. Nothing else; the web deploy, Homebrew
and the update manifest run in the pipeline.

One time:

1. Organization rulesets and merge settings on every repository (require
   `ci`, linear history, squash only, delete branches on merge, merge
   queue).
2. A `release` environment in matty with `CLOUDFLARE_API_TOKEN` and
   `CLOUDFLARE_ACCOUNT_ID`, so CI deploys matty.jha.sh.
3. `HOMEBREW_TAP_TOKEN`, before the first stable release.
4. The `karyalabs/assets` repository (D6) and the cross-repository token or
   GitHub App (section 7.4).

## 6. Decisions

Arya decided all of these on 2026-11-06: "go with the recommendations".

| # | Question | Decision |
| --- | --- | --- |
| D1 | Adopt the tiers in 5.2 (packaging, Windows, all integration versions and deny move to nightly; master keeps tests, web and one server)? | Yes, in every repository |
| D2 | Run macOS clippy on PRs (lint, about 10 s warm on the Mac mini)? | Yes |
| D3 | Protect `master`/`main`: require `ci`, linear history, squash only, delete merged branches? | Yes, as organization rulesets |
| D4 | Move the repositories into a GitHub organization, for the merge queue and shared runners? | Yes, `karyalabs` (done 2026-11-06, section 9) |
| D5 | Release by merging a release PR, or keep pushing tags? | Release PR, in every repository that publishes |
| D6 | Where do PR screenshots live instead of `docs/screenshots`? | A separate `karyalabs/assets` repository, linked from PRs |
| D7 | Release cadence: a weekly beta plus hotfixes? | Yes (Matty); libraries release on demand, at most daily |
| D8 | Deploy matty.jha.sh from the release pipeline instead of from a Mac? | Yes |
| D9 | Rewrite history to drop the old screenshots? | No: D3 and D6 stop the growth |

## 7. One project: how the repositories work together

### 7.1 The dependency chain

```
gpui-icons ──▶ gpuicn ──▶ matty ──▶ homebrew-matty
                  │
                  └──▶ ui.imajha.com (gpuicn's site)
karyalabs/.github: reusable workflows and actions used by all of them
```

Today each link is crossed by hand: a gpuicn release needs a hand-made
Matty pin bump PR, and nothing tells gpuicn that a change will break Matty
until Matty bumps the pin.

### 7.2 Shared building blocks in `karyalabs/.github`

Reusable workflows (`.github/workflows/*.yml` with `on: workflow_call`) and
composite actions (`actions/<name>/action.yml`), versioned by tag (`v1`) so a
change to them is a reviewed release too:

| Block | Kind | Used by |
| --- | --- | --- |
| `rust-toolchain` | action: installs the toolchain named in `rust-toolchain.toml`, plus components | all |
| `gpui-deps` | action: the apt packages GPUI needs on Linux | matty, gpuicn |
| `self-hosted-prep` | action: cleans a warm self-hosted workspace, keeps `target/` | matty, gpuicn |
| `macos-keychain` | action: imports the Developer ID certificate into a temporary keychain | matty, gpuicn |
| `rust-lint` | workflow: fmt and clippy on Linux and macOS (T1) | all Rust repos |
| `rust-test` | workflow: tests, MSRV check (T2) | all Rust repos |
| `release-pr` | workflow: on merge of a version bump, tag, publish to crates.io (Trusted Publishing), GitHub release | gpui-icons, gpuicn, matty (tag + release only) |
| `notarize` | workflow: sign, notarize and staple a macOS app | matty, gpuicn showcase |
| `cloudflare-deploy` | workflow: deploy a static site to Cloudflare Pages or Workers | gpuicn site, matty web |
| `pin-bump` | workflow: open a dependency bump PR in a downstream repository (7.4) | gpui-icons, gpuicn |

Each repository's own workflows become thin callers that set inputs. The
order of migration is gpui-icons (smallest), then gpuicn, then matty.

### 7.3 Same tiers, runners and secrets everywhere

- Every repository has the tiers of 5.2 and one required check named `ci`.
  gpuicn's full macOS test suite moves from T1 to T2; its `verify.yml` and
  showcase builds stay in T3 and T4.
- Runners: the OVH box and the Mac mini are organization runners. Every
  workflow reads `vars.KARYA_LINUX_RUNNER` / `vars.KARYA_MACOS_RUNNER`
  (organization variables, a JSON `runs-on` value); unset means GitHub's
  runners (`ubuntu-latest`, `macos-15`; the organization rejects
  `macos-14`). The per-repository `MATTY_*_RUNNER` and `GPUICN_*_RUNNER`
  variables are retired after the switch. Fork PRs always get GitHub's
  runners.
- Secrets: one Apple set at organization level, shared with the
  repositories that sign: `APPLE_CERTIFICATE_P12`,
  `APPLE_CERTIFICATE_PASSWORD`, `APPLE_API_KEY_P8`, `APPLE_API_KEY_ID`,
  `APPLE_API_ISSUER_ID`, `APPLE_TEAM_ID`. One Cloudflare pair
  (`CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`) for the sites. The
  updater signing key stays a Matty-only secret.
- Labels are the same in every repository: `tests`, `macos`, `linux`,
  `windows`, `web`, `integration`, `full-ci`, `release`.

### 7.4 Upstream changes are checked downstream before they ship

- **Canary.** In T2 (and nightly), gpuicn builds Matty at Matty's default
  branch against the gpuicn tree under test, through a `[patch.crates-io]`
  override, running Matty's T1 (`cargo check` and clippy). gpui-icons does
  the same against gpuicn. A failure is reported on the upstream PR and does
  not block it at first; once it has been stable for a week it becomes
  required. A deliberate breaking change is merged with the matching
  downstream PR ready.
- **Automatic pin bumps.** When gpui-icons or gpuicn publishes, its release
  workflow opens the bump PR in the consumer (gpui-icons → gpuicn,
  gpuicn → matty) with the changelog entry as the body. The bump PR runs
  the consumer's full T2 and is merged like any other green PR. This needs a
  token that can open PRs across repositories: a small organization GitHub
  App (preferred) or a fine-grained token, stored as an organization secret
  `KARYA_BOT_APP_ID` / `KARYA_BOT_PRIVATE_KEY`.
- **Release coordination.** Libraries release at most once a day and never
  while a Matty release PR is open. One changelog convention everywhere:
  `CHANGELOG.md`, Keep a Changelog headings, newest first, one entry per
  release PR.

### 7.5 Branch names

matty's default branch is `master`; every other repository uses `main`.
matty moves to `main` together with the ruleset change, so one ruleset and
one set of workflow filters fit every repository. GitHub redirects the old
branch name for PRs and links; workflows and docs are updated in the same
PR.

## 8. Order of work

Each step is small PRs, smallest risk first, merged as they go green:

1. This document as the process of record.
2. Hygiene with no policy change, in every repository: filter the
   `labeled` trigger to CI labels, take `ci.yml` out of the Windows filter,
   tighten the `ci` gate, 7-day artifact retention, read the toolchain from
   `rust-toolchain.toml`, `macos-15` fallbacks.
3. The shared building blocks in `karyalabs/.github` (7.2), then migrate
   gpui-icons, gpuicn and matty onto them, with the organization runner
   variables (7.3).
4. The tier split (D1, D2) in every repository, then the canary and the
   automatic pin bumps (7.4).
5. Release dry run and the weekly T3 dry run; the release PR model and
   versioning (D5, D7); the web deploy (D8).
6. Screenshot location (D6) and a check that rejects new images under
   `docs/`.
7. Settings Arya applies, with exact steps: rulesets and merge queue (D3),
   organization variables and secrets, the cross-repository App, the assets
   repository, matty's branch rename.
   The steps are in [org-settings.md](org-settings.md).

## 9. Moving to the karyalabs organization (D4)

Done on 2026-11-06: all four repositories, their secrets, variables and
runtime settings moved, crates.io Trusted Publishing points at `karyalabs`,
links were updated (matty#660, gpuicn#151, gpui-icons#12), and the runners
are registered at organization level. The plan is kept below for the record.

Karya Labs (`karyalabs`) is the home for Arya's open-source apps and
libraries: shared runners, shared signing and notarization secrets, org-wide
rulesets and the merge queue. The repos move one at a time, the one with the
least to lose first, and each move is checked before the next.

### Before anything moves (Arya)

Keep offline copies, in a password manager, of everything the secrets hold.
Apple's side does not change with a transfer (the certificate and the notary
key belong to the Apple Developer team), but if a secret does not travel it
must be re-entered from these:

- the Developer ID Application `.p12` and its password;
- the App Store Connect API key `.p8`, its key ID and issuer ID;
- the updater's ed25519 private key PEM (`MATTY_UPDATE_SIGNING_KEY`). Every
  installed Matty checks updates against its public half, so losing it
  strands every install. This one needs a backup whatever happens.

### Steps

| # | Who | Step | Check |
| --- | --- | --- | --- |
| 1 | Arya | Create the free organization `karyalabs`, display name "Karya Labs" | the org page loads |
| 2 | Arya | Install the Claude GitHub App on the org for its repositories, and reconnect GitHub in claude.ai | Claude can read the org |
| 3 | Arya | Transfer `gpui-icons` (Settings, General, Transfer ownership) and update its crates.io Trusted Publishing entry to `karyalabs/gpui-icons` | Claude lists which secrets, variables and environments arrived; a manual release dry run passes |
| 4 | Arya | Transfer `gpuicn`; update Trusted Publishing for `gpuicn` and `gpuicn-cli` | `release` environment and its secrets present; `release-build.yml` run by hand signs and notarizes |
| 5 | Arya | Transfer `matty` and `homebrew-matty` | secrets, variables (`MATTY_*_RUNNER`, `MATTY_UPDATE_PUBLIC_KEY`) and the `v*` tag ruleset present |
| 6 | Arya, Claude writes the commands | Register the OVH box and the Mac mini as org runners in a runner group limited to the three repos; remove the old repo runners | CI on matty runs on them; gpuicn can opt in |
| 7 | Arya | Org-level secrets: one Apple set and the Cloudflare token, shared with the repos that sign or deploy; remove the per-repo copies | release dry runs pass in matty and gpuicn |
| 8 | Claude | PRs that point every URL at `karyalabs`: Cargo `repository` fields, README badges, the updater's release URL, the Homebrew cask, docs | grep finds no `devaryakjha/` left outside history |
| 9 | Arya | Org rulesets (D3) and the merge queue on `master`/`main` | a PR merges through the queue |

GitHub redirects the old URLs after a transfer, including release downloads,
so installed betas keep finding updates. The redirect breaks if a new
repository is ever created as `devaryakjha/matty` (or the other two names),
so never reuse them.
