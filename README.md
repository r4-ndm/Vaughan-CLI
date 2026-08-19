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

- 🔒 **Sovereign Self-Custody**: Password-encrypted vault (Argon2id + AES-256-GCM) with zero plain-text storage.
- 🧾 **BIP-39 & HD Wallet**: Mnemonic creation/recovery with standard `m/44'/60'/0'/0/{index}` derivation.
- ⛓️ **Multi-Chain**: PulseChain (mainnet 369 / testnet 943), Ethereum, Sepolia, Polygon, BSC, Base.
- ⚡ **Ambire EIP-7702 Smart Accounts**: Atomic batched transfers without ERC-4337 bundler overhead.
- 🖥️ **EIP-1193 Provider Bridge**: Native local WebSocket JSON-RPC provider with trusted-host allowlisting for Freedom Browser and dApps.
- 🔍 **Contract Browser REPL (`wiz4rd-engine`)**: Real-time bytecode inspection, ERC-20/Uniswap capability probing, and dynamic ABI calls.
- 🤖 **Sandboxed AI Agent Subsystem (`vaughan-agent`)**:
  - **Human Purist Mode**: 100% cold-storage isolation with zero AI code execution.
  - **AI Assisted Mode**: Propose-only AI Advisor with sensory tools and ground-truth calldata confirmation cards.
  - **Degen Mode**: Autonomous trader running on an isolated burner wallet profile with multi-RPC quorum validation and hard circuit breakers.

## Architecture

```
vaughan-cli/
├─ vaughan-core/      # Vault encryption, HD wallets, EVM adapters, contract browser engine
├─ vaughan-aa/        # Ambire EIP-7702 batching, delegation, and signature serialization
├─ vaughan-provider/  # Local EIP-1193 WebSocket JSON-RPC server and allowlist security
├─ vaughan-agent/     # Multi-provider LLM engine (Ollama, Gemini, OpenAI), sensory & proposal tools, circuit breakers
├─ vaughan-tui/       # Ratatui terminal frontend (views, dashboard, batch send, agent chat REPL)
└─ vaughan-cli/       # Non-interactive CLI commands (send, balance, browse, agent)
```

## Build & Run

```bash
# Build all crates
cargo build --release

# Run interactive TUI
cargo run -p vaughan-tui

# Run non-interactive CLI commands
vaughan balance
vaughan browse 0x165C3410fC91EF562C50559f7d2289fEbed552d9
vaughan agent "inspect 0x165C3410fC91EF562C50559f7d2289fEbed552d9" --mode assist
```

## Documentation

- [docs/agent-configuration.md](docs/agent-configuration.md) — Complete guide on configuring Ollama, Gemini, OpenAI, and agent profiles
- [docs/AI-AGENT-ARCHITECTURE.md](docs/AI-AGENT-ARCHITECTURE.md) — AI Agent architecture and multi-tier sandboxing specification
- [docs/freedom-browser-integration.md](docs/freedom-browser-integration.md) — EIP-1193 provider bridge architecture
- [docs/ambire-aa.md](docs/ambire-aa.md) — Ambire EIP-7702 batch transactions and delegation
- [docs/browser-engine.md](docs/browser-engine.md) — Smart contract browser engine and selector probes
- [REQUIREMENTS.md](REQUIREMENTS.md) — Functional and non-functional requirements
- [TASKS.md](TASKS.md) — Implementation task breakdown and test verification matrix

---

**Built with ❤️ and 🦊⚡**
