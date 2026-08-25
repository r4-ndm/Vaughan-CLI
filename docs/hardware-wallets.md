# Hardware wallets — Ledger & Trezor plan

**Status:** Plan only (no HID crates yet). Prep after polish commit `76a7b4c`.  
**Goal:** Optional hardware signer for EOAs on Pulse/EVM, same approval UX as software.

Hardware is **deferred product** (TASKS P4) until this plan’s Phase 0–1 land.

---

## Why not yet

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
2. **Keys never leave the device.** Vaughan stores address + vendor + BIP-44 path only.
3. **Software vault stays default.** Hardware is opt-in per account (F3), not a second app mode.
4. **MCP / agents never talk to HID.** Same as today: propose → human approve → sign in Vaughan.
5. **Export refuses hardware.** No “export private key” / mnemonic for device accounts.
6. **Stealth / AA:** v1 = software accounts only. Document as out-of-scope until EOA HW is stable.
7. **Testnet-first** for any fund-moving HW flow (943 before 369).

---

## Target architecture

```
                    ┌─────────────────────┐
  TUI / Provider ─►│  WalletState        │
                    │  active account     │
                    └─────────┬───────────┘
                              │
              ┌───────────────┼───────────────┐
              ▼               ▼               ▼
        LocalBackend    LedgerBackend    TrezorBackend
        (PrivateKey)    (USB/HID)        (USB/HID)
              │               │               │
              └───────────────┴───────────────┘
                              │
                    SignRequest { kind, payload }
                    → 0x signature / raw tx
```

### New types (Phase 0 — no HID deps)

```rust
enum HardwareVendor { Ledger, Trezor }

struct HardwareAccountRecord {
    vendor: HardwareVendor,
    /// BIP-44 path, e.g. m/44'/60'/0'/0/0
    derivation_path: String,
    address: String,   // checksummed; verified on connect
    label: String,
}

enum AccountKind {
    Hd { index: u32 },
    Imported,
    Hardware(HardwareAccountRecord),
}

/// Async surface; local impl wraps PrivateKeySigner.
trait SignerBackend: Send + Sync {
    fn address(&self) -> &str;
    async fn sign_personal_message(&self, msg: &[u8]) -> Result<String, WalletError>;
    async fn sign_typed_data_hash(&self, hash: B256) -> Result<String, WalletError>;
    async fn sign_transaction(&self, tx: /* EIP-1559 fields */) -> Result<Vec<u8>, WalletError>;
}
```

Vault JSON envelope gains `hardware: Vec<HardwareAccountRecord>` (no secrets).  
Account index base (proposal): `HARDWARE_INDEX_BASE = 2_000_000` (imports stay at `1_000_000`).

### UX sketch

| Action | Behavior |
|---|---|
| Keys → “Add Ledger / Trezor” | Discover device → pick path/account → confirm address on device → persist watch record → F3 |
| F3 on HW account | Label like `Ledger m/44'/60'/0'/0/0` |
| Sign / send | TUI approve → “Confirm on device…” spinner → device reject/timeout → clear errors |
| Ctrl+Y | Still copies **address** (safe) |
| Export key / seed | Blocked with clear message for HW accounts |
| Disconnect | Signing fails soft: “Connect Ledger/Trezor” |

---

## Phased delivery

### Phase 0 — Abstraction prep (no new crates)

- [ ] `AccountKind` + `HardwareAccountRecord` in vault (serde, forward-compatible)
- [ ] `SignerBackend` trait + `LocalSignerBackend`
- [ ] Route `sign_message` / `sign_typed_data` / tx sign through backend for **local** (behavior unchanged)
- [ ] Guards: export / stealth / AA refuse HW accounts with `WalletError::HardwareUnsupported` (or similar)
- [ ] Unit tests: vault roundtrip with empty `hardware: []`; local path still passes Anvil suites
- [ ] Doc this file + TASKS checkbox “HW Phase 0”

**Exit:** software wallet identical; types ready for device backends.

### Phase 1 — Ledger EOA (USB)

- [ ] **Dependency approval:** Alloy Ledger signer + transport (exact crates TBD against allowlist review)
- [ ] `LedgerBackend`: enumerate, get address for path, sign personal / typed / EIP-1559 tx
- [ ] TUI: add device, confirm-on-device chrome, timeout / user-reject mapping
- [ ] Anvil: optional `#[ignore]` live test when device present; mock backend for CI
- [ ] Pulse testnet 943 native send + `personal_sign` smoke

**Exit:** Ledger account can send + dApp-sign on 943 with double confirm (TUI + device).

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

---

## Suggested order of work

1. **Phase 0 PR** — types + local backend routing + vault field (this prep).  
2. Dependency approval thread for Ledger crates.  
3. **Phase 1** Ledger on 943.  
4. Dependency approval for Trezor.  
5. **Phase 2** Trezor.  
6. Only then revisit AA/stealth/HW.

---

## Out of scope (explicit)

- Bluetooth / Speculos-only as production path (OK for CI mock later)
- Multisig / Safe as “hardware”
- Hosted or TEE signing
- Replacing software vault as default onboarding
