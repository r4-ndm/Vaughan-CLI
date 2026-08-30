# 9mm 9X — VB Ag quote

Browserless quotes are also available via **`api.9mm.pro`** (see `docs/aggregator.md` —
Ag screen + MCP `quote_swap` with `venue: 9mm`). VB remains the cross-check path when the
HTTP API errors on a pair.

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
- **Many fake `USDC` labels:** 9X lists a dozen tickers containing "USDC"
  (Pump.tires, Unstable Dick Coin, …). The picker reads **full row text**
  (ticker + name + `0x…` address) and selects Vaughan's registry contract:
  **bridged USDC** `0x15D38573…1f07` ("USD COIN FROM ETHEREUM") — not
  **pUSDC** `0xA0b86991…06eB48` (Ethereum fork copy; different economic
  value). Refuses to deep-click a bare "USDC" label when the registry
  address is known.
- **Amount entry (2026-08-29):** 9X's sell mask is ATM-style (implicit ÷1000).
  Pass **human units** in `amount_in` (1M PLS → `"1000000"`). The URL's
  `sellAmount=` mirrors raw field digits, not the parsed amount.
- **Layer divergence + `sell_check`:** 9X's display layer and quote engine can
  disagree (sell `$` correct while MIN RECEIVED is 1000× off). For stablecoin
  routes, `browser_read_quote`'s `sell_check` uses **`check: sell_vs_out`**
  (page sell USD vs quoted USDC out on the same screen) — a ratio far from 1
  flags misparse.
- **Digit-leading tickers:** the BUY default `9MM` used to defeat the token
  picker's button regex (required a leading letter) so output picks landed on
  the SELL leg and legs flip-flopped. Fixed in `open_token_picker.js`
  (2026-08-29) — digit-leading tickers (9MM, 1INCH) now match.
- **Quote-engine outage (2026-08-29):** after ~20 automated quotes in a
  session, 9X's backend degraded in stages: `NO_QUOTE` / "Something went
  wrong" on every request (deep links included), then ~1h later **garbage
  quotes at any amount with any input method** (1,000 PLS → `MIN RECEIVED
  171,426 USDC`, rate line `1 PLS = 5.77 USDC`, values changing randomly
  between reads). DApp-side, NOT a typing problem — an insane `sell_check`
  ratio (`sell_vs_out` far from 1) is the automated tell. Back off and retry
  later; don't hammer.
- Quote panel: route split across venues (PULSEX_V1/PHUX/SWITCHX/…), `VS SELL`
  impact %, MIN RECEIVED, EST GAS. Renders without wallet connect. Default
  slippage 1.00%.
