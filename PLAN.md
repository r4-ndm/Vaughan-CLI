# Vaughan-CLI — Plan

## Vision

Vaughan-CLI is a Rust multi-chain CLI wallet TUI:

- **Alloy** — wallet core: keys, signing, RPC, transaction building/broadcast.
- **kohaku-rs** — privacy/provider (stealth, later railgun), consumed as a git dep.
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
│  ├─ core/              # WalletState, AccountManager, NetworkService, StateManager,
│  │                     #   TransactionService
│  ├─ security/          # hd_wallet (BIP-39/32/44), encryption (Argon2id + AES-256-GCM)
│  └─ error.rs, logging.rs
├─ vaughan-tui/          # ratatui binary
│  └─ views/             # onboarding, unlock, dashboard, send, receive, settings
└─ vaughan-provider/     # [Phase 2] local EIP-1193 bridge + approval UX + trusted hosts
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
| Language | Rust (2021) | Alloy + ratatui + kohaku-rs are Rust; matches Vaughan-Dioxus |
| Ethereum lib | alloy 1.7 | Same pin as Vaughan-Dioxus; proven API for our EVM adapter |
| TUI | ratatui + crossterm | De-facto standard, async/tokio friendly |
| Core layering | reimplement | Clean control; mirror Vaughan-Dioxus, don't vendor |
| HD wallet | bip39 + bip32 | RustCrypto stack, actively maintained; bip39 for mnemonics, bip32 for derivation |
| Vault crypto | Argon2id + AES-256-GCM | Argon2id KDF, authenticated encryption |
| kohaku-rs | git dep, later phases | Own repo; hardened in Phase 3 |
| Ambire AA | Rust + Alloy + Ambire ABI | Reimplement from the on-chain `AmbireAccount` contract; Vaughan-Dioxus as guide only |
| dApp bridge | local EIP-1193 JSON-RPC (WebSocket) | Mirrors Vaughan-Dioxus provider-style RPC; loopback only |

## Phases

### Phase 1 — EOA wallet on PulseChain
Create/restore (BIP-39), password-encrypted vault, HD `m/44'/60'/0'/0/0`, balance,
send native PLS, receive, network switching. No bridge, no tokens.

### Phase 2 — Native provider bridge
`vaughan-provider` local EIP-1193 server + TUI approval flow + trusted hosts, and a
Freedom Browser signer-backend PR (out-of-repo, MPL-2.0).

### Phase 3 — Privacy + smart accounts
Harden kohaku-rs (tests + publish), wire ERC-5564 stealth, then Ambire AA in Rust
(see `docs/ambire-aa.md`), then railgun/privacy pools when upstream stabilizes.

### Phase 4 — Contract browser (terminal DEX browsing)
Generic browser engine in `vaughan-core` (pure Rust, alloy-native: explorer-ABI
fetch + cache, selector probing, dyn-abi generic calls, event-scan pair/pool
discovery — browses/calls *any* contract, not just DEXes), surfaced as an
interactive REPL view in `vaughan-tui` (stateful context, history/completion,
batch mode). DEX-specific views (V2 reserve price, V3 `slot0` + tick math) come
from `wiz4rd-sdk` when it joins the workspace. Read-only on other DEXes in v0.1.
Full scope: `wiz4rd-swap/docs/other-dexes-scope.md` (rev 5).

## Security model

- Mnemonic encrypted at rest (Argon2id -> AES-256-GCM); plaintext only in memory while unlocked.
- Secrets zeroized; password policy enforced (>= 12 chars, mixed classes).
- Signing requires explicit user approval; dApp origins gated by a trusted-host allowlist.
- No telemetry/analytics. Testnet-first for fund-moving features.

## Build order

1. `vaughan-core`: error + logging
2. `vaughan-core`: chains (types, networks, Alloy EVM adapter)
3. `vaughan-core`: security (hd_wallet, encryption)
4. `vaughan-core`: services (accounts, persistence, wallet state, transaction)
5. `vaughan-tui`: onboarding -> unlock -> dashboard -> send -> receive -> settings
6. `cargo build` + unit tests + `clippy` + `fmt`

## Risks / open items

- **Freedom Browser bridge** — transport requires a new signer backend + local socket
  (confirmed by inspecting its `signers.js`/injection flow).
- **kohaku-rs maturity** — untested scaffolding; isolated to Phase 3, hardened before use.
- **PulseChain RPC availability** — public endpoints; may need fallback URLs.
- **Alloy 1.7 vs 2.x** — pinned to 1.7 for API stability; revisit on a later upgrade pass.
