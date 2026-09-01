---
name: wallet-account
description: Confirm F1 network + F3 wallet before propose_* — combined session check; instruct manual TUI switch (F1/F3) on mismatch.
mode: assist
kind: guide
---

# Wallet & chain session confirmation (Advisor)

Proposals sign on the **F1-active network** and **F3-active account** in the unlocked
Vaughan TUI (or `vaughan serve`). The agent **cannot** switch either — the user must
do it manually.

Read this before any `propose_*` that moves funds (LP Brews, swaps, transfers, …).

## Sense (every write flow)

```
get_control_plane_status
get_network
```

When unlocked, note:

| Source | Field | Meaning |
|--------|-------|---------|
| control plane | `active_account_index` | HD index (`0` = wallet 0) |
| control plane | `active_account_label` | e.g. `wallet 0` |
| control plane | `active_address` | Signer `0x…` |
| `get_network` | `network_id` | e.g. `pulsechain-testnet-v4` |
| `get_network` | `chain_id` | e.g. `943`, `369` |
| `get_network` | `is_testnet` | `true` / `false` |

Use the network **display name** from `get_network` context (e.g. *PulseChain Testnet V4*
for 943, *PulseChain* for 369).

## Combined confirm (default)

After sense tools, ask **once** before token / pool / amount questions:

> You are on **PulseChain Testnet V4 (943)**, **wallet 0** (`0x9274…574f`).
> Is this the **chain and wallet** you want to deploy from?

User says **yes / correct / proceed** → continue the workflow.

User says **no** → ask what to change (chain, wallet, or both) and follow **Mismatch**
below. Do **not** call `propose_*` until they confirm.

### Task-specific chain hint (LP Brew / wiz4rd)

When the user asked for **wiz4rd V3 on testnet**, the expected chain is **943**. If
active `chain_id` is not 943, say so explicitly:

> wiz4rd LP Brews on testnet require **PulseChain testnet v4 (943)**. You are on
> **chain 369** — please switch network first.

## Mismatch — wallet only

User wants **wallet 0** but active is **wallet 1** (or address mismatch):

> Please switch to **wallet 0** in Vaughan:
> **F3** (account box) → **↑ / ↓** → **Enter** on wallet 0 → tell me when ready.

Re-run `get_control_plane_status` + `get_network` when they reply *switched* / *ready* /
*done*.

## Mismatch — chain only

User wants **mainnet (369)**, **testnet (943)**, or another built-in network:

> Please switch to **PulseChain Testnet V4 (943)** in Vaughan:
> **F1** (network box) → **↑ / ↓** → **Enter** on the correct network → tell me when ready.

Re-check `get_network` before continuing.

## Mismatch — both

Handle **network first** (F1), then **wallet** (F3), then re-confirm with the combined
prompt.

## Parse user intent

| User says | Match against |
|-----------|----------------|
| `wallet 0`, `account 0` | `active_account_index == 0` |
| `0xABC…` | `active_address` (case-insensitive) |
| `testnet`, `943`, `wiz4rd testnet` | `chain_id == 943` |
| `mainnet`, `369`, `PulseChain` | `chain_id == 369` |
| `wiz4rd` (LP context) | `chain_id == 943` unless user explicitly said mainnet |

## Example (LP Brew)

```
User:  Create a wiz4rd V3 pool.

Agent: [get_control_plane_status + get_network]

       You are on **PulseChain Testnet V4 (943)**, **wallet 1** (`0x9274…574f`).
       Is this the chain and wallet you want to deploy from?

User:  No — I need wallet 0.

Agent: Switch to **wallet 0**: **F3** → **↑/↓** → **Enter**. Tell me when ready.

User:  Done.

Agent: [re-check → wallet 0 on 943]
       Great — wallet 0 on testnet 943. Which two tokens?
```

```
User:  Deploy LP on wallet 0 but I'm on mainnet by mistake.

Agent: You're on **PulseChain mainnet (369)**, not testnet. Switch network:
       **F1** → **↑/↓** → **PulseChain Testnet V4** → **Enter**. Tell me when ready.
```

## LP Brew placement

[`vaughan-brews/confirm-session.md`](../vaughan-brews/confirm-session.md) — short
checklist after control-plane sense, before token Q1.

## Forbidden

- `propose_*` while chain or wallet ≠ user intent.
- Claiming the agent switched network or account for the user.
- Mainnet writes without explicit user intent (and env gate).

## Related

- Networks / addresses: [`pulsechain-context/SKILL.md`](../pulsechain-context/SKILL.md)
- MCP / lock: [`mcp-connect/SKILL.md`](../mcp-connect/SKILL.md)
- Advisor rules: [`assist-advisor/SKILL.md`](../assist-advisor/SKILL.md)
