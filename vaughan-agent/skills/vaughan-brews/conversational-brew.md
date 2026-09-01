# Conversational Brew — wiz4rd V3 LP dialogue

Use when the user speaks casually (“create a liquidity pool on wiz4rd”) instead of
dumping all parameters at once. **Ask questions → map answers → confirm → one tool call.**

Advisor mode only. User approves in Vaughan TUI after `propose_v3_lp_deploy`.

## Opening intent (user)

Examples that start this flow:

- *“I want to create a liquidity pool on V3 wiz4rd-swap.”*
- *“Add LP on Wizard swap for my two tokens.”*
- *“Create a wiz4rd pool for TEST1 and TEST2.”*

## Agent reply (first turn)

Do **not** call write tools yet. Reply with a short intro + **question 1**:

> I can set up a full wiz4rd V3 pool on Pulse testnet (943): create pool → set
> starting price → approve → mint. You’ll approve each step in the Vaughan wallet
> (about 5–6 confirms for a brand-new pair).
>
> **Which two tokens?** Names/symbols (e.g. TEST1, TEST2) or paste both `0x` addresses.

Prerequisites (mention once if `get_control_plane_status` fails):

- Vaughan TUI unlocked, **Human → Advisor**, network **943**, status **`· MCP on`**.

**Wallet & chain (before tokens):** confirm F1 network + F3 account — see [`confirm-session.md`](confirm-session.md)
/ [`wallet-account`](../wallet-account/SKILL.md). Combined prompt: *“You are on PulseChain
testnet 943, wallet 0 — is this the chain and wallet you want to deploy from?”* If **no**,
user switches **F1** (network) and/or **F3** (wallet); re-check before continuing.

## Question script (in order)

Ask **one question at a time** unless the user already answered it. Skip questions
already clear from context.

| # | Question | Maps to | Notes |
|---|----------|---------|--------|
| 1 | Which two tokens? | `token_a`, `token_b` | [`token-resolve`](../token-resolve/SKILL.md) — confirm each `0x` before continuing |
| 2 | Starting price? *“1 TOKEN_A = ? TOKEN_B”* | `price` | **Skip if pool Ready** — ratio fixed; see [`existing-v3-pool.md`](existing-v3-pool.md). See **Price rule** below if user states price inverted. |
| 3 | How much liquidity — amount and which token? | `deposit`, `deposit_token` | One-sided OK; other side computed at pool or chosen price |
| 3b | *(balance preflight)* | — | [`balance-preflight.md`](balance-preflight.md) — both legs vs wallet; stop if short |
| 4 | Fee tier for this **new** pool? | `fee` (bps) | **Skip if discovery found Ready** — use discovered fee |
| 5 | Range — full range or min/max prices? | `range` | Default **full** if user says “full” / “max” |
| 6 | *(summary)* — OK to propose? | `explanation` | User must confirm before write |

### After Q1 — pool detection (mandatory)

```
discover_v3_pool_fee { token_a, token_b }
```

| Result | Next question | Agent must say |
|--------|---------------|----------------|
| **Missing** (no fee) | Q2 starting price | “New pool — you pick the starting ratio.” |
| **Uninitialized** | Q2 starting price (initialize) | “Pool exists but price not set yet — one-time initialize sets ratio.” |
| **Ready** | **Skip Q2 & Q4** → Q3 deposit | **“Pool already exists — ratio is fixed; you can only add liquidity at the current price.”** + `get_v3_pool` for live ratio |

Full script: [`existing-v3-pool.md`](existing-v3-pool.md).

### Fee tier table (show when asking Q4)

| User says | `fee` (bps) |
|-----------|-------------|
| 0.01% | 100 |
| 0.05% | 500 |
| 0.25% | 2500 |
| 1% | 10000 |
| 2% | 20000 |

For a **new pair**, never default to 500 without asking.

### Price rule (`price` = token_b per token_a)

Always set `token_a` / `token_b` to the **names the user used** (before on-chain sort).
`price` is **token_b per 1 token_a** — not “whichever way the user phrased it.”

| User says (token_a = first name, token_b = second) | `price` field |
|----------------------------------------------------|---------------|
| 1 TEST1 = 2 TEST2 | `"2"` |
| 1 TEST2 = 0.3 TEST1 *(inverted)* | **not** `"0.3"` — invert: 1 TEST1 = 3.333… TEST2 → `"3.333333333333"` |
| “3.33 T2 per T1” with T1 first | `"3.333333333333"` |

Restate the ratio in the **Q6 summary** in the user’s words; pass the converted value in
`propose_v3_lp_deploy`. Wrong `price` → wrong mint leg and preflight failures (INC-7).

## Example dialogue (TEST1 / TEST2)

```
User:  I want to create a liquidity pool on V3 wiz4rd-swap.

Agent: [get_control_plane_status + get_network]

       You are on **PulseChain Testnet V4 (943)**, **wallet 0** (`0x…`).
       Is this the chain and wallet you want to deploy from?

User:  Yes.

Agent: Which two tokens? Symbols or 0x addresses.
```

User:  TEST1 and TEST2 — I just launched them on 943.

Agent: [token-resolve: paste + confirm TEST1, TEST2 contracts]
       Starting price: how many TEST2 per 1 TEST1?

User:  1 TEST1 = 2 TEST2

Agent: How much liquidity to add, and in which token?

User:  1000 TEST1

Agent: [balance preflight: ~2000 TEST2 implied — check balances]
       [if short → warn; if OK → fee question]

User:  2%

Agent: Full range OK?

User:  Yes

Agent: Summary — please confirm:
       • Pair: TEST1 / TEST2
       • Price: 1 TEST1 = 2 TEST2
       • Deposit: 1000 TEST1 (TEST2 side computed)
       • Fee: 2% (20000 bps), full range
       • Network: wiz4rd 943
       • You approve ~5–6 steps in Vaughan TUI
       Proceed?

