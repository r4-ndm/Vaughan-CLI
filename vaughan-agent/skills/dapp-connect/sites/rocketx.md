# RocketX

## Identity

- **Name:** RocketX
- **Canonical URL:** `https://www.rocketx.exchange/`
- **Other hosts / mirrors:** `rocketx.exchange` / `www.rocketx.exchange`
- **Chain(s):** Multi-chain swap/bridge aggregator (200+ networks). Confirm network in
  Vaughan before signing; not PulseChain-native.

## Tags

`inject-eip1193` `wallet-modal`

## How humans connect

1. Unlock Vaughan → Web → RocketX → Enter (or MCP `browser_open` with the exchange URL).
2. Green **VB injected** toast (bottom-right).
3. Connect → Injected / MetaMask / Vaughan.
4. Approve connect in the **TUI**; approve each swap/bridge sign there too.
5. Switch Vaughan network if the dApp requests a different chain.

## What “success” looks like

- Connected address in RocketX UI.
- Sign / send prompts appear in Vaughan TUI (never a browser extension popup).

## Failure modes

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| Host not allowlisted | Old vault | Unlock so `merge_default_trusted_dapps` runs |
| Wrong network | Route targets another EVM chain | Approve `wallet_switchEthereumChain` in TUI |
| “Confirm in wallet” hang | Waiting on extension UI | Approve in Vaughan TUI |

## Provider quirks

- Cross-chain aggregator — not Vaughan `propose_swap` / Pulse Ag path.
- Origin allowlist covers `www.rocketx.exchange` and `rocketx.exchange`.

## Vaughan notes

- Seeded in `default_trusted_dapps()` / `core_trusted_dapps()` as RocketX.
- Assist can drive VB UI; every sign stays human-approved in TUI.
