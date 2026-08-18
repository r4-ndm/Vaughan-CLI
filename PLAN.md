# Vaughan-CLI — Plan

## Vision

Vaughan-CLI is a Rust multi-chain CLI wallet TUI:

- **Alloy** — wallet core: keys, signing, RPC, transaction building/broadcast.
- **kohaku-rs** — privacy (stealth, later railgun), consumed as a git dep; **deferred
  by decision** (see `docs/kohaku-go-no-go.md`).
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
| Ethereum lib | alloy 2.x (resolved 2.4.1) | Single workspace version; upgraded from 1.7 in 2026-08 (Vaughan is early-stage, so the Vaughan-Dioxus pin no longer applies) |
| TUI | ratatui + crossterm | De-facto standard, async/tokio friendly |
| Core layering | reimplement | Clean control; mirror Vaughan-Dioxus, don't vendor |
| HD wallet | bip39 + bip32 | RustCrypto stack, actively maintained; bip39 for mnemonics, bip32 for derivation |
| Vault crypto | Argon2id + AES-256-GCM | Argon2id KDF, authenticated encryption |
| kohaku-rs | git dep, later phases | **Deferred** — upstream RAILGUN key derivation is incompatible with the canonical engine (see `docs/kohaku-go-no-go.md`) |
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
Ambire AA in Rust (see `docs/ambire-aa.md`) is **done**: `vaughan-aa` verified
byte-for-byte against EVM-reference fixtures, 7702 self-pay proven end-to-end on a
forked testnet, and the TUI batched-send view ships it (ERC-4337 `UserOperation`
stays deferred by decision — self-pay 7702 is the route).

kohaku-rs (hardening, ERC-5564 stealth, railgun/privacy pools) is **deferred by
decision**: upstream's RAILGUN key derivation (BIP-32) is incompatible with the
canonical engine's babyjubjub seed tree, so keys could be unrecoverable. Revisit
only when upstream fixes it (with proof vectors) and tags a release, or when a
spec-first ERC-5564 implementation is scoped. See `docs/kohaku-go-no-go.md`.

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
- **kohaku-rs maturity** — **deferred by decision**: upstream's RAILGUN key
  derivation is incompatible with the canonical engine (unrecoverable-funds risk)
  and upstream is unaudited; not hardened or consumed until resolved (see
  `docs/kohaku-go-no-go.md`).
- **PulseChain RPC availability** — public endpoints; may need fallback URLs.
- **Alloy version** — on 2.x since 2026-08 (early-stage project, so the 1.7
  Vaughan-Dioxus pin was dropped). Future major upgrades are isolated to a
  single workspace pin (`Cargo.toml`).
