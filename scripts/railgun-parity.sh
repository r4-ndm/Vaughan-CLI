#!/usr/bin/env bash
# Run railgun-rust's official-engine parity tests and its railgun-wallet /
# broadcaster-core suites at the commit Vaughan pins, resolved with Vaughan's
# Cargo.lock (so they exercise the exact dependency versions we ship).
#
# Required on every pin bump and Alloy upgrade; see docs/railgun-go-no-go.md.
# Usage: ./scripts/railgun-parity.sh

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO="https://github.com/triamazikamno/railgun-rust"
REV="$(grep -m1 -oE 'railgun-wallet = \{[^}]*rev = "[0-9a-f]{40}"' "$ROOT/Cargo.toml" | grep -oE '[0-9a-f]{40}')"
if [[ -z "$REV" ]]; then
  echo "could not read the railgun-wallet rev from Cargo.toml" >&2
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

git -C "$WORK" init -q
git -C "$WORK" remote add origin "$REPO"
git -C "$WORK" fetch -q --depth 1 origin "$REV"
git -C "$WORK" checkout -q FETCH_HEAD

cp "$ROOT/Cargo.lock" "$WORK/Cargo.lock"
echo "railgun-rust $REV with Vaughan's Cargo.lock"
(cd "$WORK" && CARGO_TARGET_DIR="$ROOT/target/railgun-parity" \
  cargo test -p railgun-wallet -p broadcaster-core)
