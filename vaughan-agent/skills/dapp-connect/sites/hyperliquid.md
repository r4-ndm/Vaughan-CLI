# Hyperliquid

## Identity

- **Name:** Hyperliquid
- **Canonical URL:** `https://app.hyperliquid.xyz/trade`
- **Other hosts / mirrors:** origin `app.hyperliquid.xyz` (trade / deposit / portfolio paths)
- **Chain(s):** HyperCore trading (app UI); deposits usually **Arbitrum One (42161)** USDC;
  HyperEVM apps use chain id **999** (not a Vaughan built-in yet)

## Tags

`inject-eip1193` `wallet-modal`

## How humans connect

1. Unlock Vaughan → Web → Hyperliquid → Enter (or MCP `browser_open` with the trade URL).
2. Green **VB injected** toast (bottom-right).
3. Connect → Injected / MetaMask / Vaughan.
4. **Enable Trading** — gasless EIP-712 / personal sign; approve in the **TUI** (no browser popup).
5. Deposit: switch wallet to **Arbitrum One**, use native Circle USDC, confirm deposit tx in TUI.

## What “success” looks like

- Connected address shown in Hyperliquid UI.
- Enable Trading signature approved in Vaughan TUI.
- After deposit, HyperCore margin balance updates (perps ready).

## Failure modes

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| Host not in VB allowlist | Old vault missing merge | Restart / unlock so `merge_default_trusted_dapps` runs; or re-add bookmark |
| Deposit / switch chain fails | Not on Arbitrum | Settings → **Arbitrum One** (42161) |
| “Confirm in wallet” hang | Waiting on extension UI | Approve in Vaughan TUI |
| Red “Vaughan denied/error” right after approving Connect | Site fired a second `eth_requestAccounts` while the card was open; old TUI refused it (“another approval is pending”) | Fixed: duplicates from the same site now share the card's answer. Check `vaughan.log` for `provider request refused` |
| Trezor One: “firmware too old for EIP-712” on Enable Trading | Modern Trezor One uses the Model T USB id, so the full-streaming path was chosen | Fixed: model read from device `Features`; T1 signs domain/message hashes (match them on device) |
| Connect click does nothing, no TUI prompt; log shows `missing/invalid session token` | Saved profile (`s`) ran Chromium's cached `background.js` with an old token | Fixed: VB clears `Default/Service Worker` in saved profiles each launch. Rebuild, close VB, reopen |
| Wrong USDC | Bridged USDC.e vs native | Use Circle native USDC on Arbitrum |

## Provider quirks

- Connect + Enable Trading are signature-heavy; perps orders are HyperCore actions (not Vaughan `propose_swap`).
- Chart layout and the Enable Trading agent key live in browser localStorage. VB profiles are
  throwaway by default, so both reset each launch. Web → select Hyperliquid → **`s`** keeps this
  site's profile (`· saved`); the agent key then sits unencrypted in
  `~/.local/share/vaughan-cli/vb-profiles/app.hyperliquid.xyz/` (it can trade, not withdraw).
  Pressing `s` again deletes that folder. MCP `browser_open` launches stay throwaway.
- Site often labels the provider MetaMask/Injected; Vaughan MetaMask-family flags apply.

## Vaughan notes

- Seeded in `default_trusted_dapps()` as Hyperliquid → `/trade`.
- LibertySwap Arb ↔ Pulse is the intended post-trade bridge path back to PulseChain.
- No MCP Hyperliquid order tools yet — Assist can drive VB UI; signing stays in TUI.
