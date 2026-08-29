# LibertyX (LibertySwap) — **not** PLS → HEX

## Identity

- **Venue id:** `libertyx` / `LibertyX`
- **URL:** `https://libertyswap.finance/` (bridge UI only)
- **Product:** cross-chain **USDC** bridge — Pulse ↔ Base / Eth / BSC / …

## Do not use for Ag PLS → HEX tours

`browser_open_agg` with `pls_hex: true` **returns None** in the catalog — by design.
The web UI shows **USD stablecoin bridge** flows, not native PLS → HEX on PulseChain.

Use Vaughan **Bridge screen (`f`)** + `quote_bridge` for LibertySwap routes.
See [`docs/bridge.md`](../../../docs/bridge.md).

## MCP

```json
{ "venue": "libertyx", "pls_hex": true }
```

→ error: no public swap web UI for same-chain Ag quote.

## Related

- Connect: [`dapp-connect/sites/libertyswap.md`](../../dapp-connect/sites/libertyswap.md)
