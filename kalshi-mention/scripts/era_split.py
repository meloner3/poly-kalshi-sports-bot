import numpy as np, pandas as pd
from kalshi_mention.verdict import add_trades, cluster_boot

snaps = pd.read_csv("market_baseline/market_baseline_snapshots.csv")
preds = pd.read_csv("component_a/component_a_predictions.csv")
snaps = snaps.merge(preds[["ticker", "event_type", "n_prior_hits"]], on="ticker", how="left")
t = add_trades(snaps, "poisson_hl30_cal")

MONTHS = {m: i + 1 for i, m in enumerate(
    ["JAN","FEB","MAR","APR","MAY","JUN","JUL","AUG","SEP","OCT","NOV","DEC"])}

def day_to_date(d):
    if not isinstance(d, str) or len(d) < 7:
        return None
    core = d[-7:]
    return "20%s-%02d-%02d" % (core[:2], MONTHS[core[2:5]], int(core[5:7]))

t["date"] = t["event_day"].map(day_to_date)
t["era"] = np.where(t["date"] >= "2026-05-27", "live", "historical")

for era, g in t.groupby("era"):
    sel = g[(g["h"] == 6) & (g["edge"].abs() > 0.10)]
    if len(sel) == 0:
        print(era, ": no trades")
        continue
    y = sel["y"].to_numpy(float)
    pm = np.clip(sel["p_market"].to_numpy(float), 1e-6, 1 - 1e-6)
    ll = float(-(y * np.log(pm) + (1 - y) * np.log(1 - pm)).mean())
    m, lo, hi = cluster_boot(sel) if len(sel) >= 30 else (float("nan"),) * 3
    print("%-10s trades=%5d days=%4d net=%+.4f CI=[%+.4f,%+.4f] mkt_logloss=%.3f med_ask=%.3f yes_rate=%.2f"
          % (era, len(sel), sel["event_day"].nunique(), m, lo, hi, ll,
             float(sel["ask"].median()), float(y.mean())))