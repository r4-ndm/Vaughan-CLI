# Pulse DeFi skill pack (MCP P2+)

Agent-facing tools for inspect → quote → propose trade on PulseChain.
Signing stays in the Vaughan TUI.

## Flow

```
inspect_contract / get_v3_pool / get_dex_reserves
        ↓
   quote_v3_swap       (wiz4rd)  or  quote_swap (aggregators)
        ↓
 propose_v3_swap       or propose_agg_swap / propose_swap
        ↓
   TUI approval card   (re-sim + human y/n)
```

## Tools

| Tool | Kind | Notes |
|------|------|--------|
| `inspect_contract` | read | Capability fingerprint |
| `get_dex_reserves` | read | V2 pair reserves |
| `search_pairs` | read | Factory log scan |
| `get_v3_pool` | read | wiz4rd V3 slot0 + liquidity (943) |
| `quote_v3_swap` | read | wiz4rd exact-in quote |
| `quote_swap` | read | Squirrel / PulseSwap / Piteas |
| `propose_swap` | write | Direct V2/PulseX router path |
| `propose_v3_swap` | write | wiz4rd V3 SwapRouter (allowlisted) |
| `propose_agg_swap` | write | Aggregator → allowlisted router |

## Example (Cursor) — wiz4rd testnet

1. Unlock Vaughan on PulseChain testnet v4.
2. Ask: *"Quote 0.01 WZRD → WPLS on wiz4rd fee 500, then propose the swap."*
3. Agent: `get_v3_pool` → `quote_v3_swap` → `propose_v3_swap` → approve once in TUI.

Smoke WZRD: `0x29bab93456c0E97EE931C1554c7C215480aa7766` — see [`wiz4rd-addresses.md`](wiz4rd-addresses.md).

Mainnet writes require `VAUGHAN_MCP_ALLOW_MAINNET=1`.

LP mint (`propose_v3_mint`) is still Phase D.
