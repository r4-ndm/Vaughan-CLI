---
name: lp-brew-incidents
description: Known LP Brew bugs and fixes from real 943 walkthroughs — ghost approve, stale binary, mint never queued, propose_v3_mint timeout, swapped job amounts, get_balance quirk, createPool OOG. Read when any V3 LP Brew step fails, card vanishes, or MCP times out mid-Brew.
mode: assist
kind: guide
---

# LP Brew incidents (943 wiz4rd field notes)

Catalog of **real failures** from the T1/T2 full-range 2% Brew on Pulse **943**
(`lp_50838085`, Sep 2026). Use this **with** [`workflow-recovery`](../workflow-recovery/SKILL.md)
and [`lp-gas-preflight`](../lp-gas-preflight/SKILL.md).

## Sense first (every incident)

```
get_control_plane_status
list_pending_proposals
discover_v3_pool_fee     → lifecycle for pair + fee
list_v3_positions        → mint already done?
```

On disk (`~/.local/share/vaughan-cli/`):

| Path | Check |
|------|--------|
| `proposals/pending/` | Card waiting for **y** |
| `proposals/approved/` | Step broadcast (not proof of pool/NFT) |
| `lp_deploy_jobs/lp_*.json` | `last_label`, `pending_wait`, `params.amount0/amount1` |

**Launch Vaughan from a fresh build** — not an old PATH install:

```bash
cd ~/Desktop/Vaughan-CLI && cargo run -p vaughan-cli
```

Stale `~/.local/bin/vaughan` (weeks old) misses ghost-card fixes, gas re-estimate, and
verification tables.

---

## INC-1 — createPool reverted (gas too low)

| | |
|---|---|
| **Symptoms** | User pressed **y**; maybe saw hash; **`discover_v3_pool_fee` still Missing**; ~487k gas used, status `0x0`. |
| **Cause** | Proposal `gas_limit` **500_000** — wiz4rd createPool needs **~5.9M** on-chain. |
| **Agent** | [`lp-gas-preflight`](../lp-gas-preflight/SKILL.md) — gate **≥ 6_000_000** before **y**. |
| **User** | Rebuild + restart TUI; re-propose same Brew fields once. |
| **Verify** | Pool address exists — not `approved/` file alone. |

---

## INC-2 — Ghost approve screen (blank card)

| | |
|---|---|
| **Symptoms** | After **y**, empty approve box; **Esc** / **n** do nothing; stuck on Approve screen. |
| **Cause** | Old TUI: `pending_approval` cleared but screen stayed `Approve → Approve`. |
| **Fix (user)** | **Esc** once (fixed build → dashboard); wait for next card. Rebuild if still broken. |
| **Agent** | Do not assume approval succeeded. Check `list_pending_proposals` + on-chain lifecycle. |

---

## INC-3 — Stale Vaughan binary

| | |
|---|---|
| **Symptoms** | Fixes “in repo” but behavior unchanged; ghost card; wrong gas; no verification table. |
| **Cause** | User runs `vaughan` from `~/.local/bin` (old install) instead of workspace build. |
| **Fix** | `cargo run -p vaughan-cli` or `cargo install --path vaughan-cli --locked`. Restart TUI **and** Cursor MCP server. |

---

## INC-4 — MCP JSON shows 500k gas; TUI shows higher

| | |
|---|---|
| **Symptoms** | Agent tool response `gas_limit: 500000`; user card shows **7340847 (network estimate)**. |
| **Cause** | Cursor MCP may queue stale cap; TUI **re-estimates** LP Brew steps at approve time. |
| **Agent** | For **createPool**, still enforce **≥ 6M in JSON** before telling user to approve. For later LP steps, tell user to confirm card shows **`(network estimate)`** and sensible cap (~800k+ mint). |
| **Forbidden** | “JSON says 500k so it’s fine” on createPool. |

---

## INC-5 — Mint never queued (job stuck `AfterApprove`)

| | |
|---|---|
| **Symptoms** | createPool + initialize + approve T1 + approve T2 **done**; **no mint card**; `lp_deploy_jobs/lp_*.json`: `"pending_wait": "AfterApprove"`, `"last_label": "approve token1 for LP"`. |
| **Cause** | TUI auto-advance after last approve **failed** (often preflight — see INC-7). Log: `LP Brew auto-advance failed`. |
| **Diagnose** | `list_pending_proposals` empty; `list_v3_positions` — no pair NFT yet. |
| **Recovery** | 1) Fix job `amount0`/`amount1` if swapped (INC-7). 2) Dev requeue: `cargo test -p vaughan-core --test lp_requeue_manual -- --nocapture` (edit test `job_id` first). 3) Or **`propose_v3_lp_deploy` once** with **same** human fields when lifecycle **Ready** (mint path only) — only if preflight passes. |
| **Forbidden** | Looping `propose_v3_mint` (INC-6). |

---

## INC-6 — `propose_v3_mint` MCP timeout (-32001)

