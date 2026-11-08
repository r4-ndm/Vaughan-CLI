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

CI runs fmt, clippy, and the full test suite on every push and PR, plus a separate
RAILGUN job. A red CI is not an opinion.

## Crate map

| Crate | Holds |
|---|---|
| `vaughan-core` | wallet, keys, signing, RPC, quoting (`quote_aggregator`), stealth |
| `vaughan-tui` | the dashboard; owns the only broadcast paths (`UiJob::SendEvmWithFee`, `WalletState::send_transaction`) |
| `vaughan-cli` | the `vaughan` binary; links `vaughan-tui` as a library |
| `vaughan-hl` | Hyperliquid bot: strategies, replay, live runner, TUI; ships binary + lib |
| `vaughan-railgun` | RAILGUN shielded balances (opt-in `railgun` feature, Sepolia only) |
| `vaughan-agent` | AI agent tools and proposals |
| `vaughan-mcp` / `vaughan-provider` | agent surface and local EIP-1193 RPC |

## Rules that must not be broken

Read `docs/audit-playbook.md` for the reasoning. Short form:

1. The HL **agent key cannot withdraw** — automated profit paths stop at a quoted intent.
2. **Never fabricate a transaction.** Model `Quoted` / `Broadcast { tx_hash }` /
   `Skipped { reason }`. A placeholder hash returned as real is a critical bug.
3. **Never guess a constant on a money path** (account, price, conversion rate). Fail to
   `Skipped { reason }` instead.
4. **Custody stays where the wallet is.** No `vaughan-core` dependency in `vaughan-hl`;
   no signing inside the bot binary.
5. **New spending ships disabled** and reports why it is off. Withdrawals stay a
   deliberate user action in the TUI.
6. **Re-quote immediately before signing**; persisted quotes are display-only.
7. **Allowlist routers/spenders.** An unknown contract in an execution path is a refusal.

## Working style

- Load `docs/audit-playbook.md` when touching signing, bridging, swaps, RAILGUN, or the
  bot's live path — those are the surfaces that break silently.
- Verify with the artifact, not the log line: `strings` the binary, read the file, run
  the command. Exit codes from agent wrappers lie.
- Feature-gated code needs a feature check: RAILGUN is `default = []`, so the private
  panel and the `0` shortcut do not exist in a plain build.
- Write artifacts to absolute paths, not CWD-relative ones — the TUI launches the bot
  from another directory.
- Update docs in the same commit as the behaviour change. Stale docs are a bug here.
