# Vaughan-CLI — Requirements

A Rust multi-chain CLI wallet TUI. Alloy for the wallet core, ERC-5564 stealth
in `vaughan-core`, and a ratatui terminal frontend. EVM-first, PulseChain-optimized.
Architecture modeled on the `vaughan-core` layering from Vaughan-Dioxus.

Requirement IDs are referenced by `TASKS.md`.

## Goals

- Self-custody wallet usable entirely from a terminal.
- Wallet core (chains, keys, signing, RPC, tx building) built on Alloy.
- EVM first-class; PulseChain mainnet (369) and testnet v4 (943) as the primary targets.
- Native provider bridge so Freedom Browser (and other EIP-1193/6963 browsers) can use
  Vaughan as the signing wallet.
- Smart accounts via Ambire (ERC-4337/7702 AA) reimplemented in Rust — done
  (FR-3.3, see `docs/ambire-aa.md`).
- Privacy: ERC-5564 stealth in `vaughan-core` (no Kohaku). RAILGUN still
  **deferred** (see `docs/kohaku-go-no-go.md` and `docs/erc-5564-stealth.md`).

## Non-goals (deferred, not dropped)

- Full smart-account UX (batching, recovery, multisig) in Phase 1.
- ERC-20 token management and transaction history in Phase 1.
- Hardware wallet (Ledger/Trezor) support.
- A **general-purpose** open-internet browser / Chrome replacement inside Vaughan.
  An **optional**, modular, allowlisted Chromium dApp shell (`vaughan-dapp-browser`,
  Tauri + CEF) with CDP agent control is in scope as a side door — see FR-7.* and
  `docs/dapp-browser-strategy.md`. It must not live in `vaughan-core`, must not
  auto-sign, and default `vaughan-cli` builds must not require CEF.

## Functional requirements

### Phase 1 — EOA wallet on PulseChain

- **FR-1.1** Create a new wallet with a 12-word BIP-39 mnemonic (English).
- **FR-1.2** Restore a wallet from a mnemonic; validate before storing.
- **FR-1.3** Password-protected vault: Argon2id KDF derives an AES-256-GCM key that
  encrypts the mnemonic; only ciphertext is written to disk.
- **FR-1.4** Password strength policy: >= 12 chars, uppercase, lowercase, digit, symbol.
- **FR-1.5** Lock / unlock flow: on launch the wallet is locked; unlock decrypts the
  mnemonic into memory only.
- **FR-1.6** HD derivation at `m/44'/60'/0'/0/{index}` producing an Alloy local signer;
  support a list of derived accounts (index 0 active by default).
- **FR-1.7** Built-in networks: PulseChain mainnet (369), PulseChain testnet v4 (943),
  Ethereum (1), Sepolia (11155111), Polygon (137), BSC (56), Base (8453).
- **FR-1.8** Dashboard: show active account address and native balance.
- **FR-1.9** Send native asset: recipient + amount -> fee estimate -> sign -> broadcast
  -> show tx hash.
- **FR-1.10** Receive: display the active address.
- **FR-1.11** Settings: switch the active network; selection persists.

### Phase 2 — Native provider bridge (Freedom Browser)

- **FR-2.1** Local EIP-1193 JSON-RPC server (WebSocket on 127.0.0.1), loopback only.
- **FR-2.2** Implement `eth_accounts`, `eth_requestAccounts`, `eth_chainId`,
  `eth_sendTransaction`, `personal_sign`, `eth_signTypedData_v4`,
  `wallet_switchEthereumChain`, and account/chain change events.
- **FR-2.3** Every signing / send request requires an explicit approve/deny prompt in
  the TUI. Never auto-sign.
- **FR-2.4** Trusted-host allowlist for dApp origins (borrowed from Vaughan-Dioxus's
  `vaughan-trusted-hosts`).
- **FR-2.5** Freedom Browser integration: a signer backend (analogous to its Ledger
  backend) that forwards sign requests to Vaughan's local endpoint (out-of-repo PR).

### Phase 3 — Privacy + smart accounts

- **FR-3.1** Harden kohaku-rs: real test coverage, fix the railgun build, publish
  `kohaku-core` / `kohaku-stealth` to crates.io. — **deferred by decision**: upstream
  `ethereum/kohaku` derives RAILGUN keys with BIP-32 while the canonical RAILGUN
  engine uses a babyjubjub seed tree, producing incompatible/unrecoverable keys;
  upstream is also unaudited and pre-production. See `docs/kohaku-go-no-go.md`.
