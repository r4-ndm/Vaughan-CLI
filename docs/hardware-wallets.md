# Hardware wallets — Ledger & Trezor plan

**Status:** Phase 0 in progress / landed (abstraction, no HID). Readiness accepted 2026-08-25.  
**Goal:** Optional hardware signer for EOAs on Pulse/EVM, same approval UX as software.

Hardware is **deferred product** (TASKS P4) until Phase 1 lands device backends. Phase 0 seams live under `vaughan-core::security::hardware`.

---

## Readiness check (2026-08-25)

**Phase 0 verdict: Go-with-fixes.** Abstraction can start with **no** HID crates. Software EOA signing is fully `PrivateKeySigner`-shaped; Phase 0 must re-route those call sites before any device code. Deps/allowlist only gate Phase 1+.

**Deps snapshot:** Workspace `alloy` = `signers` + `signer-local` (no `signer-ledger`). Root `Cargo.toml` avoids Alloy’s top-level `eip712` because it pulls optional cloud/ledger crates (`alloy-dyn-abi` carries EIP-712). `Cargo.lock` has **no** `ledger` / `trezor` / `hidapi`.

### A. Production sign / broadcast inventory

| Path | Role | Files |
|---|---|---|
| **EOA funnel (all TUI fund-moves)** | Unlock → `active_signer()` → `EvmAdapter::with_signer` → `build_signed_envelope` (`EthereumWallet::from(PrivateKeySigner)`) → `broadcast_raw` | `wallet.rs` (`broadcast`, `send*`, `replace_broadcast`, `signed_adapter_and_tx`); `chains/evm/adapter.rs`; `core/transaction.rs`; `core/account.rs` |
| **TUI callers** | Fee confirm → funnel | `vaughan-tui` `app.rs` jobs; views `send`, `dex`, `bridge`, `ag`, `approvals`, `browser`, `history` |
| **Off-chain EIP-191 / EIP-712** | Sync `SignerSync` on local key | `wallet.rs` `sign_message` / `sign_typed_data`; `security/signing.rs` |
| **Provider / Freedom / MCP approve** | Human approve → wallet APIs (+ AA / stealth branches) | `vaughan-provider/src/methods.rs`; `vaughan-tui/src/provider.rs` `execute_approval` |
| **CLI send** | Prints request → `send_transaction` | `vaughan-cli/src/main.rs` |
| **AA / 7702** | `active_signer` + `sign_hash_sync` / SCW envelope | `aa_send` view; `vaughan-aa/{sign,adapter,build}.rs`; provider `Batch7702` |
| **Stealth** | Pay via EOA funnel; sweep via ephemeral local signer from mnemonic meta-keys | `core/stealth.rs`; `security/stealth.rs`; MCP `sweep_stealth_note` |
| **Keys export / import** | Assumes seed or hex exists | `account.rs` / `wallet.rs` export; `vault_secrets.rs`; Keys view |
| **DegenTrader** | Isolated burner `PrivateKeySigner` (not main vault) | `vaughan-agent/src/degen/trader.rs` |
| **MCP tools** | Propose / read only — no HID, no direct sign | `vaughan-mcp/`, agent `propose_*` |

**Account model today:** HD (`0..N`) + imported (`IMPORTED_INDEX_BASE = 1_000_000`). `Account.is_imported` only — no `AccountKind`, no hardware records. `VaultSecrets` = `{ mnemonic, imported[] }` only.

### B. Phase 0 — must change vs leave alone

**Must change (still no HID):**

1. `security/hardware/` module skeleton + `AccountKind` + `HardwareAccountRecord` (`family`, opaque path, optional `network_id`); vault `hardware: []`; index base `2_000_000`.
2. Family-agnostic `SignerBackend` + `SignRequest`/`SignResult` + `LocalSignerBackend` (EVM variants only in v1); thin EVM helpers OK.
3. Stop treating `active_signer() -> PrivateKeySigner` as the universal EOA API; fail closed for HW.
4. `EvmAdapter`: today `signer: Option<PrivateKeySigner>` + `EthereumWallet::from` — long-term seam is **unsigned build → backend sign → `broadcast_raw`** (device I/O never inside the adapter).
5. `DeviceSession` trait stub (no impl) so Ledger/Trezor later share one USB-facing contract without EVM types.
6. `security::signing::*` stay local helpers; backends wrap them. Async trait surface (Ledger later is async-only).
7. Export / stealth meta-keys / AA: refuse HW with `WalletError::HardwareUnsupported` even before devices exist.
8. Tests: vault roundtrip `hardware: []`; Anvil local paths green; TASKS “HW Phase 0”.

