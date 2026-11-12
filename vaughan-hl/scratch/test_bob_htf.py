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

results = []

for coin in ["xyz:CL", "BTC"]:
    for interval in ["3m", "1m"]:
        for htf in ["15m", "30m", "1h", "4h"]:
            for crsi_len in [1, 2, 3]:
                for crsi_thresh in [25.0, 30.0, 35.0]:
                    for sl_mult in [1.5, 2.0, 2.5]:
                        for tp_rr in [2.5, 3.15, 4.0]:
                            for t1_pct in [33.3, 50.0]:
                                lev = 20 if coin.startswith("xyz") else 10
                                toml = f"""name = "bob_{coin}_{interval}_{htf}"

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
connors_rsi = {{ rsi_period = {crsi_len}, streak_period = 4, rank_period = 100 }}
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
    "htf_stoch_k < 55.0",
]

[entry.short]
all = [
    "connors_rsi crosses_below {100.0 - crsi_thresh}",
    "rsi < rsi_ma",
    "close < ema_trend",
    "adx > 18.0",
    "minus_di > plus_di",
    "htf_stoch_k < htf_stoch_d",
    "htf_stoch_k > 45.0",
]

[exit]
stop_loss = {{ kind = "atr", multiplier = {sl_mult}, min_stop_bps = 45, max_stop_bps = 250 }}
t1 = {{ rr = 1.8, close_pct = {t1_pct}, move_sl_to_breakeven = true }}
take_profit_rr = {tp_rr}
cooldown_bars = 2
"""
                                res = run_test(toml)
                                if res["trades"] >= 5 and res["net_r"] >= 4.0 and res["pf"] >= 1.5:
                                    is_wf = res["h1_r"] > 0 and res["h2_r"] > 0
                                    results.append((is_wf, res["net_r"], res["pf"], coin, interval, htf, crsi_len, crsi_thresh, sl_mult, tp_rr, t1_pct, res))

results.sort(key=lambda x: (x[0], x[1]), reverse=True)
print(f"Total strong configs: {len(results)}")
for r in results[:20]:
    wf, net, pf, coin, interval, htf, clen, cth, sl, tp, t1p, res = r
    tag = "★ WALK-FORWARD PASSED ★" if wf else "  IS ONLY              "
    print(f"{tag} | {coin} [{interval}] HTF={htf} CRSI={clen}@{cth} SL={sl}x TP={tp}R T1={t1p}% | Net={net:+.2f}R | PF={pf:.2f} | Tr={res['trades']} | WR={res['win_pct']:.1f}% | H1={res['h1_r']:+.2f}R | H2={res['h2_r']:+.2f}R | DD={res['dd_r']:.2f}R")
