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

## Phase 2 — Native provider bridge (Freedom Browser — **parked**)

> **Parked** until upstream [PR #195](https://github.com/solardev-xyz/freedom-browser/pull/195)
> merges. Vaughan-side bridge is done; no active Freedom work until then.
> Status: [docs/freedom-browser-status.md](docs/freedom-browser-status.md)
> Integration research: [docs/freedom-browser-integration.md](docs/freedom-browser-integration.md)

- [x] `vaughan-provider` crate: local EIP-1193 WebSocket server (loopback) (FR-2.1)
- [x] Implement provider methods: accounts, chainId, sendTransaction, sign, signTypedData_v4, switchEthereumChain + `vaughan_signTransaction` (FR-2.2)
- [x] TUI approval flow: approve/deny prompts for sign/send (FR-2.3). `ProviderHost` (a `WalletHandle` impl) forwards every provider request to the UI thread over an MPSC channel; sign/send requests surface as a full-screen approve/deny prompt, and the provider server auto-starts on app launch. Core gained `personal_sign` (EIP-191), `eth_signTypedData_v4` (EIP-712), `vaughan_signTransaction` (raw signed tx), and general `send_transaction`. Approval details now include fee before user consent (from explicit gas/fee fields when present, otherwise pre-estimated over RPC).
- [x] Trusted-host allowlist (borrow `vaughan-trusted-hosts`) (FR-2.4). `ProviderServer::with_trusted_origins` now enforces a canonicalized `Origin` allowlist (missing/untrusted origins are rejected at connection time); Vaughan always merges Freedom's Origin `https://freedom.browser`, plus optional `VAUGHAN_PROVIDER_TRUSTED_ORIGINS` and persisted dApp origins.
- [x] Trusted-host startup validation path: TUI tests now cover env-derived origin parsing and startup-time server wiring with allowlist enforcement (missing-origin clients are rejected; trusted-origin clients are served).
- [x] Account/chain change event push to clients (`EventBus` → JSON-RPC notifications) (FR-2.2)
- [x] Freedom Browser signer backend PR (out-of-repo) (FR-2.5) — **open, parked**: https://github.com/solardev-xyz/freedom-browser/pull/195 (`feat/vaughan-signer-backend`; Vaughan `freedom_bridge_smoke` green; **no further Freedom integration until merge**)

## Phase 3 — Privacy + smart accounts

- [ ] Harden kohaku-rs: add stealth test vectors + `kohaku-core` tests (FR-3.1) — **deferred by decision**: upstream RAILGUN key-derivation bug (BIP-32 vs babyjubjub seed tree) makes keys incompatible/unrecoverable; see `docs/kohaku-go-no-go.md`
- [ ] Fix kohaku-rs railgun build (git dep / submodule instead of sync script) (FR-3.1) — deferred with FR-3.1
- [ ] Publish `kohaku-core` + `kohaku-stealth` to crates.io (FR-3.1) — deferred with FR-3.1
- [x] ERC-5564 scheme-1 crypto in `vaughan-core` (spec-first, no Kohaku) (FR-3.2)
- [x] CREATE2-deploy canonical announcer on PulseChain testnet 943 (FR-3.2) — live at `0x55649E01B5Df198D18D95b5cc5051630cfD45564` (tx `0x1df79490a33e146b4915a0cab2e293f2b711c07f08a966b3a3795d6ad070ce98`, block 25174175)
- [x] Live 943 send → announce → scan → sweep E2E (`vaughan-core/tests/stealth_943.rs`, ignored by default)
- [x] Anvil stealth tests: core send/scan/sweep, Alice→Bob isolation, dust stipend, scan after later blocks, TUI `st:` send/scan/sweep
- [x] TUI: stealth receive URI, send `st:`, scan, sweep (FR-3.2)
- [ ] CREATE2-deploy announcer on PulseChain 369 after 943 E2E (FR-3.2) — codesize still `0` on `https://rpc.pulsechain.com`; ready via `RPC_URL=https://rpc.pulsechain.com PRIVATE_KEY=0x… ./scripts/deploy-erc5564-announcer.sh` (needs funded PLS key; not run from this session)
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



### DEX views — from wiz4rd-sdk (integrated into workspace)
- [x] Protocol views by capability: V2 price (`getReserves` ratio), V3 pool state (`slot0` + wiz4rd-math tick math), token metadata probes
- [ ] Deferred: write calls on other DEXes, cross-DEX routing
- [x] Piteas aggregator client scaffold (`vaughan-core::core::piteas`): quote API, optional encrypted partner key vault, `docs/piteas.md` — Ag uses public beta (no key)
- [x] Ag screen (`g`): SquirrelSwap Brain primary (no key); also PulseSwap + Piteas; catalog of other aggs (`docs/aggregator.md`)

### Custom tokens + dApp whitelist (added 2026-08-20)
- [x] Import custom ERC-20s (meme coins) into Assets (`i`); persist in vault JSON; show even at zero balance
- [x] ERC-20 send from Assets (↑↓ select, Enter → Send with token transfer)
- [x] Trusted dApp whitelist TUI (`w` / Settings): add/remove URLs, Enter opens Freedom (or system browser); origins merge into provider allowlist on launch
- [x] Freedom bridge smooth path: always allowlist `https://freedom.browser`; Web (`w`) shows bridge listen status; unlock-before-connect copy; Freedom PR #195 rebased onto main (OpenLV coexistence). Manual Connect Vaughan smoke still recommended when testing a local Freedom checkout.
- [ ] Freedom auto-connect on launch — **blocked on PR #195 merge**; primary web path is **VB** + Browserless Pulse

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
- [x] Test: dApp `eth_sendTransaction` with explicit EIP-1559 fees → on-chain maxFeePerGas/tip match; malformed `maxFeePerGas` → `-32602`, no prompt, no broadcast
- [x] Test: deny returns EIP-1193 **4001** and nothing broadcasts (nonce untouched)
- [x] Test: **locked wallet** — reads (`eth_accounts`) answer `[]`, `eth_sendTransaction` rejects with **4100** and never shows a prompt
- [x] ⚠️ Behavior fix: locked wallet no longer prompts — `execute_approval` (shared by TUI + tests) rejects early with `Unauthorized`; the app loop skips the prompt entirely (previously a locked wallet showed a prompt that would fail at execution)
- [x] `execute_approval` is now truly async (shared by the TUI's sync wrapper and async callers)
- [x] 145 tests green workspace-wide (incl. 8 anvil integration tests); no new clippy warnings
- [x] Fee editor in the dApp approval prompt (added 2026-08-26): transaction prompts (send / sign-only) now carry the Send view's speed presets — `1`–`5` Slow/Normal/Fast/Ape/Custom (↑↓ cycle, Tab focuses the custom gwei input) — with the `Fee:` line updating live. On approve, `apply_fee_override` pins the adjusted gas fields onto the tx (legacy `gasPrice` shape preserved; EIP-1559 max/tip otherwise) so what was shown is what gets signed. Custom input validates on Enter and blocks approval while invalid/focused. Anvil test: legacy-priced dApp tx + prompt override → on-chain `gasPrice` matches the override.
- [x] Persistent site grants (added 2026-08-26): `eth_requestAccounts` grants survive TUI restarts via `site-grants.json` (origins only, `0o600` beside the vault; new `core::site_grants` mirroring `ProviderSessionToken`). Explicit wallet lock still clears all grants; per-request sign/send approval is unchanged. Fixes dApps throwing 4100 after every TUI restart.
- [x] Manual validation (2026-08-26, Freedom Browser + PulseX on PulseChain testnet v4): dApp swap → TUI prompt → custom gwei override (network suggestion was ~6 tPLS vs a 4 tPLS balance) → broadcast + confirmed. Also flushed out and fixed a decision-key trap: `y`/`n` now always work while the custom gwei input is focused (approve validates first).

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
- [x] Gas speed presets: confirm lists Slow/Normal/Fast/Ape; digit keys change selection; Ape broadcast lands with scaled `maxFeePerGas` via `send_with_fee`
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

> **Embedded LLM UI/CLI retired (2026-08-23); superseded by Phase 6 MCP.** In-TUI chat,
> provider setup, mode picker, `vaughan agent`, and `vaughan policy` are removed.
> `vaughan-agent` remains as a slim library (proposals, tools, circuit breakers) consumed
> by `vaughan mcp`. Setup: `docs/mcp.md`. Historical spec: `docs/AI-AGENT-ARCHITECTURE.md`.

### Step 1: 3-Tier Operating Mode & Profile Isolation (Done)
- [x] `OperatingMode` enum: `HumanOnly`, `AiAssisted`, `SentientTrader` (FR-5.1)
- [x] Session-level immutability: mode is selected at startup/welcome screen and locked permanently for that process (FR-5.1)
- [x] Profile Directory Isolation: Sentient mode runs in isolated directory `~/.vaughan/profiles/sentient/` with separate keys/vault (FR-5.2)
- [x] Dynamic dashboard badge rendering and CLI `--profile <name>` / `--mode <human|assist|sentient>` flags
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

### Step 5: Sentient Mode Autonomous Trader & Circuit Breakers (Done)
- [x] Autonomous execution loop with isolated burner wallet signer (`SentientTrader`) (FR-5.6)
- [x] Circuit breaker: position sizing limit (max % of balance per trade) (FR-5.6)
- [x] Circuit breaker: gas burn ceiling & consecutive error tripwire (FR-5.6)
- [x] Circuit breaker: hard slippage limit (max 1.0%) (FR-5.6)
- [x] Multi-RPC quorum validation to defeat rogue/compromised or stale RPCs (FR-5.6)
- [x] Emergency stop (kill-switch via `Esc`/`q`) (FR-5.6)
- [x] Deterministic Anvil autonomous trader test suite passing (FR-5.6)

### Step 6: TUI Agent View & CLI Non-Interactive Execution (Done)
- [x] `vaughan-tui/src/views/agent.rs`: Interactive chat REPL with tool status, cold-storage human-only barrier, and confirmation cards (FR-5.8)
- [x] Welcome screen 3-way mode selector UI & dashboard operating badges (FR-5.1)
- [x] Welcome AI provider + API key setup after Assist/Sentient selection (Ollama / Gemini / OpenAI-compatible; key encrypted with vault password into `agent.key.json`)
- [x] Agent skills folder (`vaughan-agent/skills/`): mandatory rules + mode guides injected into the system prompt; user overrides via `<profile>/skills/*/SKILL.md`
- [x] `vaughan agent "<prompt>"` non-interactive CLI subcommand with sensory & proposal tool invocation (FR-5.8)
- [x] Token streaming: OpenAI/Ollama SSE via `LlmClient::stream`, Gemini falls back to `complete`; Assist chat turns (`run_assist_turn`) + TUI live deltas + Esc cancel; CLI free-form streams to stdout (FR-5.8)
- [x] Unlock → AI setup screen when Assist/Sentient lacks `agent.toml` or cloud API key
- [x] Agent status chrome: `provider/model · skills: N must`
- [x] In-chat `/model` picker (OpenCode-style UX) + `/provider` from Agent REPL; persists model to `agent.toml`
- [x] LLM I/O via `genai` multi-provider client (plug Ollama / Gemini / OpenAI-compatible)
- [x] Sentient dry-run: `VAUGHAN_SENTIENT_DRY_RUN` / `SentientTrader::with_dry_run` (validate + simulate, no broadcast)
- [x] Assist guard: refuse `propose_*` unless a sensory tool already succeeded in the same turn

### Step 7: Bomb-Proofing & Anvil Integration Tests (Done)
- [x] `vaughan-agent/tests/agent_anvil.rs`: End-to-end tests driving the agent against local Anvil node (FR-5.9)
- [x] Test: Agent correctly inspects Anvil-deployed token and executes read tools (FR-5.9)
- [x] Test: Agent proposes transfer -> Human approves -> Tx confirms on Anvil (FR-5.9)
- [x] Test: Sentient mode circuit breaker triggers and halts on excessive gas or high slippage (FR-5.9)
- [x] Extra Anvil AI-mode suite (`ai_modes_anvil.rs`): Assist sense→propose→broadcast, propose guard (refuse / failed-sensory), `propose_swap` + `search_pairs`, Sentient dry-run→live, position size, sim-revert tripwire, gas ceiling, emergency stop, multi-RPC quorum agree/diverge

### Extra local Anvil coverage (added 2026-08-19)
- [x] `vaughan-core/tests/wallet_anvil.rs`: native send + nonce, sequential sends, fee estimate, sign-then-broadcast, HD account #1 send, planted WPLS `token_balance`/`assets`, Transfer-log discovery, Slow/Normal/Fast/Ape `send_with_fee` on-chain maxFeePerGas, plain `send()` re-estimate vs stale UI fee, tip>max clamp
- [x] CLI: `vaughan assets` lists tPLS; `send <contract> --data` call (not create) receipts `0x1`
- [x] Browser engine: `scan_pair_created_logs` after a planted factory emits `PairCreated`
- [x] TUI browser: `callraw` against a planted `RETURN 0x2a` runtime
- [x] Provider: `eth_sendTransaction` with calldata to a planted contract

### TUI keys + async chrome (added 2026-08-20)
- [x] Key import/export UI (`Keys` screen): export recovery phrase / active private key / import hex key — password re-check, reveal cleared on leave, vault JSON stores optional imports (`VaultSecrets`)
- [x] Async TUI jobs: balance / assets / fee estimate / send no longer `block_on` on the UI thread (`jobs` module + worker thread); pending braille spinners on dashboard/assets/send
- [x] Chrome polish: ASCII Vaughan wordmark in the title area (pure ratatui, no new graphics crates)

## Browserless Pulse (active product thesis)

> **Pitch:** “The wallet that doesn’t need Chrome.” Approve calldata, not websites.
> Primary path = Dashboard → Ag / Dex / Contract browser / MCP. **Freedom is
> parked** until [PR #195](https://github.com/solardev-xyz/freedom-browser/pull/195)
> merges ([docs/freedom-browser-status.md](docs/freedom-browser-status.md)).
> **VB** is the active optional web side door.
>
> Exit demo (no browser window): unlock → Ag swap → contract probe → MCP propose
> → stealth receive.
>
> Related: `docs/browserless-pulse.md`, `docs/dapp-browser-strategy.md`,
> `docs/aggregator.md`, `docs/mcp.md`, Phase 4 Dex/Ag scaffolds, Phase 6 MCP,
> Phase 7 optional Chromium agent browser.

### Sentient session policy (guardrails the user owns)

> Burner/`sentient` profile only. Agent may **explain** `/policy` commands; only the
> human writes `sentient-policy.toml`. Esc emergency-stop always works.

- [x] `AgentSessionPolicy` + `sentient-policy.toml` load/save (`vaughan-agent::sentient::policy`)
- [x] Enforcement modes: `enforced` | `warn-only` | `disabled` (disabled needs `acknowledge_unsafe` / `/policy confirm-unsafe`)
- [x] Wire policy into `CircuitBreakerConfig` + session `SentientTrader` construction
- [x] Agent `/policy` show · reload · set · confirm-unsafe (hot-reload live breaker)
- [x] Skills: core-rules + sentient-trader updated (assist config; never silent disable)
- [x] Approval card: `propose_policy` tool → `[a]` apply / `[d]` deny (Sentient Agent)
- [x] CLI: `vaughan [--profile sentient] policy show|set|confirm-unsafe|reload`

### P0 — Finish in-TUI trade (kills PulseX-in-Chrome)
- [x] Ag Anvil: SquirrelSwap `/swap` fixture → mock router → native + approve/swap broadcast (`vaughan-tui/tests/ag_view.rs`)
- [x] Ag end-to-end polish: SquirrelSwap quote → route preview → ERC-20 approve (if needed) → swap confirm → broadcast (explicit approval only)
- [x] Piteas Ag venue via public `sdk.piteas.io` (`LiveNoKey`; partner key optional for higher limits — see `docs/piteas.md`)
- [x] Dex (`d`) write path for curated V2/V3 routers (Pulse-first); reuse Send approval card + fee estimate
- [x] wiz4rd V3 on Pulse testnet 943: addresses + Dex venue **Wiz4rd** + MCP `get_network.wiz4rd` (`docs/wiz4rd-addresses.md`)
- [x] wiz4rd MCP Phase B+C: `get_v3_pool`, `quote_v3_swap`, `propose_v3_swap` (`docs/pulse-defi-skills.md`)
- [x] wiz4rd MCP Phase D: `propose_v3_mint` + `list_v3_positions`
- [x] wiz4rd MCP Phase E: `propose_v3_increase` / `_decrease` / `_collect`
- [x] MCP wrap / unwrap / revoke / approve / list_allowances
- [x] MCP bridge: `quote_bridge` / `propose_bridge` (LibertySwap)
- [x] MCP history + token: `list_transfers`, `resolve_token`, `import_token`
- [x] MCP stealth send propose (`propose_stealth_send`); scan/sweep MCP tools shipped
- [x] MCP `watch_balance` for sentient threshold loops
- [x] Token discovery without a site: `resolve_token` + `import_token` (TUI paste polish still optional)

### P1 — Agent is the URL bar
- [x] EmpX / EmpSeal Alloy client (on-chain path-find + swap calldata; PulseChain 369)
- [x] Agent tools: `quote_swap` / `propose_agg_swap` (+ existing `propose_swap`) with ground-truth approval cards (Assist never auto-broadcasts)
- [x] Intent macros: `/swap …`, `/inspect 0x…`, `/revoke …`, `/stealth receive` in Browser REPL → existing surfaces (`vaughan-tui/src/intent.rs`)
- [x] Pulse DeFi skill pack aligned with Ag/Dex (inspect / quote / route / trade) — MCP tools `quote_swap` + `propose_agg_swap`
- [x] **DeFi agent parity** — must-have verbs in [`docs/defi-agent-parity.md`](docs/defi-agent-parity.md); EmpX + 7702 AA exec + stealth scan/sweep MCP shipped
- [x] **MCP sentient mode** — `vaughan-sentient` / `--profile sentient` auto-exec when TUI unlocked (re-sim + policy; no approval card); `default` / `vaughan` stays **adviser**. See [`docs/agent-roles.md`](docs/agent-roles.md)
- [x] **Sentient skill presets** — premade packs (`high-risk-gambler`, `balanced`, `quant-risk-reward`, `cautious`) + docs; human copies into profile / customizes ([`docs/sentient-presets.md`](docs/sentient-presets.md), `vaughan-agent/presets/`)
- [x] **`vaughan preset apply <id>`** — copy a bundled preset into the active profile (skills + `sentient-policy.toml`)
- [x] **`vaughan serve`** — headless unlock + MCP control plane (`--password-env`); see `docs/mcp.md`

### P2 — Replace explorer & settings tabs people open constantly
- [x] Activity / History screen (`m`): ERC-20 Transfer logs (sent/received) over recent window; reload; native-only without token log still needs explorer later
- [x] Anvil: ERC-20 approve → revoke (`approve(spender,0)`) clears allowance (`browserless_anvil.rs` + `build_revoke_tx`)
- [x] Approvals manager (`j`): list ERC-20 allowances vs known Ag/Dex/Bridge spenders; one-shot revoke via confirm card
- [x] Anvil: WPLS wrap (`deposit`) / unwrap (`withdraw`) against MockWeth (`browserless_anvil.rs` + `build_wrap_tx` / `build_unwrap_tx`)
- [x] Bridge (`f`): LibertySwap USDC cross-chain quote → approve → source broadcast (`docs/bridge.md`; not official Omnibridge)
- [ ] Official Pulse Omnibridge / PulseRamp client (lock-and-mint ETH ↔ Pulse) — **deferred** (polish first; LibertySwap remains Bridge)
- [x] Wrap / unwrap WPLS as a first-class tiny flow (`e`)
- [x] Contract browser gated writes: `write` / `writeraw` from REPL → fee confirm → broadcast (same path as Send)

### P3 — Earn / advanced (only when real)
- [ ] LP positions (V2 balance + remove liquidity); V3 only if demand is proven
- [x] Bridge (`f`): LibertySwap convenience wrapper (source broadcast; dest async) — see above
- [ ] Official Omnibridge UI *or* keep documenting “use LibertySwap Bridge / Ag / Dex for in-chain” — keep documenting Liberty until Omnibridge is un-deferred
- [x] Local EIP-712: paste/load typed-data JSON → Approve view — `vaughan sign-typed-data`, browser `sign-typed`
- [x] Watch mode (MCP): `watch_balance` + `watch_quote` threshold snapshots; agent owns poll loop
- [x] Sentient always-on: `vaughan serve` + example systemd unit + `get_control_plane_status` ([`docs/sentient-ops.md`](docs/sentient-ops.md))
- [x] Batch7702 fee-spike: stamp via `estimate_self_pay_fee`, check at approve (same as other MCP writes)

### Positioning (UX + docs)
- [x] Demote Dapps screen: relabel to “Optional web / VB” (`w` Web); chrome emphasizes Ag / Dex / Browse / MCP
- [x] README + CONTRIBUTING blurb: browserless Pulse thesis; Freedom parked pending PR #195 (`docs/freedom-browser-status.md`)
- [x] One recorded demo reel matching the exit demo above (no Chrome/Freedom in frame) — operator script: [`docs/browserless-pulse-demo.md`](docs/browserless-pulse-demo.md)

### Explicitly out of scope for this thesis
- Making a webview the **default** wallet identity (Browserless Pulse stays primary)
- Open-internet general browsing / Chrome replacement / extensions
- WalletConnect-as-default identity (keeps users married to websites)
- 1:1 clones of every Pulse website — prefer verbs: swap, inspect, revoke, send, stealth
- Hosted multi-tenant / cloud fire-and-forget signing (see [`docs/sentient-ops.md`](docs/sentient-ops.md))
- Full `rmcp` MCP rewrite — **not needed now**; deferred until revisit triggers in [`docs/mcp-transport.md`](docs/mcp-transport.md) (prefer smallest hand-rolled fix first)
- CEF / Chromium linked into `vaughan-core` (optional browser is a separate binary — see Phase 7)

## Phase 7 — Optional Chromium dApp browser + agent control

> Strategy: [`docs/dapp-browser-strategy.md`](docs/dapp-browser-strategy.md).
> Modular Tauri+CEF side door; multi-chain EVM; CDP agents; never auto-sign.
> **VB** (system Chromium today) is the active web side door; Freedom **parked**
> until [PR #195](https://github.com/solardev-xyz/freedom-browser/pull/195) merges.

### Phase 0 — Docs
- [x] `docs/dapp-browser-strategy.md` (CEF, modularity, multi-chain, CDP, kill-switch)
- [x] `docs/browserless-pulse.md` + REQUIREMENTS FR-7.* + this TASKS section

### Phase 0.5 — CEF spike (gate)
- [x] crates.io survey: `cef` published; `tauri-runtime-cef` not on crates.io (git/wrymium required)
- [x] Localhost CDP interactive refs smoke (`docs/spikes/cef-tauri` `cdp_ax_smoke`) — agent path proven
- [x] Confirm default `cargo build -p vaughan-cli` still does not fetch CEF
- [ ] Linux: Tauri + `tauri-runtime-cef` from git (or wrymium) loads allowlisted HTTPS — **moved to Phase 1** (runtime git-only)

### Phase 1 — Modular shell + wallet (FR-7.1–7.4)
- [x] Workspace member `vaughan-dapp-browser` (Chromium/CDP deps only here — CEF embed still deferred)
- [x] Soft-launch from `w` when binary on PATH/config; Freedom fallback when missing
- [x] Host allowlist + EIP-1193 page inject → `vaughan-provider` → TUI approve
- [x] CDP export only with `--cdp-port` / `VAUGHAN_DAPP_BROWSER_CDP_PORT` (default off)
- [x] Anvil smoke: dApp page Origin + extension Origin → connect + send + multi-chain switch (`vaughan-tui/tests/dapp_browser_bridge_smoke.rs`)
- [x] Manual smoke: Pulse / 9inch / Squirrel / Liberty headed Chromium (CSP-safe extension; PulseX via IPFS mirror)
- [x] Navigation allowlist enforcement after first load — extension MV3 + MCP `browser_navigate` checks `vb.session` suffixes
- [ ] Tauri + CEF embed (replace system Chromium) — still gated on git `tauri-runtime-cef`

### Phase 2 — MCP B1 (FR-7.5)

- [x] `browser_open` / `browser_navigate` / `browser_status` (allowlisted; CDP via `vb.session`)
- [x] Structured unavailable when binary/child absent; document in `docs/mcp.md`

### Phase 3 — MCP B2 (FR-7.5)
- [x] `browser_snapshot` / `browser_click` / `browser_type` / `browser_press` / `browser_wait` via CDP evaluate + Input (refs e0..e49)
- [x] Settings toggle default off; never auto-sign; kill-switch path documented (FR-7.6) — [`docs/vb-kill-switch.md`](docs/vb-kill-switch.md); Settings **`p`**; `vaughan config agent-browser`

### Parked — Fable 5 comprehensive audit (before release tag)

> Prompt ready: [`docs/fable-5-audit-prompt.md`](docs/fable-5-audit-prompt.md). Run after Phase 7
> browser MCP tools stabilize or before mainnet-facing release. Not a CI gate.

### Parked — Blunt.cash payments (Browserless Pulse)

> **Parked** 2026-08-27. Full plan: [`docs/blunt-integration-plan.md`](docs/blunt-integration-plan.md).
> Prototype: `/home/r4/Desktop/Blunt-vaughan`. Official API:
> [blunt.cash/merchant/docs/reference/api](https://blunt.cash/merchant/docs/reference/api).
> **v1 = direct wallet pay only; no VaughanPaymentRouter / protocol fees.**
> Resume when Blunt merchant API key is obtained (Phase 0 below).

#### Phase 0 — Prerequisites (human, before code)

- [ ] Sign up at [blunt.cash/merchant/auth](https://blunt.cash/merchant/auth); save Secret Key + PIN
- [ ] Dashboard → **Wallets** → register PulseChain (`pls`) payout address
- [ ] Dashboard → **API Keys** → copy secret key (never commit)
- [ ] Smoke-test `merchant-create-payment` + `get-payment` via curl
- [ ] Record redacted sample JSON in `docs/blunt.md` (create when resuming)

#### Phase 1 — `vaughan-core::core::blunt`

- [ ] `client.rs` — official endpoints (`blunt.cash/functions/v1/*`), not Desktop `api.blunt.cash/v1`
- [ ] `types.rs` + `chain_map.rs` (`pls` ↔ pulsechain mainnet; PLS native only)
- [ ] `config.rs` — encrypted API key (Piteas pattern); env `BLUNT_API_KEY`
- [ ] `resolve_payment_for_pay()` helper for existing send path
- [ ] Mock HTTP unit tests (CI without live key)

#### Phase 2 — Pay flow

- [ ] Orchestrator: get-payment → validate pending → direct native transfer → poll confirmed
- [ ] Standard TUI approve gate / CLI confirm (no auto-sign)

#### Phase 3 — CLI

- [ ] `vaughan blunt configure | invoice | pay | status` with `--json`

#### Phase 4 — TUI

- [ ] Send sub-mode: pay invoice by `payment_id`
- [ ] Receive sub-mode: create invoice (merchant)
- [ ] Settings: Blunt API key setup

#### Phase 5 — Docs + requirements

- [ ] `docs/blunt.md` (operator guide)
- [ ] FR-8.* in REQUIREMENTS.md
- [ ] Browserless Pulse demo step in `docs/browserless-pulse.md`

#### Explicitly deferred (post-v1)

- [ ] `VaughanPaymentRouter.sol` deploy + fee split
- [ ] MCP `blunt_create_invoice` / `blunt_pay_invoice`
- [ ] Pay-with-any-token (DEX swap before pay)
- [ ] ASCII QR terminal (`qrcode` crate — not on allowlist yet)

## Later — DeFi AI king / Coinbase compete (deferred)

> Narrative + phased roadmap: local notes under `private/` (gitignored).
> Do **not** start until Pulse quote/swap is demo-stable or we explicitly want MCP.
>
> Session policy foundation shipped under **Browserless Pulse → Sentient session policy**.

- [x] P0: `AgentSessionPolicy` + wire Sentient breakers + Agent `/policy` (see Browserless Pulse)
- [x] P1: Vaughan MCP server for Claude/Codex/Gemini (no key exposure; Assist approve / Sentient under policy) — `vaughan mcp`, hybrid IPC, `docs/mcp.md`
- [x] P2: Pulse DeFi skill pack (inspect / quote / route / trade; Earn only when real) — `quote_swap`, `propose_agg_swap`, `propose_swap`
- [ ] P3: x402 client (opportunistic — only with real counterparties)
- [ ] P4: gas tank, optional hardware signer, local deny lists (never hosted TEE / KYT telemetry)
  - Plan: [`docs/hardware-wallets.md`](docs/hardware-wallets.md) (Ledger + Trezor; Phase 0 abstraction first, no HID crates until approved)
  - [x] HW readiness check (2026-08-25): Phase 0 = Go-with-fixes; see readiness section in plan doc
  - [x] HW Phase 0: modular `security/hardware/` + family-agnostic `SignerBackend` + EVM profile + vault `hardware[]` (no new deps; multichain-ready seams)
  - [x] HW Phase 1: Ledger EOA (`alloy-signer-ledger`, Keys Add Ledger, mock Anvil); live 943 device smoke still optional
  - [ ] HW Phase 2: Trezor EOA parity
  - [ ] HW Phase 3: hardening (re-verify, blind-sign policy); AA/stealth on HW stay out of scope

## Later — non-EVM families (deferred, no FR yet)

- [ ] `chains/bitcoin/` adapter (UTXO model, coin selection, `bdk`)
- [ ] `chains/polkadot/` adapter (Substrate, SS58 addresses, weight-based fees, `subxt`)

- [ ] Family-aware derivation schemes (BIP-44 per coin type; Substrate `//`-path secret URIs)

## Robustness (fund-safety + ops, not new product)

> Order: CI/Anvil → vault durability → post-broadcast receipt → RPC clarity →
> stealth mainnet. Omnibridge / LP / non-EVM stay deferred.

- [x] CI installs Foundry/Anvil so bomb-proof suites actually run (green ≠ skipped)
- [x] Vault durability: keep `wallet.json.bak` of last good write; load falls back on corrupt primary
- [x] Post-broadcast: poll receipt after Send Done (Pending / Confirmed / Failed); `r` re-check
- [x] RPC user messages: distinguish “all endpoints failed” / fallback exhausted
- [x] Dex builders: no `unwrap` after validate (map parse errors to `Result`)
- [x] Session recent-broadcast list on History; cancel / speed-up pending (EIP-1559); live smoke cron
- [ ] Stealth announcer CREATE2 on Pulse 369 (ops; needs funded PLS — see Phase 3)
