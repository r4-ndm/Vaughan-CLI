# Vaughan-HL Strategies

This directory contains declarative TOML strategy files for `vaughan-hl`.

## Quick Start
You can backtest the public reference strategy:
```bash
vaughan-hl replay --strategy strategies/example_strategy.toml
```

Or run it in live simulation / paper trading:
```bash
vaughan-hl live --strategy strategies/example_strategy.toml --dry-run
```

## Creating Your Own Strategies
Vaughan-HL evaluates strategies on confirmed closed candles with zero repainting.

See [`STRATEGY_GUIDE.md`](../STRATEGY_GUIDE.md) for full documentation on supported indicators (RSI, ConnorsRSI, KST, MACD, Bollinger Bands, ATR, Supertrend, ADX, StochRSI, HTF slope filters), order brackets, multi-target take-profits, and session time filters.

> **Note**: Custom and proprietary strategies placed in this directory are gitignored by default to protect trader IP.
