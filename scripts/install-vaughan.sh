#!/usr/bin/env bash
# Build THIS checkout and install it as the `vaughan` command.
#
# Installs into the same directory scripts/install.sh uses — $VAUGHAN_INSTALL_DIR,
# default ~/.local/bin — so a dev build replaces whatever the release installer
# put there. It deliberately does not write to ~/.cargo/bin: on a machine where
# both exist, ~/.local/bin usually comes first on PATH, so installing to
# ~/.cargo/bin reports success while `vaughan` keeps running the older copy. That
# failure is silent and costs an afternoon, so this script verifies that the
# binary it just wrote is the one `vaughan` resolves to and fails if it is not.
#
# TWO binaries, matching what users actually get:
#   vaughan    default features — byte-for-byte the shape `npm i -g vaughan` and
#              the GitHub release tarballs ship. No RAILGUN, so no BUSL-1.1
#              `wasmer-compiler-singlepass` in the graph.
#   vaughan-p  --features railgun — the private panel. docs/decisions/railgun-go-no-go.md
#              allows this for development and testing only; a user-facing
#              release with `railgun` is blocked on the Singlepass licence, so
#              this must never be the everyday default command.
#
# Usage:
#   scripts/install-vaughan.sh           # build both, copy into place
#   scripts/install-vaughan.sh --plain   # skip the railgun build (faster)
#   scripts/install-vaughan.sh --link    # symlink instead of copying, so every
#                                        # later `cargo build --release` is live
#                                        # (a `cargo clean` leaves it dangling)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

INSTALL_DIR="${VAUGHAN_INSTALL_DIR:-${HOME}/.local/bin}"
LEGACY_DIR="${HOME}/.cargo/bin"
PLAIN_BIN="target/release/vaughan"
RAILGUN_DIR="target/railgun"
RAILGUN_BIN="${RAILGUN_DIR}/release/vaughan"
LINK_MODE=0
WITH_RAILGUN=1

for arg in "$@"; do
    case "$arg" in
        --link) LINK_MODE=1 ;;
        --plain) WITH_RAILGUN=0 ;;
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

install_one() { # built_path dest_path
    if [ "$LINK_MODE" -eq 1 ]; then
        echo "Linking ${2} -> ${REPO_ROOT}/${1}..."
        ln -sfn "${REPO_ROOT}/${1}" "$2"
    else
        echo "Installing ${2}..."
        cp "$1" "${2}.tmp" && mv -f "${2}.tmp" "$2"
    fi
}

echo "Building release, default features (the shape releases ship)..."
cargo build --release -p vaughan-cli
install_one "$PLAIN_BIN" "$DEST"

# The default command must be free of the private panel, exactly like a release.
plain_count=$(strings "$DEST" | grep -c 'private (RAILGUN)' || true)
if [ "$plain_count" -ne 0 ]; then
    echo "Error: ${DEST} contains 'private (RAILGUN)' (${plain_count} hit(s))." >&2
    echo "       The default build must match the released shape — the feature gate is broken." >&2
    exit 1
fi

if [ "$WITH_RAILGUN" -eq 1 ]; then
    echo ""
    echo "Building release with railgun feature (CARGO_TARGET_DIR=${RAILGUN_DIR})..."
    CARGO_TARGET_DIR="$RAILGUN_DIR" cargo build --release -p vaughan-cli --features railgun
    install_one "$RAILGUN_BIN" "$DEST_P"

    railgun_count=$(strings "$DEST_P" | grep -c 'private (RAILGUN)' || true)
    if [ "$railgun_count" -lt 1 ]; then
        echo "Error: ${DEST_P} reported ${railgun_count} for 'private (RAILGUN)' (expected >= 1)" >&2
        exit 1
    fi
else
    # A stale vaughan-p from an earlier run would silently be the old build.
    if [ -e "$DEST_P" ]; then
        echo ""
        echo "NOTE: --plain skipped the railgun build; ${DEST_P} is left as it was."
    fi
    railgun_count="(not built)"
fi

echo ""
echo "=== Vaughan Binaries Installed ==="
echo "Default command (release shape):"
echo "  Path:     $DEST"
echo "  Version:  $("$DEST" --version 2>/dev/null || echo unknown)"
echo "  Size:     $(wc -c < "$DEST" | tr -d ' ') bytes"
echo "  RAILGUN:  none (${plain_count} hits — matches a release build)"
if [ "$WITH_RAILGUN" -eq 1 ]; then
    echo ""
    echo "Private panel (development/testing only — BUSL, see docs/decisions/railgun-go-no-go.md):"
    echo "  Path:     $DEST_P"
    echo "  Version:  $("$DEST_P" --version 2>/dev/null || echo unknown)"
    echo "  Size:     $(wc -c < "$DEST_P" | tr -d ' ') bytes"
    echo "  RAILGUN:  ${railgun_count} hits ('private (RAILGUN)')"
fi
if [ "$LINK_MODE" -eq 1 ]; then
    echo ""
    echo "Mode: symlinks into ${REPO_ROOT}/target (rebuilds are live)"
fi

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
    echo ""
    echo "NOTE: an older install is still at ${LEGACY_DIR}/vaughan ($("${LEGACY_DIR}/vaughan" --version 2>/dev/null || echo unknown))."
    echo "      Nothing runs it while ${INSTALL_DIR} is earlier on PATH. To remove:"
    echo "        rm ${LEGACY_DIR}/vaughan ${LEGACY_DIR}/vaughan-p"
fi
