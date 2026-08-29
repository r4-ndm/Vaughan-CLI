# EmpX — browserless only

## Identity

- **Venue id:** `empx` / `EmpX`
- **Web swap UI:** none (API / browserless integration)

## Quote path

Use MCP **`quote_swap`** (or `propose_agg_swap`) — not VB.

EmpX often returns the **best browserless** PLS→HEX among live API integrations (~0.01 HEX per 1 PLS in recent tests; market-dependent).

## Quirks

- **Canonical USDC (`0x15D38573…1f07`) routes are broken** (verified 2026-08-29):
  `findBestPath` routes WPLS → eHEX/HEX/DAI → USDC through dust pools, quoting
  ~1000× off market (1M PLS → "12.05 USDC"; 1k PLS → "0.0135 USDC"). The router's
  adapter set misses the deep PulseX USDC pools. **pUSDC (`0xA0b86991…06eB48`)
  quotes fine** (single-hop WPLS, 1M PLS → 12,069 USDC). Cross-check EmpX
  stablecoin quotes against 9X/Switch before trusting them.

## MCP

`browser_open_agg` with `empx` may fail or open nothing useful — expected.
