# Contributing

Thanks for helping improve Vaughan-CLI. This is a security-sensitive wallet;
small, reviewable changes beat large drive-by refactors.

## Product thesis (Browserless Pulse)

Vaughan’s default story is **in-terminal** Pulse DeFi: Ag / Dex / Browse /
approvals — approve calldata, not websites. External agents use **MCP**
(`vaughan mcp`); **VB** is the optional web side door. **Freedom Browser is
parked** until [PR #195](https://github.com/solardev-xyz/freedom-browser/pull/195)
merges ([docs/freedom-browser-status.md](docs/freedom-browser-status.md)).
See [docs/browserless-pulse.md](docs/browserless-pulse.md). Prefer that framing
in docs and UX copy; don’t push WalletConnect or an in-wallet dApp browser as
the main path.

## Before you start

1. Read [CLAUDE.md](CLAUDE.md) — engineering rules, accepted dependencies, and
   security guardrails are binding.
2. Skim [REQUIREMENTS.md](REQUIREMENTS.md) and [TASKS.md](TASKS.md) for context.
3. For non-trivial work, open an issue or comment on an existing one so scope is
   clear before you invest a large diff.

## Development setup

```bash
git clone https://github.com/r4-ndm/Vaughan-CLI.git
cd Vaughan-CLI
cargo build --workspace
cargo test --workspace
cargo fmt --all
cargo clippy --workspace -- -D warnings
```

Integration tests spin up local [Anvil](https://book.getfoundry.sh/anvil/) where
noted; install Foundry if those tests fail with "anvil not found".

## Pull request checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] No secrets, mnemonics, or real keys in code, tests, logs, or fixtures
- [ ] Signing / fund-moving paths keep explicit user approval (never auto-sign)
- [ ] New dependencies are listed in CLAUDE.md allowlist or called out in the PR
- [ ] Public types and modules have brief `///` / `//!` docs where non-obvious

## Code style

- Rust 2021, `unsafe` forbidden
- Match existing module layering in `vaughan-core`
- Reuse Alloy / battle-tested crates; do not hand-roll crypto or chain logic
- Testnet-first for anything that moves funds

## License

By contributing, you agree that your contributions are licensed under the same
terms as the project: **MIT OR Apache-2.0**, at the recipient's choice.
