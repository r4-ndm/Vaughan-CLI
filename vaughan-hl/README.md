# vaughan-hl

Modular, allowlist-pure Rust Hyperliquid strategy lab, walk-forward `R:R` backtester, L1 Phantom-Agent wire signer, encrypted `0600` Agent Keystore, and policy-bound execution engine for `Vaughan-CLI`.

> **CRITICAL CUSTODY RULE — AN HL AGENT KEY IS NOT A BURNER:**
> Unlike a spot burner wallet, a Hyperliquid Agent Wallet can trade the entire backing account's equity at leverage. **The agent key must only ever be approved for a dedicated Hyperliquid sub-account (or isolated risk-capital EOA) funded with risk capital only—never your main perps account.**

## Modular Architecture (`src/`)

Designed for minimal binary weight (`unsafe_code = "forbid"`, zero external MsgPack crates, reuses `Vaughan-CLI`'s exact allowlisted dependencies):

- [`src/market_data.rs`](src/market_data.rs) — Closed-candle OHLCV fetcher (`now_ms >= close_time_ms + 2_000`), derived bar aggregation (`10m`), and local JSON cache.
- [`src/indicators/mod.rs`](src/indicators/mod.rs) — $O(N)$ indicator suite: Wilder `RSI` + `RSI_MA`, Martin Pring `KST`, `EMA`, `SMA`, `MACD`, Wilder `ADX` (`+DI`/`-DI`), Wilder `ATR`, `Bollinger Bands`, `StochRSI`, `ConnorsRSI`, `SuperTrend`, `VolumeRatio`, and non-repainting higher-timeframe (`htf`/`htf2`) filters.
- [`src/strategy.rs`](src/strategy.rs) — Declarative `.toml` rule compiler (`all`/`any`, crossovers), `$12` minimum notional guard, and Stop-Loss vs. Liquidation safety guard (`max_stop_bps <= 70%` of estimated isolated liquidation distance).
- [`src/replay.rs`](src/replay.rs) — Deterministic `R:R` simulator (`1R` equity-risk sizing, `T1` partial + breakeven, `T2` runner, conservative intra-bar SL-first resolution, `3.5 bps` fee + `1.5 bps` slippage) and parameter sweep.
- [`src/wire.rs`](src/wire.rs) — Hyperliquid L1 wire formatting (`format_sz`, `format_px`), HIP-3 asset ID resolver, monotonic `NonceAllocator`, zero-dependency deterministic MsgPack encoder, L1 Phantom-Agent EIP-712 signer (`chainId = 1337`), and `ApproveAgent` EIP-712 generator for `vaughan sign-typed-data`.
- [`src/keystore.rs`](src/keystore.rs) — Encrypted `0600` Agent Keystore (`Argon2id` + `AES-256-GCM` + `secrecy` + `zeroize`) bound to the dedicated backing sub-account and network.
- [`src/engine.rs`](src/engine.rs) — Deterministic 128-bit `cloid` generator and closed-candle bracket order builder (`Entry` + `SL` + `T1` + `TP`).
- [`src/tui.rs`](src/tui.rs) — Feature-gated (`#[cfg(feature = "tui")]`) interactive Ratatui terminal dashboard (`HlTuiApp`) displaying the strategy leaderboard, walk-forward (`H1` / `H2`) metrics, latest closed-candle indicators, live tick signal telemetry, and trade tape.

## Public Example Strategy

`vaughan-hl` includes a public reference strategy in [`strategies/example_strategy.toml`](strategies/example_strategy.toml):
- **Market & Timeframe**: `BTC` `1h` (1:2.5 R:R with conservative ATR stop).
- **Indicators**: Bollinger Bands + Wilder RSI + ConnorsRSI exhaustion filters.
- **Walk-Forward Performance**: Net `+17.17R`, `1.59` Profit Factor, verified in both `H1` (`+7.66R`) and `H2` (`+4.86R`).

```bash
# 1. Run replay backtest with Walk-Forward (H1 vs H2) split verification:
cargo run -- replay --strategy strategies/example_strategy.toml

# 2. Paper trade / live simulation:
cargo run -- live --strategy strategies/example_strategy.toml --dry-run
```

To create your own custom strategies, see [`STRATEGY_GUIDE.md`](STRATEGY_GUIDE.md). Custom `.toml` strategies placed in `strategies/` are gitignored by default to protect proprietary trading algorithms.

## Quick Start & `Vaughan-CLI` Integration

```bash
# 0. Launch the interactive TUI dashboard (default when run with no subcommand):
cargo run
# (Inside TUI: ↑/↓ to switch strategies, Space to arm LIVE/SIM, Esc to return, q to quit)

# 1. Run replay backtest:
cargo run -- replay --strategy strategies/example_strategy.toml

# 2. Generate encrypted 0600 Agent Keystore:
cargo run -- agent-init --backing-account 0xYourDedicatedSubAccount

# 3. Sign once via Vaughan-CLI or 1-click connect in TUI:
vaughan sign-typed-data --file data/approve_agent.json

# 4. Evaluate latest closed candle and emit deterministic L1 bracket order plan:
cargo run -- tick --strategy strategies/example_strategy.toml --asset-id 0 --sz-decimals 4

# 5. Emergency kill-switch:
cargo run -- kill
```
