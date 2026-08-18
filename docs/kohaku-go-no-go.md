# Kohaku (stealth addresses / privacy) — go/no-go decision

> Status: **NO-GO for now — defer FR-3.1/FR-3.4 until the RAILGUN derivation
> incompatibility is resolved upstream.**
> Recorded 2026-08. This is a decision, not a task — revisit only when the
> conditions below change.

## TL;DR

Vaughan was planning to use `kohaku-rs` for stealth addresses (FR-3.1/3.2) and
RAILGUN privacy pools (FR-3.4). Both are **blocked by a correctness risk in
upstream kohaku**, not by missing engineering. Building on top of it now would
risk producing a wallet that can *shield funds that can't be reliably
recovered or moved elsewhere* — the worst failure mode for a privacy feature.

## The blocking risk (from `docs/kohaku-risks.md`)

1. **RAILGUN key-derivation incompatibility.** Upstream `ethereum/kohaku`'s
   plugin flow derives RAILGUN keys with standard BIP-32 secp256k1 derivation,
   while the canonical RAILGUN engine uses a different "babyjubjub seed"
   derivation tree for spending/viewing keys. Same mnemonic → **unrelated keys**.
   A wallet built on kohaku-railgun would be incompatible with the wider RAILGUN
   ecosystem, and funds shielded through it could be unrecoverable elsewhere.
2. **Unaudited / pre-production upstream.** 12 commits, mostly scaffolding; the
   Rust port is new enough to have open correctness bugs. No semver stability;
   upstream's crates may not be structured for external consumption at all.

## What this means for each FR

| FR | What | Verdict |
|---|---|---|
| FR-3.1 | Harden kohaku-rs (test vectors, `kohaku-core` tests) | **Defer** — investing in a foundation with a known-derivation bug is wasted effort; the bug would have to be fixed upstream first |
| FR-3.2 | Wire ERC-5564 stealth addresses into Vaughan | **Defer** — ERC-5564 itself is fine, but the only Rust implementation we'd consume is kohaku's |
| FR-3.4 | Railgun / privacy pools | **Defer** — blocked by the derivation incompatibility (risk #1) |

## Why not just fork and fix it?

Fixing the derivation means **reimplementing the canonical RAILGUN engine's
derivation tree in Rust** — which is exactly the kind of security-critical,
hard-to-verify crypto work this project shouldn't take on from an unaudited
base. If RAILGUN support becomes a hard product requirement later, the honest
path is a **fresh, spec-first implementation validated against the canonical
engine's test vectors** — not a patch on kohaku.

## Revisit conditions (any one of these flips this to GO)

- Upstream kohaku resolves the derivation issue (fix + test vectors proving
  keys match the canonical engine) and reaches a tagged, reviewed release.
- We decide ERC-5564 stealth addresses are worth shipping **without** RAILGUN,
  using a non-kohaku implementation path (e.g. a minimal, spec-first ERC-5564
  implementation in `vaughan-core`, validated against public test vectors).
- RAILGUN becomes a product requirement and we commit to the fresh
  spec-first implementation above.

## What we're NOT doing

- No kohaku-rs hardening work (tests, crates.io publishing, git-dep wiring)
  while the derivation bug is open.
- No stealth-address or privacy-pool UI in the TUI.
- No removal of the existing `vaughan-aa`/Ambire work — this decision only
  covers the kohaku/stealth/privacy track.
