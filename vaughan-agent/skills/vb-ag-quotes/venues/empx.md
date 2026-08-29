# EmpX — browserless only

## Identity

- **Venue id:** `empx` / `EmpX`
- **Web swap UI:** none (API / browserless integration)

## Quote path

Use MCP **`quote_swap`** (or `propose_agg_swap`) — not VB.

EmpX often returns the **best browserless** PLS→HEX among live API integrations (~0.01 HEX per 1 PLS in recent tests; market-dependent).

## Quirks

- **USDC address on PulseChain:** human/VB quotes use **bridged USDC**
  `0x15D38573…1f07` (same value as Ethereum USDC). The fork copy at
  `0xA0b86991…06eB48` (often labeled **pUSDC** on 9X) is a separate token.
- **EmpX router quirk (2026-08-29):** `findBestPath` to bridged USDC
  (`0x15D38573…`) may route through thin pools and quote badly; pUSDC paths
  can look healthier on browserless EmpX alone. Cross-check EmpX stablecoin
  quotes against 9X/Switch VB reads before trusting browserless numbers.

## MCP

`browser_open_agg` with `empx` may fail or open nothing useful — expected.