**Leave alone in Phase 0:** fee/RPC/caches; MCP propose registry; DegenTrader; Ambire digest math; stealth ERC-5564 crypto; Alloy features / HID; TUI “Confirm on device…” chrome; Freedom upstream; Bitcoin/Polkadot profile bodies.

### C. Phase 0 exit checklist

- [ ] Modular `security/hardware/` + serde (`hardware: []`, `family: Evm`) without breaking unlock of legacy vaults
- [ ] Local backend parity for EVM personal / typed / EIP-1559 sign+broadcast via `SignRequest`
- [ ] Core trait has **no** EVM-only method names (helpers may wrap)
- [ ] Export + AA + stealth + Degen/HW policy guards stubbed
- [ ] `cargo test --workspace` / Anvil suites green
- [ ] No new allowlisted crates

### D. Phase 1 Ledger blockers

1. Allowlist approval for Alloy Ledger + HID (not on `CLAUDE.md` today).
2. Must not re-open broken top-level Alloy `eip712` → optional ledger/cloud resolve path.
3. `LedgerSigner` is **async-only** — sync `SignerSync` / `sign_hash_sync` (AA, `signing.rs`) will not map; sign via async backend then `broadcast_raw`.
4. Choke points: `WalletState::active_signer()`, provider AA branch, anything cloning a local key into the adapter.
5. Implement `DeviceSession` for Ledger **without** embedding EIP-1559 builders in the transport module.
6. Device UX + Linux udev/HID not started.
7. CI: mock `DeviceSession` + optional `#[ignore]` live device; 943 smoke for native send + `personal_sign`.

### E. Phase 2 Trezor blockers

- No first-class Alloy Trezor twin; separate client (protobuf, heavier deps).
- License review (AGPL/GPL risk) before allowlist.
- Passphrase = extra secret — never log; zeroize discipline.
- Shared `SignerBackend` + vendor-parameterized TUI; 943 parity with Ledger.

### F. Out-of-scope (confirmed)

| Surface | Phase 0–2 stance |
|---|---|
| AA / EIP-7702 Ambire | Software EOAs only; HW → refuse |
| Stealth (ERC-5564) | HD vault only; HW → refuse |
| MCP / agents | Never talk to HID |
| DegenTrader | No HW auto-sign; local burner only |
| Bluetooth / Speculos-as-prod / multisig-as-HW / hosted TEE | Still out |

### G. Top 5 risks before any HW code

1. Sync-everywhere (`SignerSync`, `active_signer() -> PrivateKeySigner`) vs Ledger async-only — half-migrate = footguns.
2. Baking EVM into `SignerBackend` method names / `EvmAdapter` owning HID — blocks Bitcoin/Polkadot and couples vendor to fees/RPC.
3. `EthereumWallet` + local signer in `EvmAdapter` — central coupling; redesign to signed-bytes + `broadcast_raw` before HID.
4. Export / Keys UI assumes exportable material — must fail closed for HW.
5. Vendor SDK license / telemetry / Linux HID — block Phase 1–2 deps until reviewed.

**Suggested next step after acceptance:** Phase 0 PR only (modular skeleton + local EVM profile). Then dependency-approval thread for Ledger before any HID.

---

## Why not yet (summary)

Today every sign path assumes an in-process [`PrivateKeySigner`](../vaughan-core/src/security/):

| Surface | Coupling |
|---|---|
| `AccountManager::signer` / `active_signer` | Always returns `PrivateKeySigner` |
| `EvmAdapter::with_signer` | Stores `Option<PrivateKeySigner>` |
| EIP-191 / EIP-712 | `security::signing::*` take `&PrivateKeySigner` |
| AA / stealth | Compose with local signers |
| Keys export | Assumes seed or hex key exists |

Vault secrets = mnemonic + imported hex keys only. No watch/hardware records.

Allowlist (`CLAUDE.md`) has no Ledger/Trezor transport crates — **explicit approval required** before adding them.

