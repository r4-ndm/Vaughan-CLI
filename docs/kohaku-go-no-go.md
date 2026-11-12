# Kohaku (privacy) — go/no-go

> Recorded 2026-08. Updated 2026-08-19: **FR-3.2 ERC-5564 is GO** on a
> spec-first path in `vaughan-core` (no Kohaku). **FR-3.1 stays NO-GO.**
> Updated 2026-10-02: **FR-3.4 RAILGUN moved to `docs/railgun-go-no-go.md`**
> (pinned upstream `railgun-rust`, not Kohaku).

## TL;DR

Kohaku is **not** used for stealth or RAILGUN. ERC-5564 scheme 1 lives in
`vaughan-core::security::stealth` (see `docs/erc-5564-stealth.md`). RAILGUN now
goes through `triamazikamno/railgun-rust`, whose derivation matches the official
engine; see `docs/railgun-go-no-go.md`. `ethereum/kohaku` has since removed its
Rust RAILGUN crates (RAILGUN there is a TypeScript package "under maintenance"),
so Kohaku-rs has nothing upstream to wrap.

## Per-FR

| FR | What | Verdict |
|----|------|---------|
| FR-3.1 | Harden kohaku-rs / publish crates | **Defer**: upstream Rust RAILGUN crates removed; unused for stealth |
| FR-3.2 | ERC-5564 stealth | **GO**: spec-first in `vaughan-core`, ScopeLift-compatible `h(S)` |
| FR-3.4 | RAILGUN pools | **Superseded**: via `railgun-rust`, see `docs/railgun-go-no-go.md` |

## Why not Kohaku for stealth

Kohaku's stealth helper generates **random** spend/view keys (lost on reinstall)
and is tied to an unaudited RAILGUN stack. Vaughan derives keys from the vault
mnemonic at frozen paths and hashes the compressed ECDH point like ScopeLift.

## Still not doing

- No Kohaku git dep or crates.io publish
- No stealth TUI until 943 announcer + send/scan/sweep work
- Ambire AA is unrelated and stays
