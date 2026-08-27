# Vaughan-CLI — Plan

## Vision

Vaughan-CLI is a sovereign Rust multi-chain CLI wallet TUI:

- **Alloy** — wallet core: keys, signing, RPC, transaction building/broadcast.
- **wiz4rd-engine** — contract browser, dynamic call encoder/decoder, capability prober, DEX factory/pair indexer.
- **External agents via MCP** — Cursor / Claude / Codex call `vaughan mcp`; the TUI owns keys and approvals. Embedded in-wallet LLM chat is retired.
- **vaughan-agent** — library only: proposal engine, sensory/write tools, Sentient circuit breakers (no LLM client).
- **ERC-5564 stealth** — spec-first in `vaughan-core` (`docs/erc-5564-stealth.md`). RAILGUN / Kohaku remain deferred (`docs/kohaku-go-no-go.md`).
- **ratatui** — terminal UI.
- **Freedom Browser** — dApp browser that uses Vaughan as its native signing provider.

EVM-first, PulseChain-optimized. Architecture mirrors Vaughan-Dioxus's `vaughan-core`
(chain adapters, core services, security, persistence).

## Workspace layout

```
vaughan-cli/
├─ Cargo.toml            # workspace + shared deps
├─ vaughan-core/         # library — no UI, fully testable
│  ├─ chains/            # ChainAdapter trait + ChainRegistry + types; evm/ (Alloy), bitcoin/ + polkadot/ (future)
│  ├─ core/              # WalletState, AccountManager, NetworkService, StateManager, TransactionService
│  ├─ browser/           # wiz4rd-engine (ABI resolver, prober, selectors, sigdb, call, events)
│  ├─ security/          # hd_wallet (BIP-39/32/44), encryption (Argon2id + AES-256-GCM)
│  └─ error.rs, logging.rs
├─ vaughan-aa/           # EIP-7702 / Ambire smart accounts & batching
├─ vaughan-agent/        # [Phase 5] proposal engine, tool registry, Sentient circuit breakers (no LLM)
├─ vaughan-mcp/          # [Phase 6] MCP stdio server for external agents
├─ vaughan-provider/     # [Phase 2] local EIP-1193 bridge + approval UX + trusted hosts
├─ vaughan-cli/          # Unified `vaughan` binary (TUI + CLI: send, balance, browse, mcp, …)
└─ vaughan-tui/          # Interactive ratatui TUI (views, dashboard, approvals)
```

## Multi-chain architecture

EVM is the only implemented family; the contract is built so Bitcoin and
Polkadot (Substrate) can be added later without touching the UI or services.

- **`ChainType`** (`#[non_exhaustive]`): `Evm`, plus reserved `Bitcoin`/`Polkadot`.
- **`ChainAdapter` trait** (`chains/mod.rs`): the single, minimal contract —
  `chain_type`, `chain_info`, `validate_address`, `get_balance`, `estimate_fee`,
  `send_transaction`, `get_tx_status`, `get_transaction_history`. Family-specific
  concerns (nonce, coin selection, fee application, address encoding, signing)
  stay inside each adapter, never in the trait.
- **Tagged payloads**: `ChainTransaction` and `FeeDetails` are family-tagged
  enums; `ChainInfo.network_id` is an opaque per-family id (EVM chain id string,
  Bitcoin network, Polkadot genesis hash).
- **`ChainRegistry`** (`chains/mod.rs`): builds `Box<dyn ChainAdapter>` for a
  family + network. `NetworkService` and the TUI talk to `dyn ChainAdapter` and
  never match on family.
- **Per-family modules**: `chains/{family}/` each own `types.rs`, `networks.rs`,
  `adapter.rs`. Today: `evm/`. Future: `bitcoin/` (UTXO + coin selection via
  `bdk`), `polkadot/` (Substrate via `subxt`, SS58 addresses, weight-based fees).
- **Derivation is family-aware** (planned): `AccountManager`/`hd_wallet` must
  not hardcode BIP-44. EVM = BIP-44 coin 60; Bitcoin = coin 0/84; Polkadot uses
  Substrate `//soft`/`/hard` secret-URI paths with sr25519 — so the derivation
  scheme is pluggable, not a coin-type constant.

## Technology choices

| Decision | Choice | Rationale |
|---|---|---|
| Language | Rust (2021) | Alloy + ratatui + k256; matches Vaughan-Dioxus layering |
| Ethereum lib | alloy 2.x (resolved 2.4.1) | Single workspace version; upgraded from 1.7 in 2026-08 (Vaughan is early-stage, so the Vaughan-Dioxus pin no longer applies) |
| TUI | ratatui + crossterm | De-facto standard, async/tokio friendly |
| Core layering | reimplement | Clean control; mirror Vaughan-Dioxus, don't vendor |
| HD wallet | bip39 + bip32 | RustCrypto stack, actively maintained; bip39 for mnemonics, bip32 for derivation |
| Vault crypto | Argon2id + AES-256-GCM | Argon2id KDF, authenticated encryption |
| Contract Engine | pure Rust (`wiz4rd-engine`) | Alloy JSON ABI + `alloy-dyn-abi` dynamic encoding + PUSH4 parser + event log scanner |
| External agents | `vaughan-mcp` + `vaughan-agent` | MCP stdio; propose-only writes; TUI approval; Sentient circuit breakers |
| ERC-5564 stealth | `k256` + keccak256 in `vaughan-core` | Spec-first scheme 1; Kohaku unused |
| kohaku-rs / RAILGUN | not a dep | **Deferred** — upstream RAILGUN derivation incompatibility |
| Ambire AA | Rust + Alloy + Ambire ABI | Reimplement from the on-chain `AmbireAccount` contract; Vaughan-Dioxus as guide only |
| dApp bridge | local EIP-1193 JSON-RPC (WebSocket) | Mirrors Vaughan-Dioxus provider-style RPC; loopback only |

