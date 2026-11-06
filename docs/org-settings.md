# Karya Labs settings for Arya

The one-time settings the engineering process
([engineering-process.md](engineering-process.md)) needs, which only an
organization owner can change. Each step says where, what, and how Claude
checks it. They are independent; the order is by value.

## 1. Runner variables for every repository

Organization → Settings → Secrets and variables → Actions → **Variables** →
New organization variable, repository access "All repositories":

| Name | Value |
| --- | --- |
| `KARYA_LINUX_RUNNER` | `["self-hosted","Linux","X64","matty"]` |
| `KARYA_MACOS_RUNNER` | `["self-hosted","macOS","ARM64","matty"]` |

Every workflow reads these before the old `MATTY_*_RUNNER` and
`GPUICN_*_RUNNER` repository variables, so nothing changes at first. Once
a matty and a gpuicn run have used them, delete the four old repository
variables. Deleting an organization variable sends every repository back to
GitHub's runners at once, which is the switch for a machine that is down.

## 2. Merge settings on each repository

For matty, gpuicn, gpui-icons, homebrew-matty and .github: Settings →
General → Pull Requests:

- Allow squash merging only (untick merge commits and rebase merging);
  default message "Pull request title and description".
- Tick "Always suggest updating pull request branches", "Allow auto-merge"
  and "Automatically delete head branches".

## 3. A ruleset for every default branch (D3)

Organization → Settings → Repository → **Rulesets** → New branch ruleset:

- Name `default branches`, enforcement Active, no bypass list.
- Target repositories: matty, gpuicn, gpui-icons and .github (the ones
  with a `ci` check; homebrew-matty has none); target branches: "Default
  branch".
- Rules: Restrict deletions; Require linear history; Block force pushes;
  Require status checks to pass, with the check `ci` (source GitHub
  Actions) and "Require branches to be up to date" left off (the merge
  queue does that); **Require merge queue**: merge method Squash, build
  concurrency 5, group size 1 to 5, wait 1 minute, "Only merge non-failing
  pull requests", status check timeout 60 minutes.

Leave "Require a pull request before merging" off for now: the release
workflows of gpuicn and gpui-icons push only tags, and Claude merges
through the queue. Claude checks it by sending one PR through the queue in
each repository. With the queue on, Claude merges by enabling auto-merge,
which queues the PR.

## 4. Let release workflows create tags

Matty's release workflow now creates the `v*` tag itself
([releasing.md](releasing.md)). In karyalabs/matty → Settings → Rules →
Rulesets → the `v*` tag ruleset (`admin-only-tag-release`) → Bypass list →
Add bypass → **GitHub Actions** (always allow). Without it the first
release through a release PR fails at "Create the release", and re-running
it after this change finishes it.

## 5. The Karya bot app, for pin bump pull requests

So a gpui-icons release opens gpuicn's bump PR and a gpuicn release opens
Matty's (section 7.4 of the process). Organization → Settings → Developer
settings → GitHub Apps → New GitHub App:

- Name `karya-bot`, homepage `https://github.com/karyalabs`, webhook off.
- Repository permissions: Contents read and write, Pull requests read and
  write, Metadata read. Nothing else.
- "Only on this account". Create, then Generate a private key (a `.pem`
  downloads) and note the App ID.
- Install it on the organization for matty, gpuicn, gpui-icons and
  homebrew-matty.
- Organization secrets (Secrets and variables → Actions → Secrets), access
  "Selected repositories": gpuicn, gpui-icons, matty:
  - `KARYA_BOT_APP_ID`: the App ID;
  - `KARYA_BOT_PRIVATE_KEY`: the whole `.pem` file.

Claude checks it with the next gpui-icons or gpuicn release: the release
run's "bump" job links the pull request it opened. Pull requests the app
opens run CI like any other.

## 6. Cloudflare for matty.jha.sh (D8)

karyalabs/matty → Settings → Environments → `release` (it exists once a
release has run; create it otherwise) → Environment secrets:

- `CLOUDFLARE_API_TOKEN`: a token from Cloudflare → My Profile → API
  Tokens → "Edit Cloudflare Workers" template, scoped to the account and
  the `jha.sh` zone;
- `CLOUDFLARE_ACCOUNT_ID`: the account ID from the Workers overview.

gpuicn's `release` environment already has a pair for ui.imajha.com; the
same token works if it covers `jha.sh`. Claude checks it with the next
release, or a manual run of Release with "Publish the release" ticked.

## 7. The assets repository for screenshots (D6)

New repository `karyalabs/assets`, public, with a README, default branch
`main`; add it to the Claude app's repositories. PR screenshots go there
under `<repo>/<PR number>/`, and matty's CI rejects new images under
`docs/`.

## 8. Later, when convenient

- **One Apple secret set** at organization level, shared with matty and
  gpuicn: `APPLE_CERTIFICATE_P12`, `APPLE_CERTIFICATE_PASSWORD`,
  `APPLE_API_KEY_P8`, `APPLE_API_KEY_ID`, `APPLE_API_ISSUER_ID`,
  `APPLE_TEAM_ID`. Tell Claude when they exist; the workflows then read
  them first, and the per-repository copies can go after a release dry run
  passes. `MATTY_UPDATE_SIGNING_KEY` stays a matty secret.
- **`HOMEBREW_TAP_TOKEN`** in matty, before the first stable release, or
  install the Karya bot on homebrew-matty (done in step 5) and Claude
  switches the cask bump to it.
- **Rename matty's `master` to `main`** (Settings → General → Default
  branch → rename) after Claude's PR that updates the workflows and docs
  merges; GitHub redirects the old name.
- **Fork pull requests**: Organization → Settings → Actions → General →
  "Require approval for all external contributors".
