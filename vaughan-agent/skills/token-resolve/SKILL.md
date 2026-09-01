---
name: token-resolve
description: Resolve ticker-only tokens to confirmed 0x contracts — list_assets, paste, resolve_token, user OK before propose.
mode: assist
kind: guide
---

# Token resolve & contract confirmation

Users often say **TEST1**, **BOB**, or a ticker without a contract. The agent must **not**
guess addresses. Resolve or ask for a paste, then **confirm** before any `propose_*`.

## Tools

| Tool | When |
|------|------|
| `list_assets` | Active wallet — may already show symbol + `contract` |
| `resolve_token` | User pasted `0x…` — read symbol, name, decimals (read-only) |
| `import_token` | After user confirms — add to profile Assets (needs unlocked session) |
| `inspect_contract` | Optional — user unsure if contract is a standard ERC-20 |

Built-in resolver symbols on **943** (no paste needed): **WPLS**, **WZRD**, smoke **BOB**, **JANE**, **JIM**.
Everything else (e.g. **TEST1**, **TEST2**) needs a contract unless already in `list_assets`.

## Flow per token

### 1 — Symbol only (no `0x`)

```
list_assets   → match symbol (case-insensitive) on active chain?
```

| Result | Next |
|--------|------|
| Found | Go to **3 — Confirm** using that `contract` |
| Not found | **2 — Ask for paste** |

### 2 — Ask for paste

> I don't have a contract for **TEST1** on chain **943**. Please paste the **checksummed
> `0x` address** for TEST1 (from your launch tx or block explorer).

Wait for paste. Do not invent an address.

### 3 — Confirm (mandatory before propose)

```
resolve_token { token: "0x…" }
```

Tell the user:

> **TEST1** → *Token Name* · **18** decimals · `0x1234…5678` — is this the correct token?

User must say **yes / correct** before you use it in `propose_v3_lp_deploy` or other writes.

Optional after yes:

```
import_token { token: "0x…" }
```

(so F2 Assets shows it in the TUI).

### 4 — User pasted `0x` directly

Still run **resolve_token** and **confirm** — tickers lie; same symbol can exist on
multiple contracts.

## Pair dialogue (LP Brew)

After user names two tokens:

1. Resolve/confirm **token A** completely.
2. Resolve/confirm **token B** completely.
3. Only then `discover_v3_pool_fee` / deposit questions.

Example:

```
User:  TEST1 and TEST2.

Agent: [list_assets — neither listed]
       Paste the **TEST1** contract on 943.

User:  0xAa…

Agent: [resolve_token]
       **TEST1** = "My Test One" · 18 dec · `0xAa…` — correct?

User:  Yes.

Agent: Paste the **TEST2** contract.

User:  0xBb…

Agent: [resolve_token] … confirm …
       Got both. Starting price: how many TEST2 per 1 TEST1?
```

## Forbidden

- Proceeding with an unresolved ticker (no `0x`, not in catalog, not in `list_assets`).
- Skipping confirmation after `resolve_token`.
- Picking a random pool token with the same symbol from memory / training data.
- Mainnet contract on testnet (or vice versa) — always match `get_network` `chain_id`.

## Related

- LP Brews: [`vaughan-brews/conversational-brew.md`](../vaughan-brews/conversational-brew.md)
- Contract safety: [`contract-inspection/SKILL.md`](../contract-inspection/SKILL.md)
- Pulse catalog: [`pulsechain-context/SKILL.md`](../pulsechain-context/SKILL.md)
