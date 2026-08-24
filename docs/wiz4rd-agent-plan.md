# Plan: Agents + wiz4rd V3 on Pulse testnet

**Context:** wiz4rd-swap (Pancake V3–family) contracts are deployed on PulseChain
testnet **943**. Vaughan already ships `wiz4rd-sdk` (pool reads, offline quotes,
swap + NFT liquidity tx builders) and MCP propose/approve. They are not fully
wired together yet.

## Short answers

| Question | Answer |
|----------|--------|
| Can we play with the deploy in Vaughan **today**? | **Yes (swaps).** Dex venue **Wiz4rd** on 943 + addresses in `docs/wiz4rd-addresses.md` / `vaughan_core::core::wiz4rd`. MCP `get_network` returns the deploy. LP mint via MCP is still Phase D. |
| Do agents need Vaughan to *access* V3 contracts? | **No for reads.** Any RPC + ABI/`eth_call` works. **Yes for writes that spend user funds** — agents must not hold keys; Vaughan is the approval + signing gate. |

## What agents need (capability map)

```
┌─────────────────────────────────────────────────────────────┐
│  AGENT (Cursor / Claude) — untrusted planner                │
│  • Intent: “swap X”, “mint LP in range”, “collect fees”      │
│  • Never: keys, passwords, auto-broadcast                   │
└───────────────┬─────────────────────────────┬───────────────┘
                │ read (RPC / MCP)            │ propose only
                ▼                             ▼
┌───────────────────────────┐   ┌─────────────────────────────┐
│  Public chain data        │   │  Vaughan (MCP + TUI)        │
│  • slot0, liquidity       │   │  • allowlisted routers/NPM  │
│  • positions of address   │   │  • approve ERC-20           │
│  • quote math (local OK)  │   │  • propose_* → approval card│
│  No Vaughan required      │   │  • re-sim + human y/n       │
└───────────────────────────┘   │  • sign + broadcast         │
                                └─────────────────────────────┘
```

### Read tools agents need (no vault)

| Need | Source today | Gap |
|------|--------------|-----|
| Factory / router / NPM addresses on 943 | `vaughan_core::core::wiz4rd` + MCP `get_network.wiz4rd` | Done (Phase A) |
| Pool state (`slot0`, liquidity, ticks) | `wiz4rd-sdk::pool` | MCP: `get_v3_pool` |
| Offline / local quote | `wiz4rd-sdk::quote` | MCP: `quote_v3_swap` (complement `quote_swap` aggregators) |
| List NFT positions for address | `wiz4rd-sdk::positions` | MCP: `list_v3_positions` |
| Token metadata / balances | MCP `get_balance` / `list_assets` | OK |
| Simulate calldata | MCP `simulate_call` | OK |

### Write tools agents need (Vaughan-only)

| Need | Builder exists? | MCP today | Plan |
|------|-----------------|-----------|------|
| ERC-20 approve → router/NPM | `wiz4rd-sdk::allowance` | via `propose_contract_call` only | `propose_approve` (or fold into swap/mint prep) |
| Exact-in / exact-out V3 swap | `tx::swap` | No (agg/V2 only) | `propose_v3_swap` |
| Mint concentrated LP | `tx::liquidity::build_mint_tx` | **No** | `propose_v3_mint` |
| Increase / decrease / collect | SDK builders | **No** | `propose_v3_increase` / `_decrease` / `_collect` |
| Batch approve+mint (7702) | Ambire path | Deferred | Later; sequential proposes OK for v1 |

**Hard rule (unchanged):** every write is `propose_*` → TUI card → re-sim → human approve. MCP never signs.

## How Vaughan gives them “everything”

### Layer 0 — Ground truth addresses (**done**)

