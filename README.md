# Vaughan-CLI

<p align="center">
    <a href="https://github.com/r4-ndm/Vaughan-CLI"><img width="450" alt="Vaughan CLI Logo" src="branding/vaughan-logo-small.png" /></a><br />
</p>

<p align="center">A multi-chain wallet for the terminal. 🐸⚡</p>

<p align="center">
  <a href="https://github.com/r4-ndm/Vaughan-CLI/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/r4-ndm/Vaughan-CLI/actions/workflows/ci.yml/badge.svg" /></a>
  <a href="LICENSE-MIT"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg" /></a>
</p>

**Vaughan-CLI** is a Rust CLI wallet TUI:

- **Alloy** for the wallet core — keys, signing, RPC, transaction building and broadcast
- **ratatui** for the terminal interface
- **In-core ERC-5564 stealth** (Kohaku / RAILGUN deferred — see [docs/kohaku-go-no-go.md](docs/kohaku-go-no-go.md))
- **Freedom Browser** integration — use Vaughan as the native signing provider for dApps

EVM-first and PulseChain-optimized, architected after the `vaughan-core` layering from
[Vaughan-Dioxus](https://github.com/r4-ndm/Vaughan-Dioxus).

> **Status:** public **prototype** on `main` — expect rough edges and breaking
> changes. Phase 1–5 core wallet features are implemented; DEX / aggregator
> paths are still evolving. **Plan change:** embedded in-wallet LLM chat is
> retired — agents use Vaughan via MCP (`vaughan mcp`; see [docs/mcp.md](docs/mcp.md)).
> Prefer testnet. Review [SECURITY.md](SECURITY.md) before trusting any build
> with real funds.

## Install

**One-liner** (downloads a release tarball, verifies SHA256, installs to `~/.local/bin`):

```bash
curl -fsSL https://raw.githubusercontent.com/r4-ndm/Vaughan-CLI/main/scripts/install.sh | sh
```

**Manual tarball** (auditable — no pipe-to-shell):

```bash
VERSION=v0.1.0
PLATFORM=linux-x86_64   # or linux-aarch64, macos-aarch64

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

See [CONTRIBUTING.md](CONTRIBUTING.md) for fmt/clippy/test expectations.

## Features

- 🔒 **Sovereign self-custody**: password-encrypted vault (Argon2id + AES-256-GCM) with zero plain-text storage
- 🧾 **BIP-39 & HD wallet**: mnemonic create/recover with standard `m/44'/60'/0'/0/{index}` derivation
- ⛓️ **Multi-chain**: PulseChain (mainnet 369 / testnet 943), Ethereum, Sepolia, Polygon, BSC, Base
- ⚡ **Ambire EIP-7702 smart accounts**: atomic batched transfers without ERC-4337 bundler overhead
- 🖥️ **EIP-1193 provider bridge**: local WebSocket JSON-RPC provider with trusted-host allowlisting for Freedom Browser and dApps
- 🔍 **Contract browser REPL (`wiz4rd-engine`)**: bytecode inspection, ERC-20/Uniswap capability probing, dynamic ABI calls
- 🤖 **MCP for external agents**: `vaughan mcp` — Cursor / Claude / Codex propose txs; TUI approves; keys never leave the wallet process

## Architecture

```
vaughan-cli/
├─ vaughan-core/      # Vault encryption, HD wallets, EVM adapters, contract browser engine
├─ vaughan-aa/        # Ambire EIP-7702 batching, delegation, and signature serialization
├─ vaughan-provider/  # Local EIP-1193 WebSocket JSON-RPC server and allowlist security
├─ vaughan-agent/     # Library: proposal engine, sensory tools, circuit breakers
├─ vaughan-mcp/       # MCP stdio server for external agents (Cursor, Claude, …)
├─ vaughan-tui/       # Ratatui terminal frontend (views, dashboard, batch send)
└─ vaughan-cli/       # Unified `vaughan` binary: TUI by default, CLI subcommands
```

## Documentation

- [docs/mcp.md](docs/mcp.md) — MCP setup (external agents; replaces embedded LLM chat)
- [docs/ai-tool-surface.md](docs/ai-tool-surface.md) — Public tool contract for agents
- [docs/mcp-threat-model.md](docs/mcp-threat-model.md) — MCP threat model
- [docs/freedom-browser-integration.md](docs/freedom-browser-integration.md) — EIP-1193 provider bridge
- [docs/ambire-aa.md](docs/ambire-aa.md) — Ambire EIP-7702 batch transactions
- [docs/browser-engine.md](docs/browser-engine.md) — Smart contract browser engine
- [docs/piteas.md](docs/piteas.md) — PulseChain Piteas swap / quote integration notes
- [docs/aggregator.md](docs/aggregator.md) — Multi-DEX aggregator surface
- [PLAN.md](PLAN.md) — Architecture and phase plan (incl. MCP pivot)
- [REQUIREMENTS.md](REQUIREMENTS.md) — Functional and non-functional requirements
- [TASKS.md](TASKS.md) — Implementation backlog and test matrix
- [SECURITY.md](SECURITY.md) — Vulnerability reporting ([@VaughanWallet](https://x.com/VaughanWallet))
- [CONTRIBUTING.md](CONTRIBUTING.md) — Dev setup and PR checklist

## License

Licensed under **MIT OR Apache-2.0** at your option. See [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE).

---

**Built with ❤️ and 🐸⚡**
