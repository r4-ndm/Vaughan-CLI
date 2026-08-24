# Vaughan AI Tool Surface (MCP v1)

Public contract for external agents (Cursor, Claude Code, Codex, etc.) using
Vaughan via MCP or `vaughan --json` CLI. Keys never leave the Vaughan process
(MCP never unlocks or exports a vault).

**Safeguard model:** **role split** + whose seed (see [`agent-roles.md`](agent-roles.md)).

- **Adviser** (`default` / MCP `vaughan`) — human uses Vaughan; agent advises;
  propose → approve.
- **Sentient** (`sentient` / MCP `vaughan-sentient`) — agent’s own seed; full
  control under policy. Human may partner by sharing that seed.

## Architecture (v1)

- **v1:** Hybrid IPC — TUI owns `WalletState`; MCP proposes (or, on `sentient`,
  requests auto-exec) via loopback socket / file queue.
- **v2 (deferred):** `vaughan serve` wallet daemon; TUI/MCP/CLI become thin clients.

See [`mcp-threat-model.md`](mcp-threat-model.md) for security controls.

## Grant levels

| Level | Role | Profile | MCP name | Read | Writes | Signing |
|-------|------|---------|----------|------|--------|---------|
| **Adviser** | Human-led | `default` | `vaughan` | Yes | Propose only | Human in TUI |
| **Sentient** | Agent-led | `sentient` | `vaughan-sentient` | Yes | Same verbs; auto under policy | Vaughan signs for that seed |

**Shipped today:** Adviser MCP (`default` propose → approve).  
**Next:** Sentient MCP (`--profile sentient` auto-exec). Legacy profile name
`degen` aliases to `sentient`.

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

## Write tools

Same tool names on both profiles. Behavior differs by grant level:

| Tool | Status | Description |
|------|--------|-------------|
| `propose_transfer` | Yes | Native or ERC-20 transfer |
| `propose_contract_call` | Yes | Arbitrary contract call |
| `propose_swap` | Yes | Direct V2/PulseX router swap (path + amounts) |
| `propose_agg_swap` | Yes | Aggregator quote → proposal (allowlisted routers only) |
| `propose_v3_swap` | Yes | wiz4rd V3 exact-in swap (allowlisted SwapRouter on 943) |
| `propose_batch_7702` | Deferred | EIP-7702 batched send |

On **`default`:** return `proposal_id` + `status: pending_user`; human approves.  
On **`sentient` (target):** simulate → policy check → Vaughan signs/broadcasts for
the sentient vault; return `tx_hash` or breaker trip — still no keys in MCP.

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
