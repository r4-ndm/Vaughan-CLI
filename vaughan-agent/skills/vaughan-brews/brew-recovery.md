# LP Brew — recovery (short)

Full guides:

- **Incident catalog (943 walkthrough bugs):** [`lp-brew-incidents/SKILL.md`](../lp-brew-incidents/SKILL.md)
- **General recovery:** [`workflow-recovery/SKILL.md`](../workflow-recovery/SKILL.md)
- **createPool gas:** [`lp-gas-preflight/SKILL.md`](../lp-gas-preflight/SKILL.md)

## Quick fixes (most common)

| Problem | First move |
|---------|------------|
| No approval card | `list_pending_proposals` → user TUI Advisor + **MCP on** → restart TUI |
| Card vanished / MCP timeout | **Do not** loop `propose_v3_mint` — see [`lp-brew-incidents` INC-5/6/9](../lp-brew-incidents/SKILL.md) |
| createPool reverted | Pool still Missing → gas was &lt; 6M → rebuild Vaughan → re-propose |
| Approves done, no mint | Check `lp_deploy_jobs` amounts (INC-7) → `propose_v3_lp_deploy` once (Ready) |
| Wrong balances in chat | Use **`list_assets`**, not `get_balance` for ERC-20 (INC-8) |
| Old behavior after “fix” | User on stale `~/.local/bin/vaughan` → `cargo run -p vaughan-cli` (INC-3) |

## No approval card

1. `list_pending_proposals`
2. User: TUI unlocked, Advisor, **`· MCP on`**, dashboard, wait or **restart TUI**
3. If pending **empty** → `discover_v3_pool_fee` → **`propose_v3_lp_deploy` again** (same human fields)

## Mid-Brew / new chat

```
discover_v3_pool_fee → lifecycle
list_pending_proposals
list_v3_positions
```

Re-propose **once** with original human fields; skipped steps are inferred from chain.

## After failed Y or blank card

Check pending + lifecycle; **rebuild + restart TUI** if on old binary; see
[`lp-brew-incidents`](../lp-brew-incidents/SKILL.md) for ghost approve (INC-2).

## Brew complete?

`list_v3_positions` shows pair NFT with liquidity → done. Job file may still say `active` —
on-chain wins.

## createPool reverted (pool still missing)

Proposal `gas_limit` must be **≥ 6_000_000** before **y**; after **y**, `discover_v3_pool_fee`
must show the pool. Details: [`lp-gas-preflight/SKILL.md`](../lp-gas-preflight/SKILL.md).