---

## Principles (non‑negotiable)

1. **No auto-sign.** Device confirm is *in addition to* TUI / provider approve — never instead of.
2. **Keys never leave the device.** Vaughan stores address + vendor + derivation metadata only (no secrets).
3. **Software vault stays default.** Hardware is opt-in per account (F3), not a second app mode.
4. **MCP / agents never talk to HID.** Same as today: propose → human approve → sign in Vaughan.
5. **Export refuses hardware.** No “export private key” / mnemonic for device accounts.
6. **Stealth / AA:** v1 = software accounts only. Document as out-of-scope until EOA HW is stable.
7. **Testnet-first** for any fund-moving HW flow (943 before 369).
8. **Modular + multichain-ready.** Hardware stacks like `ChainAdapter`: vendor transport ≠ chain-family signing. EVM is the first *profile*; Bitcoin/Polkadot must plug in without rewriting Ledger/Trezor HID or the TUI shell. No EVM-only types on the core backend trait.

---

## Target architecture

Align with [`PLAN.md`](../PLAN.md) multi-chain layering: UI/services never talk to HID or match on vendor/family internals.

```
  TUI / Provider / WalletState
            │
            ▼
     SignerBackend          ← family-agnostic async surface
            │                 SignRequest / SignResult (tagged)
            │
   ┌────────┼────────────────────────────┐
   ▼        ▼                            ▼
 Local   DeviceSession                 (future)
         (vendor-agnostic              other
          open / path /                backends
          sign bytes)
            │
   ┌────────┴────────┐
   ▼                 ▼
 LedgerTransport  TrezorTransport   ← HID only; no chain_id / EIP knowledge
   │                 │
   └────────┬────────┘
            ▼
   FamilySignProfile                ← one module per ChainType
   ├─ EvmHwProfile (v1)               personal / typed / EIP-1559
   ├─ BitcoinHwProfile (later)        PSBT / BIP-322 …
   └─ PolkadotHwProfile (later)       extrinsic / sr25519 …
```

### Module layout (Phase 0 skeleton; HID impls later)

Keep hardware **one concern** under `vaughan-core`, parallel to `chains/{family}/`:

```
vaughan-core/src/security/hardware/
  mod.rs              // re-exports; no vendor crates
  types.rs            // HardwareVendor, HardwareAccountRecord, SignRequest/Result
  backend.rs          // SignerBackend trait + LocalSignerBackend
  session.rs          // DeviceSession trait (enumerate, address_for_path, sign_raw)
  profiles/
    mod.rs
    evm.rs            // EVM SignRequest variants ↔ Alloy local / future Ledger
    // bitcoin.rs     // later — do not stub code until family exists
    // polkadot.rs
  // ledger.rs        // Phase 1 — feature-gated or separate cfg; implements DeviceSession
  // trezor.rs        // Phase 2
```

**Rules:**

| Layer | Owns | Must not own |
|---|---|---|
| `WalletState` / TUI | Approve UX, pick active account, call `SignerBackend` | HID, APDU, coin-type math |
| `SignerBackend` | Dispatch `SignRequest` by account kind | Vendor USB details |
| `DeviceSession` | Open device, path→address, raw sign ops | EIP-1559 / PSBT construction |
| `*HwProfile` | Family payload encode/decode + path defaults | Vendor-specific USB |
| `Ledger`/`Trezor` transport | HID + vendor protocol | Chain registry, fees, RPC |
| `EvmAdapter` / `ChainAdapter` | Build unsigned tx, broadcast raw | Device I/O (receives signed bytes) |

Signing stays **outside** `ChainAdapter::send_transaction` for HW: adapter builds + broadcasts; backend signs. Same pattern should work when Bitcoin/Polkadot adapters arrive (UTXO PSBT sign → broadcast).

### New types (Phase 0 — no HID deps)

