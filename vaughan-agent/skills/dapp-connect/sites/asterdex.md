# Asterdex

## Identity

- **Name:** Asterdex
- **Canonical URL:** `https://www.asterdex.com/en/trade/pro/futures/CLUSD1`
- **Other hosts / mirrors:** `asterdex.com` / `www.asterdex.com`
- **Chain(s):** Confirm in-app (often EVM inject + futures venue settlement). Prefer small
  connect/sign test before size. Not PulseChain-native.

## Tags

`inject-eip1193` `wallet-modal`

## How humans connect

1. Unlock Vaughan → Web → Asterdex → Enter (or MCP `browser_open` with the futures URL).
2. Green **Vaughan injected** banner.
3. Connect → Injected / MetaMask / Vaughan.
4. Approve connect / enable-trading style signatures in the **TUI**.
5. Fund per the site’s deposit flow; switch Vaughan network if the dApp requests it.

## What “success” looks like

- Connected address in Asterdex UI.
- Sign / send prompts appear in Vaughan TUI (never a browser extension popup).

## Failure modes

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| Host not allowlisted | Old vault | Unlock so `merge_default_trusted_dapps` runs |
| Wrong network | dApp expects another EVM chain | Approve `wallet_switchEthereumChain` in TUI |
| “Confirm in wallet” hang | Waiting on extension UI | Approve in Vaughan TUI |

## Provider quirks

- Futures venue — not Vaughan `propose_swap` / Pulse Ag path.
- Market deep-links (e.g. `CLUSD1`) are fine; origin allowlist covers the host.

## Vaughan notes

- Seeded in `default_trusted_dapps()` as Asterdex.
- Assist can drive VB UI; every sign stays human-approved in TUI.
- Compare with [`hyperliquid.md`](hyperliquid.md) for the other perps bookmark.
