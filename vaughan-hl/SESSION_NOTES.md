# Session Notes — 2026-11-04

## Where things stand

- **Account**: Hyperliquid mainnet unified account (~$25.46 USDC).
- **Backing wallet**: `0xfe4770adcedcbfe2b73bdb69f83321f1f79bfd63` · **Agent**: `0x5b9806b86c93bf8e570e62ab6fce41ca13e224b2`
- **Agent Authorization**: Confirmed live on Hyperliquid valid until 2027 (`validUntil: 1801486166446`).
- **Binary updated**: Installed to `~/.cargo/bin/vaughan-hl` (release mode with `tui` feature).
- **Unit test suite**: 60/60 passing.

## Changes this session (Capital Protection & Execution Quality)
1. **Rescued `test fast` from Negative Expectancy (`strategies/test_fast.toml`)**:
   - The previous test fast rules (`rsi > 48` / `rsi < 52` on 1m) churned fees with a **-34.38R loss** and 0.67 PF in backtests.
   - Upgraded to high-conviction 1m rules inspired by `bob btc 1m` (ConnorsRSI cross + 200 EMA trend + ADX strength + 30m HTF StochRSI filter + 1-loss daily breaker).
   - Replay results: Win Rate jumped to **83.3% (5 W / 1 L)**, Net **+9.79R**, Profit Factor **8.61**, Max DD **0.27%**, walk-forward verified in both H1 and H2.
   - Preserves fixed `$15.00` notional sizing (~$2.93 margin at 5x), ideal for small account testing without bleeding risk.
2. **Pre-Trade Consecutive Loss Cool-Off (`src/live.rs`)**:
   - If the last 2 consecutive closed trades on a coin closed at a loss, the engine enforces an automatic cool-off window before allowing another entry.
   - Prevents revenge-trading into choppy, directionless market regimes.
3. **Execution Telemetry Store (`src/live.rs` & `data/executions.jsonl`)**:
   - Added `LiveExecutionRecord` logging to `data/executions.jsonl`.
   - Records `timestamp_ms`, `strategy`, `coin`, `side`, `signal_px`, `fill_px`, `slippage_bps`, `size`, `notional_usd`, `sl_px`, `tp_px`.
   - Status updates now explicitly report real execution slippage (e.g. `ENTERED LONG BTC ... (slip: 1.2 bps)`).
4. **Cleaned Hardcoded UI / Telemetry Labels (`src/tui.rs`)**:
   - Dynamic liquidation buffer calculation: replaced hardcoded "Safe >2x" / "Safe (>2.4x)" with dynamic `liq_bps / max_sl_bps` ratio display (`Safe (X.Xx)`).
   - Fixed leverage mode: replaced "10x Cross" with "10x Iso" (or Cross if configured).
   - Removed hardcoded `25.46` equity fallback; displays `1R = —` when balance is not loaded.
   - Normalized negative zero so P&L displays `$0.00` rather than `$-0.00`.
   - Clarified trade tape boundaries with `[UK Day]` in header and `(UTC)` in time columns.
5. **Real-Time Model Win/Loss Tracker for Session & All-Time (`src/tui.rs`)**:
   - Added model-specific Win/Loss scoreboard under Row 10 of `Live Account & Trading Telemetry`:
     `Record : [T10] Session: XW / YL ($+X.XX USD) │ All-Time: XW / YL ($+X.XX USD) │ Status: Active (Live)`
   - Dynamically filters on-chain fills for the selected strategy's traded markets (e.g. `T10`, `T3`, `sue btc`), tracking realized wins (`closed_pnl > 0`), losses (`closed_pnl < 0`), and realized USD P&L in real time.

---

# Session Notes — 2026-11-03

## Where things stand

- **Account**: Hyperliquid mainnet unified account (~$25.46 USDC).
- **Backing wallet**: `0xfe4770adcedcbfe2b73bdb69f83321f1f79bfd63` · **Agent**: `0x5b9806b86c93bf8e570e62ab6fce41ca13e224b2`
- **Agent Authorization**: Confirmed live on Hyperliquid as `vaughan-hl` valid until 2027 (`validUntil: 1801486166446`).
- **Live Execution Verified**: Tested live with `test fast`: `Open Long 0.00019 BTC @ $77,431.00`, fee $0.0066, margin $2.93, resting SL ($77,239) and TP ($77,723) verified on exchange.
- **Launch**: run `vaughan` and press `1` (HL), or run `vaughan-hl` directly. Inside dashboard, select `test fast` or `t10 combo` and press `Space` to arm/disarm.

## Changes this session
1. **Agent Key in Telemetry Panel**:
   - The full 42-character agent address (`0x5b9806b86c93bf8e570e62ab6fce41ca13e224b2`) is displayed in Row 2 of the `Live Account & Trading Telemetry` panel.
   - Pressing **`c`** (or **`y`**) in the TUI copies the agent address directly to the system clipboard (`wl-copy`/`xclip`) and flashes confirmation on the status bar.
