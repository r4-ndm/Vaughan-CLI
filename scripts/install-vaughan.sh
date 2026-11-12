#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

CARGO_BIN_DIR="${HOME}/.cargo/bin"
mkdir -p "$CARGO_BIN_DIR"

# 1. Backup current binary
if [ -f "${CARGO_BIN_DIR}/vaughan" ]; then
    echo "Backing up current binary to ${CARGO_BIN_DIR}/vaughan.pre-two-build..."
    cp "${CARGO_BIN_DIR}/vaughan" "${CARGO_BIN_DIR}/vaughan.pre-two-build"
fi

# 2. Build single release with railgun feature
echo "Building release with railgun feature (CARGO_TARGET_DIR=target/railgun)..."
CARGO_TARGET_DIR=target/railgun cargo build --release -p vaughan-cli --features railgun

# 3. Install binary
VAUGHAN_DEST="${CARGO_BIN_DIR}/vaughan"
VAUGHAN_P_DEST="${CARGO_BIN_DIR}/vaughan-p"

echo "Installing vaughan binary to ${VAUGHAN_DEST}..."
cp target/railgun/release/vaughan "${VAUGHAN_DEST}.tmp" && mv -f "${VAUGHAN_DEST}.tmp" "$VAUGHAN_DEST"

# 4. Create relative symlink vaughan-p -> vaughan
echo "Creating relative symlink ${VAUGHAN_P_DEST} -> vaughan..."
(cd "$CARGO_BIN_DIR" && ln -sfn vaughan vaughan-p)

# 5. Verify string counts and symlink
vaughan_count=$(strings "$VAUGHAN_DEST" | grep -c 'private (RAILGUN)' || true)
vaughan_p_count=$(strings "$VAUGHAN_P_DEST" | grep -c 'private (RAILGUN)' || true)
vaughan_p_link=$(readlink "$VAUGHAN_P_DEST")

if [ "$vaughan_count" -lt 1 ]; then
    echo "Error: ${VAUGHAN_DEST} reported ${vaughan_count} for 'private (RAILGUN)' (expected >= 1)" >&2
    exit 1
fi

if [ "$vaughan_p_count" -lt 1 ]; then
    echo "Error: ${VAUGHAN_P_DEST} reported ${vaughan_p_count} for 'private (RAILGUN)' (expected >= 1)" >&2
    exit 1
fi

vaughan_size=$(ls -lh "$VAUGHAN_DEST" | awk '{print $5}')
vaughan_bytes=$(stat -c '%s' "$VAUGHAN_DEST")

echo ""
echo "=== Vaughan Binary Installed Successfully ==="
echo "Primary binary:"
echo "  Path:     $VAUGHAN_DEST"
echo "  Size:     $vaughan_size (${vaughan_bytes} bytes)"
echo "  Count:    $vaughan_count ('private (RAILGUN)')"
echo ""
echo "Symlink alias:"
echo "  Path:     $VAUGHAN_P_DEST"
echo "  Target:   $vaughan_p_link"
echo "  Count:    $vaughan_p_count ('private (RAILGUN)')"
