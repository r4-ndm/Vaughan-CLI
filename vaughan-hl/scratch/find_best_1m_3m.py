#!/usr/bin/env python3
import subprocess
import re
import os

def run_test(toml_content):
    path = "scratch/temp_strat.toml"
    with open(path, "w") as f:
        f.write(toml_content)
    cmd = ["vaughan", "replay", "-s", path]
    res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    out = res.stdout
    
    trades = 0
    win_pct = 0.0
    net_r = -999.0
    pf = 0.0
    dd_r = 999.0
    h1_r = -999.0
    h2_r = -999.0
    
    m_trades = re.search(r"Trades\s+:\s+(\d+)", out)
    if m_trades:
        trades = int(m_trades.group(1))
    m_win = re.search(r"Win Rate\s+:\s+([\d\.]+)%", out)
    if m_win:
        win_pct = float(m_win.group(1))
    m_net = re.search(r"Net\s*=\s*([+-]?[\d\.]+)R", out)
    if m_net:
        net_r = float(m_net.group(1))
    m_pf = re.search(r"Profit Factor\s*=\s*([\d\.]+)", out)
    if m_pf:
        pf = float(m_pf.group(1))
    m_dd = re.search(r"Max Drawdown\s*:\s*([\d\.]+)R", out)
    if m_dd:
        dd_r = float(m_dd.group(1))
    m_wf = re.search(r"H1.*?=\s*([+-]?[\d\.]+)R.*?H2.*?=\s*([+-]?[\d\.]+)R", out)
    if m_wf:
        h1_r = float(m_wf.group(1))
        h2_r = float(m_wf.group(2))
        
    return {
        "trades": trades,
        "win_pct": win_pct,
        "net_r": net_r,
        "pf": pf,
        "dd_r": dd_r,
        "h1_r": h1_r,
        "h2_r": h2_r,
        "out": out
    }

# 1. Test KST + RSI + EMA Trend on 3m and 1m (T10 / Jim architecture)
def test_kst_trend(coin, interval):
    best = []
    for rsi_low in [25.0, 30.0, 35.0]:
        for rsi_high in [58.0, 62.0, 68.0]:
            for adx_min in [15.0, 18.0, 22.0]:
                for sl_mult in [2.0, 2.5, 3.0]:
                    for tp_rr in [2.5, 3.0, 4.0]:
                        for t1_rr in [0, 1.0, 1.5]:
                            toml = f"""name = "kst_test"
[market]
coin = "{coin}"
interval = "{interval}"
leverage = 10
is_cross = false
direction = "both"

[sizing]
mode = "risk_pct"
risk_pct = 2.0
initial_equity_usd = 1000.0

[indicators]
rsi = {{ period = 14 }}
kst = {{ r1 = 10, r2 = 15, r3 = 20, r4 = 30, s1 = 10, s2 = 10, s3 = 10, s4 = 15, signal = 9 }}
ema_fast = {{ period = 21 }}
ema_trend = {{ period = 200 }}
adx = {{ period = 14 }}
atr = {{ period = 14 }}

[entry]
max_losses_per_window = 2

[entry.long]
all = [
    "kst_line crosses_above kst_signal",
    "rsi > {rsi_low}",
    "rsi < {rsi_high}",
    "close > ema_trend",
    "adx > {adx_min}",
]

[entry.short]
all = [
    "kst_line crosses_below kst_signal",
    "rsi < {100.0 - rsi_low}",
    "rsi > {100.0 - rsi_high}",
    "close < ema_trend",
    "adx > {adx_min}",
]

[exit]
stop_loss = {{ kind = "atr", multiplier = {sl_mult}, min_stop_bps = 35, max_stop_bps = 300 }}
"""
                            if t1_rr > 0:
                                toml += f"t1 = {{ rr = {t1_rr}, close_pct = 33.3, move_sl_to_breakeven = true }}\n"
                            toml += f"""take_profit_rr = {tp_rr}
cooldown_bars = 2
"""
                            res = run_test(toml)
                            if res["trades"] >= 6 and res["net_r"] > 4.0:
                                best.append((res["h1_r"] > 0 and res["h2_r"] > 0, res["net_r"], res, toml))
    best.sort(key=lambda x: (x[0], x[1]), reverse=True)
    return best

print("Testing KST trend on BTC 3m...")
res_btc_3m = test_kst_trend("BTC", "3m")
for wf, net, r, toml in res_btc_3m[:5]:
    tag = "[WF PASSED]" if wf else "[IS]"
    print(f"{tag} BTC [3m] Net={r['net_r']:+.2f}R Tr={r['trades']} WR={r['win_pct']:.1f}% PF={r['pf']:.2f} H1={r['h1_r']:+.2f}R H2={r['h2_r']:+.2f}R DD={r['dd_r']:.2f}R")

print("\nTesting KST trend on xyz:CL 3m...")
res_cl_3m = test_kst_trend("xyz:CL", "3m")
for wf, net, r, toml in res_cl_3m[:5]:
    tag = "[WF PASSED]" if wf else "[IS]"
    print(f"{tag} xyz:CL [3m] Net={r['net_r']:+.2f}R Tr={r['trades']} WR={r['win_pct']:.1f}% PF={r['pf']:.2f} H1={r['h1_r']:+.2f}R H2={r['h2_r']:+.2f}R DD={r['dd_r']:.2f}R")
