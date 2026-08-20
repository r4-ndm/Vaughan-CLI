# CLAUDE.md

Guidance for AI agents working in this repository. Read this first; it is
authoritative for how work gets done here.

## What this repo is

**Vaughan-CLI** — a Rust multi-chain CLI wallet TUI. EVM-first, PulseChain-optimized.

- `vaughan-core/` — library: chain adapters (Alloy EVM), core services, browser engine (`wiz4rd-engine`), security, persistence
- `vaughan-aa/` — EIP-7702 / Ambire smart accounts and batched sends
- `vaughan-agent/` — [Phase 5] AI Agent engine, LLM clients, tool registry, Degen circuit breakers
- `vaughan-provider/` — [Phase 2] local EIP-1193 bridge for Freedom Browser
- `vaughan-cli/` — unified `vaughan` binary (TUI by default; CLI subcommands for scripts)
- `vaughan-tui/` — ratatui terminal frontend (library + dev `vaughan-tui` binary)

This is a Rust-only repository.

## Read these before planning any change

- `REQUIREMENTS.md` — what we're building and why (requirement IDs FR/NFR/C)
- `PLAN.md` — architecture, technology choices, phases, risks
- `TASKS.md` — the task backlog; keep it in sync as you work

## Non-negotiable rules

1. **Rust only.** Pure Rust, no Node runtime. Edition 2021.
2. **Never write secrets unencrypted.** Mnemonic/keys are encrypted (Argon2id +
   AES-256-GCM) before touching disk, and zeroized in memory after use. Never log,
   print, or commit a mnemonic, private key, password, or `.env`.
3. **No telemetry / analytics / data collection** of any kind.
4. **Testnet-first.** Any feature that moves funds is exercised on testnet before
   mainnet. Never hardcode a mainnet-only path for a new flow.
5. **Signing always requires explicit user approval.** No auto-sign anywhere.
6. **Never push, deploy, or run destructive/irreversible commands** unless the user
   explicitly asks. `git push` requires an explicit request.
7. **Match existing conventions.** Follow rustfmt + clippy; `unsafe` is forbidden.
   Keep the `vaughan-core` module layering (chains / core / security / error / logging).

## Security & crypto guardrails

These expand rules 2, 4, 5, and 7. Every one is binding.

1. **Key material lives behind `secrecy` + `zeroize`.** Mnemonics, private keys,
   and passwords are held in `secrecy::SecretString` (or equivalent) and zeroized
   on drop. Never `Clone` a secret into a plain `String`; never implement `Debug`
   or `Display` on a type that holds secret bytes.
2. **No secret ever reaches a log, error, or UI buffer.** Mnemonics, keys, and
   passwords are banned from `tracing` macros, `WalletError` messages,
   `Debug`/`Display` output, and test assertion messages. Redact derived keys and
   derivation paths in any diagnostic output.
3. **Only Argon2id + AES-256-GCM for at-rest crypto.** No ECB, no unauthenticated
   modes, no hand-rolled ciphers, no weak KDFs. Never weaken KDF cost parameters
   outside an explicit `#[cfg(test)]` low-cost preset.
4. **Every signing path shows the user the full request first** — recipient, value,
   chain/network, and fee — and requires a fresh, explicit approval. Approval is
   never cached, and signing always uses the account the user selected.
5. **Validate before you sign or broadcast.** Addresses (checksummed), chain id,
   nonce, and amounts are validated before a transaction is built; reject malformed
   input with a `WalletError` rather than panicking.
6. **No secret material in source, tests, or fixtures.** Tests use clearly-marked,
   randomly generated test-only values. If a real secret ever lands in the repo or
   a log, treat it as compromised, rotate it, and flag it immediately.
7. **Memory safety around key material.** `unsafe` is forbidden anywhere, and any
   new dependency that touches key material must be reviewed before it is added.
8. **Only CSPRNGs for secrets.** Generate mnemonics, keys, salts, and nonces with
   a cryptographically secure RNG (`OsRng`/`ThreadRng`). Never use `StdRng`,
   `SmallRng`, or a seeded/derived value for key material.

## Engineering rules

1. **Only use the accepted libraries below — no custom code.** Everything is
   built on battle-tested crates (Alloy for EVM, `k256` for ERC-5564 stealth).
   Never hand-roll crypto, key handling, or chain logic, and never add a new
   dependency without approval.
2. **Never reinvent crypto, consensus, or key handling.** If a need isn't covered
   by the accepted libraries above, stop and ask rather than implementing it by
   hand or adding a new dependency (see the security guardrails above).
3. **Keep every module single-purpose and decoupled.** Follow the `vaughan-core`
   layering (chains / core / security / error / logging) and expose behaviour
   through traits (e.g. `ChainAdapter`). No god-objects; prefer small types and
   narrow function signatures.
4. **Document for the next dev/agent.** Every file opens with a `//!` module doc,
   public types and functions carry `///` docs explaining what and why (not how),
   and non-obvious decisions get a comment. `cargo doc` should read cleanly with
   no extra context.
5. **Sibling repos are guides, never code sources.** `Vaughan-Dioxus` and other
   in-house repos may be read to understand *how* a problem is structured — never
   copied, translated, or vendored. Every line here is written fresh and derived
   only from battle-tested upstream sources: the accepted libraries in the
   allowlist, published specs (EIPs), and the verified on-chain contracts we
   interoperate with. Where an upstream source is GPL/AGPL (e.g. Ambire's
   contracts and SDK), carry over only the *interface facts* required for interop
   (ABI selectors, struct shapes, digest/signature layouts) and write the
   implementation yourself — see `docs/ambire-aa.md`.
