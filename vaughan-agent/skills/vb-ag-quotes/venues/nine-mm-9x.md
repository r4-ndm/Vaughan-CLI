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
- **`buyToken` deep-link param is ignored** (2026-08-29): SPA reverts BUY to 9MM.
  Use the picker: click the BUY token button → search → select.
- **Two USDC entries:** `USDC` = `0x15D38573…1f07` (canonical bridged, deep
  liquidity) vs `pUSDC` = `0xA0b86991…06eB48`. Pick deliberately; Vaughan's
  curated registry maps `USDC` to the pUSDC address.
- **Amount entry (2026-08-29, updated):** with the current `vb_cdp` typing
  pipeline (per-char key events, strategy `key-events`), amounts are **literal**
  on 9X — type `1000000` for 1M PLS, no workaround. The old ÷1000 quirk was an
  insertText/paste-path artifact (the mask's paste handler reads fixed-point).
  The page URL mirrors state as `sellAmount=` — a cheap way to confirm what the
  app actually parsed (visible via `browser_status` pages list).
- **Digit-leading tickers:** the BUY default `9MM` used to defeat the token
  picker's button regex (required a leading letter) so output picks landed on
  the SELL leg and legs flip-flopped. Fixed in `open_token_picker.js`
  (2026-08-29) — digit-leading tickers (9MM, 1INCH) now match.
- **NO_QUOTE / "Something went wrong" (2026-08-29):** after ~20 automated quote
  requests in a session, 9X's quote API started erroring for every request —
  persists across reloads, amounts, pairs, and input methods (deep-link state
  included). DApp-side rate limit or outage, NOT a typing problem. Back off and
  retry later; don't hammer Refresh.
- Quote panel: route split across venues (PULSEX_V1/PHUX/SWITCHX/…), `VS SELL`
  impact %, MIN RECEIVED, EST GAS. Renders without wallet connect. Default
  slippage 1.00%.
