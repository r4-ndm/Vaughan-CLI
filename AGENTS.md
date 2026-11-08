# AGENTS.md

Instructions for any coding agent working in this repository (Claude Code, Codex,
Cursor, Antigravity, or anything else that reads `AGENTS.md`). Human contributors: the
same rules apply, see `CONTRIBUTING.md`.

## What this repo is

`Vaughan-CLI` is a Rust terminal wallet (ratatui TUI + Alloy) for PulseChain and other
EVM chains, with an MCP surface for external agents. It moves real money. Code that
compiles and passes tests can still be wrong in ways that lose funds.

## Commands

```bash
cargo build --release -p vaughan-cli        # default build (no RAILGUN)
cargo run -p vaughan-cli                    # TUI from source
scripts/install-vaughan.sh                  # install release build WITH railgun

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                      # needs anvil (Foundry v1.2.1) on PATH
scripts/railgun-parity.sh                   # upstream RAILGUN parity (opt-in stack)

scripts/audit.sh                            # mechanical audit gate
scripts/audit.sh --with-tests --binary      # release-grade gate
```

CI runs fmt, clippy, and the full test suite on every push to `main` and every PR
against `main`, plus a separate RAILGUN job. A push to any other branch runs no CI
until it is opened as a PR — run `scripts/audit.sh` locally. A red CI is not an
opinion.

## Crate map

| Crate | Holds |
|---|---|
| `vaughan-core` | wallet, keys, signing, RPC, quoting (`quote_aggregator`), stealth |
| `vaughan-aa` | EIP-7702 / Ambire smart accounts, batched sends |
| `vaughan-dapp-browser` | optional allowlisted Chromium dApp shell (EIP-1193 -> local provider) |
| `vaughan-tui` | the dashboard; user-approved broadcast via `UiJob::SendEvmWithFee` / `WalletState::send_transaction` |
| `vaughan-cli` | the `vaughan` binary; links `vaughan-tui` as a library |
| `vaughan-hl` | Hyperliquid bot: strategies, replay, live runner, TUI; ships binary + lib |
| `vaughan-railgun` | RAILGUN shielded balances (crate feature `enabled`, default off; frontends enable `vaughan-cli/railgun`; Sepolia only) |
| `vaughan-agent` | AI agent tools and proposals |
| `vaughan-mcp` / `vaughan-provider` | agent surface and local EIP-1193 RPC |
| `crates/wiz4rd-math`, `crates/wiz4rd-sdk` | Uniswap V3 tick/swap math, pool/swap/LP tx builders |

## Rules (single source of truth)

`CLAUDE.md` expands these for Claude Code sessions; this list is canonical. If the two
disagree, this file wins and `CLAUDE.md` should be fixed.

**Repo-wide**

1. **Rust only.** Pure Rust, no Node runtime, edition 2021, `unsafe` forbidden.
2. **No telemetry, analytics, or data collection** of any kind.
3. **Nothing destructive without an explicit request.** No `git push`, deploy, or
   irreversible command unless the user asked for it.
4. **Match existing conventions** — rustfmt + clippy clean, `vaughan-core` layering
   (chains / core / security / error / logging), `//!` module docs and `///` on public items.
5. **Testnet-first.** Anything that moves funds is exercised on testnet before mainnet.

**Money paths — read `docs/audit-playbook.md` before touching these**

6. **Secrets are encrypted at rest and never logged.** Argon2id + AES-256-GCM, `secrecy`
   + `zeroize` in memory; no mnemonic, key, or password in a log, error, UI buffer, or commit.
7. **Signing always requires explicit user approval.** No auto-sign, approvals are never cached.
8. The HL **agent key cannot withdraw** — automated profit paths stop at a quoted intent.
9. **Never fabricate a transaction.** Model `Quoted` / `Broadcast { tx_hash }` /
   `Skipped { reason }`. A placeholder hash returned as real is a critical bug.
10. **Never guess a constant on a money path** (account, price, conversion rate). Fail to
    `Skipped { reason }` instead.
11. **Custody stays where the wallet is.** No `vaughan-core` dependency in `vaughan-hl`;
    no signing inside the bot binary. Broadcast is not TUI-only: the chokepoint is
    `vaughan-core/.../adapter.rs`, and `vaughan-agent/src/sentient/trader.rs` signs and
    broadcasts by itself in `SentientTrader` mode — audit it with the TUI paths.
12. **New spending ships disabled** and reports why it is off. Withdrawals stay a
    deliberate user action in the TUI.
13. **Re-quote immediately before signing**; persisted quotes are display-only.
14. **Allowlist routers/spenders.** An unknown contract in an execution path is a refusal.

## Working style

- Load `docs/audit-playbook.md` when touching signing, bridging, swaps, RAILGUN, or the
  bot's live path — those are the surfaces that break silently.
- Verify with the artifact, not the log line: `strings` the binary, read the file, run
  the command. Exit codes from agent wrappers lie.
- Feature-gated code needs a feature check: RAILGUN is `default = []`, so a plain
  build has no private panel and `0` only flashes "Private view needs the vaughan-p
  build (--features railgun)" — the key exists, the panel does not.
- Write artifacts to absolute paths, not CWD-relative ones — the TUI launches the bot
  from another directory.
- Update docs in the same commit as the behaviour change. Stale docs are a bug here.
