# Vaughan AI Tool Surface (MCP v1)

Public contract for external agents (Cursor, Claude Code, Codex, etc.) using
Vaughan via MCP or `vaughan --json` CLI. Keys never leave the TUI process;
write tools **propose only** and require explicit human approval.

## Architecture (v1)

- **v1:** Hybrid IPC — TUI owns `WalletState`; MCP proposes via loopback socket
  (TUI unlocked) or `proposals/pending/` file queue (TUI offline).
- **v2 (deferred):** `vaughan serve` wallet daemon; TUI/MCP/CLI become thin clients.

See [`mcp-threat-model.md`](mcp-threat-model.md) for security controls.

## Grant levels

| Level | Profile | Read tools | Write tools | Signing |
|-------|---------|------------|-------------|---------|
| **Default (v1)** | `default` | Yes | Propose only | Human in TUI |
| **Degen (deferred)** | `degen` | Yes | Policy-bound auto | Circuit breakers |

v1 ships **default profile only** for MCP writes.

## Read tools (no approval)

| Tool | Description |
|------|-------------|
| `get_balance` | Native balance for active account or explicit `address` |
| `list_assets` | Native + known ERC-20 balances |
| `get_network` | Active network id, chain id, RPC label |
| `get_address` | Active account address (requires unlocked TUI or explicit session) |
| `inspect_contract` | Capability fingerprint + ABI resolution |
| `simulate_call` | `eth_call` pre-flight |
| `get_dex_reserves` | Pair/pool reserves |
| `search_pairs` | Factory log scan for pairs |
| `quote_swap` | Pulse aggregator quote (Squirrel / PulseSwap / Piteas) — read-only |
| `get_v3_pool` | wiz4rd V3 pool slot0 / liquidity (Pulse testnet 943) |
| `quote_v3_swap` | wiz4rd V3 exact-in quote (local math on live pool) |

When the vault is locked and no explicit `address` is passed, read tools return
`wallet_locked` with guidance to unlock Vaughan or pass `account_address`.

## Write tools (propose only)

| Tool | v1 | Description |
|------|----|-------------|
| `propose_transfer` | Yes | Native or ERC-20 transfer |
| `propose_contract_call` | Yes | Arbitrary contract call |
| `propose_swap` | Yes | Direct V2/PulseX router swap (path + amounts) |
| `propose_agg_swap` | Yes | Aggregator quote → proposal (allowlisted routers only) |
| `propose_v3_swap` | Yes | wiz4rd V3 exact-in swap (allowlisted SwapRouter on 943) |
| `propose_batch_7702` | Deferred | EIP-7702 batched send |

Write tools return a `proposal_id` and `status: pending_user`. They never sign
or broadcast.

## Meta tools

| Tool | Description |
|------|-------------|
| `get_proposal_status` | `pending_user`, `approved`, `rejected`, `expired` + optional `tx_hash` |
| `list_pending_proposals` | All pending proposals for the active profile |

## Hard bans

The following must **never** appear in `tools/list` or execute:

- `sign_*`, `broadcast_*`
- `export_key`, `export_mnemonic`, `unlock`, `set_password`
- Any tool that returns key material or vault passwords

Enforced by integration tests in `vaughan-mcp`.

## Structured errors

Machine-readable `error.code` values (stable for agent self-correction):

| Code | Meaning |
|------|---------|
| `wallet_locked` | Vault locked; unlock TUI or pass explicit address |
| `network_mismatch` | Proposal `chain_id` ≠ active network |
| `proposal_expired` | TTL exceeded (default 10 min) |
| `simulation_reverted` | Re-simulation at approve failed |
| `user_rejected` | Human denied the approval card |
| `mainnet_blocked` | Write to mainnet without `VAUGHAN_MCP_ALLOW_MAINNET=1` |
| `hmac_invalid` | Tampered queue file rejected |
| `invalid_tool_call` | Bad arguments |
| `tui_offline` | Socket attach failed; proposal queued to disk |

## JSON shapes

### Proposal (returned by write tools)

```json
{
  "proposal_id": "prop_12345",
  "status": "pending_user",
  "chain_id": 943,
  "to": "0x…",
  "value_wei": "10000000000000000",
  "calldata": "0x",
  "gas_limit": 65000,
  "simulation_success": true,
  "explanation": "Send 0.01 tPLS to …"
}
```

`explanation` is **untrusted** agent text. The approval card shows decoded
calldata from `wiz4rd-engine` / Alloy independently.

### CLI JSON envelope

```json
{ "ok": true, "data": { … } }
{ "ok": false, "error": { "code": "wallet_locked", "message": "…" } }
```

## MCP client allowlist (optional, Settings)

Stored in vault JSON: `cursor`, `claude`, `codex`, `custom`.
Unknown `client_id` on socket attach is rejected when allowlist is non-empty.
