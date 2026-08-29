# Piteas — VB Ag quote

## Identity

- **Venue id:** `piteas` / `Piteas`
- **URL:** `https://app.piteas.io/`
- **PLS→HEX deep link:** base URL only
- **Chain:** PulseChain 369

## MCP open

```json
{ "venue": "piteas", "pls_hex": true }
```

## Flow

1. Wait for app shell / swap widget.
2. Connect wallet.
3. Select PLS input, HEX output if not default.
4. Type `1` in amount; wait for route refresh.
5. Read output amount from UI.

## Browserless fallback

`quote_swap` works without API key.
