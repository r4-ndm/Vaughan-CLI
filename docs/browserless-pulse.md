# Browserless Pulse

**Pitch:** the wallet that doesn’t need Chrome. Approve calldata, not websites.

## Default path (in-TUI)

```
unlock → Ag / Dex / Browse → approve → done
```

Optional side paths:

- **MCP** (`vaughan mcp`) — Cursor / Claude propose; you approve in Vaughan
- **Stealth receive** — ERC-5564 URI / scan / sweep
- **Freedom** (`w` Web) — optional EIP-1193 bridge for odd dApps only

## Not the product

- Building a general-purpose dApp browser inside Vaughan
- WalletConnect-as-default identity
- Cloning every Pulse website — prefer verbs: swap, inspect, revoke, send, stealth

## Exit demo (no browser window)

1. Unlock on PulseChain testnet  
2. Ag quote → swap confirm  
3. Contract browser probe (`c`)  
4. MCP agent proposes a small transfer → approve once  
5. Stealth receive URI  

See [TASKS.md](../TASKS.md) § Browserless Pulse, [aggregator.md](aggregator.md), [mcp.md](mcp.md).
