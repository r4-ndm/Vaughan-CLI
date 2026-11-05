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

## Top Walk-Forward Verified Strategies (`H1` & `H2` Profitable)

| Strategy | Market & TF | Trades | Win % | Net `R` (`H1` / `H2`) | Profit Factor | Max DD | Net Return (`@ 2%` risk) |
|---|---|---|---|---|---|---|---|
| `strategies/oil_15m_kst_adx_4r.toml` | `xyz:CL` `15m` | 38 | 39.5% | `+33.11R` (`+12.57R` / `+18.27R`) | `2.20` | `5.59R` (`10.7%`) | **`+84.66%`** |
| `strategies/nvda_15m_crsi_h4_stoch.toml` | `xyz:NVDA` `15m` | 28 | 46.4% | `+27.08R` (`+12.36R` / `+9.08R`) | `2.35` | `5.56R` (`10.6%`) | **`+66.44%`** |
| `strategies/btc_1h_bb_crsi_reversion.toml` | `BTC` `1h` | 45 | 46.7% | `+25.49R` (`+8.95R` / `+15.40R`) | `1.92` | `3.18R` (`6.2%`) | **`+61.61%`** |
| `strategies/btc_1h_crsi_h4_stoch.toml` | `BTC` `1h` | 41 | 41.5% | `+23.87R` (`+13.60R` / `+10.27R`) | `1.85` | `5.28R` (`10.1%`) | **`+55.86%`** |
| `strategies/hype_1h_bb_rsi_t1.toml` | `HYPE` `1h` | 68 | 52.9% | `+22.49R` (`+16.83R` / `+9.10R`) | `1.62` | `7.59R` (`14.2%`) | **`+52.34%`** |
| `strategies/gold_1h_rsi_ma_kst.toml` | `xyz:GOLD` `1h` | 20 | 45.0% | `+18.11R` (`+6.88R` / `+11.22R`) | `2.47` | `3.40R` (`6.7%`) | **`+40.43%`** |

## Quick Start & `Vaughan-CLI` Integration

```bash
# 0. Launch the interactive TUI dashboard (default when run with no subcommand):
cargo run
# (Inside TUI: ↑/↓ or j/k to switch strategies, Tab to scroll trade tape, Space to arm LIVE, r to reload, q to quit)

# 1. Run replay with automatic Walk-Forward (H1 vs H2) split verification:
cargo run -- replay --strategy strategies/oil_15m_kst_adx_4r.toml
cargo run -- replay --strategy strategies/btc_1h_bb_crsi_reversion.toml

# 2. Generate encrypted 0600 Agent Keystore + ApproveAgent EIP-712 JSON:
cargo run -- agent-init --backing-account 0xYourIsolatedSubAccount

# 3. Sign once via Vaughan-CLI (master private key never touches vaughan-hl):
vaughan sign-typed-data --file data/approve_agent.json

# 4. Evaluate latest closed candle and emit deterministic L1 bracket order plan:
cargo run -- tick --strategy strategies/oil_15m_kst_adx_4r.toml --asset-id 110000 --sz-decimals 2

# 5. Emergency kill-switch (emit ApproveAgent(0x0) revocation payload):
cargo run -- kill
```