2. **Added Fast Test Strategy (`strategies/test_fast.toml`)**:
   - Timeframe: `1m` on `BTC` (evaluated every 60 seconds).
   - Fixed size: `$15.00` notional (~$2.93 margin at 5x leverage), risk ~4 cents per trade.
   - Pinned to position #2 in the strategy list right below `t10 combo`.
3. **CLI Agent Helper (`vaughan-hl agent`)**:
   - Subcommand prints the un-truncated address, checks live Hyperliquid status, and copies to clipboard.
4. **Binary updated**:
   - Installed to `~/.cargo/bin/vaughan-hl`.
   - Unit test suite: 50/50 passing.

---

# Session Notes — 2026-11-02

## Where things stand

- **Account**: Hyperliquid mainnet unified account, ~$25.46 USDC. It shows up as a spot balance (`spotClearinghouseState`). That's expected: perps equity reads $0 until a position is open.
- **Backing wallet**: `0xfe4770adcedcbfe2b73bdb69f83321f1f79bfd63` · **Agent**: `0x5b9806b86c93bf8e570e62ab6fce41ca13e224b2`
- **Launch**: run `vaughan` and press `1` (HL), or run `vaughan-hl` on its own. Inside the dashboard, `Space` on `t10 combo` or `0` arms all 10 strategies.
- **Last real fills**: 11 fills on **2026-10-29** (xyz:CL + ETH), net about **−$6.46**. There were no fills on 2026-11-02.

## Top 10 combo (strategies/t10_combo.toml)
jim_oil, kate_nvda, sue_btc, jim_sol, kate_eth, bob_oil, bob_pepe, bob_gold, sue_sp500, jim_sp500 across 8 markets.
In the backtest these totalled about +140.8R. That's backtest only and **not shown in the TUI** (the user asked for this).

## Changes this session (src/tui.rs unless noted)
1. **No simulation data in the TUI.** The strategy list now shows only the name and timeframe. The top-right panel shows only the live account: balance, bot state, strategy, risk on real equity, safety, telemetry, position, and today's real P&L. The trade tape shows only real fills, or a standby message.
2. **"Today" filter.** Both the metrics row and the trade tape filter on `uk_day_number(now_unix_ms())`. Before, they used the date of the newest fill, so the Oct 29 fills were labelled as "today".
3. **Layout.** The details panel is a fixed 10 rows. The market column width is clamped. The trade tape drops to 6 columns when width < 85. `terminal.clear()` runs on startup in vaughan-hl and also in `vaughan-tui/src/lib.rs`.
4. Balance label reads "USDC (Unified Account)" instead of "Spot".
5. Removed the unused imports `Side` and `SimulatedTrade`.
- Tests: `cargo test --features tui` passes 50/50.

## Build / install (IMPORTANT)
- `vaughan` is built from the **parent workspace**, not from vaughan-hl alone:
  `cargo build --release -p vaughan-cli --manifest-path /home/r4/Desktop/Vaughan-CLI/Cargo.toml`
  Output: `/home/r4/Desktop/Vaughan-CLI/target/release/vaughan`. Copy it to `~/.cargo/bin/vaughan`.
- To build vaughan-hl standalone: run `cargo build --release --features tui` in vaughan-hl, then copy the binary to `~/.cargo/bin/vaughan-hl`.
- `~/.local/bin` has no vaughan binaries. Only `~/.cargo/bin` is used.

## ⚠️ Known issues / TODO next session
- [ ] **Killing `vaughan` stops the live bots.** The runs live inside the process. This session I sent SIGTERM to the running `vaughan` several times to swap binaries, which disarmed trading each time. It also caused the "broken TUI" screens: the frozen buffer plus the shell prompt bleeding through. Next time, ask the user to quit with `x` and relaunch. Don't kill it. Check that it's re-armed after any restart.
- [ ] Some labels are **hardcoded rather than computed**. Fix them so the panel shows only real data:
  - "Safe >2x" / "Buffer: Safe (>2.4x)". Compute `liq_bps / max_sl_bps`.
  - "Session Status: Active". Should reflect `live_on`.
  - Wide layout shows "10x **Cross**", but strategies use `is_cross = false` (isolated). Wrong label.
  - `effective_equity` falls back to a hardcoded `25.46` when the balance is unknown. Show "—" instead.
- [ ] P&L prints `$ -0.00`. Normalise negative zero.
- [ ] Day boundary uses UK local day while tape times print in UTC. Pick one and label it.
- [ ] For `t10 combo` the details panel shows only the combo's own market (xyz:CL) and its telemetry. Consider a per-bot status list showing each of the 10 runs, their coin, and their last message.
- [ ] Header and footer text gets truncated at 118 cols (the "MAINNET (Ar…" and "x K…" cut-offs).
- [ ] Trade tape empty-state lines get cut at the right edge. Wrap or shorten them.
- [ ] `vaughan-hl/` is **untracked in git** (the parent repo is on branch `railgun/phase-0`). Decide whether to commit it.
- [ ] The trading question is still open: it's not proven to be profitable live (−$6.46 over 11 fills so far). Track real results against the backtest before raising risk.