| | |
|---|---|
| **Symptoms** | MCP call hangs ~120s; `Request timed out`; user may never see a card. |
| **Cause** | `propose_v3_mint` used **live propose** (blocks until TUI approve). Unlike `propose_v3_lp_deploy` (**file-queue**). |
| **Fix (code)** | Mint `ContractCall` proposals are **file-queued** in MCP dispatch; gas from network estimate. |
| **Recovery** | Use **`propose_v3_lp_deploy`** for Brew continuation when lifecycle **Ready**. Tell user to watch TUI **before** you call (file-queue appears in seconds). |
| **Forbidden** | Retry `propose_v3_mint` in a loop. |

---

## INC-7 — Swapped deposit amounts in job file

| | |
|---|---|
| **Symptoms** | `propose_v3_lp_deploy` fails **`insufficient token1 balance`** despite user having 300 T2; auto-advance fails after approves. |
| **Cause** | `lp_deploy_jobs/*.json` has **deposit in wrong leg**, e.g. `amount0: "300"`, `amount1: "90.36"` when user deposited **300 T2** (T2 = token1). |
| **Fix (code)** | Jobs store `deposit_on_token0`; **`lp_deploy_fixup_swapped_amounts`** on load corrects swapped legs. |
| **Rule** | `amount0` / `amount1` are **sorted token0/token1**, not user token_a/token_b labels. |
| **Check** | `cat ~/.local/share/vaughan-cli/lp_deploy_jobs/lp_*.json` |
| **Recovery** | Rebuild Vaughan — auto-advance retries mint after approve. Else re-propose once (INC-5). |

---

## INC-8 — `get_balance` returns native tPLS for ERC-20

| | |
|---|---|
| **Symptoms** | `get_balance` with token address shows **tPLS** balance, not ERC-20. |
| **Workaround** | Use **`list_assets`** for session wallet token balances ([`balance-preflight`](../balance-preflight/SKILL.md)). |
| **Fix (code)** | `get_balance` accepts **`token`** alias for `token_address` (common agent mistake). |
| **Forbidden** | Declaring “user has 0 T2” from wrong `get_balance` read. |

---

## INC-9 — Approval card “disappeared”

| | |
|---|---|
| **Symptoms** | User saw card briefly or not at all; then dashboard; mint not done. |
| **Causes** | (a) MCP timeout cleared agent side but never queued (INC-6). (b) Auto-advance failed (INC-5). (c) User approved — check INC-10 success path. |
| **Agent** | `list_pending_proposals` → if empty, branch on lifecycle + job file (INC-5/7), not “user denied”. |

---

## INC-10 — Successful mint (what “executed” should mean)

| | |
|---|---|
| **Symptoms** | Flash: “Queued proposal executed” / back on dashboard. |
| **Verify** | `list_v3_positions` — NFT for pair, full range ticks, non-zero liquidity. |
| **TUI** | Fixed builds show **verification table** on approve (Pool, Range, Deposit T1/T2) — user confirms table + **`Gas: … (network estimate)`**. After **mint**, a **success table** flashes under the address (pair, deposits, NFT #, tx). |
| **Forbidden** | Claiming Brew done from tx hash alone. |

---

## INC-11 — Fee line “unavailable” on MCP card

| | |
|---|---|
| **Symptoms** | Approve card: `Fee: unavailable` but `Gas: 868587 (network estimate)`. |
| **Cause** | LP Brew steps skip blocking fee RPC before render; gas is re-estimated at broadcast. |
| **User** | OK to approve if verification table + gas estimate look right. |

---

## Recovery decision tree

```
list_pending_proposals non-empty?
  yes → user: TUI dashboard, Advisor, MCP on, press y
  no  → discover_v3_pool_fee
          Missing        → propose_v3_lp_deploy (check gas ≥6M) — INC-1
          Uninitialized  → propose_v3_lp_deploy — skip create
          Ready          → list_v3_positions has NFT?
                              yes → DONE (INC-10)
                              no  → check job amounts (INC-7)
                                    → propose_v3_lp_deploy once OR dev requeue (INC-5)
                                    → NOT propose_v3_mint loop (INC-6)
```

---

## Agent checklist (full new pool Brew)

1. Session + tokens + [`balance-preflight`](../balance-preflight/SKILL.md) (`list_assets`, not bad `get_balance`).
2. [`lp-gas-preflight`](../lp-gas-preflight/SKILL.md) before createPool **y**.
3. One **`propose_v3_lp_deploy`** per step wave — wait for user + verify on-chain between waves.
4. After each **y**: lifecycle / positions check — not hash alone.
5. Card missing / timeout → this skill + [`workflow-recovery`](../workflow-recovery/SKILL.md).
6. User on **fresh binary** (INC-3).

---

## Related

- Gas: [`lp-gas-preflight/SKILL.md`](../lp-gas-preflight/SKILL.md)
- General recovery: [`workflow-recovery/SKILL.md`](../workflow-recovery/SKILL.md)
- Brew flow: [`vaughan-brews/SKILL.md`](../vaughan-brews/SKILL.md)
- Short recovery: [`vaughan-brews/brew-recovery.md`](../vaughan-brews/brew-recovery.md)
- Balances: [`balance-preflight/SKILL.md`](../balance-preflight/SKILL.md)
- MCP: [`mcp-connect/SKILL.md`](../mcp-connect/SKILL.md)
