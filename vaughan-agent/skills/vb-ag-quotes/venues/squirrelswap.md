# SquirrelSwap — VB Ag quote

## Identity

- **Venue id:** `squirrel` / `SquirrelSwap`
- **URL:** `https://app.squirrelswap.pro/#/swap`
- **PLS→HEX deep link:** same as base (pick tokens in UI)
- **Chain:** PulseChain 369

## MCP open

```json
{ "venue": "squirrel", "pls_hex": true }
```

## Flow

1. Wait for “Swap” or route UI.
2. Connect wallet if prompted (TUI approval).
3. Snapshot — amount inputs appear **below** nav; nav links must not consume all refs.
4. Ensure pair is PLS → HEX (token selectors if needed).
5. Type `1` in sell amount with `clear: true`.
6. Read estimated HEX / route text from snapshot.

## Quirks

- Hash-router SPA (`#/swap`).
- React controlled inputs — use `browser_type` (not raw CDP insertText).
- Wallet may show truncated `0x92…574f` when connected.

## Browserless fallback

`quote_swap` with venue SquirrelSwap works without API key.

## Related

- Connect inject: [`dapp-connect/sites/squirrelswap.md`](../../dapp-connect/sites/squirrelswap.md)
