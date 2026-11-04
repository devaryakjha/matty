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
