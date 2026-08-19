#!/usr/bin/env bash
# CREATE2-deploy the canonical ERC-5564 announcer (same address as Ethereum).
# Usage:
#   PRIVATE_KEY=0x... ./scripts/deploy-erc5564-announcer.sh
# Default RPC is PulseChain testnet v4 (943). Override with RPC_URL.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RPC_URL="${RPC_URL:-https://rpc.v4.testnet.pulsechain.com}"
FACTORY="0x4e59b44847b379578588920cA78FbF26c0B4956C"
SALT="0xd0103a290d760f027c9ca72675f5121d725397fb2f618f05b6c44958b25b4447"
EXPECTED="0x55649E01B5Df198D18D95b5cc5051630cfD45564"

if [[ -z "${PRIVATE_KEY:-}" ]]; then
  echo "Set PRIVATE_KEY to an account that has a little tPLS (testnet) or PLS (mainnet)." >&2
  exit 1
fi

if command -v cast >/dev/null && command -v forge >/dev/null; then
  :
else
  echo "Need foundry (cast + forge) on PATH." >&2
  exit 1
fi

existing="$(cast codesize "$EXPECTED" --rpc-url "$RPC_URL")"
if [[ "$existing" != "0" ]]; then
  echo "Announcer already deployed at $EXPECTED (codesize $existing)."
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/src"
cat > "$work/foundry.toml" << 'EOF'
[profile.default]
src = "src"
out = "out"
bytecode_hash = "none"
evm_version = "paris"
optimizer = true
optimizer_runs = 10000000
solc_version = "0.8.23"
EOF
cp "$ROOT/scripts/erc5564/ERC5564Announcer.sol" "$work/src/"
(cd "$work" && forge build --silent)

init="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["bytecode"]["object"])' \
  "$work/out/ERC5564Announcer.sol/ERC5564Announcer.json")"

computed="$(cast create2 --deployer "$FACTORY" --salt "$SALT" --init-code "$init")"
if [[ "${computed,,}" != "${EXPECTED,,}" ]]; then
  echo "CREATE2 mismatch: got $computed want $EXPECTED (compiler drift?)" >&2
  exit 1
fi

payload="$(cast concat-hex "$SALT" "$init")"
from="$(cast wallet address --private-key "$PRIVATE_KEY")"
echo "Deploying from $from via $FACTORY → $EXPECTED"
echo "RPC $RPC_URL"
cast send "$FACTORY" "$payload" --rpc-url "$RPC_URL" --private-key "$PRIVATE_KEY"

size="$(cast codesize "$EXPECTED" --rpc-url "$RPC_URL")"
if [[ "$size" == "0" ]]; then
  echo "Deploy tx sent but $EXPECTED still empty." >&2
  exit 1
fi
echo "OK: announcer live at $EXPECTED (codesize $size)"
