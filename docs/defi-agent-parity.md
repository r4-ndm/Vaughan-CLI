# DeFi agent parity checklist

**Goal:** Anything a human can do in Vaughan’s browserless DeFi surfaces, an
external agent can do via MCP — same contracts, same allowlists, same builders.
Parity is about **verbs** (swap, mint, wrap, revoke, …), not about forcing a
human click on every write.

## Two relationships (read first)

See [`agent-roles.md`](agent-roles.md). Short version:

| Role | Who’s in charge | Profile | Writes |
|------|-----------------|---------|--------|
| **Adviser** | Human; agent helps | `default` | Propose → you approve |
| **Sentient** | Agent (owns its seed) | `sentient` | Auto under policy |

Parity checklist below is the **shared verb set**. Agency (click vs auto) is
picked by role/profile, not by withholding tools.

## Whose seed?

| Profile | Seed owner | Agent writes | Notes |
|---------|------------|--------------|-------|
| **`default`** (adviser) | Human savings | Propose → TUI approve | Don’t point `vaughan-sentient` here by accident |
| **`sentient`** | The sentient agent | Full control under policy | Optional human partnership = shared mnemonic |

MCP **never** holds or exports vault keys. Signing stays in Vaughan.

**How to use:** Check items off as MCP tools land. Keep this file and `TASKS.md`
in sync. Named tools preferred over raw `propose_contract_call` so cards / policy
logs stay readable and routers stay allowlisted.

Legend: `[x]` agent-ready · `[~]` partial / escape-hatch only · `[ ]` missing

---

## 0. Session / wallet basics

| Human (TUI) | Agent tool | Status |
|-------------|------------|--------|
| See address | `get_address` | [x] |
| See network / chain | `get_network` | [x] |
| Native + token balances | `get_balance`, `list_assets` | [x] |
| Switch network | *(human only in Settings)* | [ ] named `switch_network` propose — or document “human switches” |
| Unlock / lock vault | **never** MCP | n/a (hard ban) |
| Export keys / mnemonic | **never** MCP | n/a (hard ban) |
| Agent auto-trade without click | MCP `vaughan-sentient` / `--profile sentient` | [x] TUI auto-exec when unlocked |

---

## 1. Inspect / research

| Human | Agent | Status |
|-------|-------|--------|
| Contract browser (`c`): probe, ABI, call | `inspect_contract`, `simulate_call` | [x] |
| Discover pairs / pools | `search_pairs`, `get_dex_reserves` | [x] |
| wiz4rd pool state | `get_v3_pool` | [x] |
| Token “what is 0x…” → Assets import | — | [ ] |
| Activity / history (`m`) | — | [ ] `list_transfers` or similar |
| Local EIP-712 paste → approve | — | [ ] `propose_typed_data` |

---

## 2. Send / call

| Human | Agent | Status |
|-------|-------|--------|
| Native / ERC-20 send | `propose_transfer` | [x] |
| Arbitrary write (browser `send`) | `propose_contract_call` | [x] / [~] (TUI browser write gate still open) |
| EIP-7702 Ambire batch send | `propose_batch_7702` | [ ] deferred (FR-3.3) |

---

## 3. Swap (trade)

| Human | Agent | Status |
|-------|-------|--------|
| Ag (`g`): quote aggregators | `quote_swap` | [x] |
| Ag: approve + swap | `propose_agg_swap` (+ approve via call/path) | [x] |
| Dex (`d`): V2 path swap | `propose_swap` | [x] |
| Dex: curated V2/V3 write polish | — | [ ] human Dex write path still open in TASKS |
| wiz4rd V3 quote | `quote_v3_swap` | [x] |
| wiz4rd V3 swap | `propose_v3_swap` | [x] |
| EmpX / EmpSeal path-find | — | [ ] |
| Exact-out / multi-hop V3 | — | [ ] when product needs it |

---

## 4. Liquidity (LP / Earn)

| Human | Agent | Status |
|-------|-------|--------|
| Open V3 position (mint NFT) | `propose_v3_mint` | [x] Phase D |
| List my V3 positions | `list_v3_positions` | [x] |
| Increase / decrease liquidity | `propose_v3_increase` / `_decrease` | [ ] Phase E |
| Collect fees | `propose_v3_collect` | [ ] Phase E |
| V2 add / remove LP | — | [ ] only if product demand |
| TUI positions screen | — | [ ] Phase E UI |

---

## 5. Approvals / wrap / bridge

| Human | Agent | Status |
|-------|-------|--------|
| Approvals manager (`j`): list | `list_allowances` | [x] |
| Revoke allowance | `propose_revoke` | [x] |
| ERC-20 approve spender | via `propose_contract_call` | [~] prefer named `propose_approve` later |
| Wrap / unwrap WPLS (`e`) | `propose_wrap` / `propose_unwrap` | [x] |
| LibertySwap bridge (`f`) | — | [ ] `quote_bridge` / `propose_bridge` |
| Official Omnibridge | — | [ ] separate track |

---

## 6. Privacy / AA (wallet pillars, not classic AMM)

| Human | Agent | Status |
|-------|-------|--------|
| Stealth receive / send / scan / sweep | — | [ ] MCP stealth tools when ready |
| Ambire AA batched UX | `propose_batch_7702` | [ ] FR-3.3 |

---

## 7. Meta (proposal lifecycle)

| Human | Agent | Status |
|-------|-------|--------|
| See pending MCP cards | `list_pending_proposals` | [x] |
| Poll proposal outcome | `get_proposal_status` | [x] |
| Reject / expire | human in TUI | [x] (agent sees status) |

---

## Suggested build order (parity-first)

0. **Sentient MCP** (`vaughan-sentient`) — done (auto-exec when TUI unlocked).  
1. **Approvals + wrap** — **done** (`list_allowances`, `propose_revoke`, wrap/unwrap).  
2. **Phase D mint + `list_v3_positions`** — **done**.  
3. **Phase E** increase / decrease / collect.  
4. **Bridge quote/propose** — mirror LibertySwap TUI.  
5. **History + token import** — research loop without PulseScan tab.  
6. **Ambire batch + stealth MCP** — pillars, after core DeFi verbs feel complete.

Hard rules:

- Keys never enter the MCP/agent process.
- `default` writes: `propose_*` → re-sim → human approve.
- `sentient` writes: same tools, auto-exec under policy (agent’s seed).
- Shared seed = intentional partnership; shape behavior with **skill presets**, not contracts.
- Router / spender allowlists + sim still apply on both profiles.

## Related docs

- [`agent-roles.md`](agent-roles.md) — adviser (human-led) vs sentient (agent-led)  
- [`ai-tool-surface.md`](ai-tool-surface.md) — shipped tool contract  
- [`pulse-defi-skills.md`](pulse-defi-skills.md) — agent playbook  
- [`wiz4rd-agent-plan.md`](wiz4rd-agent-plan.md) — wiz4rd phases A–E  
- [`browserless-pulse.md`](browserless-pulse.md) — product thesis  
