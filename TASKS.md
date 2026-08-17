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
- [ ] TUI approval flow: approve/deny prompts for sign/send (FR-2.3)
- [ ] Trusted-host allowlist (borrow `vaughan-trusted-hosts`) (FR-2.4)
- [x] Account/chain change event push to clients (`EventBus` → JSON-RPC notifications) (FR-2.2)
- [ ] Freedom Browser signer backend PR (out-of-repo) (FR-2.5)

## Phase 3 — Privacy + smart accounts

- [ ] Harden kohaku-rs: add stealth test vectors + `kohaku-core` tests (FR-3.1)
- [ ] Fix kohaku-rs railgun build (git dep / submodule instead of sync script) (FR-3.1)
- [ ] Publish `kohaku-core` + `kohaku-stealth` to crates.io (FR-3.1)
- [ ] Wire ERC-5564 stealth addresses into Vaughan (FR-3.2)
- [ ] Ambire smart accounts in Rust (borrow `ambire_abi`/`scw_transaction`) (FR-3.3)
- [ ] Railgun / privacy pools (FR-3.4)

## Later — non-EVM families (deferred, no FR yet)

- [ ] `chains/bitcoin/` adapter (UTXO model, coin selection, `bdk`)
- [ ] `chains/polkadot/` adapter (Substrate, SS58 addresses, weight-based fees, `subxt`)
- [ ] Family-aware derivation schemes (BIP-44 per coin type; Substrate `//`-path secret URIs)
