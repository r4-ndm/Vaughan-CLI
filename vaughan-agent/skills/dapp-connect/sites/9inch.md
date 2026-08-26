# 9inch

## Identity

- **Name:** 9inch
- **Canonical URL:** `https://app.9inch.io/swap?chain=pulse`
- **Chain(s):** PulseChain (`chain=pulse` query)

## Tags

`inject-eip1193` `csp-blocks-ws` `wallet-modal`

## How humans connect

1. Unlock Vaughan → Web → 9inch → Enter.
2. Green **Vaughan injected** banner (required).
3. Connect → Injected / MetaMask / Vaughan.
4. If the site says “Please confirm in Injected”, check the TUI — do not wait
   for a browser extension popup.

## What “success” looks like

- Connected account on Pulse chain in 9inch UI.
- Swaps / approvals surface in Vaughan TUI.

## Failure modes

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| “Confirm in Injected” hang | Page CSP `connect-src` allows `https:` / `wss:` only — **blocks page `ws://127.0.0.1`** | Must use extension **background** WS relay (current `vaughan-dapp-browser`); rebuild + relaunch |
| Bridge offline banner | Vaughan locked | Unlock; confirm `ws://127.0.0.1:8745` |
| Wrong chain | Not on Pulse | `wallet_switchEthereumChain` / switch in TUI to 369 |

## Provider quirks

- Strict CSP is why Squirrel/Liberty “just worked” while 9inch hung on the old
  MAIN-world WebSocket inject.
- Site may present the wallet as MetaMask/Injected; Vaughan sets MetaMask-family
  convenience flags for interop (`isMetaMask`, `wallet_requestPermissions`).

## Vaughan notes

- Seeded in `default_trusted_dapps()`.
- Routers also appear in Vaughan Dex catalog / pulsechain-context skill.
