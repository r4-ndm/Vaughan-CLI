# Existing V3 pool — fixed ratio (add liquidity only)

Use when the user wants to **create** or **add** LP on wiz4rd V3, but a pool for their
pair **already exists on-chain**. The agent must **detect this early** and explain that
the **price ratio is fixed** — it cannot be changed through Brew or mint.

Advisor mode only. Read with [`conversational-brew.md`](conversational-brew.md).

## When to run detection

As soon as both tokens are known (after conversational **Q1**, or at the start of direct mode):

```
resolve_token(token_a)
resolve_token(token_b)
discover_v3_pool_fee { token_a, token_b, venue: wiz4rd }
```

Optional detail for user-facing price:

```
get_v3_pool { token_a, token_b, fee: <discovered fee> }
```

## Lifecycle → what to tell the user

| `discover_v3_pool_fee` | Meaning | User can set price? | Brew steps |
|------------------------|---------|----------------------|------------|
| `lifecycle: null`, `fee: null` | No pool at any catalog tier | **Yes** — Q2 starting price | Full pipeline (~5–6 TUI approvals) |
| `lifecycle: "Uninitialized { … }"` | Pool contract exists, price not set | **Once** — initialize sets ratio | initialize → approve → mint |
| `lifecycle: "Ready"` | Pool live with `slot0` price | **No — ratio is fixed** | approve both tokens → mint (~3 TUI steps) |

### Ready pool — mandatory user notice

After `get_v3_pool`, tell the user plainly:

> **This pool already exists** on wiz4rd V3 (fee **X%**, pool `0x…`).
> The **exchange ratio is fixed** by the current on-chain price — you **cannot**
> choose a new starting price or change the ratio. Adding liquidity only deposits
> both tokens at the **existing** ratio (if you specify one side, the other is computed).

Show the live ratio in **their token names** (map token0/token1 from the tool):

> Current price: **1 TOKEN_A ≈ N TOKEN_B** (from pool `slot0`).

**Do not** ask conversational Q2 (“starting price”) or Q4 (“fee tier for new pool”) when
`lifecycle` is **Ready** — use discovered `fee` and on-chain price.

### Wrong fee tier

If the user asks for a fee that has **no** pool but another tier is **Ready**:

> A **Y%** pool already exists for this pair. V3 does not let you “move” liquidity to
> another fee tier — you add to the existing pool at **Y%**, or create a **separate**
> new pool at your chosen fee (different NFT, different address).

Run `discover_v3_pool_fee` again or check each tier before promising a new-pool Brew.

## Conversational branch (after Q1)

```
User:  BOB and JANE on wiz4rd.

Agent: [resolve_token ×2, discover_v3_pool_fee → fee 20000, lifecycle Ready]
       [get_v3_pool → price_token1_per_token0_approx]

       This **BOB/JANE pool already exists** at **2%** fee. The ratio is **fixed**
       on-chain — you can't set a new starting price.

       Current price: **1 BOB ≈ 0.42 JANE** (live pool price).

       How much liquidity do you want to add, and in **BOB or JANE**?
       (The other token amount is computed at the fixed ratio.)

User:  500 BOB, full range.

Agent: Summary — add liquidity only (no create/initialize):
       • Pair: BOB / JANE @ 2% (existing pool)
       • Deposit: 500 BOB (+ JANE computed at pool price)
       • Range: full
       • ~3 steps in Vaughan TUI (approve both tokens + mint)
       Proceed?

User:  Yes

Agent: [propose_v3_lp_deploy once — omit custom price intent; use deposit + discovered fee]
```

## `propose_v3_lp_deploy` fields (existing Ready pool)

| Field | Rule |
|-------|------|
| `token_a`, `token_b` | User’s names/addresses |
| `fee` | From `discover_v3_pool_fee` — **required**, do not guess |
| `deposit`, `deposit_token` | User anchor amount |
| `price` | Still passed for Brew encoding, but orchestrator **ignores** user price when lifecycle is Ready — amounts come from live `slot0` |
| `range` | `"full"` unless user gives min/max |

Tell the user: **your stated price is ignored** if they mentioned one — only the pool price applies.

## Forbidden (existing Ready pool)

- Promising “we’ll set 1 A = 2 B” when the pool already trades at another ratio.
- Asking fee tier “for this new pool” when discovery returned Ready.
- Calling `propose_v3_create_pool` or `propose_v3_initialize_pool` for a Ready pool.
- Skipping the fixed-ratio warning before `propose_v3_lp_deploy`.

## Sense checklist (existing pool)

```
get_control_plane_status  → wallet_unlocked
get_network               → chain_id 943
resolve_token             → both addresses
discover_v3_pool_fee      → fee + lifecycle
get_v3_pool               → live price (Ready only)
list_v3_positions         → optional: show user’s existing NFTs for this pair
```

## Related

- New pool dialogue: [`conversational-brew.md`](conversational-brew.md)
- Checklist: [`new-v3-pool-943.md`](new-v3-pool-943.md)
