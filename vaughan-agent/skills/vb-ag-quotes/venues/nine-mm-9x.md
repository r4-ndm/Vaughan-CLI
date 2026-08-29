# 9mm 9X — VB Ag quote

## Identity

- **Venue id:** `9mm` / `NineMm9X`
- **URL:** `https://9x.9mm.pro/#/swap?chainId=369` (9X aggregator — not `9mm.pro/swap`, which 404s)
- **Chain:** PulseChain 369

## MCP open

```json
{ "venue": "9mm", "pls_hex": true }
```

## Flow

1. Load swap page; connect wallet.
2. Select PLS / HEX if required.
3. Enter `1` PLS; read quote.

## Notes

- Trusted dApp: `9mm.pro`.
- **SELL** leg = input (PLS) · **BUY** leg = output (HEX). Agent targets SELL/BUY labels for token pickers.
- Browserless path may be limited — prefer VB for parity with human UX.
