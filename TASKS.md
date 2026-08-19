# Vaughan-CLI — Tasks

Checkbox tasks, ordered by phase. Requirement IDs reference `REQUIREMENTS.md`.

## Phase 1 — EOA wallet on PulseChain

### vaughan-core
- [x] `chains/mod.rs` + `chains/evm/mod.rs` module wiring (FR-1.7)
- [x] Multi-chain contract: `ChainAdapter` trait + `ChainRegistry` + tagged `ChainTransaction`/`Fee`/`ChainInfo` (Bitcoin/Polkadot reserved)
- [x] `security/hd_wallet.rs` — BIP-39 generate/validate + BIP-32/44 derivation (FR-1.1, FR-1.2, FR-1.6)
- [x] `security/encryption.rs` — Argon2id + AES-256-GCM encrypt/decrypt + password policy (FR-1.3, FR-1.4)
- [x] `core/persistence.rs` — `PersistedState` + `StateManager` (save/load vault, active network) (FR-1.3, FR-1.11)
- [x] `core/account.rs` — `Account`/`AccountManager` (derive, list, active account, unlock) (FR-1.6)
- [x] `core/network.rs` — `NetworkService` (built-in networks, active selection) (FR-1.7)
- [x] `core/wallet.rs` — `WalletState` (lock/unlock, active account/network, balance, send) (FR-1.5, FR-1.8, FR-1.9)
- [x] `core/transaction.rs` — `TransactionService` (build native tx, estimate, sign, broadcast) (FR-1.9)
- [x] Unit tests: encryption roundtrip, HD derivation, password policy, networks, persistence, accounts, transactions, wallet lifecycle

### vaughan-tui
- [x] App shell + event loop (ratatui + crossterm) (NFR-4)
- [x] Onboarding: create wallet (show mnemonic) / restore (FR-1.1, FR-1.2)
- [x] Unlock / lock screen (FR-1.5)
- [x] Dashboard: address + native balance (FR-1.8)
- [x] Send: recipient + amount -> fee -> confirm -> broadcast -> tx hash (FR-1.9)
- [x] Receive: display address (FR-1.10)
- [x] Settings: switch active network (FR-1.11)

### Quality gate
- [x] `cargo build --workspace` passes
- [x] `cargo test --workspace` passes
- [x] `cargo fmt --check` clean
- [x] `cargo clippy --workspace -- -D warnings` clean

## Phase 2 — Native provider bridge (Freedom Browser)

> Integration research + browser-side plan: `docs/freedom-browser-integration.md`

- [x] `vaughan-provider` crate: local EIP-1193 WebSocket server (loopback) (FR-2.1)
- [x] Implement provider methods: accounts, chainId, sendTransaction, sign, signTypedData_v4, switchEthereumChain + `vaughan_signTransaction` (FR-2.2)
- [x] TUI approval flow: approve/deny prompts for sign/send (FR-2.3). `ProviderHost` (a `WalletHandle` impl) forwards every provider request to the UI thread over an MPSC channel; sign/send requests surface as a full-screen approve/deny prompt, and the provider server auto-starts on app launch. Core gained `personal_sign` (EIP-191), `eth_signTypedData_v4` (EIP-712), `vaughan_signTransaction` (raw signed tx), and general `send_transaction`. Approval details now include fee before user consent (from explicit gas/fee fields when present, otherwise pre-estimated over RPC).
- [x] Trusted-host allowlist (borrow `vaughan-trusted-hosts`) (FR-2.4). `ProviderServer::with_trusted_origins` now enforces a canonicalized `Origin` allowlist (missing/untrusted origins are rejected at connection time); `vaughan-tui` enables it from `VAUGHAN_PROVIDER_TRUSTED_ORIGINS` (comma-separated origins).
- [x] Trusted-host startup validation path: TUI tests now cover env-derived origin parsing and startup-time server wiring with allowlist enforcement (missing-origin clients are rejected; trusted-origin clients are served).
- [x] Account/chain change event push to clients (`EventBus` → JSON-RPC notifications) (FR-2.2)
- [ ] Freedom Browser signer backend PR (out-of-repo) (FR-2.5)

## Phase 3 — Privacy + smart accounts

