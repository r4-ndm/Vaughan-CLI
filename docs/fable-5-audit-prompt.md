# Fable 5 comprehensive audit — parked prompt

**Status:** Parked — run when Phase 7 / MCP browser work stabilizes or before a
release you trust with real funds.  
**Model:** Claude Fable 5 (high thinking) in Cursor Agent or equivalent.  
**Repo:** [r4-ndm/Vaughan-CLI](https://github.com/r4-ndm/Vaughan-CLI)

---

## When to run

Run this audit when **any** of these is true:

- You are about to tag a release or recommend mainnet use to others
- Phase 7 MCP `browser_*` tools land and you want a security pass before enabling CDP by default
- A large refactor touched signing, MCP, provider, or sentient auto-exec
- You want a second opinion after weeks of incremental changes

**Do not run** as a daily gate — use CI (`fmt`, `clippy`, `cargo test --workspace`) for that.

---

## Before you start (human checklist)

1. Push `main` so the audit matches remote: `git push origin main`
2. Note the commit SHA in the prompt (replace `<COMMIT_SHA>` below)
3. Ensure `cargo test --workspace` passes locally
4. Decide scope: **Full** (both passes) or **Security-only** / **Architecture-only**

---

## How to invoke in Cursor

1. Open Agent mode with **Claude Fable 5** (high thinking).
2. Paste the **Master prompt** below (fill in commit SHA and scope).
3. Ask for findings as **P0 / P1 / P2** with file paths and repro steps.
4. File actionable items in `TASKS.md` or GitHub issues — do not leave a chat-only report.

Optional: run Pass A and Pass B in **separate sessions** for deeper focus.

---

## Master prompt (copy from here)

```markdown
You are performing a **comprehensive audit** of the Vaughan-CLI repository — a
Rust self-custody wallet TUI (EVM-first, PulseChain-optimized). This is
**read-only analysis** unless I explicitly ask you to fix issues.

**Commit under review:** `<COMMIT_SHA>` on `main`  
**Scope:** `<Full | Security-only | Architecture-only>`

### Authoritative context (read first)

- `CLAUDE.md` — engineering rules, allowlisted deps, security guardrails
- `REQUIREMENTS.md` / `TASKS.md` / `PLAN.md` — what we claim to ship
- `docs/browserless-pulse.md` — default product path (Ag / Dex / Browse / MCP)
- `docs/dapp-browser-strategy.md` — VB (`vaughan-dapp-browser`) optional web side door
- `docs/freedom-browser-status.md` — Freedom Browser **parked** until upstream PR #195 merges
- `docs/mcp.md` + `docs/mcp-threat-model.md` + `docs/ai-tool-surface.md` — MCP contract
- `docs/Security-Table.md` — threat model by path
- `SECURITY.md` — scope for vuln reports

### Product paths (must stay consistent in your review)

| Path | Role |
|------|------|
| **Browserless Pulse** | Default — TUI Ag/Dex/Browse, human approve |
| **MCP adviser** (`--profile default`) | Propose only → TUI approval |
| **Sentient** (`--profile sentient`) | Auto-exec under policy + circuit breakers |
| **VB** | Optional Chromium + extension → provider → TUI approve |
| **Freedom** | Parked — dev fallback only until PR #195 |

### Non-negotiable invariants (flag any violation)

1. **No auto-sign** on default/adviser profile or MCP propose path without explicit TUI approval
2. **No secret material** in logs, errors, UI buffers, tests, or git
3. **Signing always** shows recipient, value, chain, fee before user confirms (never cached approval)
4. **Testnet-first** for new fund-moving flows; mainnet MCP writes gated
5. **Rust only**; no new deps outside `CLAUDE.md` allowlist without explicit approval
6. **Provider** binds loopback only; untrusted origins rejected
7. **Sentient** isolation: burner profile, breakers, policy file; legacy `degen` profile alias OK

---

## Pass A — Security (adversarial)

Hunt for ways **funds, keys, or approvals** could be bypassed. Prioritize:

### A1. Vault & crypto

- `vaughan-core/src/security/` — Argon2id + AES-256-GCM, zeroization, no weak KDF in prod
- Unlock paths, password handling, `.bak` recovery, file permissions (`0o600` / `0o700`)

### A2. Signing & approval gates

- `vaughan-tui/src/provider.rs`, `views/approve.rs` — EIP-1193 + MCP unified gate
- Fee re-estimation, simulation at approve time, fee-spike rejection
- Hardware wallet refusal paths (export/AA/stealth on HW)

### A3. MCP & sentient auto-exec

- `vaughan-mcp/`, `vaughan-tui/src/sentient_mcp.rs`, `vaughan-cli/src/serve.rs`
- Loopback IPC, session tokens, queue HMAC, rate limits
- Sentient: `gate_sentient_proposal`, breaker trip, policy precedence (`sentient-policy.toml` vs legacy `degen-policy.toml`)
- Tools that must **never** appear: `sign_*`, `export_*`, `unlock`, key material

### A4. Provider & VB bridge

- `vaughan-provider/` — origin allowlist, read RPC proxy allowlist, token requirements
- `vaughan-dapp-browser/`, `vaughan-tui/src/dapp_browser.rs` — nav gate, CDP auth, session dir
- Tamper watchdog, EIP-6963 re-announce, privacy launch flags

### A5. Agent / sentient trader

- `vaughan-agent/src/sentient/` — circuit breakers, quorum, position/slippage/gas limits
- `execute_sentient_swap` + legacy alias `execute_degen_swap`
- Dex router allowlist (`vaughan-core/src/core/dex_routers.rs`)

### A6. Dependency & supply chain

- Workspace `Cargo.toml` pins; any crate touching keys or TLS
- No `unsafe`; no hand-rolled crypto

**Deliverable for Pass A:** Table of findings with **P0** (exploit / fund loss), **P1** (likely bug / policy gap), **P2** (hardening). Each row: ID, path, scenario, impact, suggested fix, test idea.

---

## Pass B — Architecture & product truth

Verify **docs, TASKS, and code** agree. Prioritize:

### B1. Default path clarity

- README, CONTRIBUTING, TUI labels — Browserless Pulse first; VB optional; Freedom parked
- No doc still implying Freedom is the active integration target

### B2. Phase completeness vs claims

- `TASKS.md` checkboxes vs reality (Phase 7 gaps: post-load nav allowlist, MCP `browser_*`, CEF embed)
- `REQUIREMENTS.md` FR IDs traceable to code or explicitly deferred

### B3. Module layering

- `vaughan-core` layering respected; no CEF/Chromium in default `vaughan-cli` build
- `vaughan-agent` has no vault unlock / no key imports

### B4. Test gaps

- Critical paths with **no** Anvil/integration test named
- Freedom smoke tests marked optional vs required

### B5. UX footguns

- Terminal URL click → system browser (documented mitigations)
- Brave wallet vs Vaughan inject confusion
- PulseX IPFS mirror hops / allowlist completeness

**Deliverable for Pass B:** Drift list (doc says X, code does Y), missing tests, and **recommended TASKS.md entries** (checkbox + one line each).

---

## Out of scope (do not spend tokens here)

- Style, rustfmt, clippy nits (CI handles)
- Rewriting MCP to `rmcp` (deferred per `docs/mcp-transport.md`)
- Freedom Browser upstream PR #195 review (out of repo)
- Kohaku / RAILGUN (NO-GO per `docs/kohaku-go-no-go.md`)
- Feature requests unrelated to security or product truth
- Bitcoin / Polkadot profile bodies (future)

---

## Acceptance criteria (audit “done”)

- [ ] Zero **unmitigated P0** findings (or explicit user acceptance with documented risk)
- [ ] Every **P1** has a TASKS.md checkbox or GitHub issue link
- [ ] Product table (Browserless / MCP / Sentient / VB / Freedom parked) matches README + `docs/freedom-browser-status.md`
- [ ] Signing invariant verified on at least: Send, Ag swap, MCP propose, provider `eth_sendTransaction`, sentient auto-exec (if in scope)
- [ ] Auditor ran `cargo test --workspace` (or notes why not)

---

## Suggested follow-up workflow

1. Triage P0 → fix immediately on a branch  
2. P1 → schedule before next release tag  
3. P2 → backlog in `TASKS.md`  
4. Re-run **Pass A only** after fixes touching signing/MCP/provider  

---

## Revision log

| Date | Notes |
|------|--------|
| 2026-08-27 | Initial parked prompt — post Sentient rename + Freedom parked docs |
