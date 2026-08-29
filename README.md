# Vaughan-CLI

<p align="center">
    <a href="https://github.com/r4-ndm/Vaughan-CLI"><img width="450" alt="Vaughan CLI Logo" src="branding/vaughan-logo-small.png" /></a><br />
</p>

<p align="center">A multi-chain wallet for the terminal. 🐸⚡</p>

<p align="center">
  <a href="https://github.com/r4-ndm/Vaughan-CLI/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/r4-ndm/Vaughan-CLI/actions/workflows/ci.yml/badge.svg" /></a>
  <a href="LICENSE-MIT"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg" /></a>
</p>

**Vaughan-CLI** is a Rust terminal wallet for **Browserless Pulse** — swap, inspect,
and approve in the TUI without Chrome. Agents talk to Vaughan via MCP; **VB**
(`vaughan-dapp-browser`) is the optional owned Chromium side door.
**Freedom Browser is parked** until [upstream PR #195](https://github.com/solardev-xyz/freedom-browser/pull/195)
merges — see [docs/freedom-browser-status.md](docs/freedom-browser-status.md).

- **Alloy** for the wallet core — keys, signing, RPC, transaction building and broadcast
- **ratatui** for the terminal interface
- **In-core ERC-5564 stealth** (Kohaku / RAILGUN deferred — see [docs/kohaku-go-no-go.md](docs/kohaku-go-no-go.md))
- **MCP** for external agents — Cursor / Claude propose; you approve in Vaughan ([docs/mcp.md](docs/mcp.md))
- **VB** (`vaughan-dapp-browser`) — optional allowlisted Chromium shell + CDP ([docs/dapp-browser-strategy.md](docs/dapp-browser-strategy.md))
- **Freedom Browser** — **parked** (upstream [PR #195](https://github.com/solardev-xyz/freedom-browser/pull/195) pending); dev fallback only

EVM-first and PulseChain-optimized. Thesis: [docs/browserless-pulse.md](docs/browserless-pulse.md).
Architecture mirrors the `vaughan-core` layering from
[Vaughan-Dioxus](https://github.com/r4-ndm/Vaughan-Dioxus).

> **Status:** public **prototype** on `main` — expect rough edges and breaking
> changes. Primary UX is Dashboard → Ag / Dex / Browse / MCP — not a web wallet.
> Embedded in-wallet LLM chat is retired. Prefer testnet. Review
> [SECURITY.md](SECURITY.md) before trusting any build with real funds.

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

## Getting started for humans

Vaughan is a **terminal wallet for PulseChain**. You stay in control: every spend
needs your explicit approval unless you deliberately set up a separate
“sentient” burner profile (see [docs/agent-roles.md](docs/agent-roles.md)).

**Start on testnet (chain 943)** until you are comfortable. Mainnet (369) is
supported but real money — treat mistakes as permanent.

### First run

1. Run `vaughan` and **create** or **restore** a wallet (12-word phrase).
2. Pick **PulseChain testnet** in Settings if it is not already active.
3. Get test PLS from a faucet if your balance is zero.
4. Stay on the **Dashboard** — that is home. Other screens are one key away.

### Where to do what (TUI shortcuts)

| Key | Screen | Use it for |
|-----|--------|------------|
| `s` | Send | Send native PLS or ERC-20 |
| `g` | Ag | Aggregator swap (best route across venues) |
| `d` | Dex | Swap on a named DEX (PulseX, Wiz4rd, …) |
| `c` | Browse | Inspect any contract — read-only REPL |
| `e` | Wrap | Wrap / unwrap WPLS |
| `m` | History | Recent token transfers |
| `j` | Approvals | See and revoke token allowances |
| `w` | Web | Optional VB / Freedom fallback (Freedom parked — see docs) |
| `Tab` | — | Cycle Ag → Dex → Browse → … |

**Browserless Pulse** means: swap, inspect, and revoke **without opening
PulseX (or similar) in a browser**. Prefer Ag / Dex / Browse / MCP; use **VB**
when a whitelisted site needs a page. Freedom stays parked until upstream PR
#195 merges.

### Hardware wallets on Linux (udev)

USB needs vendor udev rules (CachyOS/Arch included). In the TUI:
**Settings (`n`) → `h`**, or see
[`docs/hardware-wallets.md`](docs/hardware-wallets.md#linux-usb-udev--ledger--trezor).

- Ledger: [Fix USB connection issues](https://support.ledger.com/article/115005165269-zd)
- Trezor: [Udev rules](https://trezor.io/guides/trezorctl/udev-rules)

Then Keys → **4 Add Ledger** when ready (Trezor signing is Phase 2).

### Typical flows

**Swap without a website**

1. Unlock → press `g` (Ag) or `d` (Dex).
2. Enter amount and tokens (Dex prefills routers for known venues).
3. Review the confirm screen — **fee is shown before you approve**.
4. For token→token swaps, you may approve the router once, then confirm the swap.

**Send to someone**

1. `s` → recipient address → amount → confirm (fee shown) → broadcast.

**Inspect a contract**

1. `c` → `browse 0x…` → `call balanceOf(0x…)` or `probe` to see what it is.

### Using AI safely (MCP)

External agents (Cursor, Claude, Codex) do **not** get your keys. They call
Vaughan over MCP, which **proposes** transactions; **you** approve in the TUI.

Rough flow:

1. Configure MCP per [docs/mcp.md](docs/mcp.md) (`vaughan mcp` stdio server).
2. Ask the agent to quote or inspect (read-only tools are safe to run freely).
3. When it calls `propose_*`, open Vaughan — a proposal card appears.
4. Read the decoded calldata and fee, not just the agent’s explanation text.
5. Approve or deny. Denied proposals are discarded; approved ones broadcast once.

Guards you get by default: re-simulation at approve time, chain mismatch
rejection, fee-spike rejection if gas jumped more than ~10% since the proposal,
session token for the local queue, and testnet-first mainnet gating for MCP
writes. Details: [docs/mcp-threat-model.md](docs/mcp-threat-model.md).

### What is in good shape vs still rough

| Area | Status |
|------|--------|
| Core wallet (create, send, receive, networks) | Solid |
| Ag + Dex swaps with fee on confirm | Solid |
| MCP propose → human approve | Solid |
| Stealth send / receive (ERC-5564) | Solid on testnet |
| Smart-account batch sends (7702) | Testnet-first |
| Slash commands (`/swap …` in Browser REPL) | Shipped (jump to Ag / browse / Approvals / Receive) |
| Contract browser **writes** from REPL | Shipped (`write` / `writeraw` → fee confirm) |
| Recorded demo walkthrough | Not shipped yet |
| Official Omnibridge / PulseRamp | Deferred (use LibertySwap Bridge) |

This repo is a **prototype** — expect rough edges. Read [SECURITY.md](SECURITY.md)
before mainnet funds.

### Headless / scripting

```bash
vaughan balance --network pulsechain-testnet-v4
vaughan send 0x… --value 0.01 --network pulsechain-testnet-v4
vaughan serve --password-env VAUGHAN_PASSWORD   # unlock + MCP listener for agents
```

For agent tool names and limits, see [docs/ai-tool-surface.md](docs/ai-tool-surface.md).

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

- 🐸 **Browserless Pulse**: Ag / Dex / contract browser / approvals in-TUI — approve calldata, not websites
- 🔒 **Sovereign self-custody**: password-encrypted vault (Argon2id + AES-256-GCM) with zero plain-text storage
- 🧾 **BIP-39 & HD wallet**: mnemonic create/recover with standard `m/44'/60'/0'/0/{index}` derivation
- ⛓️ **Multi-chain**: PulseChain (mainnet 369 / testnet 943), Ethereum, Sepolia, Polygon, BSC, Base
- ⚡ **Ambire EIP-7702 smart accounts**: atomic batched transfers without ERC-4337 bundler overhead
- 🤖 **MCP for external agents**: `vaughan mcp` — Cursor / Claude / Codex propose txs; TUI approves; keys never leave the wallet process
- 🔍 **Contract browser REPL (`wiz4rd-engine`)**: bytecode inspection, ERC-20/Uniswap capability probing, dynamic ABI calls
- 🖥️ **VB + provider bridge**: optional allowlisted Chromium shell (`vaughan-dapp-browser`); Freedom **parked** until PR #195

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

- [docs/browserless-pulse.md](docs/browserless-pulse.md) — Product thesis (TUI-first; VB optional web)
- [docs/freedom-browser-status.md](docs/freedom-browser-status.md) — Freedom **parked** until upstream PR #195
- [docs/fable-5-audit-prompt.md](docs/fable-5-audit-prompt.md) — Parked comprehensive audit prompt (Fable 5; run before release)
- [docs/wiz4rd-addresses.md](docs/wiz4rd-addresses.md) — wiz4rd V3 deploy on Pulse testnet 943
- [docs/wiz4rd-agent-plan.md](docs/wiz4rd-agent-plan.md) — Agents + wiz4rd capability plan
- [docs/mcp.md](docs/mcp.md) — MCP setup (external agents; replaces embedded LLM chat)
- [docs/mcp-smoke.md](docs/mcp-smoke.md) — Cursor smoke checklist + conformance tests
- [docs/mcp-transport.md](docs/mcp-transport.md) — Hand-rolled MCP vs `rmcp` (no rewrite now)
- [docs/sentient-ops.md](docs/sentient-ops.md) — Always-on serve, watch loops, multi-tenant boundaries
- [docs/ai-tool-surface.md](docs/ai-tool-surface.md) — Public tool contract for agents
- [docs/pulse-defi-skills.md](docs/pulse-defi-skills.md) — Pulse DeFi MCP skill pack (quote / trade)
- [vaughan-agent/skills/INDEX.md](vaughan-agent/skills/INDEX.md) — **Agent playbooks index** (mcp-connect, vb-ag-quotes, dapp-connect, …)
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
