# Vaughan-CLI

<p align="center">
    <a href="https://github.com/r4-ndm/Vaughan-CLI"><img width="450" alt="Vaughan CLI Logo" src="branding/vaughan-logo-small.png" /></a><br />
</p>

<p align="center">A multi-chain wallet for the terminal. 🦊⚡</p>

**Vaughan-CLI** is a Rust CLI wallet TUI:

- **Alloy** for the wallet core — keys, signing, RPC, transaction building and broadcast
- **ratatui** for the terminal interface
- **kohaku-rs** for privacy/provider (stealth addresses, later railgun)
- **Freedom Browser** integration — use Vaughan as the native signing provider for dApps

EVM-first and PulseChain-optimized, architected after the `vaughan-core` layering from
[Vaughan-Dioxus](https://github.com/r4-ndm/Vaughan-Dioxus).

> Status: prototype under active development. Phase 1 (EOA wallet) in progress.

## Features

- 🔒 Self-custody with a password-encrypted vault (Argon2id + AES-256-GCM)
- 🧾 BIP-39 mnemonic create/restore, HD derivation at `m/44'/60'/0'/0/{index}`
- ⛓️ Multi-chain: PulseChain (mainnet 369 / testnet 943), Ethereum, Sepolia, Polygon, BSC, Base
- 💸 Check balances and send native assets
- 🖥️ Native EIP-1193 provider bridge for Freedom Browser *(Phase 2)*
- 🕵️ Privacy (ERC-5564 stealth, railgun) and Ambire smart accounts *(Phase 3)*

## Architecture

```
vaughan-cli/
├─ vaughan-core/      # library: chains (Alloy EVM + PulseChain), core services,
│                     #   security (HD wallet, encryption), persistence
├─ vaughan-tui/       # ratatui frontend: onboarding, unlock, dashboard, send,
│                     #   receive, settings
└─ vaughan-provider/  # [Phase 2] local EIP-1193 bridge + approval UX
```

## Build & run

```bash
cargo build --release
cargo run -p vaughan-tui
```

## Networks

| Network | Chain ID | Native | RPC |
|---|---|---|---|
| PulseChain Mainnet | 369 | PLS | `https://rpc.pulsechain.com` |
| PulseChain Testnet V4 | 943 | tPLS | `https://rpc.v4.testnet.pulsechain.com` |
| Ethereum Mainnet | 1 | ETH | `https://eth.llamarpc.com` |
| Ethereum Sepolia | 11155111 | ETH | `https://ethereum-sepolia-rpc.publicnode.com` |
| Polygon | 137 | MATIC | `https://polygon-bor-rpc.publicnode.com` |
| BSC | 56 | BNB | `https://bsc-dataseed.binance.org` |
| Base | 8453 | ETH | `https://mainnet.base.org` |

## Security

- Mnemonic/keys never touch disk unencrypted; zeroized in memory after use
- Password policy: >= 12 chars, uppercase, lowercase, digit, symbol
- Signing always requires explicit user approval
- No telemetry, no analytics, no data collection

## Documentation

- [REQUIREMENTS.md](REQUIREMENTS.md) — goals, functional and non-functional requirements
- [PLAN.md](PLAN.md) — architecture, technology choices, phases, risks
- [TASKS.md](TASKS.md) — checkable task breakdown by phase

## Roadmap

1. **Phase 1** — EOA wallet on PulseChain (create/restore, balance, send, receive, networks)
2. **Phase 2** — Freedom Browser native provider bridge
3. **Phase 3** — kohaku-rs privacy + Ambire smart accounts

---

**Built with ❤️ and 🦊⚡**
