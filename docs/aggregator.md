# Aggregator (Ag) screen

`g` opens the aggregator view (AI Agent is on the **Tab** cycle after Ag).

## Focus: SquirrelSwap (no API key)

Public Brain API — same engine as [`squirrelswap-mcp`](https://www.npmjs.com/package/squirrelswap-mcp):

| | |
|---|---|
| Base | `https://api.squirrelswap.pro` |
| Preview | `GET /quote?tokenIn&tokenOut&amountIn&compact=1` |
| Prepare tx | `POST /swap` `{tokenIn,tokenOut,amountIn,slippage,recipient}` |
| Auth | **None** — only optional `X-SS-Client` attribution |
| Native PLS | `0x000…000` |
| Router (from `tx.to`) | Must be on Vaughan’s Ag allowlist (e.g. `0xDa8953Fc…`); unknown `to`/`spender` refused |

Vaughan flow: ↑/↓ → amount → **Enter** → `POST /swap` → allowlist check →
approve if `approvalNeeded` → confirm → sign. User always approves.

### Testing without PLS

| Mode | How |
|---|---|
| **Anvil (CI)** | `cargo test -p vaughan-tui --test ag_view` — fixture `/swap` → mock router broadcast |
| **Quote-only (live Brain)** | `cargo test -p vaughan-core live_preview_quote -- --ignored --nocapture` — `GET /quote`, no wallet |
| **Inspect prepared swap** | `cargo test -p vaughan-core live_prepare_swap_inspect -- --ignored --nocapture` — `POST /swap`, print router/calldata, **no sign** |
| **TUI** | Ag on chain 369 → Enter for prepare → **Esc** before confirm (never signs) |

## Other live (also no key)

| Venue | API |
|---|---|
| PulseSwap | `quotes.pulseswap.io` advanced |
| Piteas | `sdk.piteas.io/quote` (public beta; partner key optional for higher limits) |
| **9mm 9X** | `api.9mm.pro/v1/{chain}/swap/*` (anon tier; docs at `/docs`) |

### 9mm 9X (no API key)

Unified gateway — same 9x routing as the web app:

| | |
|---|---|
| Base | `https://api.9mm.pro` |
| Price (compare) | `GET /v1/pulse/swap/price?sellToken&buyToken&sellAmount` |
| Quote (sign) | `GET /v1/pulse/swap/quote?…&takerAddress&slippagePercentage` |
| Auth | **None** on anon tier (read `X-RateLimit-*` headers; request free/pro key only if you hit `429`) |
| Native PLS | `0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE` |
| Chains | `pulse` (369), `eth`, `base`, `sonic`, `robinhood` — slug in path |
| Client | `User-Agent` required (Cloudflare blocks bare library defaults) |

`takerAddress` must be the active wallet with PLS for gas (API simulates before returning calldata).
Execution router is allowlisted after verification from live quotes (`0xd5b775…` on PulseChain).

**Compare-all** uses `/swap/price` for 9mm (no PLS required). Picking 9mm and pressing Enter
re-fetches `/swap/quote` for executable calldata before confirm.

Live probe: `cargo test -p vaughan-core live_price_pls_to_hex -- --ignored --nocapture`

## No-permission matrix (Pulse aggs)

Which venues we can integrate **without** partner signup / API keys:

| Venue | Need permission? | Vaughan today | Without asking? |
|---|---|---|---|
| **SquirrelSwap** | No | **Live** (`api.squirrelswap.pro`) | Already done |
| **Piteas** | No (public beta; key optional) | Client + Ag `LiveNoKey` | Already done |
| **EmpSeal / EmpX** | No | **Live** (Alloy on-chain) | Done — `findBestPath` + swap calldata |
| **Switch.win** | **Yes** — `x-api-key` on `quote.switch.win` | `NeedsApiKey` | No (without a key) |
| **Jolt (CURV)** | **Yes** — powered by Switch.win routing | Listed as CURV | No — same gate as Switch |
| **9X (9mm)** | No | **Live** (`api.9mm.pro`) | Done — `/v1/{chain}/swap/quote` |

**Without begging:** Squirrel + Piteas + EmpX + **9mm 9X** (shipped on anon tier).  
**Needs a key:** Switch, Jolt — use **VB human path** via MCP `browser_open_agg` (see skill above).  
**VB cross-check:** 9X web UI still useful when API returns 500/111 on a pair.

EmpX is on-chain routing (`empx-swap-sdk` → `findBestPath` / `getSwapCalldata`), reimplemented with Alloy (ABI/interop only — no vendoring their TS). PulseChain mainnet **369 only**.

## Listed / gated

| Venue | Why not live |
|---|---|
| Switch.win | Needs `x-api-key` |
| Empseal (EmpX) | **Live** — Alloy `findBestPath` / `swapNoSplit*` on PulseChain 369 |
| 9mm 9X | **Live** — `api.9mm.pro` `/v1/{chain}/swap/quote` (anon; `takerAddress` + PLS for gas) |
| CURV / Jolt | Switch routing engine — same API key gate |
| Internet Money, PortalX | Cross-chain / wallet products |
| LibertyX | Use **Bridge (`f`)** — LibertySwap wrapper (`docs/bridge.md`) |

## Related

- **Agent playbook (VB quotes):** [`vaughan-agent/skills/vb-ag-quotes/SKILL.md`](../vaughan-agent/skills/vb-ag-quotes/SKILL.md) — multi-venue PLS→HEX tours, `browser_open_agg`, Switch.win without API key
- `docs/piteas.md` — public beta + optional partner key vault
- Dex (`d`) — direct Uni V2/V3 routers
