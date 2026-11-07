# Upstream pins

Matty pins its UI stack exactly. These versions move together: gpuicn is
built and tested against one GPUI Kit release, which in turn pins one
`gpui-pre` runtime. **Recheck before each milestone and never upgrade
silently**: a pin change is its own pull request that updates this table.

| Dependency | Pin | Source of truth | Checked |
| --- | --- | --- | --- |
| Rust toolchain | 1.99.1 (`rust-toolchain.toml`; MSRV 1.98) | gpuicn `release-pins.toml` `[rust]` | 2026-11-03, Claude for Arya |
| gpuicn | `=0.9.11` (tag `v0.9.11`), crate mode | crates.io `gpuicn` | 2026-11-07, Claude for Arya |
| GPUI Kit | `gpui-kit =0.9.0` (`7591a35`) | gpuicn `release-pins.toml` `[gpui_kit]` | 2026-11-03, Claude for Arya |
| GPUI runtime | `gpui-pre-platform =0.3.11` (`gpui-pre` `f816e1f`) | gpuicn `release-pins.toml` `[gpui]` | 2026-11-03, Claude for Arya |
| gpui-icons | `=0.6.0` (Lucide 1.63.0) | gpuicn `release-pins.toml` `[gpui_icons]` | 2026-11-03, Claude for Arya |
| Geist fonts | 1.7.1, SHA-256 checked by `gpuicn init` | gpuicn `release-pins.toml` `[geist]` | 2026-11-03, Claude for Arya |
| Web toolchain | `nightly-2026-08-17` with `rust-src` (atomics, `-Z build-std`), wasm-bindgen CLI = Cargo.lock's `wasm-bindgen`, binaryen 132 | gpuicn `release-pins.toml` `[wasm]`; `scripts/web` | 2026-11-04, Claude for Arya |

Gaps Matty reported in gpuicn, and their status, are listed in [`gpuicn-gaps.md`](gpuicn-gaps.md).

## How gpuicn is consumed

`crates/matty-app` uses gpuicn in **crate mode** (`gpuicn.toml`,
`mode = "crate"`): `src/ui/mod.rs` re-exports the `gpuicn` crate, so code
reads `ui::button::Button` and so on. A component is ejected (`gpuicn eject
<name>`) only to fix a bug that is also reported upstream; Matty never keeps a
local substitute for a gpuicn component.

Import GPUI types through `gpui_kit` so Matty and gpuicn share one set of GPUI
types.

## Changing a pin

1. Read the gpuicn release notes and its `release-pins.toml` for the new
   release.
2. Update `[workspace.dependencies]` in `Cargo.toml`, `rust-toolchain.toml`
   and the CI toolchain together, then this table with the date and who
   checked.
3. Run the full CI locally (`cargo clippy`, `cargo test`, `scripts/check-no-gpui.sh`)
   and attach before/after screenshots when anything visible changes.