1. Recorded 943 deploy: [`docs/wiz4rd-addresses.md`](wiz4rd-addresses.md) + `vaughan_core::core::wiz4rd`
2. Dex screen venue **Wiz4rd** (default on chain 943, V3-only)
3. MCP `get_network` returns `wiz4rd { factory, swap_router, position_manager, … }`
4. SwapRouter allowlisted in `dex_routers` for Degen/sim gates

### Layer 1 — Play in Vaughan (human)

| Path | How |
|------|-----|
| Inspect | TUI Browse `c` → paste pool / NPM / router from addresses doc |
| Swap | Dex `d` → **Wiz4rd** → V3 → fee 500 → confirm |
| LP | Still Phase D (MCP `propose_v3_mint`) — use `wiz4rd` CLI or Browse for now |

Optional parallel path already sketched in SDK config: `vaughan_provider` WS → EIP-1193 approve (same TUI gate as Freedom). Useful for a wiz4rd CLI; MCP remains the agent path.

### Layer 2 — Agent read surface (no keys)

Wire `wiz4rd-sdk` into MCP sensory tools:

1. `get_v3_pool` — token0/1, fee, slot0, liquidity  
2. `quote_v3_swap` — exact in/out + price impact hint  
3. `list_v3_positions` — tokenIds + ticks + liquidity for `account_address`  
4. Keep existing `inspect_contract` / `simulate_call`

Agents can also skip Vaughan and use cast/ethers against RPC — **that’s fine for research**. Vaughan tools exist so the *same* agent session can go propose → approve without copy-paste.

### Layer 3 — Agent write surface (Vaughan must own)

1. Allowlist 943 wiz4rd `swap_router` + `position_manager` (same pattern as agg routers).  
2. MCP tools:
   - `propose_v3_swap`
   - `propose_v3_mint` (+ optional auto-`propose_approve` steps or multi-card sequence)
   - later: increase / decrease / collect  
3. Approval card shows: venue **Wiz4rd**, pool key, ticks, amounts, spender — agent `explanation` untrusted.  
4. Testnet-first; mainnet still `VAUGHAN_MCP_ALLOW_MAINNET=1`.

### Layer 4 — Product narrative

- **Browserless Pulse:** human Dex/Ag without Chrome.  
- **MCP:** agent planner; Vaughan executor.  
- **wiz4rd:** *our* V3 stack on Pulse — first-class venue, not “paste any router.”  
- Aggregators (`quote_swap`) stay for multi-DEX routing; wiz4rd tools are for *this* deployment + LP.

## Suggested build order

| Phase | Deliverable | Outcome |
|-------|-------------|---------|
| **A** | Pin 943 addresses + Dex “Wiz4rd” venue + docs | **Done** — humans can swap against deploy in TUI |
| **B** | MCP read: pool / quote / positions | **Done** (pool + quote); positions still open |
| **C** | MCP `propose_v3_swap` + allowlist + Anvil/943 tests | **Done** — `propose_v3_swap`; live 943 test `#[ignore]` |
| **D** | MCP `propose_v3_mint` (+ approve sequence) | Agents open LP |
| **E** | Collect / decrease / UI positions screen | Full LP lifecycle |

## What *not* to do

- Don’t put private keys or unlock in the MCP process.  
- Don’t let agents broadcast “raw” txs without Vaughan’s gate.  
- Don’t treat aggregator `propose_agg_swap` as covering wiz4rd LP — different contracts and risk.  
- Don’t require agents to use Vaughan for pure chain research — document both paths.

## Success demo (943)

1. Unlock Vaughan on Pulse testnet.  
2. Agent: `get_v3_pool` → `quote_v3_swap` → `propose_v3_swap` → human approves.  
3. Agent: `propose_v3_mint` (with ticks) → approve ERC-20s if needed → mint → `list_v3_positions` shows NFT.  
4. No Chrome; no keys in the agent.

## Prerequisites from you

~~Paste 943 addresses~~ — landed from `wiz4rd-swap/docs/addresses.md` into
[`docs/wiz4rd-addresses.md`](wiz4rd-addresses.md).
