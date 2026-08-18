# vaughan-aa

Ambire smart-account (AA) support for Vaughan-CLI, reimplemented from the
on-chain `AmbireAccount` contract. This crate is **MIT OR Apache-2.0**.

## Provenance & license boundary

- Ambire's on-chain smart account is `AmbireAccount` (`AmbireTech/wallet`,
  **AGPL-3.0**) and its off-chain SDK is GPL-family (`ambire-common`). Neither
  can be absorbed into an MIT/Apache project, so **no Ambire code is copied,
  translated, or vendored** here.
- We carry over only **interface facts** required for interop: the ABI selectors
  and struct shapes (`Transaction { to, value, data }`, `execute`), the digest
  formula `keccak256(abi.encode(account, chainId, nonce, txns))`, and the 66-byte
  `r ‖ s ‖ v ‖ mode` signature layout.
- Every line of encoding, hashing, and signing here is written fresh against the
  ABI/EIP specs and the verified contract's behavior. `Vaughan-Dioxus` is a
  reference guide only — never a code source (see CLAUDE.md).

See `docs/ambire-aa.md` for the full rationale and the differential-test plan.

## What's implemented

- `abi` — `AmbireAccount` ABI (`sol!`): `Transaction` + `execute`/`nonce`.
- `scw` — `ScwTransaction` and its signing digest + `SignatureMode`.
- `encode` — `execute(Transaction[], bytes)` calldata encoding.
- `sign` — the 66-byte `r ‖ s ‖ v ‖ mode` signature (raw-hash or EIP-191).
- `build` — EIP-7702 assembly: sign the `Authorization` delegating the account
  EOA to the Ambire implementation, and build the `TxEip7702` carrying
  `execute(txns, signature)`.
- `adapter` — self-pay broadcast through the existing `EvmAdapter`: pending
  nonce + EIP-1559 fee estimate, envelope signing, primary/fallback submission.

## Deferred (see TASKS.md FR-3.3)

- `build` ERC-4337 — the `UserOperation` / `getUserOpHash` path (needs an
  EntryPoint/bundler decision).
- `adapter` relayer-pay / bundler broadcast routes (the self-pay route is done).
- `tests/differential.rs` — byte-equality vs Ambire fixtures (harness ready;
  fixtures must be captured outside this workspace — see `tests/fixtures/README.md`).
