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

- [ ] Harden kohaku-rs: add stealth test vectors + `kohaku-core` tests (FR-3.1)
- [ ] Fix kohaku-rs railgun build (git dep / submodule instead of sync script) (FR-3.1)
- [ ] Publish `kohaku-core` + `kohaku-stealth` to crates.io (FR-3.1)
- [ ] Wire ERC-5564 stealth addresses into Vaughan (FR-3.2)
- [ ] Ambire smart accounts in Rust — see `docs/ambire-aa.md` (FR-3.3)
  - [x] Create the `vaughan-aa` workspace crate and document the AGPL-3.0/GPL → MIT/Apache reimplementation boundary
  - [x] Define the smart-account ABI (`sol!`) + `scw_transaction`/`SignatureMode` types from the on-chain `AmbireAccount` contract (Vaughan-Dioxus as guide only)
  - [x] Digest = `keccak256(abi.encode(account, chainId, nonce, txns))`; sign raw hash (`sign_hash`, mode `0`) or EIP-191 (`personal_sign`, mode `1`), append the mode byte. Core gained `security::signing::sign_hash`; the digest is verified byte-for-byte against a hand-built ABI-spec vector.
  - [x] Encode the inner `Transaction[]` batch calldata (`execute` selector + `abi.encode`, round-trip tested). *(Fixture byte-equality is covered by the differential harness below — still pending fixtures.)*
  - [x] Sign a `scw_transaction` and recover/verify the 66-byte `r‖s‖v‖mode` signature (raw-hash + EIP-191)
  - [x] EIP-7702 assembly (`build.rs`): sign the `Authorization` delegating the account EOA to the Ambire implementation and build the `TxEip7702` carrying `execute(txns, signature)` (self-pay, testnet-first). Authority/chain-id are validated against the batch before assembling.
  - [ ] ERC-4337 `UserOperation` / `getUserOpHash` assembly (`build.rs`) — needs an EntryPoint/bundler decision, deferred
  - [x] Broadcast via `EvmAdapter` (`adapter.rs`): the **self-pay** path is wired — fetch the account's *pending* nonce (uncached), derive EIP-1559 fees through the adapter's existing heuristic (pinned gas limit, since `eth_estimateGas` can't price a pre-delegation 7702 call), sign the 7702 envelope (auth nonce = account nonce + 1 per EIP-7702's "after the sender's nonce is incremented"), and submit via the adapter's primary + fallback broadcast. Relayer / bundler routes still TBD.
  - [ ] Differential test harness: `tests/differential.rs` scaffolded (consumes `tests/fixtures/*.json`, skips when none); fixtures must be captured outside this workspace — see `tests/fixtures/README.md`
  - [ ] (Later) TUI integration: AA account type + batched send UX
- [ ] Railgun / privacy pools (FR-3.4)

## Phase 4 — Contract browser (terminal DEX browsing)

> Full scope: `wiz4rd-swap/docs/other-dexes-scope.md` (rev 5). **Pure Rust, no cast subprocess** — alloy is the battle-tested core (it's the library Foundry itself is built on). Engine is generic wallet tooling (browses/calls *any* contract, not just DEXes). Read-only on other DEXes in v0.1; money-moving flows only through Vaughan's existing alloy signer layer. No FRs yet — scoped cross-repo, tracked here as the implementation home.

### vaughan-core — browser engine (generic, no DEX knowledge)
- [ ] ABI resolution: explorer `getabi` fetch (verified working on `api.scan.pulsechain.com`) + local cache; probe fallback for unverified contracts
- [ ] Selector-probe capability library (ERC20 / V2 factory+pair / V3 factory+pool / WETH / …) → protocol fingerprint
- [ ] Selector extraction from bytecode: PUSH4 opcode parse over `getCode` (~30 lines, unit-tested)
- [ ] Signature lookup: 4byte.directory HTTP API (reqwest)
- [ ] Generic call: alloy dyn-abi encode/decode (read-only) — `alloy-dyn-abi` already a core dep
- [ ] Event-scan discovery: `pairs <factory>` via `PairCreated`/`PoolCreated` logs (no init hashes needed)
- [ ] Unit tests: probe fingerprints against known contracts (PulseX factory `0x29eA…C523`, Multicall3 `0xcA11…`), PUSH4 parser, ABI cache

### vaughan-tui — browser REPL view
- [ ] `views/browser.rs`: input line + scrolling output pane, reusing existing `input.rs` (ratatui + crossterm)
- [ ] Stateful context: `browse 0x…` sets current contract; `pairs`, `call …`, `info`, `probe` operate on it
- [ ] History + tab completion + `help` (small glue; e.g. `tui-input`)
- [ ] Batch mode for scripting: `vaughan browser -c "cmd"` (non-interactive, same engine)

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

## Later — non-EVM families (deferred, no FR yet)

- [ ] `chains/bitcoin/` adapter (UTXO model, coin selection, `bdk`)
- [ ] `chains/polkadot/` adapter (Substrate, SS58 addresses, weight-based fees, `subxt`)
- [ ] Family-aware derivation schemes (BIP-44 per coin type; Substrate `//`-path secret URIs)