## Phases

### Phase 1 — EOA wallet on PulseChain (Done)
Create/restore (BIP-39), password-encrypted vault, HD `m/44'/60'/0'/0/0`, balance,
send native PLS, receive, network switching.

### Phase 2 — Native provider bridge (Done)
`vaughan-provider` local EIP-1193 server + TUI approval flow + trusted hosts, and a
Freedom Browser signer-backend PR (out-of-repo, MPL-2.0).

### Phase 3 — Privacy + smart accounts (AA done; stealth on 943)
Ambire AA in Rust (see `docs/ambire-aa.md`) is **done**. ERC-5564 scheme-1 crypto,
TUI `st:` send/scan/sweep, and the canonical announcer on PulseChain testnet 943
are in (`docs/erc-5564-stealth.md`). Mainnet 369 announcer is still open.
RAILGUN remains deferred.

### Phase 4 — Contract browser & DEX engine (`wiz4rd-engine`) (Done)
Generic browser engine in `vaughan-core` (pure Rust, alloy-native: explorer-ABI
fetch + cache, selector probing, dyn-abi generic calls, event-scan pair/pool
discovery), surfaced as an interactive REPL view in `vaughan-tui` and non-interactive
CLI batch execution (`vaughan browse <address>`). Anvil test suite verified.

### Phase 5 — AI Agent Integration & Multi-Mode Security Sandbox
Shipped the tool/proposal/circuit-breaker foundation and (historically) an embedded
LLM chat path. **Plan change (2026-08-23):** embedded in-wallet LLM UI/CLI was
retired in favour of Vaughan-as-MCP-tool for external agents. Historical design:
`docs/AI-AGENT-ARCHITECTURE.md`. Retired UX guide: `docs/agent-configuration.md`.

### Phase 6 — External Agent / MCP (Current)
Vaughan is a **signing wallet that agents call**, not a host for its own LLM:

- `vaughan mcp` — hand-rolled MCP JSON-RPC over stdio (`docs/mcp.md`); transport decision: `docs/mcp-transport.md` (no `rmcp` rewrite now)
- Hybrid IPC — loopback `127.0.0.1:8746` when TUI unlocked; file queue when offline
- Unified approval with EIP-1193 (`ApprovalKind::McpProposal`); re-simulate at approve
- MCP never unlocks the vault; testnet-first writes (`VAUGHAN_MCP_ALLOW_MAINNET=1`)
- Tool contract + threat model: `docs/ai-tool-surface.md`, `docs/mcp-threat-model.md`
- Requirements: FR-6.1–FR-6.8 in `REQUIREMENTS.md`

**Why the pivot:** one approval gate for dApps and agents; no API keys or model
routing inside the wallet; agents bring their own model (Cursor, Claude Code, …);
smaller attack surface (no `genai` / chat / provider setup in-process).

**v2 (minimal):** `vaughan serve --password-env …` — headless unlock + MCP control
plane; stdio MCP stays the agent client.

## Security model

- **Mnemonic encrypted at rest**: Argon2id -> AES-256-GCM; plaintext only in memory while unlocked.
- **Zero AI key exposure**: MCP and `vaughan-agent` never unlock the vault or hold signers.
- **Ground-truth rendering**: The TUI approval card shows calldata/value/network; agent explanations are labelled untrusted.
- **Physical capital isolation**: Sentient profile paths remain available for high-risk sessions (`~/.vaughan/profiles/sentient/`; legacy `…/degen/`).
- **Circuit breakers**: Max position sizing % per trade, dual-horizon gas caps, adaptive slippage ceilings, and emergency kill-switches (`Esc`/`q`).
- **No telemetry/analytics**: Testnet-first for all fund-moving features (including MCP writes).

## Build order

1. `vaughan-core`: chains, core, security, browser engine (`wiz4rd-engine`), proposal queue + MCP IPC
2. `vaughan-provider`: local EIP-1193 bridge + trusted hosts
3. `vaughan-aa`: EIP-7702 smart account batching
4. `vaughan-agent`: tool registry, proposals, circuit breakers (no LLM client)
5. `vaughan-mcp`: MCP stdio server for external agents
6. `vaughan-tui`: views + MCP listener + unified approval gate
7. `vaughan-cli`: CLI commands (send, balance, browse, propose, mcp, …)
8. `cargo build` + unit tests + Anvil e2e tests + `clippy` + `fmt`

## Risks / open items

- **Freedom Browser bridge** — transport requires a new signer backend + local socket.
- **PulseChain RPC availability** — public endpoints; fallback routing handled via `EvmAdapter::with_provider`.
- **MCP client diversity** — hand-rolled JSON-RPC subset by design; validate against Cursor / Claude Code via [`docs/mcp-smoke.md`](docs/mcp-smoke.md) + `vaughan-mcp` conformance tests. Full `rmcp` rewrite is **not scheduled** — see [`docs/mcp-transport.md`](docs/mcp-transport.md).
- **Wallet daemon (v2)** — minimal `vaughan serve` shipped; full thin-client TUI still optional polish.
