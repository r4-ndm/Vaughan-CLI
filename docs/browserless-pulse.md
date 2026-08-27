# Browserless Pulse

**Pitch:** the wallet that doesn’t need Chrome. Approve calldata, not websites.

## Default path (in-TUI)

```
unlock → Ag / Dex / Browse → approve → done
```

Optional side paths:

- **MCP** (`vaughan mcp`) — Cursor / Claude propose; you approve in Vaughan
- **Stealth receive** — ERC-5564 URI / scan / sweep
- **VB** (`vaughan-dapp-browser`, when installed) — allowlisted multi-chain EVM
  dApps + CDP agent control; signing still TUI-only.
  See [dapp-browser-strategy.md](dapp-browser-strategy.md).
- **Freedom** (`w` Web) — **parked** until upstream
  [PR #195](https://github.com/solardev-xyz/freedom-browser/pull/195) merges;
  dev fallback only. See [freedom-browser-status.md](freedom-browser-status.md).

## Not the product

- Making the webview the default wallet identity
- Open-internet general browsing / Chrome replacement
- WalletConnect-as-default identity
- Cloning every Pulse website — prefer verbs: swap, inspect, revoke, send, stealth
- Shipping CEF inside `vaughan-core` (browser is a modular optional binary)
- Active Freedom Browser integration (blocked on upstream PR #195)

## Exit demo (no browser window)

1. Unlock on PulseChain testnet  
2. Ag quote → swap confirm  
3. Contract browser probe (`c`)  
4. MCP agent proposes a small transfer → approve once  
5. Stealth receive URI  

See [TASKS.md](../TASKS.md) § Browserless Pulse, [aggregator.md](aggregator.md), [mcp.md](mcp.md).
Demo recording script: [browserless-pulse-demo.md](browserless-pulse-demo.md).
Implementation backlog: [browserless-pulse-next-bets.md](browserless-pulse-next-bets.md).
