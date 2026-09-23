# Security delta prep — working tree (high leverage)

**Purpose:** Freeze intent so a **new Claude model** can run one focused pass.  
**Not** a full-repo audit — use [`fable-5-audit-prompt.md`](fable-5-audit-prompt.md) for that.  
**Status:** Prep ready. Diff is still **uncommitted** on `main`.

---

## When to switch to the new Claude model

Run the new model **once** when **all** of these are true:

1. You have stopped editing signing / vault / DCA / Trezor / allowlist code for this batch
2. Optional but preferred: `cargo test -p vaughan-core -p vaughan-agent --lib` is green
3. You paste the **Delta prompt** below (or ask Cursor Security Review: *uncommitted changes* + the same invariants)

**Do not** switch mid-edit. After that pass: triage P0/P1 here (or cheaper model), fix, then re-run new Claude **only if** you touched those same paths again.

Full pre-release audit → still use [`fable-5-audit-prompt.md`](fable-5-audit-prompt.md) on a **pushed commit SHA**.

---

## Scope (in)

| Area | Paths | Why |
|---|---|---|
| **DCA (new)** | `vaughan-agent/src/dca/`, `tools/{propose,list,cancel}_dca_plan.rs`, `vaughan-tui/src/views/dca.rs`, `app.rs` (`poll_dca_due` / `DcaSlice`) | Recurring auto-spend under Sentient |
| **Vault password rotate** | `wallet.rs` `change_password`, `encryption.rs`, Settings UI | Re-encrypt vault + Piteas side blob |
| **Trezor host UI** | `security/hardware/trezor/{usb,ui_bridge}.rs`, `scripts/trezor_personal_sign.py` | PIN/passphrase/button confirm paths |
| **Entitlement chrome cache** | `assist_entitlement.rs` `assist_unlock_cached` | Disk-only unlock signal (not signing) |
| **Allowlists** | `dex_catalog` / `dex_routers` / `dex_lp` / `v2_lp`, `persistence` trusted dApps | Wrong router / origin → wrong spend |
| **Sentient trader** | `sentient/trader.rs` (dry-run / allowlist / breakers) | DCA execution sink |

## Scope (out)

- Docs-only / TASKS checkbox churn  
- Pure TUI copy / layout unless it hides approval or mislabels dry-run  
- Full MCP/provider surface already covered by prior audits (unless delta touches it)

---

## Trust boundaries (money path)

```
[MCP propose_dca_plan | TUI DCA n]
        → human confirms plan → dca-plans.json (0600)
                ↓
[Unlocked Sentient + mcp_auto_exec + breaker OK]
        → poll_dca_due → fire_plan → quote (agg|dex)
                ↓
SentientTrader::execute_swap_maybe_dry
        → router allowlist + breakers + eth_call
                ↓
sign + broadcast  (skipped if dry_run)
```

**Adviser / default profile:** plans may exist; **must not** auto-fire.  
**Sentient:** plan approval is the human gate; **per-slice** signing is intentional auto-exec under policy — flag any path that fires without plan consent, wrong chain/account, or off-allowlist router.

Other gates (unchanged product rules):

| Boundary | Invariant |
|---|---|
| Unlock → derive | Secrets in `SecretString` / zeroized; never in logs/errors/UI |
| Approve → sign | Default/MCP propose: full request + fresh TUI approval |
| HW confirm | Device button for tx/personal_sign; host PIN/passphrase never logged |
| Provider | Loopback + origin allowlist; session token required |

---

## Invariants to flag if broken

1. No auto-sign on **default/adviser** (or MCP propose without TUI confirm)
2. No secret material in logs, errors, flashes, tests, or git
3. Argon2id + AES-256-GCM only; password policy enforced on rotate
4. DCA: fail-closed unknown JSON; `token_out` / venue validated; router must hit DEX or agg allowlist
5. DCA fire requires: unlocked + `OperatingMode::SentientTrader` + auto-exec enabled + breaker not tripped
6. `dry_run` never signs/broadcasts
7. `change_password` verifies current password; new ≠ old; side blobs re-keyed or left unloadable under old password
8. Trezor: `track_button` only for signing/confirm flows — Initialize/GetAddress must not spoof “confirm on device” into a phishing UX, but must not skip real ButtonRequest on sign

---

## Likely hotspots (review first)

1. **Plan-file integrity** — local OS-user can rewrite `token_out` / amounts → same trust as session token; confirm fail-closed + no silent schema widen
2. **`poll_dca_due` gates** — any path that fires outside Sentient + auto-exec
3. **DEX route calldata** — `swapExactETHForTokens` recipient / path vs plan `token_out`
4. **`change_password`** — race with lock; incomplete side-blob rotation; plaintext in Settings buffers
5. **`assist_unlock_cached`** — must not be mistaken for entitlement to move funds
6. **Trusted dApp + catalog growth** — phishing / wrong-chain hosts; routers not on allowlist used by trader
7. **`trezor_personal_sign.py`** — reads `provider.session`; ensure it stays local smoke-only (no committed address/token)

---

## Human checklist before new-model run

- [ ] Stop coding the in-scope paths for this batch  
- [ ] Prefer freeze: commit WIP **or** leave dirty tree but do not keep editing mid-review  
- [ ] Note dirty highlights: DCA (untracked), `change_password`, Trezor `track_button`, assist cache, trusted dApps  
- [ ] Paste **Delta prompt** below into new Claude (Agent) — or Security Review on *uncommitted changes* with these invariants as custom instructions  

---

## Findings — delta pass (2026-09-23, uncommitted tree)

