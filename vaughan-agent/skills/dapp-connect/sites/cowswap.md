# CoW Swap

## Identity

- **Name:** CoW Swap
- **Canonical URL:** `https://swap.cow.fi/`
- **Other hosts / mirrors:** none allowlisted (`cow.fi` is the marketing site, not the app)
- **Chain(s):** Ethereum, Arbitrum One, Base, Gnosis, and other EVM chains CoW supports.
  Not PulseChain. Confirm the network in Vaughan before signing.

## Tags

`inject-eip1193` `wallet-modal`

## How humans connect

1. Unlock Vaughan → Web → CoW Swap → Enter (or MCP `browser_open` with the swap URL).
2. Green **VB injected** toast (bottom-right).
3. Connect wallet → Injected / MetaMask / Vaughan.
4. Approve connect in the **TUI**.
5. Pick the network in CoW's selector; approve `wallet_switchEthereumChain` in the TUI.

## What “success” looks like

- Connected address and balances shown in CoW Swap.
- Order signatures and approvals appear in the Vaughan TUI (never a browser popup).

## Failure modes

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| Host not allowlisted | Old vault | Unlock so `merge_default_trusted_dapps` runs |
| Wrong network | CoW defaults to Ethereum mainnet | Switch in CoW's selector; approve in TUI |
| “Confirm in wallet” hang | Waiting on extension UI | Approve in Vaughan TUI |

## Provider quirks

- Orders are **off-chain EIP-712 signatures** (`eth_signTypedData_v4`), not
  transactions. Solvers settle on-chain; network fee is taken from the sell token.
- Gasless approval: tokens with EIP-2612 permit (e.g. native USDC) are approved
  via a signed permit; others need a normal `approve` tx (needs native gas).
- A signed order or permit still authorizes spending — review token, amount,
  and spender in the TUI before approving.

## Vaughan notes

- Seeded in `default_trusted_dapps()` / `core_trusted_dapps()` as CoW Swap.
- Not the Vaughan `propose_swap` / Pulse Ag path; every sign stays human-approved in TUI.