User:  Yes

Agent: [discover_v3_pool_fee → missing]
       [propose_v3_lp_deploy once — stop]
       [gate: first step createPool → proposal gas_limit ≥ 6_000_000 — else rebuild + re-propose]

       First step is queued — open Vaughan (fresh build: `cargo run -p vaughan-cli`).
       On each card: read the **verification table** (pair, range, deposit legs) and
       **Gas: … (network estimate)** — then **y**. ~5 steps for a new pool.
       If anything fails → [`lp-brew-incidents`](../lp-brew-incidents/SKILL.md).
```

### Inverted price (T1 / T2 — real 943 case)

User names tokens **T1** then **T2**, but states *“1 T2 = 0.3 T1”*:

```
token_a: T1 address
token_b: T2 address
price: "3.333333333333"    ← T2 per 1 T1 (not "0.3")
deposit: "300", deposit_token: T2 address
fee: 20000, range: "full"
```

Summary line for user: *“1 T1 ≈ 3.33 T2 at starting price; deposit 300 T2 (+ T1 computed).”*

## Example dialogue (existing pool — BOB / JANE)

```
User:  I want to create a liquidity pool on V3 wiz4rd-swap.

Agent: Which two tokens?

User:  BOB and JANE.

Agent: [discover_v3_pool_fee → fee 20000, lifecycle Ready]
       [get_v3_pool → live price]

       **This BOB/JANE pool already exists** at **2%** on wiz4rd. The **ratio is fixed**
       on-chain — you can't set a new starting price. Current price: **1 BOB ≈ … JANE**.

       How much liquidity to add, and in BOB or JANE?

User:  500 BOB, full range.

Agent: Summary — add liquidity only (existing pool, fixed ratio):
       • BOB/JANE @ 2%, deposit 500 BOB (+ JANE computed)
       • ~3 steps in Vaughan TUI (approve both tokens + mint)
       Proceed?

User:  Yes

Agent: [propose_v3_lp_deploy once]
```

## After `propose_v3_lp_deploy`

1. Tell user to watch **Vaughan TUI** (not the chat) for approval cards.
2. Do **not** call `propose_v3_create_pool` / `_initialize_pool` / `_mint` separately.
   Do **not** use `propose_v3_mint` for Brew recovery (MCP timeout — INC-6).
3. **Gas gate (createPool only):** read `proposal.gas_limit` in the tool JSON — must be
   **≥ 6_000_000** before user presses **y** ([`gas-preflight.md`](gas-preflight.md)).
   Later steps may show **`Fee: unavailable`** but **`Gas: … (network estimate)`** — that is OK (INC-11).
4. User should confirm the **verification table** on each card (pool, range, deposit legs) plus gas line (INC-10).
5. User must run a **fresh Vaughan build** if cards look blank or behavior is stale (INC-3).
6. After user says they finished → **`list_v3_positions`** to verify NFT (on-chain wins over job file). On success, TUI shows a **bordered summary table** under the address (~10s).

### When something fails mid-Brew

Stop retrying tools in a loop. Run sense checklist from [`lp-brew-incidents`](../lp-brew-incidents/SKILL.md),
then one recovery action ([`brew-recovery.md`](brew-recovery.md)).

## Sense-before-propose (agent checklist)

Before question 6 summary, silently (or briefly report):

```
get_control_plane_status  → wallet_unlocked, ready_for_writes, active_account_*
get_network               → chain_id 943
resolve_token             → both addresses (after user confirms paste)
list_assets              → balance preflight before summary (not get_balance for ERC-20 — INC-8)
discover_v3_pool_fee      → lifecycle + fee (run right after Q1)
get_v3_pool               → live ratio when lifecycle Ready
```

Run **session confirm** ([`confirm-session.md`](confirm-session.md)) immediately after
`get_control_plane_status` + `get_network`.

| Lifecycle | Dialogue | Brew |
|-----------|----------|------|
| **Missing** | Q2 price + Q4 fee | create → init → approve → mint |
| **Uninitialized** | Q2 price (initialize only) | init → approve → mint |
| **Ready** | **No price question** — warn fixed ratio | approve → mint only |

See [`existing-v3-pool.md`](existing-v3-pool.md) for exact user-facing wording.

## Short answers → field mapping

| User answer | Agent sets |
|-------------|------------|
| “1000 TEST1” | `deposit: "1000"`, `deposit_token: "TEST1"` |
| “half half” / “500 each” | Prefer one-sided: ask which token is the anchor, or split into two mint-only flow (escape hatch — not Brew) |
| “2%” / “two percent” | `fee: 20000` |
| “full” / “max range” | `range: "full"` |
| “wiz4rd” / “wizard swap” | `venue: wiz4rd`, chain 943 |

## Forbidden mid-dialogue

- Calling `propose_v3_lp_deploy` before Q6 confirmation.
- Default fee 500 on a new pool without asking.
- Passing inverted user price without converting to **token_b per token_a**.
- Using `get_balance` for ERC-20 deposit checks on 943 (use `list_assets`).
- Looping `propose_v3_mint` when mint card is missing (use `propose_v3_lp_deploy` when Ready).
- Telling user to approve createPool when proposal `gas_limit` **< 6_000_000**.
- Sentient auto-exec or “I’ll sign for you”.
- Mainnet without explicit user intent + env gate.

## Related

- Incidents: [`lp-brew-incidents`](../lp-brew-incidents/SKILL.md) · Recovery: [`brew-recovery.md`](brew-recovery.md)
- Gas: [`gas-preflight.md`](gas-preflight.md) · Balances: [`balance-preflight.md`](balance-preflight.md)
