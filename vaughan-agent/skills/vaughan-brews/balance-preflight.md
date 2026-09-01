# LP Brew — balance preflight

Full patterns: [`balance-preflight/SKILL.md`](../balance-preflight/SKILL.md).

## When

After deposit amount(s) are known and **both token contracts are confirmed** — **before**
the final summary and **before** `propose_v3_lp_deploy`.

## Steps

1. Compute **both legs** the mint will need:
   - **One-sided deposit** (e.g. 100 BOB): use user's **price** (new pool) or **`get_v3_pool`**
     price (existing Ready pool) to derive the other token amount.
   - **Both sides stated** (e.g. 100 BOB + 200 JANE): use those numbers directly.
2. Check balances on the **confirmed session wallet**:
   - **`list_assets`** (preferred — human `formatted` amounts)
   - Avoid `get_balance` for ERC-20 on 943 — can return tPLS wrongly ([`lp-brew-incidents` INC-8](../lp-brew-incidents/SKILL.md))
3. If either leg **short** → warn and stop (no propose):

   > You asked for **200 JANE** but this wallet only has **~150 JANE**. Lower the deposit,
   > add tokens, or switch wallet (**F3**).

4. User adjusts amount or fixes balance → re-run check → then summary → propose.

## Never

Call `propose_v3_lp_deploy` when balances are clearly insufficient (on-chain preflight
will reject anyway — catch it in chat first).
