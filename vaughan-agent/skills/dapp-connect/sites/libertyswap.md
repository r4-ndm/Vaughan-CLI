# LibertySwap

## Identity

- **Name:** LibertySwap
- **Canonical URL:** `https://libertyswap.finance/`
- **Chain(s):** multi-chain; Vaughan Bridge view uses Liberty for USDC routes

## Tags

`inject-eip1193` `wallet-modal` `auto-eth-accounts`

## How humans connect

1. Unlock Vaughan → Web → LibertySwap → Enter.
2. Green **Vaughan injected** banner.
3. Connect in-app (Injected). Often connects with little friction (same class as
   SquirrelSwap).

## What “success” looks like

- Connected address in Liberty UI.
- Cross-chain / bridge txs still approve in the TUI (or use Vaughan Bridge view).

## Failure modes

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| Wrong network | Active chain ≠ site expectation | Switch network in Vaughan TUI first |
| Bridge offline banner | Provider WS down | Unlock Vaughan |

## Provider quirks

- No known CSP `connect-src` block of `ws://`.
- For USDC bridge flows, Vaughan’s built-in Bridge view may be clearer than the web UI.

## Vaughan notes

- Seeded in `default_trusted_dapps()`.
- See `docs/bridge.md` for API / router allowlists.