- **FR-3.2** ERC-5564 stealth addresses — **implemented on testnet**, spec-first in
  `vaughan-core` (scheme 1, HD `m/5564'/60'/0'/{0,1}'`). Not Kohaku.
  See `docs/erc-5564-stealth.md`. TUI `st:` send/scan/sweep and the 943 announcer
  are live; CREATE2-deploy on PulseChain 369 is still open.
- **FR-3.3** Ambire smart accounts (ERC-4337/7702 AA) reimplemented in Rust via Alloy +
  Ambire ABI (borrowed from Vaughan-Dioxus). — **implemented**: `vaughan-aa` with
  differential (EVM-reference) fixtures and a live 7702 self-pay E2E; TUI batched-send
  view landed (see `docs/ambire-aa.md`). ERC-4337 `UserOperation` stays deferred by
  decision (self-pay 7702 is the testnet-first route).
- **FR-3.4** Railgun / privacy pools — **deferred by decision** (see
  `docs/kohaku-go-no-go.md`); revisit only when the derivation incompatibility is
  resolved upstream or a spec-first implementation is scoped.

### Phase 4 — Contract Browser & DEX Engine (`wiz4rd-engine`)

- **FR-4.1** Explorer ABI Resolution: Fetch verified smart contract ABIs from block explorer APIs (`api.scan.pulsechain.com`) with a persistent local disk cache.
- **FR-4.2** Capability Selector Probing: Fingerprint contract types (ERC-20, Uniswap V2/PulseX Factory & Pair, Uniswap V3 Factory & Pool, WETH, Multicall3) by executing non-reverting `eth_call` probes against known selector suites.
- **FR-4.3** Bytecode Selector Extraction: Parse PUSH4 opcode candidate function selectors directly from deployed bytecode (`eth_getCode`) for unverified contracts.
- **FR-4.4** Signature Database Lookup: Reverse-lookup extracted 4-byte selectors via 4byte.directory HTTP API with graceful offline fallback to hex selectors.
- **FR-4.5** Generic Dynamic Calls: Execute read-only contract function calls with dynamic type encoding and decoding via `alloy-dyn-abi`.
- **FR-4.6** Event-Scan Pair Discovery: Discover DEX liquidity pairs and pools by scanning `PairCreated` and `PoolCreated` logs directly from factory contracts without hardcoded init-code hashes.
- **FR-4.7** Interactive Browser REPL & Batch Mode: Surfaced as a stateful interactive REPL view in `vaughan-tui` (`browse <address>`, `call <sig>`, `pairs`, `token`, `info`, `probe`) and non-interactive CLI batch execution (`vaughan browse <address> --call <sig>`).

### Phase 5 — AI Agent Integration & Multi-Mode Security Sandbox

- **FR-5.1** 3-Tier Operating Mode Selection: Decision at startup/welcome screen (`HumanOnly`, `AiAssisted`, `DegenTrader`). The selection is immutable for the lifetime of that process session (impossible to toggle mid-session).
- **FR-5.2** Profile & Vault Physical Isolation: `DegenTrader` mode runs strictly in a dedicated isolated sub-profile directory (`~/.vaughan/profiles/degen/`) with separate seed phrases, ensuring primary funds are physically inaccessible.
- **FR-5.3** AI Agent Engine & Tool Registry (`vaughan-agent` crate): Core agent runtime with schema-driven tool calling, conversation history, and multi-turn planning.
- **FR-5.4** Autonomous Read/Inspect Tools: Wrap `wiz4rd-engine` and `vaughan-core` for contract capability probing, balance inspection, selector reverse lookup, DEX reserves, and pre-flight call simulation without user prompts.
- **FR-5.5** Guarded Propose-Only Write Tools (Assist Mode): Draft transfers, DEX swaps, and EIP-7702 batched calls into structured `TxProposal`s that require explicit human approval via the TUI/CLI confirmation card. Private keys are never exposed to the agent.
- **FR-5.6** Autonomous Execution with Circuit Breakers (Degen Mode): In degen mode, automated signing is governed by hardcoded Rust circuit breakers (max position size %, gas burn rate ceiling, maximum 1.0% slippage, emergency kill-switch).
- **FR-5.7** ~~Multi-Model Provider Integration~~ — **retired** (2026-08-23). Superseded by FR-6 (external agents via MCP).
- **FR-5.8** ~~TUI Agent Console & CLI Commands~~ — **retired** (2026-08-23). Superseded by FR-6 (external agents via MCP).

