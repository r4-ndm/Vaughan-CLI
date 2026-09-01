---
name: workflow-recovery
description: Recover failed or stuck MCP workflows — pending proposals, LP Brew jobs, ghost approve, mint never queued, MCP timeouts, mid-session crashes, missing TUI approval cards.
mode: assist
kind: guide
---

# Workflow recovery (Advisor)

Use when a Brew/script **crashes mid-session**, the user **does not see an approval
card**, a proposal **failed on Y**, or the user starts a **new chat** and wants to
continue the same LP / transaction.

## Sense first (every recovery)

```
get_control_plane_status   → unlocked, ready_for_writes, MCP on
list_pending_proposals     → anything waiting in file queue?
discover_v3_pool_fee       → LP: which steps already on-chain? (lifecycle)
list_v3_positions          → LP: NFT already minted?
```

On-disk state (same profile as MCP, usually `~/.local/share/vaughan-cli/`):

| Path | Meaning |
|------|---------|
| `proposals/pending/` | Awaiting TUI **y** |
| `proposals/approved/` | Broadcast succeeded |
| `proposals/rejected/` | User denied or agent error |
| `lp_deploy_jobs/lp_*.json` | In-flight LP Brew job (`status`: active/done/failed) |

Proposals **expire after ~10 minutes** if not approved — file removed; job may remain.

## Missing approval card

Tell the user:

1. **Vaughan TUI focused** — unlock, **Human → Advisor**, **`· MCP on`** in status bar.
2. **Dashboard** (Esc) — wait 5–10s for file-queue poll; **stay on the Vaughan terminal tab**
   while approving (switching away is OK, but the card reappears after a dismiss — rebuild
   if you were on an older binary that auto-rejected at 60s).
3. **Quit and reopen Vaughan** (unlock again) — clears stuck “already shown” state.
4. Agent: `list_pending_proposals` — if **empty**, re-propose (below); if **pending**, user
   should see card after TUI restart.

Do **not** spam `propose_*` while an id is still `pending_user`.

## LP Brew recovery (wiz4rd V3)

### A — Pending proposal exists

`list_pending_proposals` shows e.g. `lp-lp_70002268-createPool`:

> Your Brew step is queued. Open Vaughan TUI and press **y** on the MCP approval card.
> If you don't see it, restart the TUI while staying unlocked.

### B — No pending proposal, pool not finished

1. `discover_v3_pool_fee` + `get_v3_pool` → map lifecycle:
   | Lifecycle | Next step |
   |-----------|-----------|
   | Missing | createPool → … |
   | Uninitialized | initialize → approve → mint |
   | Ready | approve → mint only |
2. Confirm **same** session (wallet + chain) — [`wallet-account`](../wallet-account/SKILL.md).
3. **`propose_v3_lp_deploy` once** with the **same** human fields (tokens, price, deposit,
   fee). Orchestrator is **lifecycle-aware** — skips steps already on-chain.
4. User approves remaining cards in TUI.

Do **not** call separate `propose_v3_create_pool` / `_initialize_pool` for a full Brew
unless user explicitly wants the escape hatch.

### C — User pressed Y but saw “failed” / “query failed”

- Proposal often **still pending** or was **never broadcast** — check `list_pending_proposals`
  and on-chain lifecycle.
- Ask user to **rebuild Vaughan** if they were told a fee/approve fix landed, then **restart
  TUI** and press **y** again.
- If pending is empty and pool unchanged → **re-propose** (B).

### C2 — createPool reverted (out of gas)

Symptoms: user pressed **y**, maybe saw a tx hash, **`discover_v3_pool_fee` still no pool**,
`gas_limit` in approved proposal was **500_000** or **< 6_000_000**.

Read [`lp-gas-preflight/SKILL.md`](../lp-gas-preflight/SKILL.md) — full gate + recovery.

Short path:

1. Confirm pool still **Missing** on-chain.
2. User **rebuilds + restarts Vaughan** (required if gas cap was wrong).
3. **`propose_v3_lp_deploy` once** (same human fields) → verify response `gas_limit` **≥ 6M**.
4. User **y** → **`discover_v3_pool_fee`** must show pool before claiming success.

Do **not** treat `proposals/approved/` or a broadcast hash as a created pool.

### D — Brew complete

`list_v3_positions` shows T1/T2 NFT → tell user done; optional `lp_deploy_jobs` entry may
still say `active` — on-chain state wins.

### E — Ghost approve / blank card after Y

Symptoms: empty approve screen, **Esc**/**n** dead, user still on Approve view.

- User on **stale binary** → [`lp-brew-incidents` INC-3](../lp-brew-incidents/SKILL.md).
- Fixed build: **Esc** → dashboard; wait for next MCP card.
- Agent: do **not** assume step succeeded — check lifecycle + `list_pending_proposals`.

Full write-up: [`lp-brew-incidents/SKILL.md`](../lp-brew-incidents/SKILL.md) INC-2.

### F — Approves done, mint card never appears

Symptoms: createPool + initialize + approve T1/T2 on-chain; `list_pending_proposals` empty;
`lp_deploy_jobs/*.json` stuck `"pending_wait": "AfterApprove"`.

1. `list_v3_positions` — NFT already minted? → done.
2. **Fixed in current build:** TUI auto-advance retries after approve with deposit-leg fixup
   (`lp_deploy_retry_after_approve`). Rebuild Vaughan if on an old binary.
3. Inspect job `params.amount0` / `amount1` — swapped legs auto-correct when
   `deposit_on_token0` is set (see INC-7).
4. Fallback: **`propose_v3_lp_deploy` once** (same human fields, lifecycle **Ready**).
5. Dev-only requeue: `lp_requeue_manual` test after fixing job file.

### G — `propose_v3_mint` MCP timeout

**Fixed in current build:** standalone mint proposals are **file-queued** (same as LP Brew
steps) — MCP returns immediately; card appears via TUI poll.

Do **not** retry in a loop. Prefer **`propose_v3_lp_deploy`** for Brew continuation.

## Failed non-LP proposals

```
get_proposal_status { proposal_id }
```

| Status | Action |
|--------|--------|
| `pending_user` | TUI approval / restart TUI |
| `rejected` | Ask why; fix params; new `propose_*` |
| `approved` | Done — verify tx in TUI history / explorer |
| `unknown` / expired | New `propose_*` with corrected params |

Never re-propose with **different** calldata without telling the user the prior draft is void.

## New agent session (user returns later)

Opening line:

> I'll check your wallet and any pending Vaughan proposals, then see what's already on-chain
> for your pool.

Run **Sense first**, then branch A–D. Restate what's left:

> createPool done, still need initialize + mint — I'll queue the next Brew step.

## Forbidden

- Claiming a tx succeeded without `approved` status or explorer confirmation.
- Abandoning an `active` LP job without checking lifecycle.
- Mainnet recovery without explicit user intent.

## Related

- MCP connect: [`mcp-connect/SKILL.md`](../mcp-connect/SKILL.md)
- LP Brews: [`vaughan-brews/SKILL.md`](../vaughan-brews/SKILL.md)
- **Field incident catalog:** [`lp-brew-incidents/SKILL.md`](../lp-brew-incidents/SKILL.md)
- createPool gas / OOG: [`lp-gas-preflight/SKILL.md`](../lp-gas-preflight/SKILL.md)
- Session wallet/chain: [`wallet-account/SKILL.md`](../wallet-account/SKILL.md)
