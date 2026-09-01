# LP Brew — gas preflight

Full guide: [`lp-gas-preflight/SKILL.md`](../lp-gas-preflight/SKILL.md).

## When

- **Before** `propose_v3_lp_deploy` when `discover_v3_pool_fee` shows **no pool** (createPool step).
- **Immediately after** propose returns — read `proposal.gas_limit` in JSON.
- **After** user presses **y** on createPool — `discover_v3_pool_fee` must show the pool.

## createPool rule (943 wiz4rd)

| | |
|---|---|
| **Need** | `gas_limit` **≥ 6_000_000** in the proposal |
| **Bad** | `500_000` (reverts on-chain; pool never created) |
| **Fix** | User rebuilds Vaughan → agent re-proposes same Brew fields → verify gas → then **y** |

## Quick gate after propose

```
step == "createPool"  AND  gas_limit >= 6_000_000  →  OK, user may approve
else  →  stop, rebuild + re-propose
```

## After approve

```
discover_v3_pool_fee  →  pool must exist
```

Still missing → reverted createPool → [`brew-recovery.md`](brew-recovery.md) + gas skill recovery section.
