# Switch.win — VB Ag quote

## Identity

- **Venue id:** `switch` / `SwitchWin`
- **URL:** `https://www.switch.win/`
- **PLS→HEX deep link:** base URL
- **Chain:** PulseChain 369

## Why VB

Browserless `quote_swap` for Switch typically requires a **developer API key**. The public website does not — this is the main reason to use the VB human path.

## MCP open

```json
{ "venue": "switch", "pls_hex": true }
```

## Prerequisites

- **One Vaughan TUI only** — unlocked on PulseChain 369. Two TUIs = provider/connect prompts go to the wrong window.
- `vaughan config agent-browser on` (or Settings → `p`)
- MCP env: `VAUGHAN_DAPP_BROWSER_CDP_PORT=9222`

## Flow

1. Open **`https://www.switch.win/dapp`** (swap widget — not the marketing homepage).
2. **`browser_connect_wallet`** — opens Connect modal, clicks **Vaughan** (shadow DOM / snapshot ref).
3. Approve **`eth_requestAccounts`** in Vaughan TUI if prompted (one-time per session).
4. **`browser_setup_swap`** with `token_in: PLS`, `token_out: HEX`, `amount_in: 1` — selects tokens in the embedded swap iframe, sets amount, clicks **Switch Now**.
5. **`browser_snapshot`** — read HEX output from the quote panel.

Or one shot:

```json
{ "venue": "switch", "token_in": "PLS", "token_out": "HEX", "amount_in": "1", "connect_wallet": true }
```

(`browser_open_agg` connects Vaughan then runs setup_swap by default.)

## Success signal

Page title: *Switch.win - The Premiere DEX Aggregator* (or similar).

## Quirks

- No EmpX-style REST quote without API key — VB is authoritative for Switch.
- Fresh VB session per long tour avoids stale navigation.
- Token picker drives the modal **search box** — long-tail tokens (M3M3, …) work via one-shot `browser_open_agg`; no manual steps.
- The venue's own **Switch** tab matches ticker-shaped buttons — the picker open step skips it (`notToken`) and retries with `avoid` when a click opens no modal.
- Quote amounts render only after expanding **Swap details** — `browser_read_quote` auto-expands it and parses `Total/Expected/Minimum Output` rows (`quote.labeled`).
