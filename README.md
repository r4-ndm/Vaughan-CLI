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

> Status: active development. Phase 1–5 core features implemented; install via release tarball or `cargo build`.

## Install

**One-liner** (downloads a release tarball, verifies SHA256, installs to `~/.local/bin`):

```bash
curl -fsSL https://raw.githubusercontent.com/r4-ndm/Vaughan-CLI/main/scripts/install.sh | sh
```

**Manual tarball** (auditable — no pipe-to-shell):

```bash
VERSION=v0.1.0
PLATFORM=linux-x86_64   # or linux-aarch64, macos-x86_64, macos-aarch64

curl -fsSL -O "https://github.com/r4-ndm/Vaughan-CLI/releases/download/${VERSION}/vaughan-${PLATFORM}.tar.gz"
curl -fsSL -O "https://github.com/r4-ndm/Vaughan-CLI/releases/download/${VERSION}/SHA256SUMS"
grep "vaughan-${PLATFORM}.tar.gz" SHA256SUMS | sha256sum -c -

mkdir -p ~/.local/bin
tar -xzf "vaughan-${PLATFORM}.tar.gz" -C /tmp
install -m 755 /tmp/vaughan ~/.local/bin/vaughan
export PATH="$HOME/.local/bin:$PATH"
```

If no GitHub release exists yet, the install script falls back to `cargo install --git …`.

Pin a version: `VAUGHAN_VERSION=v0.1.0 curl -fsSL …/install.sh | sh`

## Launch

```bash
vaughan              # interactive wallet TUI (default)
vaughan tui          # same as above
vaughan balance      # scriptable CLI subcommands
vaughan send …
vaughan browse 0x…
vaughan agent "inspect 0x…" --mode assist
```

Ensure `~/.local/bin` (or `~/.cargo/bin` when using the cargo fallback) is on your `PATH`.

## Build from source (developers)

```bash
git clone https://github.com/r4-ndm/Vaughan-CLI.git
cd Vaughan-CLI
cargo build --release -p vaughan-cli
install -m 755 target/release/vaughan ~/.local/bin/vaughan
```

Or run without installing:

```bash
cargo run -p vaughan-cli          # TUI
cargo run -p vaughan-cli -- balance
cargo run -p vaughan-tui           # TUI-only crate (dev convenience)
```

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
└─ vaughan-cli/       # Unified `vaughan` binary: TUI by default, CLI subcommands
```

- [docs/agent-configuration.md](docs/agent-configuration.md) — Complete guide on configuring Ollama, Gemini, OpenAI, and agent profiles
- [docs/AI-AGENT-ARCHITECTURE.md](docs/AI-AGENT-ARCHITECTURE.md) — AI Agent architecture and multi-tier sandboxing specification
- [docs/freedom-browser-integration.md](docs/freedom-browser-integration.md) — EIP-1193 provider bridge architecture
- [docs/ambire-aa.md](docs/ambire-aa.md) — Ambire EIP-7702 batch transactions and delegation
- [docs/browser-engine.md](docs/browser-engine.md) — Smart contract browser engine and selector probes
- [REQUIREMENTS.md](REQUIREMENTS.md) — Functional and non-functional requirements
- [TASKS.md](TASKS.md) — Implementation task breakdown and test verification matrix

---

**Built with ❤️ and 🦊⚡**
