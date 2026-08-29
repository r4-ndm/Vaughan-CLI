# Pulse DeFi skill pack (MCP P2+)

Agent-facing tools for inspect → quote → propose trade / LP / hygiene on PulseChain.
Signing stays in Vaughan (adviser card or sentient auto-exec).

## Flow

```
inspect_contract / get_v3_pool / list_allowances / list_v3_positions
        ↓
   quote_v3_swap       (wiz4rd)  or  quote_swap (aggregators)
        ↓
 propose_v3_swap / propose_v3_mint / propose_wrap / propose_revoke / …
        ↓
   TUI: approve (default)  or  auto-exec (sentient unlocked)
```

## Tools

| Tool | Kind | Notes |
|------|------|--------|
| `inspect_contract` | read | Capability fingerprint |
| `get_dex_reserves` | read | V2 pair reserves |
| `search_pairs` | read | Factory log scan |
| `get_v3_pool` | read | wiz4rd V3 slot0 + liquidity (943) |
| `quote_v3_swap` | read | wiz4rd exact-in quote |
| `list_v3_positions` | read | wiz4rd LP NFTs |
| `list_allowances` | read | Known spender allowances |
| `quote_swap` | read | Squirrel / PulseSwap / Piteas / EmpX (369) |
| `propose_swap` | write | Direct V2/PulseX router path |
| `propose_v3_swap` | write | wiz4rd V3 SwapRouter (allowlisted) |
| `propose_v3_mint` | write | wiz4rd open LP (NPM allowlisted) |
| `propose_wrap` / `propose_unwrap` | write | PLS ↔ WPLS |
| `propose_revoke` | write | `approve(spender, 0)` |
| `propose_agg_swap` | write | Aggregator → allowlisted router |

## Example (Cursor) — wiz4rd testnet

1. Unlock Vaughan on PulseChain testnet v4.
2. Ask: *"Quote 0.01 WZRD → WPLS on wiz4rd fee 500, then propose the swap."*
3. Agent: `get_v3_pool` → `quote_v3_swap` → `propose_v3_swap` → approve (or sentient auto).

LP: *"Mint a wide LP on WZRD/WPLS fee 500 with small amounts."* → `propose_v3_mint`
(approve tokens to NPM first if needed via `propose_approve`).

Smoke WZRD: `0x29bab93456c0E97EE931C1554c7C215480aa7766` — see [`wiz4rd-addresses.md`](wiz4rd-addresses.md).

Mainnet writes require `VAUGHAN_MCP_ALLOW_MAINNET=1`.

Full checklist: [`defi-agent-parity.md`](defi-agent-parity.md).

## Ag quotes in Vaughan Browser (VB)

When `quote_swap` is gated (e.g. Switch.win API key) or the user wants UI parity,
use MCP **`browser_open_agg`** + CDP snapshot/type. **Read first:**

[`vaughan-agent/skills/vb-ag-quotes/SKILL.md`](../vaughan-agent/skills/vb-ag-quotes/SKILL.md)

Venue table: [`vaughan-agent/skills/vb-ag-quotes/venues/INDEX.md`](../vaughan-agent/skills/vb-ag-quotes/venues/INDEX.md)