Read-only audit; **fixes applied in the same working tree** (status column).

| ID | Sev | Status | Location | Finding | Fix |
|---|---|---|---|---|---|
| D1 | P1 | **fixed** | `dca/types.rs`, `engine.rs`, `aggregator/routers.rs`, `trader.rs` | Plans store no `chain_id` / wallet; agg routers chain-agnostic; empty `eth_call` OK | Bind `chain_id`+`account`; agg 369-only; `is_allowed_agg_router_on_chain`; `eth_getCode` before sim |
| D2 | P1 | **fixed** | `dca/engine.rs` | Agg `tx.value` ≠ plan slice; breaker used plan amount | Reject mismatch; breaker uses prepared value |
| D3 | P1 | **fixed** | `dca/engine.rs` `persist_outcome` | Stale `replace_all` resurrected cancel | Reload + `update_plan`; skip if not Active |
| D4 | P1 | **fixed** | `wallet.rs` `change_password`, `StateManager::mirror_primary_to_backup` | `.bak` left under old password | Mirror primary→bak after rotate |
| D5 | P2 | **fixed** | `dca/engine.rs` | No pair quorum; `deadline=u64::MAX` | `get_v2_pair_address` + 20m deadline |
| D6 | P2 | **fixed** | `wallet.rs` `change_password` | Vault before Piteas | Re-encrypt Piteas first |
| D7 | P2 | **fixed** | `persistence.rs` | Force-merge new ETHW dApps | `seed_only_trusted_dapps` (new vaults only) |
| D8 | P2 | **fixed** | `trader.rs` `allow_agg` | Agg allowlist on all callers | DEX-only default; DCA agg path opts in |
| D9 | P2 | **fixed** | `dex_routers.rs` `venue_wrapped_native` | ETHW always WETHW | Per-venue wrap (WETH vs WETHW) |
| D10 | P3 | **partial** | `views/dca.rs` confirm | Raw wei; MCP propose not TxProposal | Confirm shows human amounts + budget; MCP queue typing deferred |

No issues found (unchanged): Trezor `track_button`, Settings password UI, `assist_unlock_cached`, `trezor_personal_sign.py`.

### Second pass — review of the D1–D10 fixes

| ID | Sev | Status | Where | Issue | Fix |
|---|---|---|---|---|---|
| S1 | P2 | **fixed** | `dca/types.rs` | Required `chain_id`/`account` made any pre-binding `dca-plans.json` fail to load (whole list bricked) | `#[serde(default)]`; `fire_plan` refuses unbound plans ("re-create") |
| S2 | P2 | **fixed** | `dca/engine.rs` `persist_outcome` | Cancel/pause during a fire dropped the broadcast slice → `spent_wei` under-counted vs chain | Log slice + spend, keep the user's status |
| S3 | P2 | **fixed** | `wallet.rs` `change_password` | Piteas re-keyed before vault; vault write failure left Piteas under `new` | Roll Piteas back to `current` on vault failure |
| S4 | P2 | **fixed** | `wallet.rs` `change_password` | `.bak` mirror error returned `Err` after the vault was already rotated (UI retries with a dead password) | Best-effort mirror; on failure delete `.bak` + warn; return `Ok` |
| S5 | P2 | **fixed** | `propose_agg_swap.rs` | Human proposal path still used Pulse-only allowlist regardless of chain → code-less target on ETHW etc. | `assert_agg_exec_targets_on_chain(proposal_chain, …)` |
| S6 | P3 | **fixed** | `trader.rs` | `getCode` ran before local breaker checks (RPC before cheap reject) | Moved after `validate_trade`, still before sim/broadcast |
| S7 | P3 | open | `dca/engine.rs` | DEX pair lookup `.ok()` silently skips reserve quorum | Accept (quorum is extra; sim + minOut still guard) |

Test fallout fixed: `send_view` exhaustive `UiJob` match (`DcaSlice`), Anvil trader tests now plant a stub contract (+ new code-less refusal test). Legacy on-disk plans using `"venue": "auto"` (pre tagged-enum) still fail closed on load.

---

## Delta prompt (copy into new Claude)

```markdown
Read-only security review of Vaughan-CLI **uncommitted / working-tree delta**.
Do not fix code unless asked. Prioritize fund loss / key leak / approval bypass.

Authoritative: CLAUDE.md security guardrails; docs/security-delta-prep.md (this
batch); docs/dca.md; docs/mcp-threat-model.md if MCP touched.

Scope: only the dirty tree — especially:
- vaughan-agent/src/dca/** and DCA MCP tools + TUI poll_dca_due / DcaSlice
- WalletState::change_password + encryption + Settings password UI
- Trezor usb/ui_bridge track_button + scripts/trezor_personal_sign.py
- assist_unlock_cached
- dex_* allowlists / catalog / trusted dApps
- SentientTrader execute_swap_maybe_dry used by DCA

Invariants (flag violations):
1. No auto-sign on default/adviser or MCP propose without TUI confirm
2. No secrets in logs/errors/UI/tests/git
3. Argon2id + AES-256-GCM; password policy on rotate; side blobs re-keyed
4. DCA auto-fire only when unlocked + SentientTrader + mcp_auto_exec + breaker OK
5. dry_run never signs/broadcasts; routers must be allowlisted
6. HW: real confirm on sign; no secret logging of PIN/passphrase

Deliverable: table P0 / P1 / P2 — columns: ID, Severity, Location (file:line),
Scenario, Impact, Suggested fix, Test idea. Omit noise. No architecture essay.
```
