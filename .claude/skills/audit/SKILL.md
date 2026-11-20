---
name: audit
description: Run Vaughan-CLI's two-part audit gate — the mechanical script and the judgement playbook — before declaring any money-path change, release, or large refactor audited. Use when the user asks to "audit", "review for the audit", "run the audit", or before a release.
disable-model-invocation: true
---

# Audit

Vaughan-CLI moves real money. This skill is a pointer, not a new procedure — the actual
rules live in two files that must stay the single source of truth:

- **`docs/audit-playbook.md`** — the full procedure: before-you-start checklist, evidence
  rules, the non-negotiable P0 invariants, Passes A (funds/signing) / B (claim vs reality)
  / C (mechanics), repo-specific traps, and the reporting format.
- **`scripts/audit.sh`** — the mechanical gate (fmt, clippy on both feature sets, `cargo
  audit`, module-size ratchet, tracked-file hygiene, secrets scan, default-build RAILGUN
  leak checks). `--with-tests` and `--binary` add the slow checks; `--history` adds a
  full-repo secrets walk.

Read `docs/audit-playbook.md` in full before producing any findings — don't rely on this
summary for the invariants or the Pass checklists, it will drift out of date and the
playbook won't.

## When invoked

1. Read `docs/audit-playbook.md` now, start to finish.
2. Run `git status --short` and `git log --oneline -10` — never audit a dirty tree silently,
   and state the commit SHA you audited.
3. Run `scripts/audit.sh` (add `--with-tests --binary` for a release-grade pass). This is
   the *mechanical* half only — a green run is not an audit on its own.
4. Do the *judgement* half: work Pass A, then B, then C from the playbook, as separate
   passes if the diff is large.
5. Report findings in the playbook's format (P0/P1/P2, one-line title, where/impact/
   evidence/fix), say explicitly what you did **not** cover, and file actionable items in
   `TASKS.md` or as GitHub issues — a chat-only report is lost.

## Do not

- Report `cargo test` passing as "audited." Tests prove the stub compiles, not that funds
  move correctly.
- Skip reading `docs/audit-playbook.md`'s "Repo-specific traps" section — several real bugs
  look identical to known-safe patterns in this repo (feature-gated UI, chain-specific
  empty states, stale installs, pinned-tool version skew).
- Widen the secrets-scan allowlist in `scripts/audit.sh` without re-deriving the complete
  set per the playbook's instructions first.
