---
name: balance-preflight
description: Check active-wallet token balances before propose_* — compare deposit legs, warn on shortfall (e.g. 150 JANE vs 200 needed).
mode: assist
kind: guide
---

# Balance preflight (Advisor)

Before any `propose_*` that spends tokens, verify the **session wallet** can cover the
amounts. Catch shortfalls in chat — do not queue a proposal that will fail simulation.

Especially important for **LP Brews** where one-sided deposits imply a **second leg**
computed from price.

## Tools

| Tool | Use |
|------|-----|
| `list_assets` | **Preferred** — `formatted` balance + `contract` for tokens in wallet UI |
| `get_balance` | `{ token_address: "0x…" }` — native PLS only reliable; **ERC-20 may wrongly show tPLS on 943** (use `list_assets` instead) |
| `resolve_token` | Decimals to convert `balance_raw` → human amount |
| `get_v3_pool` | Live ratio when adding to an **existing** pool (Ready) |

Omit `account_address` on `get_balance` — uses the connected session wallet.

**Known bug (943):** `get_balance` with an ERC-20 address can return **native tPLS**
instead of the token. Always use **`list_assets`** for LP deposit preflight — see
[`lp-brew-incidents` INC-8](../lp-brew-incidents/SKILL.md).

## Compute what you need

| User intent | Token A need | Token B need |
|-------------|--------------|--------------|
| **100 BOB** only, price 1 BOB = 2 JANE (new pool) | 100 BOB | **200 JANE** (computed) |
| **100 BOB and 200 JANE** explicit | 100 BOB | 200 JANE |
| **500 BOB** only, **existing** Ready pool | 500 BOB | computed from **`get_v3_pool`** price |
| Native PLS for gas | Keep ~buffer for approve/mint txs (mention if PLS very low) |

Round computed legs **up** slightly when warning — exact mint math is on-chain; preflight
is conservative UX, not exact tick math.

## Check each leg

For each required `(symbol, human_amount, contract)`:

1. Try `list_assets` — find matching `contract`, compare `formatted` to need.
2. Only if token missing from assets: `resolve_token` + warn user to import token — avoid
   `get_balance` for ERC-20 on 943 (returns wrong asset — INC-8).

## Warn & stop (no propose)

Template:

> **Insufficient {SYMBOL}:** you need **~{need} {SYMBOL}** for this LP deposit but this
> wallet has **~{have} {SYMBOL}** (`{account_label}` on chain {chain_id}).
>
> Options: lower the deposit, transfer more {SYMBOL} to this wallet, or switch account
> (**F3**). Tell me the new amount when ready.

Do **not** call `propose_v3_lp_deploy` until user adjusts or you re-check and balances
cover both legs.

## Example (BOB / JANE)

```
User:  100 BOB and 200 JANE, full range, 2% fee.

Agent: [tokens confirmed, price/fee OK]
       [list_assets / get_balance]
       BOB: have 500 · need 100 ✓
       JANE: have ~150 · need 200 ✗

       You only have **~150 JANE** but this deposit needs **200 JANE**. Lower JANE side,
       add JANE to the wallet, or switch wallet. What would you like to do?
```

One-sided variant:

```
User:  100 BOB only, 1 BOB = 2 JANE.

Agent: That implies **~200 JANE** as the other leg.
       [balance check → 150 JANE]
       Short **~50 JANE** — adjust before I propose.
```

## LP Brew placement

Run after deposit Q, before Q6 summary — see [`vaughan-brews/balance-preflight.md`](../vaughan-brews/balance-preflight.md).

## Related

- Session wallet: [`wallet-account/SKILL.md`](../wallet-account/SKILL.md)
- Token contracts: [`token-resolve/SKILL.md`](../token-resolve/SKILL.md)
