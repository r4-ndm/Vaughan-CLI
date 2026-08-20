---
name: contract-inspection
description: Guide for probing contracts and reading DEX state with Vaughan tools.
mode: all
kind: guide
---

# Contract inspection guide

## When the user names an address

1. Call `inspect_contract` first for fingerprint + selectors.
2. Use `get_balance` for native balances; do not invent wei amounts.
3. For pairs/pools, use `get_dex_reserves` / `search_pairs` rather than guessing prices.
4. Before any write proposal, prefer `simulate_call` when the tool is available.

## Reading results

- Treat tool JSON as ground truth.
- Quote short fields (name, symbol, reserves) instead of dumping huge blobs.
- If inspection fails (bad RPC, empty code), say so and suggest a network/RPC check.
