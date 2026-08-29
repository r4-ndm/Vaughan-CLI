---
name: vb-ag-quotes
description: >-
  HOW TO quote PulseChain Ag catalog venues. Read when user wants aggregator quotes,
  PLS→HEX comparison, Switch.win without API key, browser_open_agg, quote tour across
  Ag screen venues, or VB human path vs quote_swap. Canonical path:
  vaughan-agent/skills/vb-ag-quotes/SKILL.md
mode: all
kind: guide
---

# VB Ag quotes

> **Any agent:** this file is the single source of truth. Venue details:
> [`venues/INDEX.md`](venues/INDEX.md). Skills catalog: [`../INDEX.md`](../INDEX.md).

## Quick start (60 seconds)

1. **Unlock** Vaughan on PulseChain (369) — TUI or headless serve:
   `vaughan serve --password-env VAUGHAN_WALLET_PASSWORD`
2. **Enable** agent browser: `vaughan config agent-browser on` — then **reconnect MCP** ([`mcp-connect`](../mcp-connect/SKILL.md)).
3. **One-shot VB quote** (agent, no manual steps):
   `browser_open_agg { venue, token_in: "PLS", token_out: "HEX", amount_in: "1" }`
   → opens `/dapp`, dismisses consent modals, selects tokens in iframe, sets amount, clicks quote CTA.
   Wallet connect runs **only when the control plane is unlocked** (`connect_wallet` defaults to that).
4. **Browserless fast path** (Squirrel / PulseSwap / Piteas / EmpX): MCP `quote_swap` with `amount_in` in **wei**.
5. **Read quote:** `browser_snapshot` or `browser_read_quote` (visible body text, all frames) or parse `setup_swap` in the `browser_open_agg` response.

Never auto-sign swaps — user approves in the TUI (or sentient policy on `serve`).

### Autonomous agent checklist

| Step | MCP tool | Pass when |
|------|----------|-----------|
| 0 | `get_control_plane_status` | `wallet_unlocked: true` for Switch / 9mm (wallet-gated quotes) |
| 1 | `browser_open_agg` | `cdp_alive: true`, `setup_swap.input/output.pick.ok` |
| 2 | `browser_snapshot` / `browser_read_quote` | Output leg shows non-zero HEX (`quote.summary` or `quote.best`) |

**Do not** use `setup_tokens: false` unless you intend manual `browser_select_token` — the old step-by-step path is for debugging only.

## When to use this skill

Load this playbook when the user (or task) mentions any of:

- Compare quotes across **aggregators** / **Ag screen** / **Ag catalog**
- **PLS → HEX** (or other pair) quote tour
- **Switch.win** / **CURV** without developer API key
- **`browser_open_agg`**, **`browser_snapshot`**, VB + CDP quote flow
- “Quote like a human” / web UI instead of API

## Two paths (pick one)

| Path | MCP tools | Use when |
|------|-----------|----------|
| **Browserless** | `quote_swap`, `propose_agg_swap` | Squirrel, PulseSwap, Piteas, EmpX — fast, no browser |
| **VB human** | `browser_open_agg`, `browser_snapshot`, `browser_click`, `browser_type`, `browser_wait` | Switch.win (API gated), token pickers, visual check |

EmpX and PortalX have **no public swap web UI** — always `quote_swap` for those.

## Prerequisites

| # | Requirement |
|---|-------------|
| 1 | Unlocked TUI on chain **369** |
| 2 | `vaughan config agent-browser on` or Settings → **`p`** |
| 3 | `vaughan-dapp-browser` on `PATH` |
| 4 | MCP env: `VAUGHAN_DAPP_BROWSER_CHROME`, optional `VAUGHAN_DAPP_BROWSER_CDP_PORT=9222` |
| 5 | Restart MCP host after config changes |

Docs: [`docs/vb-mcp-smoke.md`](../../docs/vb-mcp-smoke.md) · [`docs/mcp.md`](../../docs/mcp.md)

## Standard VB quote flow (PLS → HEX)

```
browser_open_agg { venue, token_in: "PLS", token_out: "HEX", amount_in: "1" }
  → opens swap UI + runs token pickers (not page defaults)
  → browser_snapshot → read output HEX / route
```

Or step-by-step after any `browser_open`:

```
browser_setup_swap { token_in: "PLS", token_out: "HEX", amount_in: "1" }
browser_select_token { symbol: "HEX", side: "output" }   # single leg
```

**Do not** assume deep links or landing defaults picked the right pair — always
call `browser_setup_swap` or explicit `browser_select_token` for both legs.

```
browser_open_agg { venue, pls_hex: true }   # legacy — still runs setup_tokens by default
  → browser_wait { text: "Swap" }
  → browser_snapshot → dismiss modals (browser_click)
  → connect wallet if needed (TUI prompt)
  → browser_type { ref, text: "1", clear: true }   # only if amount not set by setup
  → browser_snapshot → read output HEX / route
```

**Venue aliases** for `browser_open_agg`: `squirrel`, `pulseswap`, `piteas`, `switch`, `9mm`, `curv`, `internetmoney`, `libertyx`, `empx`, `portalx`.

**Next venue:** kill VB + clear `~/.local/share/vaughan-cli/vb.session`, or `browser_navigate` to the next catalog URL.

## Venue playbooks

See **[`venues/INDEX.md`](venues/INDEX.md)** — one markdown file per Ag catalog venue.

## Code / catalog references

| What | Where |
|------|--------|
| Web URLs | `vaughan-core/src/core/aggregator/catalog.rs` — `web_url()`, `web_url_pls_hex()` |
| Ag TUI status `· VB: …` | `vaughan-tui/src/views/ag.rs` |
| CDP snapshot / React typing | `vaughan-core/src/core/vb_cdp.rs` |
| MCP tool | `vaughan-mcp/src/browser_bridge.rs` — `browser_open_agg` |

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| No `browser_*` tools | Enable agent browser; restart MCP |
| `chrome-error://` | Add origin to trusted dApps (Settings) |
| Snapshot only nav links | Use current `vb_cdp` (inputs-first snapshot) |
| Typed amount, no quote | `clear: true`; pick HEX token; read venue file |
| `browser_open` doesn’t navigate | Fresh VB session or `browser_navigate` |
| Switch `quote_swap` fails | Expected — use VB + `venue: switch` |

## Related skills & docs

- Connect / inject: [`dapp-connect`](../dapp-connect/SKILL.md)
- Token addresses: [`pulsechain-context`](../pulsechain-context/SKILL.md)
- Aggregator matrix: [`docs/aggregator.md`](../../docs/aggregator.md)
- MCP tools: [`docs/ai-tool-surface.md`](../../docs/ai-tool-surface.md)

Update venue markdown when you learn a new quirk — do not rely on chat memory alone.
