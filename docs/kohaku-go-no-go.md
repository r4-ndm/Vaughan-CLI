# Kohaku (privacy) — go/no-go

> Recorded 2026-08. Updated 2026-08-19: **FR-3.2 ERC-5564 is GO** on a
> spec-first path in `vaughan-core` (no Kohaku). **FR-3.1 / FR-3.4 stay NO-GO.**

## TL;DR

Kohaku is **not** used for stealth or RAILGUN. ERC-5564 scheme 1 lives in
`vaughan-core::security::stealth` (see `docs/erc-5564-stealth.md`). RAILGUN
remains blocked on upstream key-derivation incompatibility.

## Per-FR

| FR | What | Verdict |
|----|------|---------|
| FR-3.1 | Harden kohaku-rs / publish crates | **Defer** — RAILGUN derivation bug; unused for stealth |
| FR-3.2 | ERC-5564 stealth | **GO** — spec-first in `vaughan-core`, ScopeLift-compatible `h(S)` |
| FR-3.4 | RAILGUN pools | **Defer** — BIP-32 vs babyjubjub seed tree |

## Why not Kohaku for stealth

Kohaku's stealth helper generates **random** spend/view keys (lost on reinstall)
and is tied to an unaudited RAILGUN stack. Vaughan derives keys from the vault
mnemonic at frozen paths and hashes the compressed ECDH point like ScopeLift.

## Still not doing

- No Kohaku git dep, crates.io publish, or RAILGUN UI
- No stealth TUI until 943 announcer + send/scan/sweep work
- Ambire AA is unrelated and stays
