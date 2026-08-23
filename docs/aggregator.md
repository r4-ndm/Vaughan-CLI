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
| Router (from `tx.to`) | e.g. `0xDa8953Fc615d6E816b9647Afd5536123dcE70B78` (always use response) |

Vaughan flow: ↑/↓ → amount → **Enter** → `POST /swap` → approve if
`approvalNeeded` → confirm → sign. User always approves.

## Other live (also no key)

| Venue | API |
|---|---|
| PulseSwap / AggreGate | `quotes.pulseswap.io` advanced |
| Piteas | `sdk.piteas.io/quote` (beta rate limits) |

## Listed only

Switch.win (needs key), Empseal, 9mm 9X, CURV, Internet Money, LibertyX, PortalX.

## Related

- `docs/piteas.md` — partner key vault when/if issued
- Dex (`d`) — direct Uni V2/V3 routers
