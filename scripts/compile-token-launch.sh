#!/usr/bin/env bash
# Compile FixedSupplyToken and refresh pinned creation bytecode.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$WORK/src"
cp "$ROOT/scripts/token-launch/FixedSupplyToken.sol" "$WORK/src/"
cat > "$WORK/foundry.toml" << 'EOF'
[profile.default]
src = "src"
out = "out"
bytecode_hash = "none"
evm_version = "paris"
optimizer = true
optimizer_runs = 200
solc_version = "0.8.23"
EOF

(cd "$WORK" && forge build --silent)
python3 - << 'PY' "$WORK/out/FixedSupplyToken.sol/FixedSupplyToken.json" "$ROOT/scripts/token-launch/FixedSupplyToken.creation.hex"
import json, sys
artifact = json.load(open(sys.argv[1]))
hex_str = artifact["bytecode"]["object"]
if hex_str.startswith("0x"):
    hex_str = hex_str[2:]
open(sys.argv[2], "w").write(hex_str + "\n")
print(f"Wrote {sys.argv[2]} ({len(hex_str)//2} bytes)")
PY

echo "OK: refresh scripts/token-launch/FixedSupplyToken.creation.hex"
