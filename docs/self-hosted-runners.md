# Self-hosted runners

Matty's Linux and macOS CI jobs and its release builds can run on two of
Arya's machines instead of GitHub's runners: an Ubuntu box at OVH for Linux
and a Mac mini for macOS. Windows has no machine of its own and stays on
GitHub's runners, kept to a minimum (docs/CI.md).

## How the switch works

Two repository variables decide where the jobs run. Set them at
<https://github.com/devaryakjha/matty/settings/variables/actions>:

| Variable | Value once the runners are up | Unset |
| --- | --- | --- |
| `MATTY_LINUX_RUNNER` | `["self-hosted","Linux","X64","matty"]` | `ubuntu-latest` |
| `MATTY_MACOS_RUNNER` | `["self-hosted","macOS","ARM64","matty"]` | `macos-14` |

The value is a `runs-on` value written as JSON. CI reads it in its `changes`
job, the release workflow reads it directly. Deleting a variable sends the
jobs back to GitHub's runners at once, with no pull request: do that when a
machine is down for longer than a few minutes, since a job waits up to a day
for an offline runner.

What runs where once both are set:

| Job | Runner |
| --- | --- |
| CI `linux`, `web`, `linux packages`, `integration` | OVH box |
| CI `macos`, `macos universal bundle` | Mac mini |
| CI `changes`, `deny`, `ci` | GitHub's Ubuntu (seconds to a minute each) |
| CI `windows`, `windows installer` | GitHub's Windows |
| Release `macos universal` (signing, notarization) | Mac mini |
| Release `linux x86_64` | OVH box |
| Release `windows x86_64`, `GitHub Release`, `Homebrew cask bump` | GitHub's runners |
| `record fixtures` (manual) | GitHub's Ubuntu |

## Before anything else: keep fork pull requests off the machines

Matty is a public repository. A pull request from a fork can change the
workflow files it runs with, so it could ask for these runners by label and
run its own code on Arya's machines. CI already sends fork pull requests to
GitHub's runners, but only an approval step stops an edited workflow:

1. Open <https://github.com/devaryakjha/matty/settings/actions>.
2. Under **Approval for running fork pull request workflows from
   contributors**, choose the strictest option, **Require approval for all
   external contributors** (older pages say "outside collaborators").
3. Save.

Then read a fork's workflow changes before approving its run. The runners
run as an unprivileged user, but that user is in the `docker` group on the
OVH box, which is as good as root there.

## Linux: the OVH box

The script installs the packages the jobs expect (the ones GitHub's image
has), Docker if missing, Rust 1.99.1 and the web build's nightly, creates a
`matty-ci` user and registers the runners as systemd services.

1. Open <https://github.com/devaryakjha/matty/settings/actions/runners/new>,
   pick **Linux**, and copy the token from the `./config.sh` line (it starts
   with `A` and is valid for an hour). Do not run the commands on that page.
2. On the box:

   ```sh
   git clone --depth 1 https://github.com/devaryakjha/matty /tmp/matty-setup
   sudo /tmp/matty-setup/scripts/ci/setup-linux-runners --token <TOKEN>
   ```

   It registers one runner for every 4 cores and 8 GB of memory, up to 4;
   pass `--count N` to choose. Each runner runs one job at a time, so with 3
   a pull request's `linux`, `web` and two `integration` jobs run almost all
   at once. Plan for about 60 GB of disk per runner plus 10 GB for images.
3. Check the runners show as **Idle** at
   <https://github.com/devaryakjha/matty/settings/actions/runners>.
4. Set `MATTY_LINUX_RUNNER` to `["self-hosted","Linux","X64","matty"]`.

Running the script again with a fresh token adds runners up to the count and
updates the packages and toolchains.

## macOS: the Mac mini

Register a separate runner for Matty next to the ones the other repositories
use. A runner belongs to one repository, so this does not touch them.

1. Make sure the Mac has:
   - Xcode or its command line tools (`xcode-select --install`); the
     release job needs `xcrun notarytool` and `stapler`.
   - rustup with Matty's toolchain and both macOS targets:

     ```sh
     curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
     ~/.cargo/bin/rustup toolchain install 1.99.1 --profile minimal \
       -c rustfmt,clippy -t aarch64-apple-darwin,x86_64-apple-darwin
     ```

   - `gh` (`brew install gh`): the macOS job uses it to open the "macOS CI is
     failing on master" issue.
   - Optionally Rosetta (`softwareupdate --install-rosetta --agree-to-license`),
     so the universal bundle job also runs the Intel slice.
2. Open <https://github.com/devaryakjha/matty/settings/actions/runners/new>,
   pick **macOS** and **ARM64**, and copy the version in the download URL and
   the token.
3. In Terminal, as the user the other runners use:

   ```sh
   mkdir ~/actions-runner-matty && cd ~/actions-runner-matty
   curl -sSfLO https://github.com/actions/runner/releases/download/v<VERSION>/actions-runner-osx-arm64-<VERSION>.tar.gz
   tar xzf actions-runner-osx-arm64-<VERSION>.tar.gz
   ./config.sh --unattended --url https://github.com/devaryakjha/matty \
     --token <TOKEN> --name mac-mini-matty --labels matty --work _work
   echo "$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin" > .path
   ./svc.sh install && ./svc.sh start
   ```

   The service is a LaunchAgent, so that user must stay logged in, and the
   Mac must not sleep (System Settings, Energy: prevent automatic sleeping).
   Signing needs the logged-in session too.
4. Check it shows as **Idle**, then set `MATTY_MACOS_RUNNER` to
   `["self-hosted","macOS","ARM64","matty"]`.

One runner means one macOS job at a time. On master each merge queues the
`macos` and `macos universal bundle` jobs, and only the newest waiting one
of each is kept (docs/CI.md), so a burst of merges does not pile up.

The release job imports the Developer ID certificate into a temporary
keychain in the job's temp folder and deletes it at the end, even when the
job fails, so the certificate never stays on the Mac.

## Upkeep

The jobs look after themselves:

- **Warm builds.** On a self-hosted runner the checkout is reused and
  `target/` is kept between jobs, so cargo builds incrementally instead of
  restoring GitHub's cache (rust-cache is skipped there).
  `scripts/ci/prepare-workspace` runs after every checkout: it deletes every
  other untracked file, empties `target/dist` so no job uploads an older
  package, and deletes `target/` once it passes 60 GB. To change that limit,
  add `MATTY_CI_TARGET_MAX_GB=40` to the runner folder's `.env` and restart
  its service.
- **Tools.** wasm-bindgen, wasm-opt and appimagetool are downloaded once per
  runner into its tool cache (`scripts/ci/fetch-tool`). Debian packages are
  installed only if missing (`scripts/ci/apt-install`), so the jobs never
  need sudo on the box.
- **Test servers.** Each integration job starts its Mattermost server under
  its own Compose project and port, and removes it with its volumes when it
  ends. A daily cron job on the OVH box (`/usr/local/bin/matty-ci-cleanup`)
  removes any a cancelled job left, prunes dangling images and deletes
  runner logs older than two weeks. Mattermost images are kept, so later
  runs skip Docker Hub.
- **Runner updates.** The runners update themselves when GitHub ships a new
  version.

If jobs sit at "Waiting for a runner", the machine or its service is down:
run `sudo ./svc.sh status` in the runner's folder (`~matty-ci/matty-runner-1`
on the box, `~/actions-runner-matty` on the Mac), or delete the variable
to fall back to GitHub's runners.
