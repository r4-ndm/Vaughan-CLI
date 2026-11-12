# Vaughan-HL Strategy & Agent Developer Guide

This document is the authoritative reference for human traders and autonomous AI agents building, backtesting, and deploying trading strategies in `vaughan-hl`.

---

## 1. Core Philosophy: Why Vaughan-HL Strategies Make Money

1. **Asymmetric R:R (Risk vs Reward)**:
   - We do not chase high win-rates with negative risk:reward (the mistake typical trading bots make).
   - High win rates (70-80%) with wide stops and tight take-profits inevitably suffer catastrophic tail drawdowns.
   - We aim for **1:2.5 to 1:4.0 R:R**. At 1:3.5 R:R, even a 35% win rate generates substantial positive mathematical expectancy (+0.575R per trade).

2. **Zero Repainting & Strict Closed Candles**:
   - Every indicator is evaluated solely on **confirmed, closed candles**.
   - Higher-timeframe indicators (`htf`, `htf2`) update only on the close of their parent timeframe bar, guaranteeing zero lookahead bias.

3. **Walk-Forward Invariance (`H1` vs `H2`)**:
   - The backtester splits historical data into two halves: In-Sample (`H1`) and Out-of-Sample (`H2`).
   - A strategy is only viable if **both H1 and H2 produce net positive R**. Single-half curve-fitting is automatically flagged.

4. **Realistic Friction Modeling**:
   - Every simulated trade pays **3.5 bps taker fee + 1.5 bps slippage** on both entry and exit (10 bps round-trip).
   - `min_stop_bps` ensures fees never consume more than ~12% of 1R risk.
   - SL-first conservative bar resolution resolves intra-bar ambiguity defensively.

---

## 2. Complete Strategy TOML Specification

A strategy file is a clean, declarative `.toml` file placed in the `strategies/` directory.

### Minimal Working Example
```toml
name = "ETH 15m — KST Trend + ADX Momentum (1:3.0 R:R)"

[market]
coin = "ETH"             # e.g. "BTC", "ETH", "SOL", "HYPE", "xyz:CL", "xyz:GOLD", "xyz:NVDA"
interval = "15m"         # "1m", "3m", "5m", "10m", "15m", "30m", "1h", "4h", "1d"
leverage = 10            # 1 to 50
is_cross = false         # Isolated margin recommended
direction = "both"       # "long_only", "short_only", or "both"

[sizing]
mode = "risk_pct"        # "risk_pct" or "fixed_usd"
risk_pct = 2.0           # 2% equity risk per trade (1R)
initial_equity_usd = 1000.0

[indicators]
rsi = { period = 14 }
kst = { r1 = 10, r2 = 15, r3 = 20, r4 = 30, s1 = 10, s2 = 10, s3 = 10, s4 = 15, signal = 9 }
adx = { period = 14 }
atr = { period = 14 }

[entry.long]
all = [
    "kst_line crosses_above kst_signal",
    "rsi > 35.0",
    "rsi < 65.0",
    "adx > 18.0",
]

[entry.short]
all = [
    "kst_line crosses_below kst_signal",
    "rsi < 65.0",
    "rsi > 35.0",
    "adx > 18.0",
]

[exit]
stop_loss = { kind = "atr", multiplier = 2.0, min_stop_bps = 50, max_stop_bps = 300 }
take_profit_rr = 3.0
cooldown_bars = 2
```

---

## 3. Supported Indicators

Configure any of the following inside the `[indicators]` block:

