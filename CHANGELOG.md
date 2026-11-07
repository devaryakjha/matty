# Changelog

Every release of Matty, newest first. A release PR adds the entry and bumps
`version` in `Cargo.toml` in the same commit; merging it publishes the
release ([docs/releasing.md](docs/releasing.md)). The entry becomes the top
of the GitHub release notes. Headings follow
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/): Added, Changed,
Fixed, Removed.

## [Unreleased]

## [0.1.0-beta.6] - 2026-11-07

The first release cut through a release PR, and the first from the
karyalabs organization.

### Added

- Jumping to a permalink for a channel you're not in offers to join it.
- Groundwork for translations: the sidebar, channel view, right pane,
  threads, Preferences, slash commands, embeds and pop-out windows read
  their text from Fluent files (English only for now; #225).

### Changed

- The Threads header's "Mark all as read" is an icon with a tooltip.
- gpuicn 0.9.7, with its built-in strings following the app's locale.
- Repository, release and update links point at `karyalabs/matty`.
- Releases are cut by merging a release PR instead of pushing a tag; the
  version in `Cargo.toml` now carries the pre-release suffix.

## [0.1.0-beta.5] - 2026-11-05

The beta 3 review fixes (#612). Earlier betas: see the
[GitHub releases](https://github.com/karyalabs/matty/releases).

[Unreleased]: https://github.com/karyalabs/matty/compare/v0.1.0-beta.6...HEAD
[0.1.0-beta.6]: https://github.com/karyalabs/matty/releases/tag/v0.1.0-beta.6
[0.1.0-beta.5]: https://github.com/karyalabs/matty/releases/tag/v0.1.0-beta.5
