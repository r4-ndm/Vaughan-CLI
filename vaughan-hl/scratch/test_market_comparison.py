#!/usr/bin/env python3
import subprocess
import re
import os

markets = [
    ("BTC", 10),
    ("ETH", 10),
    ("SOL", 10),
    ("kPEPE", 10),
    ("xyz:CL", 20),
    ("xyz:GOLD", 20),
]

def test_config(coin, lev, interval, htf, crsi_thresh=25.0, tp_rr=3.0, t1_rr=1.8, sl_mult=2.0, min_stop=40, max_stop=250):
    safe_coin = coin.replace(":", "_")
    path = f"scratch/temp_{safe_coin}_{interval}.toml"
    
    # Check if candle file exists
    candle_file = f"data/candles/{safe_coin}_{interval}.json"
    if not os.path.exists(candle_file):
        return None
    htf_file = f"data/candles/{safe_coin}_{htf}.json"
    if not os.path.exists(htf_file):
        return None
        
    toml = f"""name = "test_{safe_coin}_{interval}"

[market]
coin = "{coin}"
interval = "{interval}"
leverage = {lev}
is_cross = false
direction = "both"

[sizing]
mode = "risk_pct"
risk_pct = 2.0
initial_equity_usd = 1000.0

[indicators]
rsi = {{ period = 14, ma_period = 14, ma_type = "sma" }}
connors_rsi = {{ rsi_period = 2, streak_period = 4, rank_period = 100 }}
ema_trend = {{ period = 200 }}
adx = {{ period = 14 }}
atr = {{ period = 14 }}

[indicators.htf]
interval = "{htf}"
stoch_rsi = {{ rsi_period = 14, stoch_period = 14, k_smooth = 3, d_smooth = 3 }}

[entry]
max_losses_per_window = 1

[entry.long]
all = [
    "connors_rsi crosses_above {crsi_thresh}",
    "rsi > rsi_ma",
    "close > ema_trend",
    "adx > 18.0",
    "plus_di > minus_di",
    "htf_stoch_k > htf_stoch_d",
]

[entry.short]
all = [
    "connors_rsi crosses_below {100.0 - crsi_thresh}",
    "rsi < rsi_ma",
    "close < ema_trend",
    "adx > 18.0",
    "minus_di > plus_di",
    "htf_stoch_k < htf_stoch_d",
]

[exit]
stop_loss = {{ kind = "atr", multiplier = {sl_mult}, min_stop_bps = {min_stop}, max_stop_bps = {max_stop} }}
t1 = {{ rr = {t1_rr}, close_pct = 33.3, move_sl_to_breakeven = true }}
take_profit_rr = {tp_rr}
cooldown_bars = 2
"""
    with open(path, "w") as f:
        f.write(toml)
        
    res = subprocess.run(["vaughan", "replay", "-s", path], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    out = res.stdout
    if os.path.exists(path):
        os.remove(path)
        
    m_tr = re.search(r"Trades\s+:\s+(\d+)\s+\(Long:\s*(\d+),\s*Short:\s*(\d+)\)", out)
    m_wr = re.search(r"Win Rate\s+:\s+([\d\.]+)%", out)
    m_net = re.search(r"Net\s*=\s*([+-]?[\d\.]+)R", out)
    m_pf = re.search(r"Profit Factor\s*=\s*([\d\.]+)", out)
    m_dd = re.search(r"Max Drawdown\s*:\s*([\d\.]+)R", out)
    m_usd = re.search(r"Net:\s*([+-]?[\d\.]+)\s*USD\s*/\s*([+-]?[\d\.]+)%,\s*Fees:\s*\$([\d\.]+)", out)
    m_wf = re.search(r"H1.*?=\s*([+-]?[\d\.]+)R.*?H2.*?=\s*([+-]?[\d\.]+)R", out)
    
    if not m_tr:
        return None
        
    return {
        "coin": coin,
        "interval": interval,
        "htf": htf,
        "trades": int(m_tr.group(1)),
        "longs": int(m_tr.group(2)),
        "shorts": int(m_tr.group(3)),
        "win_rate": float(m_wr.group(1)) if m_wr else 0.0,
        "net_r": float(m_net.group(1)) if m_net else -999.0,
        "pf": float(m_pf.group(1)) if m_pf else 0.0,
        "dd_r": float(m_dd.group(1)) if m_dd else 999.0,
        "net_usd": float(m_usd.group(1)) if m_usd else 0.0,
        "fees_usd": float(m_usd.group(3)) if m_usd else 0.0,
        "h1_r": float(m_wf.group(1)) if m_wf else 0.0,
        "h2_r": float(m_wf.group(2)) if m_wf else 0.0,
    }

print("=" * 80)
print("3m TIMEFRAME COMPARISON ACROSS ALL MARKETS (HTF = 1h)")
print("=" * 80)
for coin, lev in markets:
    r = test_config(coin, lev, interval="3m", htf="1h", crsi_thresh=25.0, tp_rr=3.0, t1_rr=1.8, sl_mult=2.0)
    if r:
        wf = "✓ BOTH H1 & H2" if (r["h1_r"] > 0 and r["h2_r"] > 0) else "✗ ONE-SIDED/NEG"
        print(f"{coin:<10} [3m] | Net R: {r['net_r']:+6.2f}R | WR: {r['win_rate']:4.1f}% ({r['trades']} tr) | PF: {r['pf']:4.2f} | MaxDD: {r['dd_r']:4.2f}R | Fees: ${r['fees_usd']:5.2f} | H1: {r['h1_r']:+5.2f}R, H2: {r['h2_r']:+5.2f}R | {wf}")
    else:
        print(f"{coin:<10} [3m] | Missing candles or 0 trades")

print("\n" + "=" * 80)
print("1m TIMEFRAME COMPARISON (HTF = 30m or 1h)")
print("=" * 80)
for coin, lev in [("BTC", 10), ("ETH", 10), ("SOL", 10), ("xyz:CL", 20)]:
    # for 1m test both 30m and 1h htf
    for htf in ["30m", "1h"]:
        r = test_config(coin, lev, interval="1m", htf=htf, crsi_thresh=30.0, tp_rr=2.5, t1_rr=1.8, sl_mult=2.0, min_stop=35)
        if r:
            wf = "✓ BOTH H1 & H2" if (r["h1_r"] > 0 and r["h2_r"] > 0) else "✗ ONE-SIDED/NEG"
            print(f"{coin:<10} [1m] HTF={htf:<3} | Net R: {r['net_r']:+6.2f}R | WR: {r['win_rate']:4.1f}% ({r['trades']} tr) | PF: {r['pf']:4.2f} | MaxDD: {r['dd_r']:4.2f}R | Fees: ${r['fees_usd']:5.2f} | H1: {r['h1_r']:+5.2f}R, H2: {r['h2_r']:+5.2f}R | {wf}")
        else:
            print(f"{coin:<10} [1m] HTF={htf:<3} | Missing candles or 0 trades")
