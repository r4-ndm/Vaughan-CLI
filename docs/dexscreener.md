# DexScreener research tools

Public market data for Advisor (and later Sentient) — **no API key**.

Patterns inspired by [pulsechain-mcp](https://github.com/DavidFeder/pulsechain-mcp)
research tools; reimplemented in Rust under `vaughan-core::core::dexscreener`
(no TypeScript vendoring).

## Module layout

| Path | Role |
|------|------|
| `vaughan-core/src/core/token_origin/` | e*/p* catalog + labels (never invent origin) |
| `vaughan-core/src/core/dexscreener/chain.rs` | `369 ↔ pulsechain` map |
| `…/types.rs` | Pair summary + soft-fail envelopes |
| `…/search.rs` | Spoof-aware rank + `catalog_coverage` (pure) |
| `…/client.rs` | HTTP + ~200ms outbound spacing |
| `vaughan-agent/src/tools/dexscreener.rs` | MCP sensory wrappers |

## Tools

| Tool | Role |
|------|------|
| `dexscreener_search` | Discovery only — may include ticker spoofs |
| `dexscreener_token_pairs` | Identity by token address |
| `dexscreener_pair` | Identity by pair/LP address |
| `dexscreener_tokens` | Batch token addresses (max 30) |

`resolve_token` and `list_assets` attach `display_symbol` / `token_origin` when
the contract is catalogued on chain **369**.

## Rules for agents

1. **Address beats ticker.** Never settle propose_* identity from search alone.
2. Read `catalog_coverage` and `recommended_address_followups` on empty/spoofed search.
3. Soft-fail JSON `{ ok: false, source: "dexscreener", reason, … }` on network/429 —
   not a hard MCP error (except bad args).
4. Default chain is PulseChain when the session is locked / unset.

## Base URL

`https://api.dexscreener.com` — rate-limit friendliness ~60/min on some routes;
client spaces outbound calls by ~200ms.
