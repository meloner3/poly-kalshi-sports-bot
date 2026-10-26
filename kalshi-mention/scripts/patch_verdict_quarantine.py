"""Patch: quarantine historical-tier quotes from the verdict. Era split proved
pre-cutoff candle quotes are placeholder junk (median ask 0.6c, market
logloss 2.25) while live-era quotes are sane (0.544)."""
from pathlib import Path

p = Path("kalshi_mention/verdict.py")
src = p.read_text()
assert "HIST_CUTOFF" not in src, "already patched"

src = src.replace('B_BOOT = 4000',
'''B_BOOT = 4000
HIST_CUTOFF = "2026-05-27"  # Kalshi historical-tier boundary
_MONTHS = {m: i + 1 for i, m in enumerate(
    ["JAN","FEB","MAR","APR","MAY","JUN","JUL","AUG","SEP","OCT","NOV","DEC"])}


def _day_to_date(d):
    if not isinstance(d, str) or len(d) < 7:
        return None
    core = d[-7:]
    try:
        return "20%s-%02d-%02d" % (core[:2], _MONTHS[core[2:5]], int(core[5:7]))
    except (KeyError, ValueError):
        return None''')

src = src.replace('''    t = add_trades(snaps, model)

    lines = ["# Divergence Verdict (day-clustered)", ""]''',
'''    t = add_trades(snaps, model)
    t["date"] = t["event_day"].map(_day_to_date)
    n_hist = int((t["date"] < HIST_CUTOFF).sum())
    t = t[t["date"] >= HIST_CUTOFF]

    lines = ["# Divergence Verdict (day-clustered, LIVE-ERA QUOTES ONLY)", ""]
    lines.append(f"Quarantined {n_hist} historical-tier trade rows (pre-{HIST_CUTOFF}):")
    lines.append("era-split analysis showed their quotes are placeholders (median ask")
    lines.append("under a cent, market logloss 2.2) while live-era quotes score 0.54.")
    lines.append("Historical markets still power Component A; only their PRICES are untrusted.")
    lines.append("")''')

p.write_text(src)
print("Patched. Verdict now runs on live-era quotes only and says so in the report.")