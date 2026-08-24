# Pulse DeFi skill pack (MCP P2)

Agent-facing tools for inspect → quote → propose trade on PulseChain.
Signing stays in the Vaughan TUI.

## Flow

```
inspect_contract / get_dex_reserves / search_pairs
        ↓
   quote_swap          (read-only aggregator quote)
        ↓
 propose_agg_swap      (or propose_swap for raw V2 path)
        ↓
   TUI approval card   (re-sim + human y/n)
```

## Tools

| Tool | Kind | Notes |
|------|------|--------|
| `inspect_contract` | read | Capability fingerprint |
| `get_dex_reserves` | read | Pair/pool reserves |
| `search_pairs` | read | Factory log scan |
| `quote_swap` | read | Squirrel / PulseSwap / Piteas — no sign |
| `propose_swap` | write | Direct V2/PulseX router path |
| `propose_agg_swap` | write | Quote → allowlisted router calldata → proposal |

`propose_agg_swap` refuses quotes whose `to` / `spender` are not on the aggregator
router allowlist (`vaughan-core::core::aggregator::routers`).

## Example (Cursor)

1. Unlock Vaughan on PulseChain (testnet first).
2. Ask: *"Quote 1 PLS → HEX on Squirrel, then propose the swap."*
3. Agent: `quote_swap` → `propose_agg_swap` → approve once in TUI.

Mainnet writes require `VAUGHAN_MCP_ALLOW_MAINNET=1`.

Earn / stake tools stay deferred until a real on-chain path exists.