6. **MetaMask-family EIP convenience layer may be borrowed.** For wallet UX /
   provider convenience (EIP-1559 fee heuristics, speed presets, EIP-1193 method
   shapes, common dApp interop quirks), battle-tested **MetaMask / ethers /
   Alloy** algorithms and patterns are an approved reference. Reimplement in
   Rust on the allowlisted crates; cite the algorithm family in a short comment
   or module doc. Do **not** copy proprietary MetaMask extension UI code, and do
   **not** treat Ambire or Kohaku as the source for fee/provider convenience —
   Ambire is AA/batching only; Kohaku is deferred privacy (see below).

### Ambire vs Kohaku (product pair ≠ code couple)

Originally called for together as complementary wallet pillars: **Ambire** for
smart-account UX (batching / EIP-7702), **Kohaku** for privacy (stealth /
RAILGUN). That product pairing still makes sense.

They are **not** the same stack, and neither implements the other:

| Piece | Role in Vaughan |
|---|---|
| **Ambire** (`vaughan-aa`) | EIP-7702 smart accounts + atomic batched sends. On-chain `AmbireAccount` ABI/interop only. |
| **Kohaku** | Deferred as a crate. ERC-5564 stealth shipped **spec-first** in `vaughan-core::security::stealth` instead (Kohaku's random spend/view keys didn't fit HD vault restore). RAILGUN stays NO-GO — see `docs/kohaku-go-no-go.md`. |

Ambire does not unblock Kohaku/RAILGUN, and Kohaku is not required for Ambire.
Keeping Ambire while replacing Kohaku-for-stealth with in-core ERC-5564 is
intentional, not a contradiction of the original pair.
### Accepted libraries (on-disk allowlist)

Versions are pinned in `Cargo.toml`; this is the set you may use. Anything not
listed here requires approval before it is added.

| Concern | Crate |
|---|---|
| EVM chains, provider, signing, tx building | `alloy` |
| Async trait objects | `async-trait` |
| Async runtime | `tokio` |
| URLs | `url` |
| In-memory caching | `moka` |
| Mnemonics (BIP-39) | `bip39` |
| HD derivation (BIP-32/44) | `bip32` |
| Key derivation function | `argon2` |
| Authenticated encryption | `aes-gcm` |
| Secret types | `secrecy` |
| Zeroize on drop | `zeroize` |
| Randomness | `rand` |
| (De)serialization | `serde`, `serde_json` |
| Error types | `thiserror` |
| Structured logging | `tracing`, `tracing-subscriber` |
| Hex encoding | `hex` |
| Data/config directories | `dirs` |
| Hashing | `sha2` |
| Terminal UI | `ratatui` |
| Terminal events | `crossterm` |
| Provider WebSocket server (Phase 2) | `tokio-tungstenite` |
| Stream/sink utilities for WS | `futures-util` |
| secp256k1 arithmetic (ERC-5564) | `k256` |
| Privacy / stealth (Phase 3) | `vaughan-core::security::stealth` (not Kohaku) |
| Test temp dirs (dev only) | `tempfile` |

## Build, test, lint

```bash
cargo build --workspace          # build
cargo run -p vaughan-cli         # run the wallet TUI (or: vaughan after install)
cargo run -p vaughan-cli -- balance   # CLI subcommand (dev)
cargo test --workspace           # tests
cargo fmt --check                # formatting
cargo clippy --workspace -- -D warnings   # lint (treat warnings as errors)
```

Run these before declaring any non-trivial change done.

## Repository navigation

```
Cargo.toml                     # workspace + shared deps (alloy 2.x, ratatui, …)
vaughan-core/src/
  chains/                      # ChainAdapter trait, types, evm/ (Alloy adapter + networks)
  core/                        # wallet state, accounts, transactions, persistence, network
  security/                    # hd_wallet (BIP-39/32/44), encryption (Argon2id + AES-256-GCM)
  error.rs                     # WalletError + retry helper
  logging.rs                   # tracing setup
vaughan-tui/src/               # ratatui app: onboarding, unlock, dashboard, send, receive, settings
REQUIREMENTS.md  PLAN.md  TASKS.md   # requirements / plan / task backlog
```

Key external references (do not edit unless asked; these are separate repos):

- `r4-ndm/Vaughan-Dioxus` — reference guide only (layering ideas, Ambire AA approach); never a code source
- `r4-ndm/Kohaku-rs` — privacy SDK reference only; **not** a Vaughan dependency (stealth is in-core; RAILGUN deferred)
- `solardev-xyz/freedom-browser` — dApp browser we bridge to (Phase 2, out-of-repo PR)
- MetaMask / ethers / Alloy — approved **algorithm** references for EIP convenience UX (fees, EIP-1193); reimplement on Alloy, do not vendor their UI
## Workflow

1. Plan against `TASKS.md`; use a todo list for multi-step work.
2. Work in small, buildable increments; run `cargo build` + `cargo test` often.
3. Keep `TASKS.md` checkboxes in sync with reality.
4. Ask before making non-obvious architecture decisions (see the rules above).
