#!/usr/bin/env bash
# Fails when a crate that must stay plain Rust (PLAN §6.1) depends on GPUI,
# directly or through another crate.
set -euo pipefail

status=0
for crate in matty-api matty-store matty-sync matty-markdown; do
    if cargo tree --locked -p "$crate" -e normal,build --prefix none \
        | grep -Eq '^(gpui-pre|gpui-kit|gpui-component|gpuicn|gpui-icons) '; then
        echo "error: $crate depends on GPUI:" >&2
        cargo tree --locked -p "$crate" -e normal,build -i gpui-pre 2>/dev/null >&2 || true
        status=1
    else
        echo "ok: $crate has no GPUI dependency"
    fi
done
exit "$status"