```rust
#[non_exhaustive]
enum HardwareVendor { Ledger, Trezor }

/// Which chain family this watch-record is for (mirrors ChainType).
#[non_exhaustive]
enum HwChainFamily { Evm, /* Bitcoin, Polkadot later */ }

struct HardwareAccountRecord {
    vendor: HardwareVendor,
    family: HwChainFamily,
    /// Opaque derivation string for that family.
    /// EVM: BIP-44 `m/44'/60'/0'/0/0` (coin 60).
    /// Future BTC: BIP-84/86 paths; DOT: Substrate URI — not hardcoded here.
    derivation_path: String,
    /// Optional opaque network hint (EVM chain id string, BTC network, DOT genesis).
    /// Address may be reused across EVM chains; still store for UX / re-verify.
    network_id: Option<String>,
    address: String,   // family-validated; verified on connect
    label: String,
}

enum AccountKind {
    Hd { index: u32 },
    Imported,
    Hardware(HardwareAccountRecord),
}

/// Family-tagged request — core trait stays multichain; EVM is first variant set.
#[non_exhaustive]
enum SignRequest {
    EvmPersonal { message: Vec<u8> },
    EvmTypedDataHash { hash: B256 },
    EvmTransaction { /* unsigned EIP-1559 fields or RLP */ },
    // BitcoinPsbt { … }, PolkadotExtrinsic { … } — add with family crates
}

#[non_exhaustive]
enum SignResult {
    SignatureHex(String),
    RawTx(Vec<u8>),
}

/// Async, family-agnostic. Local wraps PrivateKeySigner for EVM variants only in v1.
trait SignerBackend: Send + Sync {
    fn address(&self) -> &str;
    fn family(&self) -> HwChainFamily;
    async fn sign(&self, req: SignRequest) -> Result<SignResult, WalletError>;
}

/// Vendor transport only (Phase 1+). No EVM types in this trait.
trait DeviceSession: Send + Sync {
    fn vendor(&self) -> HardwareVendor;
    async fn list_paths_preview(&self, family: HwChainFamily) -> Result<Vec<(String, String)>, WalletError>;
    async fn address_for_path(&self, family: HwChainFamily, path: &str) -> Result<String, WalletError>;
    async fn sign_preimage(&self, family: HwChainFamily, path: &str, preimage: &[u8]) -> Result<Vec<u8>, WalletError>;
}
```

**EVM convenience wrappers** (thin, in `profiles/evm.rs`) may call `sign(SignRequest::Evm…)` so existing wallet call sites stay readable — but the trait itself must not be `sign_personal_message`-only.

Vault JSON envelope gains `hardware: Vec<HardwareAccountRecord>` (no secrets).  
Account index base (proposal): `HARDWARE_INDEX_BASE = 2_000_000` (imports stay at `1_000_000`).

### Multichain adaptation checklist (when adding a family)

When Bitcoin or Polkadot land (see `chains/{family}/` + PLAN derivation note):

1. Add `HwChainFamily` / `SignRequest` / `SignResult` variants — **no** change to `DeviceSession` USB surface if the vendor already exposes raw/path sign.
2. Add `profiles/{family}.rs` + default derivation helpers (do not hardcode coin 60 in `backend.rs`).
3. Teach that family’s `ChainAdapter` to accept externally signed payloads (same as EVM `broadcast_raw`).
4. TUI “Add device” gains a family picker only if multi-family HW is enabled; default remains EVM.
5. Vendor transport may need a new app/curve (e.g. Bitcoin app on Ledger) — isolate behind `DeviceSession` impl, not WalletState.

### UX sketch

| Action | Behavior |
|---|---|
| Keys → “Add Ledger / Trezor” | Discover device → family=EVM → pick path/account → confirm address on device → persist watch record → F3 |
| F3 on HW account | Label like `Ledger · EVM · m/44'/60'/0'/0/0` |
| Sign / send | TUI approve → “Confirm on device…” spinner → device reject/timeout → clear errors |
| Ctrl+Y | Still copies **address** (safe) |
| Export key / seed | Blocked with clear message for HW accounts |
| Disconnect | Signing fails soft: “Connect Ledger/Trezor” |
| Wrong family / chain app | Clear error (“Open Ethereum app” / future “Open Bitcoin app”) — never silent fallback to software key |

---

## Phased delivery

### Phase 0 — Abstraction prep (no new crates)

