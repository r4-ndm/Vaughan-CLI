# Audit playbook

How to audit this repo — by a human or by any agent. Read `AGENTS.md` first for the
crate map and the rules that must not be broken; this document is the *procedure*.

Two halves, and both are required:

1. **Mechanical** — `scripts/audit.sh`. fmt, clippy (default and railgun features),
   tracked-file hygiene, a secrets scan, and optionally the test suite and the
   installed binary. It fails loudly. Cheap, repeatable, and it is not an audit.
2. **Judgement** — this document. Reading the diff, deciding whether the code does
   what the docs claim, and whether a money path can lose funds.

The most common bad audit is a green `cargo test` reported as "audited". Unit tests
prove the stub compiles. They do not prove funds move correctly.

---

## Before you start

```bash
git status --short          # know what is uncommitted; never audit a dirty tree silently
git log --oneline -10       # what changed, and does the message match the diff
scripts/audit.sh            # mechanical gate (add --with-tests --binary for a release audit)
```

State the commit you audited in the report. An audit of an unnamed SHA is worthless
a week later.

---

## Evidence rules (agents: these are pass/fail)

- **Paste the command and its output.** "Tests pass" without the command is a claim,
  not a result. If you ran `--with-tests`, say so; if not, say what you skipped.
- **Exit codes are not evidence when the tool is wrapped.** `agy -p` exits 0 even when
  it did nothing (auto-denied permission). `grep -c` exits 1 on zero matches. Check the
  artifact — the file exists, the binary contains the string, the diff landed.
- **Distinguish verified from inferred.** "I read the code and it calls X" is a weaker
  claim than "I ran it and observed X". Label which one you are making.
- **A feature-gated path is not dead code.** Confirm the feature is actually enabled in
  the build you are judging (`strings <binary> | grep <literal>`), not just present in
  the source.
- **Nothing is fixed until the symptom is gone.** Re-run the thing the user reported.

---

## Non-negotiable invariants (any violation is P0)

Flag these before style, performance, or architecture. They are ordered by how much
money a mistake costs.

1. **The Hyperliquid agent key cannot withdraw.** Exchange rule, not a policy flag. Any
   automated profit path must stop at a quoted intent, or have the user move funds by
   hand first. A code path that assumes the bot can withdraw is wrong by construction.
2. **No fabricated transactions.** A money path models its outcome explicitly —
   `Quoted` / `Broadcast { tx_hash }` / `Skipped { reason }`. A placeholder hash
   (`0xSIM_…`, `0xDRY_RUN…`, `format!("0x{…}")`) returned as if real is a P0 bug, not a
   dry-run.
3. **No guessed constants on a money path.** Unbound recipient, unknown conversion rate,
   failed quote → `Skipped { reason }`. A hardcoded fallback account or price silently
   sizes every batch wrong and is invisible to tests.
4. **Custody stays with the crate that holds the wallet.** `vaughan-hl` must not gain a
   `vaughan-core` dependency, and signing must not move into the bot binary. The only
   broadcast paths are `vaughan-tui`'s `UiJob::SendEvmWithFee` / `WalletState::send_transaction`.
5. **Spending starts disabled and explains itself.** A new spending feature ships off by
   default and reports why it is off; approvals and withdrawals stay deliberate user
   actions in the UI.
6. **Quotes are re-fetched immediately before signing.** Calldata embeds that quote's
   min-out. A persisted quote is display-only; a stale one is a P0.
7. **Contracts are allowlisted.** Unknown router/spender in an execution path is a
   refusal, not a warning.

---

## Passes

Run them as separate sessions if the diff is large; mixing them dilutes both.

### Pass A — funds and signing

Read the diff on every path that can move value, in this order:

- account derivation and unlock (`vaughan-core/src/core/wallet.rs`, `account.rs`)
- anything that builds or signs a transaction (`send_transaction`, `UiJob::Send*`)
- swap and bridge quoting (`quote_aggregator`, Liberty client) — check latency budgets,
  min-out handling, spender allowlist, and whether the quoted calldata is rebuilt
- RAILGUN shield/unshield (`vaughan-railgun`, `views/shield_flow.rs`, `views/unshield_flow.rs`)
- the HL bot's reinvest/siphon path — confirm it really is intent-only
- MCP and the local provider: can an external process cause a signature without a human
  approval card?

For each: what is the worst outcome if this line is wrong, and does something catch it?

### Pass B — claim vs reality

Documentation is part of the product, and it drifts silently:

- Does `README.md` still describe what the binary does? Check the feature list, the
  shortcut table, and the "what is in good shape vs still rough" section against the code.
- Does every `*_enabled()` / feature gate match the docs? A panel that exists in source
  but is compiled out of the shipped binary is a documentation bug and a UX bug.
- Do the phase checklists (`TASKS.md`, `REQUIREMENTS.md`, `PLAN.md`) mark done things that
  are only scaffolding?
- Are test claims real? A test marked as covering a path that does not exercise it is
  worse than no test — it buys false confidence.

### Pass C — mechanics

`scripts/audit.sh --with-tests`, then look at what the tests do not touch:

- money paths with no `#[ignore]`d live test against the real service
- state machines initialised from a zero epoch instead of the real start time
- CWD-relative paths for artifacts the app writes (the launcher's CWD is not the repo)
- new dependencies: is the crate needed, pinned, and does it leak into the default build

---

## Repo-specific traps that look like bugs (and vice versa)

- **Feature-gated UI "disappears".** RAILGUN is `default = []`. A plain
  `cargo build -p vaughan-cli` has no privacy chip and `0` only flashes "Private view
  needs the vaughan-p build" — the shipped `scripts/install-vaughan.sh` build enables the
  feature. Check the binary before filing a UI regression:
  `strings ~/.cargo/bin/vaughan | grep 'private (RAILGUN)'`.
- **Empty private panel is correct on PulseChain.** RAILGUN is deployed on Sepolia only
  (`vaughan-railgun/src/deployment.rs`). "Isn't deployed" on 369/943 is right.
- **Stale install.** `~/.cargo/bin/vaughan` can be older than the tree. Compare with
  `cmp target/release/vaughan ~/.cargo/bin/vaughan` or a string literal that changed.
- **Anvil version skew.** CI pins Foundry v1.2.1; newer anvil replies to
  `eth_sendTransaction` before mining and makes send-then-read tests flaky.
- **`test-vectors/railgun/node_modules`** exists locally and is deliberately untracked.

---

## Reporting

Findings, not narration. For each item:

```
P0/P1/P2 — one-line title
  where:   path:line
  impact:  what can go wrong (funds lost / silent wrong size / misleading docs)
  evidence: command + output, or "read-only: no runtime check"
  fix:     the smallest change that removes the risk, or "needs design decision"
```

- **P0** — can lose funds, leak a key, or broadcast a fabricated transaction.
- **P1** — wrong behaviour on a money-adjacent path, or a real security gap that needs
  user error to exploit.
- **P2** — everything else that is worth fixing: drift, missing tests, misleading docs.

**Latent vs live:** the same defect drops one level when nothing currently reaches it —
executable calldata written to disk with no consumer is P1, the day something reads it
and signs, it is P0. Say which you judged ("latent: no reader today") so the next audit
does not have to re-derive it.

Then say explicitly what you did **not** cover. An audit that claims full coverage is
usually one that stopped early.

File the actionable ones in `TASKS.md` or as GitHub issues — a chat-only report is lost.
