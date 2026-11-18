#!/usr/bin/env bash
# Build THIS checkout and install it as the `vaughan` command.
#
# Installs into the same directory scripts/install.sh uses — $VAUGHAN_INSTALL_DIR,
# default ~/.local/bin — so a dev build replaces whatever the release installer
# put there. It deliberately no longer writes to ~/.cargo/bin: on a machine where
# both exist, ~/.local/bin usually comes first on PATH, so installing to
# ~/.cargo/bin reports success while `vaughan` keeps running the older copy. That
# failure is silent and costs an afternoon, so this script now verifies that the
# binary it just wrote is the one `vaughan` resolves to and fails if it is not.
#
# Usage:
#   scripts/install-vaughan.sh           # build release, copy into place
#   scripts/install-vaughan.sh --link    # symlink into place instead of copying,
#                                        # so every later `cargo build --release
#                                        # --features railgun` is live immediately
#                                        # (a `cargo clean` leaves it dangling)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

INSTALL_DIR="${VAUGHAN_INSTALL_DIR:-${HOME}/.local/bin}"
LEGACY_DIR="${HOME}/.cargo/bin"
BUILD_DIR="target/railgun"
BUILT_BIN="${BUILD_DIR}/release/vaughan"
LINK_MODE=0

for arg in "$@"; do
    case "$arg" in
        --link) LINK_MODE=1 ;;
        -h|--help) sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//;$d'; exit 0 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

mkdir -p "$INSTALL_DIR"
DEST="${INSTALL_DIR}/vaughan"
DEST_P="${INSTALL_DIR}/vaughan-p"

# One rolling backup, not a growing pile: the previous binary is the only one
# worth keeping, since anything older is already published as a release tag.
if [ -f "$DEST" ] && [ ! -L "$DEST" ]; then
    echo "Backing up current binary to ${DEST}.prev..."
    cp "$DEST" "${DEST}.prev"
fi

echo "Building release with railgun feature (CARGO_TARGET_DIR=${BUILD_DIR})..."
CARGO_TARGET_DIR="$BUILD_DIR" cargo build --release -p vaughan-cli --features railgun

if [ "$LINK_MODE" -eq 1 ]; then
    echo "Linking ${DEST} -> ${REPO_ROOT}/${BUILT_BIN}..."
    ln -sfn "${REPO_ROOT}/${BUILT_BIN}" "$DEST"
else
    echo "Installing vaughan binary to ${DEST}..."
    cp "$BUILT_BIN" "${DEST}.tmp" && mv -f "${DEST}.tmp" "$DEST"
fi

echo "Creating relative symlink ${DEST_P} -> vaughan..."
(cd "$INSTALL_DIR" && ln -sfn vaughan vaughan-p)

# Verify the feature actually compiled in, through the symlink in --link mode.
vaughan_count=$(strings "$DEST" | grep -c 'private (RAILGUN)' || true)
vaughan_p_count=$(strings "$DEST_P" | grep -c 'private (RAILGUN)' || true)

if [ "$vaughan_count" -lt 1 ]; then
    echo "Error: ${DEST} reported ${vaughan_count} for 'private (RAILGUN)' (expected >= 1)" >&2
    exit 1
fi

if [ "$vaughan_p_count" -lt 1 ]; then
    echo "Error: ${DEST_P} reported ${vaughan_p_count} for 'private (RAILGUN)' (expected >= 1)" >&2
    exit 1
fi

vaughan_bytes=$(wc -c < "$DEST" | tr -d ' ')
vaughan_version=$("$DEST" --version 2>/dev/null || echo "unknown")

echo ""
echo "=== Vaughan Binary Installed Successfully ==="
echo "Primary binary:"
echo "  Path:     $DEST"
echo "  Version:  $vaughan_version"
echo "  Size:     ${vaughan_bytes} bytes"
echo "  Count:    $vaughan_count ('private (RAILGUN)')"
if [ "$LINK_MODE" -eq 1 ]; then
    echo "  Mode:     symlink -> ${REPO_ROOT}/${BUILT_BIN} (rebuilds are live)"
fi
echo ""
echo "Symlink alias:"
echo "  Path:     $DEST_P"
echo "  Target:   $(readlink "$DEST_P")"
echo "  Count:    $vaughan_p_count ('private (RAILGUN)')"

# The whole point of the directory change: prove `vaughan` means THIS build.
echo ""
resolved=$(command -v vaughan 2>/dev/null || true)
if [ -z "$resolved" ]; then
    echo "WARNING: 'vaughan' is not on PATH. Add ${INSTALL_DIR}:" >&2
    echo "  export PATH=\"${INSTALL_DIR}:\$PATH\"" >&2
    exit 1
fi
resolved_dir=$(cd "$(dirname "$resolved")" && pwd)
if [ "$resolved_dir" != "$(cd "$INSTALL_DIR" && pwd)" ]; then
    echo "ERROR: 'vaughan' resolves to ${resolved}, NOT the binary just installed." >&2
    echo "       That copy is earlier on PATH and shadows ${DEST}." >&2
    echo "       Remove or rename it, then re-run:  rm ${resolved}" >&2
    exit 1
fi
echo "Verified: 'vaughan' resolves to ${resolved} (this build)."

# A stale copy in the old install location is harmless only while PATH prefers
# this one — name it so it gets cleaned up rather than rediscovered later.
if [ -e "${LEGACY_DIR}/vaughan" ]; then
    legacy_version=$("${LEGACY_DIR}/vaughan" --version 2>/dev/null || echo "unknown")
    echo ""
    echo "NOTE: an older install is still at ${LEGACY_DIR}/vaughan (${legacy_version})."
    echo "      Nothing runs it while ${INSTALL_DIR} is earlier on PATH. To remove:"
    echo "        rm ${LEGACY_DIR}/vaughan ${LEGACY_DIR}/vaughan-p"
fi
