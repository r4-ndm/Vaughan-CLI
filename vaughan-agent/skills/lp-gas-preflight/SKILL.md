---
name: lp-gas-preflight
description: Mandatory gas checks before and after V3 LP Brew propose_v3_lp_deploy — especially createPool on wiz4rd 943 (~6M+ gas). Use whenever deploying a new V3 pool, createPool reverted, pool still missing after Y, gas_limit looks low, or simulate_call succeeded but on-chain tx failed. Also read before telling the user to press y on createPool.
mode: assist
kind: guide
---

# LP gas preflight (Advisor)

`createPool` on Pulse wiz4rd **943** deploys a full Pancake-style pool contract. It needs
**~5.9M gas** on-chain — **not** the old 500k default. Proposals with too little gas
**broadcast but revert**; the TUI may flash a hash while the pool never appears.

Run this skill **after balance preflight** and **before** you tell the user to approve
`createPool`.

## When to use

| Situation | Read this |
|-----------|-----------|
| New pool (`discover_v3_pool_fee` → no pool / Missing) | **Before** `propose_v3_lp_deploy` |
| Right after `propose_v3_lp_deploy` returns | Validate `gas_limit` in the tool JSON |
| User approved **Y** but pool still missing | Post-approve verify + recovery |
| `simulate_call` OK but mined tx reverted | Likely gas — re-check limits |
| Recovery after stuck Brew | With [`workflow-recovery`](../workflow-recovery/SKILL.md) |

## Gas floors (wiz4rd 943, LP Brew steps)

| Step | Selector | Minimum `gas_limit` in proposal | Notes |
|------|----------|----------------------------------|-------|
| **createPool** | `0xa1671295` | **≥ 6_000_000** | ~5.87M estimate + headroom; **never 500_000** |
| initialize | `0xf637731d` | ≥ 400_000 | Pool `initialize(sqrtPriceX96)` |
| approve token* | `0x095ea7b3` | ≥ 100_000 | ERC-20 → NPM |
| add liquidity (mint) | `0x88316456` | ≥ 800_000 | NPM `mint` |

Other chains/venues: still run `simulate_call` + inspect proposal `gas_limit`; treat
**createPool** as **multi-million gas** on every Pancake-style V3 factory.

## Pre-propose (lifecycle Missing)

1. **Sense:** `discover_v3_pool_fee` — confirm pool does **not** exist yet.
2. **Balances:** [`balance-preflight`](../balance-preflight/SKILL.md) (both legs + PLS for gas).
3. **PLS buffer:** createPool alone can cost **~0.02–0.05 PLS** in fees on 943 at normal
   gas prices — warn if native balance is near zero (separate from token legs).
4. **Optional sanity:** `simulate_call` to factory with createPool calldata — catches bad
   tokens/fee tier, but **does not replace gas_limit checks** (eth_call gas ≠ tx gas cap).
5. **`propose_v3_lp_deploy` once** — then immediately go to **Post-propose gate**.

## Post-propose gate (mandatory)

Parse the tool response JSON:

```json
{
  "proposal": { "gas_limit": 7340847, "proposal_id": "lp-lp_…-createPool", … },
  "step": "createPool",
  "job_id": "lp_…"
}
```

| Check | If fail |
|-------|---------|
| `step` is `createPool` and `gas_limit` **≥ 6_000_000** | **Stop.** Tell user **not** to press **y** yet. |
| `gas_limit` **< 6_000_000** for createPool | Vaughan build is stale or estimate failed — user must **`cargo run -p vaughan-cli`** (rebuild) and you **re-propose** same human fields. |
| `simulation_success` false (if present) | Do not approve — fix params first. |

**User message when gas is too low:**

> This createPool draft only allows **{gas_limit}** gas but wiz4rd needs **~6M+**. The tx
> would revert after you press **y**. Rebuild/restart Vaughan, then I'll queue a fresh Brew
> step — don't approve the old card.

Only after the gate passes → tell user to approve in TUI.

## Post-approve verify (createPool)

After user says they pressed **y** (or you see `list_pending_proposals` empty for that id):

```
discover_v3_pool_fee   → pool should exist for pair + fee
get_v3_pool            → confirm non-zero address / initialized next
```

| Outcome | Meaning |
|---------|---------|
| Pool **found** | createPool succeeded — initialize card should follow (or auto-enqueued). |
| Pool **still missing** | Tx likely **reverted** (classic symptom: ~487k gas used, status `0x0`). |

Do **not** claim success from a tx hash alone. On-chain pool address is the source of truth.

## Recovery — reverted createPool (OOG)

Symptoms:

- User pressed **y**, maybe saw a hash flash, **no** initialize card / Brew stuck.
- `discover_v3_pool_fee` still **no pool**.
- `lp_deploy_jobs/lp_*.json`: `"last_label": "createPool"`, `"pending_wait": "AfterCreatePool"`.
- `proposals/approved/lp-*-createPool.json` may exist while pool does not.

Steps:

1. `get_control_plane_status` — Advisor, MCP on, `ready_for_writes`.
2. `discover_v3_pool_fee` — confirm still Missing.
3. Tell user: prior createPool **reverted** (insufficient gas cap), pool was **not** created.
4. User **rebuilds + restarts Vaughan** (required once if they were on a pre-fix binary).
5. **`propose_v3_lp_deploy` once** with **same** human fields — orchestrator skips done steps.
6. **Post-propose gate** — `gas_limit` ≥ 6M before **y**.
7. User approves; **post-approve verify** pool exists.

Do **not** call standalone `propose_v3_create_pool` for a full Brew unless user explicitly
wants the escape hatch.

Full card/session recovery: [`workflow-recovery/SKILL.md`](../workflow-recovery/SKILL.md).

## Forbidden

- Telling user to press **y** on createPool when proposal `gas_limit` **< 6_000_000**.
- Assuming `simulate_call` success means the mined tx will succeed.
- Claiming the pool exists because a proposal moved to `approved/` or a hash was shown.
- Re-proposing createPool while the same id is still `pending_user`.

## Related

- Balances: [`balance-preflight/SKILL.md`](../balance-preflight/SKILL.md)
- Brew flow: [`vaughan-brews/SKILL.md`](../vaughan-brews/SKILL.md)
- Stuck cards / sessions: [`workflow-recovery/SKILL.md`](../workflow-recovery/SKILL.md)
- **All LP Brew incidents:** [`lp-brew-incidents/SKILL.md`](../lp-brew-incidents/SKILL.md)
- New pool checklist: [`vaughan-brews/new-v3-pool-943.md`](../vaughan-brews/new-v3-pool-943.md)
