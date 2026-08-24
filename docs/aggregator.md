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

## No-permission matrix (Pulse aggs)

Which venues we can integrate **without** partner signup / API keys:

| Venue | Need permission? | Vaughan today | Without asking? |
|---|---|---|---|
| **SquirrelSwap** | No | **Live** (`api.squirrelswap.pro`) | Already done |
| **Piteas** | No (public beta; key optional) | Client + Ag `LiveNoKey` | Already done |
| **EmpSeal / EmpX** | No | **Live** (Alloy on-chain) | Done — `findBestPath` + swap calldata |
| **Switch.win** | **Yes** — `x-api-key` on `quote.switch.win` | `NeedsApiKey` | No (without a key) |
| **Jolt (CURV)** | **Yes** — powered by Switch.win routing | Listed as CURV | No — same gate as Switch |
| **9X (9mm)** | Unknown / no public quote API found | Listed only | Not cleanly — ask or skip |

**Without begging:** Squirrel + Piteas + EmpX (shipped).  
**Needs a key:** Switch, Jolt.  
**Stuck until a public API appears:** 9X.

EmpX is on-chain routing (`empx-swap-sdk` → `findBestPath` / `getSwapCalldata`), reimplemented with Alloy (ABI/interop only — no vendoring their TS). PulseChain mainnet **369 only**.

## Listed / gated

| Venue | Why not live |
|---|---|
| Switch.win | Needs `x-api-key` |
| Empseal (EmpX) | **Live** — Alloy `findBestPath` / `swapNoSplit*` on PulseChain 369 |
| 9mm 9X | No public developer quote API |
| CURV / Jolt | Switch routing engine — same API key gate |
| Internet Money, PortalX | Cross-chain / wallet products |
| LibertyX | Use **Bridge (`f`)** — LibertySwap wrapper (`docs/bridge.md`) |

## Related

- `docs/piteas.md` — public beta + optional partner key vault
- Dex (`d`) — direct Uni V2/V3 routers