### Phase 6 — External Agent / MCP Integration

- **FR-6.1** MCP stdio server (`vaughan mcp`): hand-rolled JSON-RPC subset (`initialize`, `tools/list`, `tools/call`, `ping`) over stdin/stdout; diagnostics to stderr only.
- **FR-6.2** Public tool contract documented in `docs/ai-tool-surface.md`: read tools (no approval), write tools (propose only), meta tools, hard bans, structured error codes.
- **FR-6.3** Hybrid IPC: loopback TCP (`127.0.0.1:8746`) when TUI unlocked; file queue (`proposals/pending/`) when offline; session token auth; HMAC integrity on queue files.
- **FR-6.4** Unified human approval: MCP proposals route through the same `ApprovalKind` gate as EIP-1193 dApp requests; re-simulate and re-estimate fee at approve time.
- **FR-6.5** MCP process never unlocks vault or holds key material; signing only in TUI after explicit user approval.
- **FR-6.6** CLI `--json` surface on `balance`, `assets`, `browse`, `propose`, `proposals` for script/agent use without MCP.
- **FR-6.7** Testnet-first default for MCP writes; mainnet writes require `VAUGHAN_MCP_ALLOW_MAINNET=1`.
- **FR-6.8** Threat model in `docs/mcp-threat-model.md`; zero telemetry (NFR-2).

### Phase 7 — Optional Chromium dApp browser + agent control

> Side door only. Strategy: `docs/dapp-browser-strategy.md`. Modular binary;
> multi-chain EVM; never auto-sign. Default product path remains Browserless Pulse.

- **FR-7.1** Optional `vaughan-dapp-browser` binary (Tauri + CEF / Chromium). CEF
  deps must not link into `vaughan-core` / default `vaughan-cli` builds.
- **FR-7.2** Allowlisted HTTPS navigation only; soft-launch from TUI (`w`) when
  binary present; Freedom (or clear unavailable) when absent.
- **FR-7.3** Thin-proxy EIP-1193 / EIP-6963 into existing `vaughan-provider`; all
  sign/send require TUI approve (same gate as FR-2.3 / FR-6.4).
- **FR-7.4** Multi-chain EVM: chain id from Vaughan’s active network +
  `wallet_switchEthereumChain`; allowlist is hosts/origins, not Pulse-only.
- **FR-7.5** Localhost CDP only when Settings “agent browser control” is on
  (default off); MCP `browser_open` / `navigate` / `status` (B1) then snapshot /
  click / type / wait (B2). Tools degrade if binary absent.
- **FR-7.6** Documented kill-switch: remove/disable the crate without breaking
  Browserless Pulse or the Freedom provider path.

## Non-functional requirements

- **NFR-1** Secrets never persisted unencrypted; zeroized in memory after use.
- **NFR-2** No telemetry, analytics, or external data collection.
- **NFR-3** Testnet-first: any feature that moves funds is exercised on testnet before mainnet.
- **NFR-4** Cross-platform: Linux, macOS, Windows.
- **NFR-5** Unit tests cover encryption, HD derivation, transaction building, and network config.
- **NFR-6** `cargo fmt` + `clippy` clean; `unsafe_code` forbidden.

## Constraints & decisions

- **C-1** Language: Rust (edition 2021).
- **C-2** Alloy 2.x (single workspace version; upgraded from 1.7 in 2026-08 —
  Vaughan was early-stage, so the Vaughan-Dioxus pin no longer applies).
- **C-3** TUI: ratatui + crossterm.
- **C-4** `vaughan-core` reimplements the Vaughan-Dioxus layering (not vendored).
- **C-5** Kohaku-rs is **not** a dependency. ERC-5564 is implemented in
  `vaughan-core`. RAILGUN stays deferred (`docs/kohaku-go-no-go.md`).
- **C-7** ERC-5564 v1 claim: payments are unlinkable to the recipient's
  **public** address. Sender, amount, and token stay visible. Sender attaches a
  PLS stipend so the recipient need not fund the stealth address from the main
  account.
- **C-6** PulseChain is the primary target chain; optional dApp browser is
  multi-chain EVM (FR-7.4), not Pulse-locked.
- **C-8** Optional Chromium/CEF browser is a separate binary only
  (`docs/dapp-browser-strategy.md`); never a hard dependency of `vaughan-core`.
