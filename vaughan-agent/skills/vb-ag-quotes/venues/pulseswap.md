# PulseSwap — VB Ag quote

## Identity

- **Venue id:** `pulseswap` / `PulseSwap`
- **URL:** `https://pulseswap.io/?chain=pulsechain`
- **PLS→HEX deep link:** native + HEX + `amount=1` query (see `catalog.rs` `web_url_pls_hex`)
- **Chain:** PulseChain 369

## MCP open

```json
{ "venue": "pulseswap", "pls_hex": true }
```

Prefer `pls_hex: true` so amount and tokens are pre-filled.

## Flow

1. Wait for swap form / “Swap” button.
2. Connect wallet if needed.
3. If deep link worked, confirm 1 PLS and HEX output; else set manually.
4. Read quoted HEX from output field or route summary.

## Quirks

- Best VB experience when using catalog deep link.
- Trusted dApp seed: `pulseswap.io`.

## Browserless fallback

`quote_swap` works without API key.
