# PulseX

## Identity

- **Name:** PulseX (directory)
- **Canonical URL:** `https://app.pulsex.com/`
- **Other hosts:** IPFS gateways listed on that page (Pinata, Cloudflare, ipfs.io, …);
  `https://pulsex.com/` is a related landing page, not a single fixed DEX origin
- **Chain(s):** PulseChain 369

## Tags

`ipfs-mirror-dir` `inject-eip1193` `wallet-modal`

## How humans connect

1. Unlock Vaughan → Web → **PulseX (pick IPFS mirror)** → Enter.
2. You land on a **link directory**, not the swap UI.
3. Click a community IPFS / gateway link to open the real frontend.
4. On the mirror: Connect → Injected / MetaMask / Vaughan.
5. Approve **sign/send** in the Vaughan TUI.

The dApp-browser extension Origin is always trusted, so mirror hosts do not need
to be pre-listed as provider origins. Navigation still happens in Chromium after
you pick a mirror.

## What “success” looks like

- Swap UI loaded from an IPFS gateway (URL host ≠ `app.pulsex.com`).
- Green Vaughan banner on that mirror page.
- Wallet connected; trades approve in TUI.

## Failure modes

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| Stuck on link list | Expected — not the DEX | Open an IPFS mirror link |
| “Confirm in Injected” forever | Old page-level `ws://` inject / wrong browser | Use current `vaughan-dapp-browser` (extension relay); restart Vaughan |
| Opened via terminal link click | System browser, no inject | Use Enter in Web list only |

## Provider quirks

- Do not treat `app.pulsex.com` alone as proof of DEX connectivity.
- Prefer Vaughan Dex / MCP for PulseX-style swaps when the user does not need the web UI.

## Vaughan notes

- Seeded as `PulseX (pick IPFS mirror)` → `https://app.pulsex.com/`.
- Strategy: `docs/dapp-browser-strategy.md`; browserless default: `docs/browserless-pulse.md`.
