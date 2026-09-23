# DCA (recurring buys) — TUI-hosted

Phase 1: **time-based** dollar-cost averaging on PulseChain. You leave
`vaughan` open (Sentient profile), create a plan on the DCA screen (`o`), and
watch countdown + slice log. Closing the TUI pauses execution.

## Module map

| Crate / path | Role |
|---|---|
| `vaughan-agent/src/dca/types.rs` | `DcaPlan`, triggers, status, **route** (`agg` / `dex`) |
| `…/store.rs` | Atomic `dca-plans.json` (0600, fail-closed) |
| `…/trigger.rs` | Pure “is due?” / catch-up schedule math |
| `…/build.rs` | Validate + assemble plans |
| `…/engine.rs` | Agg quote **or** DEX V2 quote+calldata → `SentientTrader` → record |
| `vaughan-tui/src/views/dca.rs` | UI only (route + venue pickers) |
| MCP `propose_dca_plan` / `list_dca_plans` / `cancel_dca_plan` | Thin wrappers |

Execution reuses battle-tested paths: `quote_aggregator` **or**
`quote_v2_exact_in` + Uni V2 `swapExactETHForTokens`, then
`SentientTrader::execute_swap` (DEX **and** aggregator allowlists). Do not
duplicate router / fee / breaker logic in the view.

## Route: aggregator vs DEX

Some memes never appear on aggregators. At create time pick:

| Route | Venue picker | Execution |
|---|---|---|
| **Aggregator** | `auto` (Squirrel) · squirrel · pulseswap · piteas · empx · 9mm | `quote_aggregator` |
| **DEX (direct)** | Catalog V2 routers on the active chain (PulseX, 9inch, …) | `getAmountsOut` + `swapExactETHForTokens` |

MCP: `route=agg|dex`, `venue=<slug>`, `protocol=v2` (DEX only; V3 DCA not wired yet).

## How to use

1. Unlock a **Sentient** profile (`vaughan --profile sentient`).
2. Press **`o`** → DCA.
3. **`n`** new plan: token out (↑↓ or paste), PLS per slice, interval (15m–24h),
   **route** (agg ↔ DEX), **venue** (←/→), max slices, dry-run toggle (default on).
4. Confirm. First slice is eligible immediately; then every interval.
5. **`p`** pause/resume · **`c`** cancel · chrome flash on each fire.
6. Leave the TUI open. Esc / other screens are fine — the App tick still polls
   due plans while unlocked in Sentient mode.

Non-sentient profiles can create/list/pause plans but will **not** auto-fire.

## Safety

- Circuit breakers (position %, slippage, session gas, consecutive errors).
- Pause after 3 consecutive slice failures (resume with **`p`**).
- At most one catch-up slice after sleep (no burst-buying).
- Ctrl+K trips the session breaker (halts DCA + MCP auto-exec).
- Prefer `dry_run=true` on testnet / first mainnet rehearsal.
- Plans bind **`chain_id` + wallet address** at create; fire refuses on mismatch.
- Aggregator route is **PulseChain mainnet (369) only**; DEX route uses
  per-venue wrapped native (WETHW vs WETH on ETHW).
- Slice `tx.value` must equal the plan amount; empty router bytecode is refused.
- Cancel/pause during a fire is respected (outcome write reloads by id).

## Threat model (plan file)

`dca-plans.json` is not secret key material, but a tampered `token_out`
redirects buys. Same trust boundary as the MCP session token (same OS user).
File mode 0600; unknown JSON fields fail closed. Bound `chain_id` / `account`
limit cross-network and cross-wallet spend if the file is edited in place.

## Phase 2 (not built yet) — indicator buys/sells

Same UI/engine; trigger becomes RSI (etc.) instead of a clock.

**Spike before coding:**

1. OHLCV source for PulseChain (leading candidate: GeckoTerminal public API —
   DexScreener has no candles).
2. RSI crate (prefer battle-tested, e.g. `yata` with Wilder/TA-Lib parity) —
   **allowlist approval required**; do not hand-roll indicator math.
3. `IndicatorTrigger` behind the same due-check seam as `Time` triggers.
4. Sells need ERC-20 → native (approve + swap) — likely Phase 2b.

## Later

- Headless `vaughan serve` DCA (run without TUI).
- On-chain DCA vault + keepers (true power-off set-and-forget).
- Direct DEX V3 path for DCA (today: use aggregator for V3 venues).