| Indicator Key | Description | Configuration Syntax | Available Rule Tokens |
|---|---|---|---|
| `rsi` | Wilder RSI | `rsi = { period = 14 }` | `rsi` |
| `rsi_ma` | Smoothed RSI | `rsi_ma = { rsi_period = 14, ma_period = 9, ma_type = "ema" }` | `rsi_line`, `rsi_ma_line` |
| `kst` | Martin Pring KST | `kst = { r1=10, r2=15, r3=20, r4=30, s1=10, s2=10, s3=10, s4=15, signal=9 }` | `kst_line`, `kst_signal`, `kst_hist` |
| `adx` | Wilder ADX (+DI/-DI) | `adx = { period = 14 }` | `adx`, `plus_di`, `minus_di` |
| `atr` | Average True Range | `atr = { period = 14 }` | `atr` |
| `bollinger` | Bollinger Bands | `bollinger = { period = 20, std_dev = 2.0 }` | `bb_upper`, `bb_lower`, `bb_mid` |
| `stoch_rsi` | Stochastic RSI | `stoch_rsi = { rsi_period = 14, stoch_period = 14, k = 3, d = 3 }` | `stoch_rsi_k`, `stoch_rsi_d` |
| `connors_rsi` | Larry Connors RSI(3,2,100) | `connors_rsi = { rsi_period = 3, streak_period = 2, rank_period = 100 }` | `crsi` |
| `supertrend` | SuperTrend Direction | `supertrend = { atr_period = 10, multiplier = 3.0 }` | `supertrend`, `supertrend_direction` (+1 / -1) |
| `volume_ratio` | Volume vs SMA(Vol) | `volume_ratio = { period = 20 }` | `volume_ratio` |
| `ema` / `sma` | Moving Averages | `ema = { period = 200 }` | `ema`, `sma` |
| `htf` | Higher Timeframe | `htf = { interval = "4h", stoch_rsi = { ... } }` | `htf_stoch_rsi_k`, `htf_stoch_rsi_d` |

Price bar tokens always available: `close`, `open`, `high`, `low`, `volume`.

---

## 4. Rule Condition Grammar

The `[entry.long]` and `[entry.short]` tables support `all = [...]` (AND) and `any = [...]` (OR) arrays.

### Supported Operators
- `>` (greater than)
- `<` (less than)
- `>=` (greater than or equal)
- `<=` (less than or equal)
- `crosses_above` (bullish crossover on this exact closed bar)
- `crosses_below` (bearish crossunder on this exact closed bar)

### Examples
- `"kst_line crosses_above kst_signal"`
- `"close > bb_upper"`
- `"crsi < 15.0"`
- `"adx > 20.0"`
- `"close > ema"`

---

## 5. Advanced Trade Management (`[exit]`)

The `[exit]` table provides production-grade risk management:

```toml
[exit]
# Stop-loss: "atr" with multiplier, or "pct" with fixed percentage
stop_loss = { kind = "atr", multiplier = 2.5, min_stop_bps = 75, max_stop_bps = 350 }

# Primary full take profit (e.g. 3.5x risk)
take_profit_rr = 3.5

# Optional Partial TP (T1): take off 50% size at 1.5R
partial_tp_rr = 1.5
partial_tp_pct = 50.0

# Optional Breakeven Move: move stop to entry price once trade reaches 1.0R
breakeven_trigger_rr = 1.0

# Cooldown: minimum bars between trade exit and next entry to prevent chop whipsaws
cooldown_bars = 2
```

---

## 6. Workflow for Autonomous AI Agents

When an agent is tasked with building or improving a strategy, it should follow this 3-step loop:

### Step 1: Ensure Market Data is Cached
```bash
# Data is cached in data/candles/<coin>_<interval>.json
# If missing, fetch from Hyperliquid public API:
cargo run -p vaughan-hl -- fetch --coin SOL --interval 15m --days 180
```

### Step 2: Write Strategy TOML to `strategies/`
Create a new file, for example: `strategies/sol_15m_kst_adx_3r.toml`.

### Step 3: Run Replay and Evaluate Walk-Forward Metrics
```bash
cargo run -p vaughan-hl -- replay --strategy strategies/sol_15m_kst_adx_3r.toml
```

### Agent Success Criteria
A newly created strategy should meet the following benchmarks before being recommended to the user:
1. **Both H1 and H2 Net R > 0** (proves resilience across different market regimes).
2. **Profit Factor >= 1.60**.
3. **Max Drawdown <= 12.0%** (under 2% risk sizing).
4. **Win Rate between 35% and 55%** (aligned with healthy 1:3+ R:R expectancy).
5. **Trade Count >= 25** over 6+ months (statistical significance).
