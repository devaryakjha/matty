#!/usr/bin/env bash
# Writes crates/matty-store/fixtures/db/v<N>.sqlite for the current (latest)
# schema version, for the upgrade tests in crates/matty-store/tests/upgrade.rs.
# Run it once when a migration is added, before that version ships; never
# regenerate the fixture of a version that has shipped.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo run --locked -q -p matty-store --example db_fixture -- "$@"
