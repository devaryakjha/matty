# Matty on the web

Matty also runs in the browser, on GPUI's web platform, at
<https://matty.jha.sh>. It is the same app as the desktop build: the same UI,
sync engine and API client, compiled to WebAssembly by `crates/matty-web`.

## What the server needs

The page talks to each Mattermost server from another origin, so a server
admin has to allow it once (**System Console → Web Server → CORS**, or
`config.json`):

| Setting | Value |
| --- | --- |
| `ServiceSettings.AllowCorsFrom` | `https://matty.jha.sh` (space separated, add others as needed) |
| `ServiceSettings.CorsExposedHeaders` | `Token ETag X-Request-Id X-Ratelimit-Limit X-Ratelimit-Remaining X-Ratelimit-Reset` |

Matty reads the session from the `Token` header that signing in returns, and
a browser only shows a page that header when the server exposes it. Without
these, the server check fails with "Matty could not reach that server".

The dev server (`scripts/dev-server`) already allows `http://127.0.0.1:8080`
and `http://localhost:8080`, which `scripts/web serve` uses.

## Building and running locally

```sh
scripts/web setup        # once: nightly toolchain, wasm-bindgen CLI, and on macOS Homebrew llvm + binaryen
scripts/web serve        # release build, then http://127.0.0.1:8080
scripts/web serve --dev  # faster build, ~200 MB of WebAssembly
```

Open `http://127.0.0.1:8080/?preview=1` for the design fixtures
(`MATTY_PREVIEW=1` on the desktop). `?backend=webgl` skips WebGPU, which
headless browsers need for screenshots.

Why nightly: GPUI's web platform runs work on Web Workers that share the
app's WebAssembly memory, which needs a standard library rebuilt with
atomics (`-Z build-std`). The page must be cross-origin isolated (the
`Cross-Origin-Opener-Policy` and `Cross-Origin-Embedder-Policy` headers in
`crates/matty-web/web/_headers`) for the browser to allow that memory.

## The demo workspace

With no account signed in, the page opens a sample workspace instead of the
sign-in card: Lumen Labs, a made-up company of twelve people a few weeks
before a launch, with 22 channels, direct and group messages, threads,
reactions, pinned and saved posts, images and files, a CI bot's message
attachments, unread channels and mentions, and a **Formatting Tour**
channel with one post per kind of message. A banner says it is a demo and
offers **Connect your server**, which goes to the sign-in card. `?demo=0`
skips it.

Nothing in the demo talks to a server. It is a snapshot of the cache the
desktop app would have after signing in to a seeded server: one SQLite file,
`crates/matty-web/demo/demo.sqlite` (about 4 MB, 6,000 posts), with the
avatars and images inside it. `scripts/web build` gzips it next to the app
(`demo.sqlite.gz`, about 1 MB), the page downloads both side by side, and
the app opens the snapshot in memory and moves every timestamp forward so
the story always reads as the last three weeks, with about half a year of
older history (`scripts/demo/history.py`) behind it. Scrolling up pages
through that history from the snapshot. What you send stays in the outbox,
since there is no server to take it.

To change it, edit `scripts/demo/content.py` (people, channels and every
post), then rebuild the snapshot and commit it:

```sh
scripts/demo/run   # fresh dev server, scripts/demo/seed.py, then matty-demo snapshots it
```

It needs Docker (for `scripts/dev-server`) and Pillow (`pip install pillow`)
for the generated avatars and images. `matty-demo` (`crates/matty-demo`) is
the snapshot tool: it signs in as the demo user and has the sync engine load
every channel, thread, avatar and image the way the app would. A snapshot
from an older schema still opens, because the store migrates it on load.

## Deploying to matty.jha.sh (manual, for now)

The site is a Cloudflare Worker that only serves static assets
(`crates/matty-web/wrangler.jsonc`), with `matty.jha.sh` as its custom
domain. Once:

1. `scripts/web setup`.
2. The `jha.sh` zone must be on the Cloudflare account you deploy with.
   Wrangler creates the `matty.jha.sh` DNS record and certificate on the first
   deploy because of `"custom_domain": true`.

Then, each time:

```sh
scripts/web deploy
```

It builds the release, checks every file is under Cloudflare's 25 MiB limit,
and runs `npx wrangler@4 deploy`. The first run opens a browser to sign in
to Cloudflare; on a machine without one, set `CLOUDFLARE_API_TOKEN` (a token
with **Workers Scripts: Edit** and **Zone: Workers Routes: Edit** on
`jha.sh`) and `CLOUDFLARE_ACCOUNT_ID`.

Without the toolchain: every CI run uploads the release build as the
`matty-web` artifact. Unzip it into `crates/matty-web/dist` and run
`cd crates/matty-web && npx wrangler@4 deploy`.

## Performance

Everything, including the sync engine, runs on the page's one thread, and a
GPUI frame costs a few times what it does natively, so the web build is
where slow frames show first. What keeps it smooth:

- The `web` build profile uses `opt-level = "s"`: `"z"` made every frame
  about 30% slower for 15% less download.
- The sidebar and the right pane are cached views (`cached_pane` in
  `main_window.rs`): scrolling or hovering the channel doesn't lay them out
  again. A view that reads a global must be refreshed when it changes
  (`apply_theme` refreshes the windows).
- Transcript rows read posts from the shared channel model instead of a
  copy per frame, and only the hovered or selected row builds the hover
  toolbar.

To measure, `node scripts/web-bench.mjs <label> <dist dir or URL> --headed`
drives Chrome (through Playwright) over a build or the live site: startup,
wheel scrolling, hovering, typing, with the main-thread time of every frame.
`--trace` also writes a Chrome trace per scenario.

## What differs from the desktop

- **Storage.** Settings and the session token live in the origin's
  `localStorage`; the message cache is SQLite in memory, so each visit syncs
  from the server again. Persisting the cache (SQLite on OPFS) is follow-up
  work.
- **Network.** Requests are browser `fetch` calls and the WebSocket is the
  browser's. The browser does TLS with its own trust store, follows
  redirects itself, and cannot send headers on the WebSocket, so the socket
  signs in with `authentication_challenge` only.
- **Runtime.** There are no threads for the sync engine, so it runs on the
  page's event loop (`matty_api::rt`). Timers go through `matty_api::rt`
  too, since tokio's have no driver in the browser.
- **Fonts.** GPUI's web text system has no system fonts and falls back to
  IBM Plex Sans, which the page bundles; Matty's own text uses Geist.
- **Not yet on the web.** Attaching files (no file picker from GPUI yet),
  desktop notifications, the tray and opening downloaded files. Downloads go
  through the browser's own download.