- [x] Module skeleton `security/hardware/` (`types`, `backend`, `session` stub, `profiles/evm`)
- [x] `AccountKind` + `HardwareAccountRecord` **with `family` + opaque path** in vault (serde, forward-compatible)
- [x] `SignerBackend` + `SignRequest`/`SignResult` + `LocalSignerBackend` (EVM variants only)
- [x] Route local personal / typed / tx sign through backend (behavior unchanged); thin EVM helpers OK
- [x] `EvmAdapter` seam: wallet `prepare_sign_raw` → backend sign → `broadcast_raw` (unsigned adapter for fees/nonce)
- [x] Guards: export / stealth / AA refuse HW; reject non-EVM requests when only EVM local exists
- [x] Unit tests: vault roundtrip `hardware: []`; local Anvil paths green; compile proves no HID deps
- [x] Doc this file + TASKS “HW Phase 0”

**Exit:** software wallet identical; types and module seams ready for Ledger **and** a future non-EVM profile without trait rewrites.

### Phase 1 — Ledger EOA (USB)

- [ ] **Dependency approval:** Alloy Ledger signer + transport (exact crates TBD against allowlist review)
- [ ] `DeviceSession` + `LedgerTransport` (HID only) + `EvmHwProfile` wiring — no fee/RPC in transport
- [ ] TUI: add device (EVM), confirm-on-device chrome, timeout / user-reject / wrong-app mapping
- [ ] Anvil: mock `DeviceSession` for CI; optional `#[ignore]` live test when device present
- [ ] Pulse testnet 943 native send + `personal_sign` smoke

**Exit:** Ledger EVM account can send + dApp-sign on 943 with double confirm (TUI + device); seams unused by BTC/DOT yet but not EVM-locked.

### Phase 2 — Trezor EOA

- [ ] **Dependency approval:** Trezor client crate(s) (often heavier / protobuf — review carefully)
- [ ] Same `SignerBackend` as Ledger; shared TUI flows parameterized by vendor
- [ ] Path / passphrase UX (Trezor passphrase = extra secret — never log)
- [ ] 943 smoke parity with Ledger

**Exit:** F3 can be Ledger *or* Trezor; docs cover both.

### Phase 3 — Hardening (optional follow-ons)

- [ ] Multiple devices / re-verify address on every unlock session
- [ ] Blind-signing policy (reject oversized typed data / unknown contracts until allowlisted)
- [ ] Freedom Browser: no change if EIP-1193 still goes through Vaughan approve
- [ ] Explicit **no** for: MCP→device, Degen auto-sign on HW, stealth meta-keys on device (until redesigned)

---

## Dependency gate (must ask before coding Phase 1+)

| Need | Candidate direction | Notes |
|---|---|---|
| Ledger | Alloy `signer-ledger` + HID transport | Prefer Alloy family; pin versions in workspace |
| Trezor | Dedicated client (evaluate AGPL/GPL) | Prefer interface-only if license conflicts with MIT/Apache vault |
| Async | Existing `tokio` | Device I/O on blocking pool or async USB |

Do **not** add crates in Phase 0.

---

## Risks

| Risk | Mitigation |
|---|---|
| HID flaky on Linux (perms, udev) | Document udev rules; clear “device locked / busy” errors |
| Blind signing on device | Prefer clear-signing apps; warn in TUI |
| AA / 7702 + HW | Out of scope v1; keep Ambire on software EOAs |
| Stealth needs spend key | Keep stealth on HD vault only |
| License / telemetry in vendor SDKs | Review before allowlist; no analytics |
| EVM-only HW trait / god-module | `SignRequest` tags + `profiles/{family}` + `DeviceSession` without chain types |
| Hardcoding BIP-44 coin 60 in transport | Opaque `derivation_path` + family profile defaults (matches PLAN HD note) |

---

## Suggested order of work

1. **Phase 0 PR** — `security/hardware/` skeleton + local EVM profile + vault field (this prep).  
2. Dependency approval thread for Ledger crates.  
3. **Phase 1** Ledger on 943 (EVM profile only).  
4. Dependency approval for Trezor.  
5. **Phase 2** Trezor (same `DeviceSession` / profiles).  
6. When Bitcoin/Polkadot adapters land: add profile + `SignRequest` variants — **not** a Ledger rewrite.  
7. Only then revisit AA/stealth/HW.

---

## Out of scope (explicit)

- Bluetooth / Speculos-only as production path (OK for CI mock later)
- Multisig / Safe as “hardware”
- Hosted or TEE signing
- Replacing software vault as default onboarding
