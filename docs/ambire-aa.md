# Ambire Smart Accounts (AA) — how Vaughan uses Ambire's code

> Reference for FR-3.3 ("Ambire smart accounts (ERC-4337/7702 AA) reimplemented in
> Rust via Alloy + Ambire ABI, borrowed from Vaughan-Dioxus").
> Written so future-us doesn't have to re-derive *how* we use Ambire's open-source wallet.

---

## 1. The one-line principle

**We do not port or translate Ambire's TypeScript into Rust. We reimplement the
off-chain logic ourselves, carrying over only the *interface* (ABI + transaction
schema), and we leave the security-critical on-chain contracts exactly as Ambire
wrote them.**

Ambire's wallet is TypeScript (`ambire-common`, `wallet`, `extension`) plus Solidity
smart contracts. You can't mechanically "translate" TS → Rust anyway. What actually
happens is narrower and deliberate:

- **Borrow (interface facts only)** — the smart-account **ABI** (`AmbireAccount`,
  its `Transaction { to, value, data }` batch element, and the `execute` entry
  point) and the **digest/signature layout**: the signature is `r ‖ s ‖ v ‖ mode`
  (66 bytes) over the digest `keccak256(abi.encode(account, chainId, nonce, txns))`.
  These facts come from Ambire's **deployed, verified on-chain contract** — not from
  `Vaughan-Dioxus`, which is a guide only (see CLAUDE.md). `scw_transaction` is how
  Ambire's TS SDK labels this payload; we reimplement the schema, we don't translate
  it.
- **Write ourselves (Rust + Alloy)** — ABI encoding of the batch, the
  `keccak256(abi.encode(...))` digest, signing (raw hash or EIP-191, with the mode
  byte appended), ERC-4337 `UserOperation` / EIP-7702 tx assembly, and broadcast
  through the existing `EvmAdapter`.
- **Never rewrite** — the on-chain smart-account contract (`AmbireAccount`,
  AGPL-3.0). Its batch semantics, replay protection, and signature validation stay
  Ambire's **deployed, verified** Solidity code. We `call` it; we never reimplement
  it.

## 2. Borrow vs. build

| Borrow (interface only) | Build ourselves (Rust + Alloy) |
|---|---|
| Smart-account ABI (`sol!`): `execute` selector, the `Transaction[]` batch call shape (`to`, `value`, `data`) | ABI **encoding** of the batch (`sol!` / dyn-abi) |
| Digest formula `keccak256(abi.encode(account, chainId, nonce, txns))` + the 66-byte `r‖s‖v‖mode` signature layout (mode `0` = raw hash, `1` = EIP-191) | **hashing + signing** the digest, appending the mode byte |
| | ERC-4337 `UserOperation` and/or EIP-7702 tx assembly |
| | Nonce/gas/validation, broadcast via `EvmAdapter` |

Note: the canonical contract is `AmbireAccount` (`AmbireTech/wallet`, AGPL-3.0).
Verify the exact deployed/verified contract and its ABI against `AmbireTech`'s
contracts repo — the names above are the *shape* of what we borrow, not a canonical
list.

## 3. Why "reimplement, don't vendor" (two reasons)

1. **Layering control.** `vaughan-core` stays a clean, single-purpose core; AA is its
   own concern and lives in a dedicated `vaughan-aa` crate (see §6).
2. **License.** Ambire's contracts are **AGPL-3.0** (`AmbireTech/wallet`) and its TS
   SDK is GPL-family (`ambire-common`); Vaughan-CLI is **MIT OR Apache-2.0**. AGPL/GPL
   are copyleft, so neither can be absorbed into an MIT/Apache project. We carry over
   only the ABI *facts* (function signatures, struct shapes, the digest/signature
   layout — interfaces required for interop, not implementation) and write fresh Rust,
   so the `vaughan-aa` crate stays MIT/Apache.

   Rule of thumb: if we ever *do* want to vendor actual Ambire implementation code
   (we shouldn't), it must live in a **separate AGPL/GPL-licensed repo**, never in
   this workspace.

## 4. How we keep "battle-tested" guarantees when we reimplement

The trust transfer is **byte-equality, not code provenance**:

- The security-critical logic (who can sign, replay protection, batch semantics) is
  the **on-chain contract**, which is Ambire's audited code — untouched by us.
- The off-chain parts we rewrite (calldata encoding, digest hashing, signature) are
  **deterministic and spec-bound**. If our Rust emits the same bytes as Ambire's SDK
  for the same inputs, it *is* equivalent by construction.

Concretely, in `vaughan-aa`'s test suite:

1. Collect Ambire **signed-transaction fixtures** (real `scw_transaction` examples)
   plus the EIP-712 / ERC-4337 **reference vectors**.
2. Run the *same inputs* through Ambire's TS SDK and through our Rust crate.
3. Assert the calldata, digest, and final signature are **byte-for-byte
   identical**.

Byte-identical output across a corpus of vectors is the proof that the reimplementation
is faithful — no code was copied, but the behavior is verified against the
battle-tested reference.

## 5. What this reuses from earlier phases

- **`security::signing`** (FR-2.3): the Ambire digest is *not* EIP-712 — it's
  `keccak256(abi.encode(account, chainId, nonce, txns))`. We sign that 32-byte hash
  directly (`sign_hash_sync`, mode `0`) or via EIP-191 (`sign_personal_message`, mode
  `1`), then append the one-byte mode. Both primitives already exist in
  `security::signing`; `vaughan-aa` composes them.
- **`EvmAdapter`** (Phase 1): the existing provider/fallback/broadcast plumbing
  carries the assembled AA transaction to the network (testnet-first, NFR-3).

## 6. Proposed crate layout

```
vaughan-aa/                  # new workspace member (or its own repo, like kohaku-rs)
  Cargo.toml                 # MIT OR Apache-2.0; deps: alloy, alloy-dyn-abi
  README.md                  # provenance + license boundary (this doc, condensed)
  src/
    lib.rs
    abi.rs                   # Ambire smart-account ABI (sol!) — interface only
    scw.rs                   # scw_transaction / Signature types + digest (keccak256(abi.encode(...)))
    encode.rs                # batch calldata encoding
    sign.rs                  # EIP-712 hash + sign (reuses security::signing)
    build.rs                 # EIP-7702 assembly (done); ERC-4337 UserOperation deferred
    adapter.rs               # broadcast via EvmAdapter (testnet-first)
  tests/
    fixtures/                # Ambire signed-tx fixtures + reference vectors
    differential.rs          # byte-equality against Ambire's TS output
```

If the crate grows or needs independent hardening/audit, extract it to its own repo
and consume it as a git dependency (the established kohaku-rs pattern).
