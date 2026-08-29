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
- **Amount entry (2026-08-29, corrected):** 9X's sell mask is ATM-style
  (last digit = smallest unit) for **every** input method — key events,
  insertText, and setter all parse ÷1000. **Workaround: type intended
  amount × 1000** (1M PLS → `1000000000`; display then values it correctly,
  e.g. `≈ $11,956`). The URL's `sellAmount=` mirrors the *raw* field digits,
  NOT the parsed amount — don't read it as confirmation of the parse.
- **Layer divergence + `out_check`:** 9X's display layer and quote engine
  can disagree about the amount (sell `$` correct while MIN RECEIVED is
  1000× off). `browser_read_quote`'s sell-side check only validates the
  display layer — always read the `out_check` block too (compares best
  output vs expected when token_out is a stablecoin).
- **Digit-leading tickers:** the BUY default `9MM` used to defeat the token
  picker's button regex (required a leading letter) so output picks landed on
  the SELL leg and legs flip-flopped. Fixed in `open_token_picker.js`
  (2026-08-29) — digit-leading tickers (9MM, 1INCH) now match.
- **Quote-engine outage (2026-08-29):** after ~20 automated quotes in a
  session, 9X's backend degraded in stages: `NO_QUOTE` / "Something went
  wrong" on every request (deep links included), then ~1h later **garbage
  quotes at any amount with any input method** (1,000 PLS → `MIN RECEIVED
  171,426 USDC`, rate line `1 PLS = 5.77 USDC`, values changing randomly
  between reads). DApp-side, NOT a typing problem — an insane `out_check`
  ratio is the automated tell. Back off and retry later; don't hammer.
- Quote panel: route split across venues (PULSEX_V1/PHUX/SWITCHX/…), `VS SELL`
  impact %, MIN RECEIVED, EST GAS. Renders without wallet connect. Default
  slippage 1.00%.
