---
name: dapp-connect
description: >-
  How to open and connect Vaughan to each trusted dApp URL (inject quirks,
  CSP, IPFS mirrors, wallet UI labels). Use when launching Web / dApp browser,
  debugging “Injected” / connect hangs, or adding a new bookmark.
mode: all
kind: guide
---

# dApp connect playbooks

Vaughan’s Chromium shell injects EIP-1193 via an **extension background
WebSocket** (CSP-safe). Signing always stays in the **Vaughan TUI** — never a
browser popup. dApps often label the provider **“Injected”** or **MetaMask**.

## Universal checklist

1. Unlock Vaughan (provider `ws://127.0.0.1:8745`).
2. Web list → select dApp → **Enter** (opens VB; Freedom only as dev fallback — parked until PR #195).
3. Look for green banner: **Vaughan injected**.
4. In the dApp: Connect → Injected / Vaughan / MetaMask.
5. If the dApp says “confirm in wallet”, switch to the **TUI** (connect is often
   auto-answered; **sign/send** always needs `y` / Enter).

Do **not** click underlined `https://` text in the terminal (opens system browser
without inject).

## Site index (read the matching file)

| Bookmark | Canonical URL | Playbook |
|----------|---------------|----------|
| SquirrelSwap | `https://app.squirrelswap.pro/#/` | [`sites/squirrelswap.md`](sites/squirrelswap.md) |
| LibertySwap | `https://libertyswap.finance/` | [`sites/libertyswap.md`](sites/libertyswap.md) |
| PulseX | `https://app.pulsex.com/` | [`sites/pulsex.md`](sites/pulsex.md) |
| 9inch | `https://app.9inch.io/swap?chain=pulse` | [`sites/9inch.md`](sites/9inch.md) |

New dApp → copy [`sites/_template.md`](sites/_template.md), fill it, add a row
above, and seed the URL in `vaughan-core` `default_trusted_dapps()`.

## Connection method tags

Use these labels in site files:

| Tag | Meaning |
|-----|---------|
| `inject-eip1193` | Uses `window.ethereum` / EIP-6963 (Vaughan path) |
| `csp-blocks-ws` | Page CSP blocks page-level `ws://` (needs extension relay) |
| `ipfs-mirror-dir` | URL is a directory of mirrors, not the live UI |
| `wallet-modal` | Connect via modal (“Injected” / WalletConnect / …) |
| `auto-eth-accounts` | Calls `eth_accounts` / `eth_requestAccounts` on load |

## Agent / coding notes

- Prefer browserless Pulse (Ag / Dex / MCP) when the user does not need the web UI.
- Owned browser is optional; never teach auto-sign in the page.
- When a connect hang is reported, read the site playbook **before** changing
  core provider code.
- Phase 1: initial `--url` allowlist only (in-tab navigation not gated yet);
  agent CDP only with `--cdp-port` / `VAUGHAN_DAPP_BROWSER_CDP_PORT`.
- `--self-check` should show inject PASS **and** bridge PASS (`eth_chainId`).
- Security: session token for extension WS, connect approve, page origin on
  prompts, approve debounce/TTL — see `docs/native-parity-tricks.md` §5.
