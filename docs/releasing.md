# Releasing Matty

A release is a pull request. This follows D5 and D7 of
[engineering-process.md](engineering-process.md): one beta a week from a
green master, plus a hotfix beta when a release-blocking fix lands.

## Cutting a release

1. Claude opens the release PR from master, titled "Release 0.1.0-beta.N":
   - `version` in `Cargo.toml`'s `[workspace.package]` becomes the new
     version (pre-release suffix included), and `Cargo.lock` follows
     (`cargo metadata --format-version 1 >/dev/null` updates it);
   - `CHANGELOG.md` gets a `## [0.1.0-beta.N] - YYYY-MM-DD` entry written
     by hand from what merged since the last release, and the link at the
     bottom.
2. Merging it is the go. `release.yml` sees an unreleased version on
   master and builds macOS (signed, notarized), Linux and Windows from that
   commit, waits for that commit's `ci` check to pass, then creates the
   tag `v<version>` and the GitHub release with the changelog entry on top,
   signs the update manifest, bumps the Homebrew cask (stable versions
   only) and deploys matty.jha.sh.
3. Nothing else is pushed by hand: no tags.

Every step skips work already done, so after a failure, fix master and run
**Release** by hand with "Publish the release" ticked; it finishes the
same version from the latest master. A version that is already released
never publishes again; bump the version instead.

## Dry runs

Running **Release** by hand without ticking "Publish the release" builds,
signs and notarizes everything and publishes nothing. The same dry run
runs every Monday at 03:17 UTC on master, so a broken step shows up before
release day.

## Versions

`Cargo.toml` is the one source of the version: `matty --version`, the
update manifest, the packages and the tag all read it. Betas are
`0.1.0-beta.N`; the Linux packages turn the `-` into `~` for rpm, and the
MSI keeps only `0.1.0`.

## Settings this needs (Arya, once)

- The `v*` tag ruleset must let the release workflow create tags: karya-karta,
  the Karya bot app, is on its bypass list, and its `KARYA_BOT_APP_ID` and
  `KARYA_BOT_PRIVATE_KEY` organization secrets reach matty
  ([org-settings.md](org-settings.md) steps 4 and 5).
- `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` in the `release`
  environment, for the matty.jha.sh deploy.
- `HOMEBREW_TAP_TOKEN`, before the first stable release.
