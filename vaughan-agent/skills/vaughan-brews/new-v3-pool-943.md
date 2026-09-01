# New V3 pool on 943 — Brew checklist

1. Unlock Vaughan **Human → Advisor** on Pulse testnet v4.
2. Resolve **user's** token addresses — [`token-resolve`](../token-resolve/SKILL.md) (paste `0x`, confirm symbol/decimals).
3. Confirm balances for **both** sides — [`balance-preflight`](../balance-preflight/SKILL.md).
4. When pool does not exist yet — **gas preflight** [`gas-preflight.md`](gas-preflight.md) · [`lp-gas-preflight`](../lp-gas-preflight/SKILL.md) (createPool needs **≥ 6M** gas in proposal).
5. `discover_v3_pool_fee` — if no pool exists, ask user for fee tier (100 / 500 / 2500 / 10000 / 20000 bps).
6. `propose_v3_lp_deploy` **once** with:
   - `token_a`, `token_b` — checksummed `0x` addresses preferred
   - `price` — token_b per token_a (user display order)
   - `deposit`, `deposit_token`
   - `fee` when creating a new pool or when discovery fails
   - `explanation`
7. **Gate:** proposal `gas_limit` ≥ 6_000_000 for createPool — else rebuild Vaughan + re-propose.
8. User approves each TUI card; Vaughan auto-enqueues the rest.
9. After createPool **y** — `discover_v3_pool_fee` must show pool (hash alone is not enough).
10. `list_v3_positions` to verify NFT.
11. If any step fails (ghost card, mint missing, MCP timeout) → [`lp-brew-incidents`](../lp-brew-incidents/SKILL.md).

**Run Vaughan from workspace build:** `cargo run -p vaughan-cli` (not stale `~/.local/bin/vaughan`).

Example Cursor prompt (generic): *"Brew full-range LP: 100 of token A, price 1 A = 0.2 B, 2% fee, wiz4rd 943 — tokens are 0x… and 0x…"*

Dev smoke tour (not a user preset): [`docs/examples/lp-brew-smoke-943.example.json`](../../../docs/examples/lp-brew-smoke-943.example.json).
