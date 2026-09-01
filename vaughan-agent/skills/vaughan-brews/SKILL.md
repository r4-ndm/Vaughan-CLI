---
name: vaughan-brews
description: Token-agnostic LP deploy Brews for Pulse wiz4rd — detect existing pools (fixed ratio), conversational Q&A or direct params; propose_v3_lp_deploy once, TUI approves.
mode: assist
kind: guide
---

# Vaughan LP Brews (Advisor mode)

Brews turn **the user's tokens** and casual inputs into the full V3 pipeline (create → initialize → approve → mint). Resolve contracts via [`token-resolve`](../token-resolve/SKILL.md) (`list_assets`, paste `0x`, confirm). Check balances via [`balance-preflight`](../balance-preflight/SKILL.md) before propose.

## Conversational mode (default for casual users)

When the user says something like *“create a liquidity pool on wiz4rd”* without numbers:

1. **Do not** call write tools on the first turn.
2. Follow the question script in [`conversational-brew.md`](conversational-brew.md) — ask, map answers, **confirm summary**, then one `propose_v3_lp_deploy`.
3. User approves in **Vaughan TUI** (~5–6 **y** for a new pool; fewer if pool already exists).

## Active wallet & chain (before tokens or propose)

Confirm F1 network + F3 account match user intent — [`confirm-session.md`](confirm-session.md) · [`wallet-account`](../wallet-account/SKILL.md).

Combined ask: *“You are on {network} ({chain_id}), {wallet label} — is this the chain and wallet you want to deploy from?”* On **no**, user switches **F1** / **F3** in TUI; re-check before propose.

## When to use (direct mode)

User already gave pair, price, deposit, and fee in one message — skip Q&A, confirm once, then propose.

## Existing pool (fixed ratio)

If `lifecycle` is **Ready**, follow [`existing-v3-pool.md`](existing-v3-pool.md):

- Tell the user the **pool already exists** and the **ratio is fixed** on-chain.
- Show live price from `get_v3_pool` — **do not** ask for a starting price.
- Use the discovered **fee** — **do not** ask “fee for new pool”.
- Brew runs **add liquidity only** (approve + mint), not create → initialize.

## Required flow (both modes)

1. **Sense:** `get_control_plane_status` + `get_network` → **session confirm** → **token resolve/confirm** → `discover_v3_pool_fee` → `get_v3_pool` when Ready.
2. **Branch:** new pool (Missing) vs existing (Ready) — see [`existing-v3-pool.md`](existing-v3-pool.md).
3. **Balance preflight** — [`balance-preflight.md`](balance-preflight.md) after deposit known.
4. **Gas preflight** — [`gas-preflight.md`](gas-preflight.md) when lifecycle is **Missing** (createPool step); re-check `gas_limit` in propose response before user presses **y**.
5. **Confirm** summary with user (conversational mode) or restate params (direct mode).
6. **One write:** `propose_v3_lp_deploy` with human fields — **stop** after the tool returns.
7. Tell the user: approve in the **Vaughan TUI** (verification **table** + gas line); later steps auto-enqueue.
8. If anything fails mid-Brew → [`lp-brew-incidents`](../lp-brew-incidents/SKILL.md) before retrying tools.

## Human field mapping (generic)

| User says | MCP field |
|-----------|-----------|
| 1 token A = 0.5 token B | `price`: `"0.5"`, `token_a` / `token_b` as **addresses** (or symbols the resolver knows) |
| 100 of token A only | `deposit`: `"100"`, `deposit_token`: same as token A |
| 0.25% fee | `fee`: `2500` (bps) |
| 2% fee | `fee`: `20000` |
| full range | `range`: `"full"` (default) |

**Price rule:** `price` is always **token_b per token_a** in the names the user used, before on-chain sort. If the user states *“1 B = X A”* but A was named first, **invert** (e.g. 1 T2 = 0.3 T1 with T1 first → `price: "3.333…"`).

**Fee rule:** If `discover_v3_pool_fee` returns no pool, user must pick an explicit `fee` for a **brand-new** pair — do not default to 500.

## User Brew files (optional)

Users keep private JSON under `~/.local/share/vaughan-cli/brews/`. Format: [`brew.example.json`](../../brews/brew.example.json). CLI: `vaughan lp plan --brew /path/to/file.json`.

## Forbidden

- Sequential `propose_v3_create_pool` → `_initialize_pool` → `_mint` for **new pools**.
- Assuming a default fee tier without discovery or user confirmation.
- `propose_v3_lp_deploy` before the user confirms the summary (conversational mode).
- Telling user to approve createPool when proposal `gas_limit` **< 6_000_000** — see [`gas-preflight.md`](gas-preflight.md).
- **`propose_v3_mint`** for Brew recovery (MCP timeout) — use **`propose_v3_lp_deploy`** instead.
- Sentient auto-exec for Brews.

See also: [`conversational-brew.md`](conversational-brew.md) · [`confirm-session.md`](confirm-session.md) · [`brew-recovery.md`](brew-recovery.md) · [`lp-brew-incidents`](../lp-brew-incidents/SKILL.md) · [`balance-preflight.md`](balance-preflight.md) · [`gas-preflight.md`](gas-preflight.md) · [`existing-v3-pool.md`](existing-v3-pool.md) · [`new-v3-pool-943.md`](new-v3-pool-943.md).
