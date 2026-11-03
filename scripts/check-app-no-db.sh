#!/usr/bin/env bash
# Fails when matty-app depends on the database directly (PLAN §6.2): the
# SQLite connection lives on the sync thread, so the app reaches the cache
# only through matty-sync's snapshots.
set -euo pipefail

direct=$(cargo tree --locked -p matty-app -e normal --depth 1 --prefix none | tail -n +2)
if grep -Eq '^(rusqlite|matty-store|libsqlite3-sys) ' <<<"$direct"; then
    echo "error: matty-app depends on the database directly:" >&2
    grep -E '^(rusqlite|matty-store|libsqlite3-sys) ' <<<"$direct" >&2
    exit 1
fi
echo "ok: matty-app reaches the database only through matty-sync"
