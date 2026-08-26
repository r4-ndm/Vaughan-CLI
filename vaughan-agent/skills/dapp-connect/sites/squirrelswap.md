# SquirrelSwap

## Identity

- **Name:** SquirrelSwap
- **Canonical URL:** `https://app.squirrelswap.pro/#/`
- **Chain(s):** PulseChain 369 (also used via Vaughan Ag / MCP)

## Tags

`inject-eip1193` `wallet-modal` `auto-eth-accounts`

## How humans connect

1. Unlock Vaughan → Web → SquirrelSwap → Enter.
2. Green **Vaughan injected** banner.
3. Connect wallet in the app (Injected / Vaughan). Often appears connected
   quickly because the site requests accounts without a heavy modal wait.

## What “success” looks like

- Address shown in the Squirrel UI.
- Swaps still require TUI approve on `eth_sendTransaction`.

## Failure modes

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| No banner | Extension not loaded | Rebuild `vaughan-dapp-browser`; close stray Chrome profiles |
| Bridge offline banner | Vaughan locked / WS down | Unlock Vaughan; confirm Web bridge line |

## Provider quirks

- Straightforward EIP-1193; no known CSP block on `ws://`.
- Aggregator API is separate (`api.squirrelswap.pro`) — browserless Ag path preferred for agents.

## Vaughan notes

- Seeded in `default_trusted_dapps()`.
- Prefer MCP / Ag for quotes when the user does not need the web UI.
