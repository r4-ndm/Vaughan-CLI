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
2. **`browser_connect_wallet`** — opens Connect modal, clicks **Vaughan** (shadow DOM / snapshot ref), then auto-selects **PulseChain** in Switch's in-app chain picker when the wallet is on chain 369.
3. Operator tier auto-grants **`eth_requestAccounts`** and **`wallet_switchEthereumChain`** on allowlisted hosts (no TUI card when sentient + Operator).
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
- **Amount entry ÷1000 quirk (2026-08-29, updated):** Switch's mask is
  ATM-style with **every** input method (key events, insertText, setter):
  typed digits get an implicit decimal 3 from the right (`1000000` → 1,000.000
  PLS state) while the field displays the raw digits. **Always pass human units
  in `amount_in`** (1M PLS → `"1000000"`, not `"1000000000"`). The typing layer
  multiplies by 1000 internally before keying into the field (1M human → 1B
  digits typed — the venue mask then yields ~1M effective). Verify with
  `browser_read_quote { expect_amount_in: "1000000", expect_token_in: "PLS" }` →
  `sell_check.suspected_amount_misparse` (typing `verified: true` only proves
  the *field* holds the scaled digits, not how the venue parsed them). 9X differs —
  see its playbook.
- One-shot output pick can fail (`token text not found`) when the open step hits
  the venue's **Switch** tab. Manual recovery: `browser_click_text` on the current
  output symbol (e.g. `HEX`) → `browser_type` the search box → `browser_click_text USDC`.
- Quotes render **without** wallet connect (Connect Wallet stays visible) — fine
  for read-only quote tours.
- **In-app chain picker:** Switch (and other multi-chain Ag UIs) show a network
  dropdown separate from the wallet provider chain switch. `browser_connect_wallet`
  / `browser_open_agg` with `connect_wallet: true` auto-clicks the wallet's
  active network label (PulseChain on 369) after connect — generic, not Switch-only.
