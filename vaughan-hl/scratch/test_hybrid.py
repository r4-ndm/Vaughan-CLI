#!/usr/bin/env python3
import subprocess
import re

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

# Test KST + SuperTrend + ConnorsRSI
for coin in ["BTC"]:
    for interval in ["3m"]:
        for kst_r1 in [5, 8, 10]:
            for st_mult in [2.0, 3.0]:
                for sl_mult in [1.5, 2.0, 2.5]:
                    for tp_rr in [2.5, 3.0]:
                        toml = f"""name = "hybrid_3m"

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
kst = {{ r1 = {kst_r1}, r2 = 12, r3 = 18, r4 = 24, s1 = 8, s2 = 8, s3 = 8, s4 = 10, signal = 8 }}
supertrend = {{ period = 10, multiplier = {st_mult} }}
ema_trend = {{ period = 200 }}
connors_rsi = {{ rsi_period = 2, streak_period = 3, rank_period = 100 }}
atr = {{ period = 14 }}

[entry]
max_losses_per_window = 1

[entry.long]
all = [
    "close > supertrend",
    "close > ema_trend",
    "connors_rsi crosses_above 25.0",
    "kst_line > kst_signal",
]

[entry.short]
all = [
    "close < supertrend",
    "close < ema_trend",
    "connors_rsi crosses_below 75.0",
    "kst_line < kst_signal",
]

[exit]
stop_loss = {{ kind = "atr", multiplier = {sl_mult}, min_stop_bps = 35, max_stop_bps = 250 }}
t1 = {{ rr = 1.5, close_pct = 33.3, move_sl_to_breakeven = true }}
take_profit_rr = {tp_rr}
cooldown_bars = 2
"""
                        res = run_test(toml)
                        if res["trades"] >= 5 and res["net_r"] >= 4.0:
                            wf = res["h1_r"] > 0 and res["h2_r"] > 0
                            tag = "[WF PASSED]" if wf else "[IS]"
                            print(f"{tag} KST_r1={kst_r1} ST_mult={st_mult} SL={sl_mult} TP={tp_rr} | Net={res['net_r']:+.2f}R | Tr={res['trades']} | WR={res['win_pct']:.1f}% | PF={res['pf']:.2f} | H1={res['h1_r']:+.2f}R | H2={res['h2_r']:+.2f}R | DD={res['dd_r']:.2f}R")