- [ ] Harden kohaku-rs: add stealth test vectors + `kohaku-core` tests (FR-3.1) — **deferred by decision**: upstream RAILGUN key-derivation bug (BIP-32 vs babyjubjub seed tree) makes keys incompatible/unrecoverable; see `docs/kohaku-go-no-go.md`
- [ ] Fix kohaku-rs railgun build (git dep / submodule instead of sync script) (FR-3.1) — deferred with FR-3.1
- [ ] Publish `kohaku-core` + `kohaku-stealth` to crates.io (FR-3.1) — deferred with FR-3.1
- [ ] Wire ERC-5564 stealth addresses into Vaughan (FR-3.2) — deferred with FR-3.1
- [ ] Ambire smart accounts in Rust — see `docs/ambire-aa.md` (FR-3.3)
  - [x] Create the `vaughan-aa` workspace crate and document the AGPL-3.0/GPL → MIT/Apache reimplementation boundary
  - [x] Define the smart-account ABI (`sol!`) + `scw_transaction`/`SignatureMode` types from the on-chain `AmbireAccount` contract (Vaughan-Dioxus as guide only)
  - [x] Digest = `keccak256(abi.encode(account, chainId, nonce, txns))`; sign raw hash (`sign_hash`, mode `0`) or EIP-191 (`personal_sign`, mode `1`), append the mode byte. Core gained `security::signing::sign_hash`; the digest is verified byte-for-byte against a hand-built ABI-spec vector.
  - [x] Encode the inner `Transaction[]` batch calldata (`execute` selector + `abi.encode`, round-trip tested). *(Fixture byte-equality is covered by the differential harness below — still pending fixtures.)*
  - [x] Sign a `scw_transaction` and recover/verify the 66-byte `r‖s‖v‖mode` signature (raw-hash + EIP-191)
  - [x] EIP-7702 assembly (`build.rs`): sign the `Authorization` delegating the account EOA to the Ambire implementation and build the `TxEip7702` carrying `execute(txns, signature)` (self-pay, testnet-first). Authority/chain-id are validated against the batch before assembling.
  - [ ] ERC-4337 `UserOperation` / `getUserOpHash` assembly (`build.rs`) — **deferred by decision**: self-pay 7702 is the broadcast route for testnet-first; 4337 only buys gas sponsorship + EntryPoint interop. See `docs/ambire-aa.md` §7.
  - [x] Broadcast via `EvmAdapter` (`adapter.rs`): the **self-pay** path is wired — fetch the account's *pending* nonce (uncached), derive EIP-1559 fees through the adapter's existing heuristic (pinned gas limit, since `eth_estimateGas` can't price a pre-delegation 7702 call), sign the 7702 envelope (auth nonce = account nonce + 1 per EIP-7702's "after the sender's nonce is incremented"), and submit via the adapter's primary + fallback broadcast. Relayer / bundler routes still TBD.
  - [x] Differential test harness: fixtures captured from the **EVM reference** (self-contained forge test in `vaughan-aa/.fixtures-capture/`, gitignored — Solidity's own encoder is the canonical independent implementation); `tests/differential.rs` asserts Rust matches byte-for-byte on preimage + digest + `execute` calldata across 5 cases (single zero-value, native transfer, multi-txn batch, empty batch revert, ERC-20 transfer). See `tests/fixtures/README.md`.
  - [x] Live E2E on a forked testnet (`tests/self_pay_e2e.rs`): forks PulseChain testnet (943, where the real `AmbireAccount` impl lives at `0x2A2b…684EF`), bootstraps the account key privilege via a self-call `setAddrPrivilege(account, bytes32(1))` 7702 tx (a fresh EOA otherwise reverts `INSUFFICIENT_PRIVILEGE`), then `submit_self_pay` signs + broadcasts the batch — the recipient receives exactly the value, and the delegation persists as `0xef0100 || impl` per the final EIP-7702 (delegations are permanent; the transient variant was dropped in the spec).
  - [x] TUI integration: AA batched-send view (`vaughan-tui/src/views/aa_send.rs`, dashboard `b` shortcut). Compose N native transfers (ctrl+a/d rows, ↑↓/tab navigation), confirm shows the per-row list + estimated fee + a one-time delegation note when the account isn't yet delegated, and Enter broadcasts through the 7702 self-pay path. New `vaughan-aa` adapter helpers: `AMBIRE_IMPLEMENTATION` const, `is_delegated` (eth_getCode), `get_account_nonce` (eth_call `nonce()`), `bootstrap_delegation` (delegate + self-call `setAddrPrivilege`), and `submit_batch` (one-shot bootstrap + sign + submit). `WalletState` gained `active_signer`/`active_rpc_url` for flows that compose outside the built-in send path. Tested end-to-end in `vaughan-tui/tests/aa_send_view.rs` against a *forked* testnet (the real AmbireAccount impl): a 2-transfer batch lands both payments on-chain and the account ends permanently delegated.
- [ ] Railgun / privacy pools (FR-3.4) — deferred with FR-3.1 (derivation incompatibility)

## Phase 4 — Contract browser & DEX engine (`wiz4rd-engine`)

> Full scope: `docs/browser-engine.md` and `wiz4rd-swap/docs/other-dexes-scope.md` (rev 5). **Pure Rust, no cast subprocess** — alloy is the battle-tested core (it's the library Foundry itself is built on). Engine is generic wallet tooling (browses/calls *any* contract, not just DEXes). Read-only on other DEXes in v0.1; money-moving flows only through Vaughan's existing alloy signer layer.

### vaughan-core — browser engine (`wiz4rd-engine`, generic, no DEX knowledge)
- [x] `browser/abi.rs`: Explorer `getabi` fetch (`api.scan.pulsechain.com`) + persistent disk cache (FR-4.1)
- [x] `browser/probe.rs`: Selector-probe capability library (ERC20 / V2 factory+pair / V3 factory+pool / WETH / Multicall3) → protocol fingerprint (FR-4.2)
- [x] `browser/selectors.rs`: PUSH4 opcode parse over bytecode from `eth_getCode` (~30 lines, unit-tested) (FR-4.3)
- [x] `browser/sigdb.rs`: Signature lookup via 4byte.directory HTTP API (reqwest) with hex fallback (FR-4.4)
- [x] `browser/call.rs`: Generic read-only call encode/decode via `alloy-dyn-abi` (FR-4.5)
- [x] `browser/events.rs`: Event-scan discovery: `pairs <factory>` via `PairCreated`/`PoolCreated` logs (no init hashes needed) (FR-4.6)
- [x] Unit tests & fixtures: probe fingerprints against known contracts, PUSH4 parser, ABI cache roundtrips, offline signature resolution


### vaughan-tui — browser REPL view
- [x] `views/browser.rs`: input line + scrolling output pane, reusing existing `input.rs` (ratatui + crossterm) (FR-4.7)
- [x] Stateful context: `browse 0x…` sets current contract; `pairs`, `call …`, `callraw …`, `info`, `probe` operate on it (FR-4.7)
- [x] History + scrolling + `help` command, dashboard `c` shortcut, Tab screen cycle (FR-4.7)
- [x] Non-interactive CLI batch mode: `vaughan browse <address> [--call "fn(args)"] [--call-raw 0x...]` (FR-4.7)
- [x] Headless ratatui TestBackend integration tests (`tests/browser_view.rs`) & Anvil E2E tests (`vaughan-cli/tests/deploy.rs`)



### DEX views — from wiz4rd-sdk (after engine exists; wiz4rd-sdk joins this workspace at integration)
- [ ] Protocol views by capability: V2 price (`getReserves` ratio), V3 pool state (`slot0` + wiz4rd-math tick math), token metadata probes
- [ ] Optional `dex price` aggregation view across probed DEXes
- [ ] Deferred: write calls on other DEXes, cross-DEX routing

### Known facts (verified 2026-08-18)
- PulseX Router `0x165C…552d9` → Factory `0x29eA…C523`, PLSX `0x95B3…90ab`, 186,244 pairs
- `getabi` endpoint works on `api.scan.pulsechain.com` (tested on Multicall3)

## Deploy CLI (`vaughan-cli`, added 2026-08-18) — done

> Non-interactive wallet commands for testnet contract deploys (and wiz4rd-swap Phase 3).

- [x] `vaughan-cli` workspace crate → `vaughan` binary
- [x] `vaughan send <to> --data <hex> [--value N] [--network id]` — builds an arbitrary contract call, estimates fee, signs via vault, broadcasts; prints tx hash. The deploy path: pass contract creation bytecode or calldata
- [x] `vaughan balance [--network id]` — active account native balance
- [x] `vaughan networks` — list built-in networks (pulsechain 369, pulsechain-testnet-v4 943, …)
- [x] `vaughan create` / `vaughan restore` — vault bootstrap (mnemonic printed once)
- [x] Password via `--password-env NAME` (automation) or interactive `rpassword` prompt
- [x] Core: `TransactionService::build_contract_call` (validates calldata hex, keeps value/data)
- [x] ⚠️ Fixed HTTPS RPC: added `reqwest-default-tls` to workspace alloy (hyper transport was HTTP-only; balance/send failed on all HTTPS RPCs)
- [x] 137 tests green workspace-wide; clippy clean; verified live: create → balance on mainnet (PLS) + testnet (tPLS)

### Bomb-proofing — Anvil integration tests (`vaughan-cli/tests/deploy.rs`, added 2026-08-18) — done

> Each test spawns its own `anvil --chain-id 943` (matches the built-in testnet network) and exercises the real `vaughan` binary. Run: `cargo test -p vaughan-cli --test deploy`.

- [x] `--rpc-url` override on `send`/`balance` (+ `WalletState::set_rpc_override`, never persisted) — points any command at a dev node
- [x] ⚠️ Core fix: adapter now emits `TxKind::Create` when `to` is the zero address and `--data` is non-empty — **contract creation was previously impossible** (always `TxKind::Call`)
- [x] Test: deploy contract, read `contractAddress` from the receipt, assert deployed code matches
- [x] Test: native transfer moves the exact wei to the recipient
- [x] Test: balance reports the funded account (anvil dev mnemonic == vault derivation, `m/44'/60'/0'/0`)
- [x] Test: insufficient funds fails with a clear error
- [x] Test: wrong password fails with a clear error
- [x] 142 tests green workspace-wide (incl. 5 anvil integration tests); clippy clean

### Provider approval flow — anvil integration tests (`vaughan-tui/tests/provider_approval.rs`, added 2026-08-18) — done

> Each test spawns its own anvil + the real `ProviderServer` + `Eip1193Handler` over WebSocket, with a simulated UI thread draining `HostRequest`s (mirroring the TUI's `poll_provider`). Run: `cargo test -p vaughan-tui --test provider_approval`.

- [x] `vaughan-tui` split into lib + bin so integration tests can drive the provider stack
- [x] Test: dApp `eth_sendTransaction` → approval prompt shown → approve → tx lands on anvil (recipient balance + sender nonce increment)
- [x] Test: deny returns EIP-1193 **4001** and nothing broadcasts (nonce untouched)
- [x] Test: **locked wallet** — reads (`eth_accounts`) answer `[]`, `eth_sendTransaction` rejects with **4100** and never shows a prompt
- [x] ⚠️ Behavior fix: locked wallet no longer prompts — `execute_approval` (shared by TUI + tests) rejects early with `Unauthorized`; the app loop skips the prompt entirely (previously a locked wallet showed a prompt that would fail at execution)
- [x] `execute_approval` is now truly async (shared by the TUI's sync wrapper and async callers)
- [x] 145 tests green workspace-wide (incl. 8 anvil integration tests); no new clippy warnings

### Sign + switch methods — anvil integration tests (added 2026-08-18) — done

- [x] `personal_sign`: approval prompt shown; signature is 65-byte `r‖s‖v` and **recovers to the active account** via foundry's `cast wallet verify`
- [x] `eth_signTypedData_v4`: approval prompt shown; signature **matches `cast wallet sign --data` byte-for-byte** for the same key + EIP-712 payload (exact reference cross-check)
- [x] `wallet_switchEthereumChain`: switch to built-in networks (testnet↔mainnet) reflects in `eth_chainId`; unknown chain rejects with EIP-1193 **4902**
- [x] Test consumer now mirrors the real app: chain id read from the wallet, switch actually switches (was hardcoded/stubbed)
- [x] 148 tests green workspace-wide (incl. 11 anvil integration tests); no new clippy warnings

### `vaughan_signTransaction` (sign-only) — anvil integration tests (added 2026-08-18) — done

- [x] Approve: returns a raw signed tx (nothing broadcast yet); anvil **accepts it via `eth_sendRawTransaction`** and mines it — proving signature recovery, nonce and chain id — and the recipient receives the exact value
- [x] Deny: EIP-1193 **4001**, no raw tx, nothing on chain, nonce untouched
- [x] 150 tests green workspace-wide (incl. 13 anvil integration tests); no new clippy warnings

### TUI native send view — anvil integration tests (`vaughan-tui/tests/send_view.rs`, added 2026-08-18) — done

> Drives the real `SendView` state machine (Input → Confirm → Done) with real key events, renders it headlessly via ratatui's `TestBackend`, and verifies broadcasts on-chain. Run: `cargo test -p vaughan-tui --test send_view`.

- [x] Happy path: recipient + amount → confirm screen shows fee + recipient → Enter broadcasts; rendered done-stage shows the tx hash, `eth_getTransactionByHash` matches the form input (to/value), receipt status `0x1`, funds moved, nonce advanced
- [x] Insufficient funds: fails cleanly, nothing lands, nonce untouched, view returns to the form
- [x] Invalid amount: never leaves the input stage, nothing lands
- [x] Esc on confirm: cancels, back to the form, nothing broadcast
- [x] Shared anvil/`funded_wallet` helpers extracted to `tests/common/mod.rs` (both test binaries consume it)
- [x] 154 tests green workspace-wide (incl. 17 anvil integration tests); no new clippy warnings

### Receive + settings views — anvil/TestBackend tests (`receive_view.rs`, `settings_view.rs`, added 2026-08-18) — done

- [x] Receive: renders the active address + network; locked wallet shows `(locked)` (no address leak); live network switch reflected in the render; Esc → dashboard
- [x] Settings: lists all built-in networks with chain ids and marks the active one with `*`; Enter switches (wallet state + marker move + `Switched to …` status); the `chainChanged` event fires with the new chain id; arrow keys move the highlight; Esc → dashboard
- [x] Headless render helper (`render_frame`) moved to `tests/common/mod.rs`; all three view test binaries share it
- [x] 162 tests green workspace-wide (incl. 25 anvil integration tests); no new clippy warnings

### Onboarding + unlock views — TestBackend tests (`onboarding_view.rs`, `unlock_view.rs`, added 2026-08-18) — done

- [x] Onboarding create flow: `c` generates and renders a 12-word mnemonic; password + confirmation create a persisted, unlocked wallet and navigate to the dashboard
- [x] Onboarding failure paths: mismatched confirmation → "Passwords do not match." back to the password stage; weak password rejected by policy; invalid restore phrase stays on phrase entry — wallet never created
- [x] Onboarding restore flow: valid phrase → password → confirmation creates the wallet; active address matches foundry's canonical derivation of the phrase
- [x] Unlock: correct password unlocks, navigates to the dashboard, and fires `accountsChanged` with the live address; wrong password keeps the wallet locked, shows "wrong password", never navigates or fires the event
- [x] `fresh_wallet` helper added to `tests/common/mod.rs` (onboarding creates its own wallet; no anvil needed — fully offline)
- [x] 169 tests green workspace-wide (incl. 32 anvil/TestBackend integration tests); no new clippy warnings

### Dashboard view — anvil/TestBackend tests (`dashboard_view.rs`, added 2026-08-18) — done

- [x] Renders the active address, network with the `(testnet)` marker, the live balance (`formatted + symbol`), and the shortcut bar
- [x] `r` refresh: fresh dashboard shows `—`, pressing `r` fetches the balance from anvil and renders it
- [x] `l` lock: wallet locks, navigates to Unlock, and publishes `accountsChanged` with an **empty** account list (dApps see the lock)
- [x] `s`/`v`/`n` navigate to Send/Receive/Settings; locked wallet renders `(locked)` instead of the address
- [x] 174 tests green workspace-wide (incl. 37 integration tests); no new clippy warnings

## Phase 5 — AI Agent Integration & Multi-Mode Security Sandbox

> Full specification: `docs/AI-AGENT-ARCHITECTURE.md`.
> Complete security sandboxing: 3-tier operating mode decided at startup, zero private key access for the advisor, isolated burner profile for degen bot, deterministic circuit breakers.

### Step 1: 3-Tier Operating Mode & Profile Isolation (Done)
- [x] `OperatingMode` enum: `HumanOnly`, `AiAssisted`, `DegenTrader` (FR-5.1)
- [x] Session-level immutability: mode is selected at startup/welcome screen and locked permanently for that process (FR-5.1)
- [x] Profile Directory Isolation: Degen Mode runs in isolated directory `~/.vaughan/profiles/degen/` with separate keys/vault (FR-5.2)
- [x] Dynamic dashboard badge rendering and CLI `--profile <name>` / `--mode <human|assist|degen>` flags
- [x] Unit & integration tests in `vaughan-core` and `vaughan-tui` passing (FR-5.1, FR-5.2)

### Step 2: `vaughan-agent` Workspace Crate & Provider Adapters (Done)
- [x] New `vaughan-agent` crate added to workspace
- [x] LLM Provider trait: `LlmClient` with completion & function calling
- [x] Local Ollama / `llama.cpp` provider (`http://127.0.0.1:11434`) (FR-5.7)
- [x] Cloud providers (Google Gemini API & OpenAI) with SecretString key handling (FR-5.7)
- [x] Unit test suite verifying message format and client instantiation passing (FR-5.7)

### Step 3: Structured Tool Registry & Sensory Layer (Done)
- [x] Tool trait & JSON schema generation (`Tool`, `ToolContext`, `ToolRegistry`) (FR-5.4)
- [x] Read-only tools wrapping `wiz4rd-engine`: `inspect_contract`, `get_balance`, `get_dex_reserves`, `search_pairs` (FR-5.4)
- [x] Pre-flight simulation tool: `simulate_call` via `eth_call` (FR-5.4)
- [x] Deterministic Anvil test suite for tool registry and bytecode execution passing (FR-5.4)

### Step 4: Propose-Only Write Tools (Assist Mode) (Done)
- [x] `TxProposal` struct (typed target, calldata, value, fee estimate, simulation result) (FR-5.5)
- [x] Proposal tools: `propose_transfer`, `propose_swap`, `propose_batch_7702`, `propose_contract_call` (FR-5.5)
- [x] Pre-flight simulation verification and deterministic Anvil proposal integration test suite (FR-5.5)

### Step 5: Degen Mode Autonomous Trader & Circuit Breakers (Done)
- [x] Autonomous execution loop with isolated burner wallet signer (`DegenTrader`) (FR-5.6)
- [x] Circuit breaker: position sizing limit (max % of balance per trade) (FR-5.6)
- [x] Circuit breaker: gas burn ceiling & consecutive error tripwire (FR-5.6)
- [x] Circuit breaker: hard slippage limit (max 1.0%) (FR-5.6)
- [x] Multi-RPC quorum validation to defeat rogue/compromised or stale RPCs (FR-5.6)
- [x] Emergency stop (kill-switch via `Esc`/`q`) (FR-5.6)
- [x] Deterministic Anvil autonomous trader test suite passing (FR-5.6)

### Step 6: TUI Agent View & CLI Non-Interactive Execution (Done)
- [x] `vaughan-tui/src/views/agent.rs`: Interactive chat REPL with tool status, cold-storage human-only barrier, and confirmation cards (FR-5.8)
- [x] Welcome screen 3-way mode selector UI & dashboard operating badges (FR-5.1)
- [x] `vaughan agent "<prompt>"` non-interactive CLI subcommand with sensory & proposal tool invocation (FR-5.8)

### Step 7: Bomb-Proofing & Anvil Integration Tests (Done)
- [x] `vaughan-agent/tests/agent_anvil.rs`: End-to-end tests driving the agent against local Anvil node (FR-5.9)
- [x] Test: Agent correctly inspects Anvil-deployed token and executes read tools (FR-5.9)
- [x] Test: Agent proposes transfer -> Human approves -> Tx confirms on Anvil (FR-5.9)
- [x] Test: Degen Mode circuit breaker triggers and halts on excessive gas or high slippage (FR-5.9)

## Later — non-EVM families (deferred, no FR yet)

- [ ] `chains/bitcoin/` adapter (UTXO model, coin selection, `bdk`)
- [ ] `chains/polkadot/` adapter (Substrate, SS58 addresses, weight-based fees, `subxt`)

- [ ] Family-aware derivation schemes (BIP-44 per coin type; Substrate `//`-path secret URIs)
