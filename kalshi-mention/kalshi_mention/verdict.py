"""Statistical verdict on the divergence backtest (the analysis that decides).

    python -m kalshi_mention.verdict --snapshots market_baseline/market_baseline_snapshots.csv \
        --predictions component_a/component_a_predictions.csv --out verdict

Trades on the same event-day share one transcript and are one observation in
disguise: naive per-trade standard errors overstate significance massively
(pass 3: 670 "trades" were 39 independent days, and an apparent +0.023 edge
was one lucky day). This module renders the honest verdict:

  - day-clustered bootstrap 95% CIs on net P&L for every snapshot x threshold
  - day-concentration check (does removing the best 3 days flip the sign?)
  - decomposition of the widest-threshold pocket by side, entry cost, phrase
    history, event type, and series

A cell "passes" only if its clustered CI lies entirely above zero AND survives
best-day removal. Anything else is reported as not distinguishable from noise.
"""

from __future__ import annotations

import argparse
import math
from pathlib import Path

import numpy as np
import pandas as pd

from .market_baseline import decompose, kalshi_fee
from . import paths, prereg
from . import manifest

SNAPSHOT_HOURS = [1, 6, 24]
EDGE_THRESHOLDS = [0.05, 0.10, 0.15]
B_BOOT = 4000
HIST_CUTOFF = "2026-05-27"  # pre-cutoff candle quotes are placeholders (era-split proof)
_MONTHS = {m: i + 1 for i, m in enumerate(
    ["JAN","FEB","MAR","APR","MAY","JUN","JUL","AUG","SEP","OCT","NOV","DEC"])}


def _day_to_date(d):
    if not isinstance(d, str) or len(d) < 7:
        return None
    core = d[-7:]
    try:
        return "20%s-%02d-%02d" % (core[:2], _MONTHS[core[2:5]], int(core[5:7]))
    except (KeyError, ValueError):
        return None


def add_trades(snaps: pd.DataFrame, model: str) -> pd.DataFrame:
    t = snaps[(snaps["bid"].notna()) & (snaps["ask"].notna())].copy()
    mid = (t["bid"] + t["ask"]) / 2.0
    yes_side = (t[model] > mid).to_numpy()
    cost = np.where(yes_side, t["ask"], 1.0 - t["bid"])
    sm, cr, fee, net = decompose(t["y"].to_numpy(dtype=float), yes_side,
                                 t["bid"].to_numpy(dtype=float),
                                 t["ask"].to_numpy(dtype=float))
    t["side"] = np.where(yes_side, "YES", "NO")
    t["cost"] = cost
    t["settle_vs_mid"] = sm      # forecast skill vs the market's own belief
    t["crossing"] = cr           # cost of taking the touch (half-spread)
    t["fee"] = fee
    t["gross"] = net + fee
    t["net"] = net
    t["edge"] = t[model] - t["p_market"]
    t["event_day"] = t["ticker"].str.extract(r"-([A-Z]*\d{2}[A-Z]{3}\d{2})")[0]
    t["series"] = t["ticker"].str.split("-").str[0]
    return t


def add_trades_vs_ask(snaps: pd.DataFrame, model: str) -> pd.DataFrame:
    """Trades selected against the price you actually PAY, not the mid.

    The original rule enters when |model - mid| > threshold. That is a biased
    selector: a wide spread mechanically inflates |model - mid| whether or not
    the forecast is any good, so the rule preferentially picks illiquid
    markets and then pays their half-spread. Measured on the live era, the
    mean half-spread at h=1 is 9.2c against a mean loss of 10-13c per
    contract -- the 1h "loss" is almost entirely this selection plus crossing,
    not a failure of the forecast.

    Correct rule -- enter only when the edge survives the touch and the fee:

        YES if  model - ask - fee(ask)        > threshold
        NO  if  bid - model - fee(1 - bid)    > threshold

    Both cannot hold at once. Markets with no post-cost edge are simply not
    traded, rather than traded at a known-negative expectation.
    """
    t = snaps[(snaps["bid"].notna()) & (snaps["ask"].notna())].copy()
    ask = t["ask"].to_numpy(dtype=float)
    bid = t["bid"].to_numpy(dtype=float)
    pm = t[model].to_numpy(dtype=float)
    fee_yes = np.array([kalshi_fee(c) for c in ask])
    fee_no = np.array([kalshi_fee(c) for c in 1.0 - bid])
    edge_yes = pm - ask - fee_yes
    edge_no = (1.0 - pm) - (1.0 - bid) - fee_no
    take_yes = edge_yes >= edge_no
    t["edge_net"] = np.where(take_yes, edge_yes, edge_no)
    cost = np.where(take_yes, ask, 1.0 - bid)
    sm, cr, fee, net = decompose(t["y"].to_numpy(dtype=float), take_yes, bid, ask)
    t["side"] = np.where(take_yes, "YES", "NO")
    t["cost"] = cost
    t["settle_vs_mid"] = sm
    t["crossing"] = cr
    t["fee"] = fee
    t["net"] = net
    t["event_day"] = t["ticker"].str.extract(r"-([A-Z]*\d{2}[A-Z]{3}\d{2})")[0]
    t["series"] = t["ticker"].str.split("-").str[0]
    return t


def cluster_boot(frame: pd.DataFrame, col: str = "net", clusters: str = "event_day",
                 B: int = B_BOOT, seed: int = 0) -> tuple[float, float, float]:
    rng = np.random.default_rng(seed)
    grouped = {d: g[col].to_numpy() for d, g in frame.groupby(clusters) if d is not None}
    days = np.array(list(grouped))
    if len(days) < 5:
        return float("nan"), float("nan"), float("nan")
    means = np.empty(B)
    for i in range(B):
        pick = rng.choice(days, size=len(days), replace=True)
        vals = np.concatenate([grouped[p] for p in pick])
        means[i] = vals.mean()
    return float(means.mean()), float(np.percentile(means, 2.5)), float(np.percentile(means, 97.5))


def run_prereg(snapshots_path: str, predictions_path: str, out_dir: str,
               start_date: str | None = None) -> None:
    """T6 tracker: evaluate ONLY eligible days after the frozen start date.

    Deliberately narrow. It applies one frozen rule to one frozen column on
    days that did not exist when the rule was written, and renders one
    three-way decision. No thresholds are scanned, no cells are compared, no
    subgroups are decomposed -- every one of those would reintroduce the
    multiplicity the freeze exists to remove.
    """
    out = paths.out_dir(out_dir)
    snaps = pd.read_csv(paths.data_path(snapshots_path))
    preds = pd.read_csv(paths.data_path(predictions_path))
    start = start_date or prereg_start_date()

    col = prereg.MODEL_COLUMN
    if col not in snaps.columns:
        if col not in preds.columns:
            raise SystemExit(f"frozen column {col!r} is in neither snapshots "
                             "nor predictions")
        snaps = snaps.merge(preds[["ticker", col]], on="ticker", how="inner")

    snaps["event_day"] = snaps["ticker"].str.extract(r"-([A-Z]*\d{2}[A-Z]{3}\d{2})")[0]
    snaps["date"] = snaps["event_day"].map(_day_to_date)
    eligible = snaps[snaps["date"] > start]

    lines = [f"# Pre-registration tracker ({prereg.VERSION})", ""]
    lines.append(f"Frozen rule, evaluated ONLY on event-days after **{start}**.")
    lines.append("")
    lines.extend(prereg.summary())
    lines.append("")
    lines.append("See PREREGISTRATION.md for the freeze and the reasons v1 is void.")
    lines.append("")

    t = prereg.select_trades(eligible, col) if len(eligible) else eligible
    n_days = int(t["event_day"].nunique()) if len(t) else 0
    lines.append("## Status")
    lines.append("")
    lines.append(f"- eligible snapshot rows after {start}: **{len(eligible)}**")
    lines.append(f"- rows passing the frozen entry rule: **{len(t)}**")
    lines.append(f"- **eligible event-days: {n_days}** of "
                 f"{prereg.PLANNED_MAX_DAYS} planned")
    lines.append("")

    if n_days < 5:
        decision, why = prereg.decide(float("nan"), float("nan"), float("nan"), n_days)
        lines.append(f"**Decision: {decision}** — {why}")
        lines.append("")
        lines.append("Nothing to evaluate yet. This is the expected state until")
        lines.append("enough post-freeze days accumulate; see the power note in")
        lines.append("analysis/verification_report.md before expecting a verdict.")
    else:
        m, lo, hi = cluster_boot(t)
        decision, why = prereg.decide(m, lo, hi, n_days)
        b = prereg.obf_boundary(n_days)
        lines.append("| trades | days | mean net | 95% CI | OBF Z | alpha spent |")
        lines.append("|---|---|---|---|---|---|")
        lines.append(f"| {len(t)} | {n_days} | {m:+.4f} | [{lo:+.4f}, {hi:+.4f}] | "
                     f"{b:.2f} | {prereg.alpha_spent(n_days):.4f} |")
        lines.append("")
        lines.append(f"**Decision: {decision}** — {why}")

    manifest.append_to(lines, preds_df=preds)
    (out / "prereg_tracker.md").write_text("\n".join(lines))
    print(f"prereg {prereg.VERSION}: {n_days} eligible event-days after {start} "
          f"-> {decision}")
    print(f"Wrote {out}/prereg_tracker.md")


# The v2 pre-registration was frozen upstream (DelphiResearch, commit 88f9fb6,
# PREREGISTRATION.md committed 2026-09-10), so the first evaluable day is
# 2026-09-11. Vendoring the file into this repo gives it a NEW git history, and
# deriving the date from that history would silently re-freeze the
# pre-registration at the vendoring date -- resetting the clock on a frozen
# experiment. The upstream date is therefore pinned, and git is consulted only
# as a check that the file has not been edited since.
PREREG_FROZEN_DATE = "2026-09-10"


def prereg_start_date() -> str:
    """First evaluable date = the day after PREREGISTRATION.md was frozen."""
    import subprocess
    from datetime import date, timedelta
    root = paths.VENDOR_ROOT
    try:
        iso = subprocess.run(
            ["git", "log", "-1", "--format=%cs", "--", "PREREGISTRATION.md"],
            cwd=root, capture_output=True, text=True, timeout=10).stdout.strip()
        if iso and iso < PREREG_FROZEN_DATE:
            # Edited before the freeze in some other lineage -- trust the file.
            return (date.fromisoformat(iso) + timedelta(days=1)).isoformat()
    except Exception:
        pass
    return (date.fromisoformat(PREREG_FROZEN_DATE) + timedelta(days=1)).isoformat()


def run(snapshots_path: str, predictions_path: str, out_dir: str, model: str | None) -> None:
    out = paths.out_dir(out_dir)
    snaps = pd.read_csv(paths.data_path(snapshots_path))
    preds = pd.read_csv(paths.data_path(predictions_path))

    model_cols = [c for c in snaps.columns
                  if c.endswith("_cal") or c in ("base_pooled", "model_b", "model_u")]
    if model is None or model == "auto":
        cands = ([c for c in model_cols if c == "model_u"]
                 or [c for c in model_cols if c == "model_b"]
                 or [c for c in model_cols if c != "base_pooled"])
        if not cands:
            raise SystemExit("no model columns found in snapshots file")
        model = cands[0]
    if model not in snaps.columns:
        raise SystemExit(f"model {model!r} not in snapshots columns; have {model_cols}")

    meta_cols = [c for c in ["ticker", "event_type", "n_prior_hits", "phrase"] if c in preds.columns]
    snaps = snaps.merge(preds[meta_cols], on="ticker", how="left")
    t = add_trades(snaps, model)
    t["date"] = t["event_day"].map(_day_to_date)
    n_hist = int((t["date"] < HIST_CUTOFF).sum())
    t = t[t["date"] >= HIST_CUTOFF]

    lines = ["# Divergence Verdict (day-clustered, LIVE-ERA QUOTES ONLY)", ""]
    lines.append(f"Quarantined {n_hist} historical-tier trade rows (pre-{HIST_CUTOFF}):")
    lines.append("their quotes are placeholders (median ask under a cent, market logloss 2.2)")
    lines.append("while live-era quotes score 0.54. Historical markets still power Component A.")
    lines.append("")
    lines.append(f"Model: `{model}`. Trades: {len(t)} rows across "
                 f"{t['event_day'].nunique()} independent event-days.")
    lines.append("A cell PASSES only if the clustered 95% CI is entirely above zero")
    lines.append("and the sign survives removing the best 3 days.")
    lines.append("")
    lines.append("Reported under TWO eligibility variants (T2). `all` uses every")
    lines.append("snapshot with a quote observably on the tape at decision time.")
    lines.append("`pre-event` further restricts to snapshots taken before 00:00 ET")
    lines.append("of the ticker's event day — conservative, since Kalshi publishes no")
    lines.append("event start time, so an appearance at 14:00 ET counts the whole")
    lines.append("preceding day as during-event.")
    lines.append("")
    lines.append("Each cell is decomposed (T3): `net = settle_vs_mid - crossing - fee`,")
    lines.append("exactly. `settle_vs_mid` is forecast skill against the market's own")
    lines.append("belief; `crossing` is the half-spread paid to take the touch; `fee` is")
    lines.append("Kalshi's charge. A large negative `net` beside a flat `settle_vs_mid`")
    lines.append("is an execution result, not a forecasting one.")
    lines.append("")
    lines.append("| eligibility | snapshot | threshold | trades | days | net avg | 95% CI | settle_vs_mid | crossing | fee | best-3 removed | verdict |")
    lines.append("|---|---|---|---|---|---|---|---|---|---|---|---|")
    any_pass = False
    for variant, frame in (("all", t), ("pre-event", t[t.get("pre_event", 0) == 1])):
      for h in SNAPSHOT_HOURS:
        for thr in EDGE_THRESHOLDS:
            sel = frame[(frame["h"] == h) & (frame["edge"].abs() > thr)]
            if len(sel) < 30:
                continue
            m, lo, hi = cluster_boot(sel)
            byday = sel.groupby("event_day")["net"].sum().sort_values()
            trimmed = byday.iloc[:-3].sum() if len(byday) > 3 else float("nan")
            passed = (lo > 0) and (trimmed > 0)
            any_pass = any_pass or passed
            verdict = "PASS" if passed else ("loss" if hi < 0 else "noise")
            lines.append(f"| {variant} | {h}h | {thr:.2f} | {len(sel)} | {sel['event_day'].nunique()} | "
                         f"{m:+.4f} | [{lo:+.4f}, {hi:+.4f}] | "
                         f"{sel['settle_vs_mid'].mean():+.4f} | "
                         f"{sel['crossing'].mean():.4f} | {sel['fee'].mean():.4f} | "
                         f"{trimmed:+.2f} | {verdict} |")

    lines.append("")
    lines.append("## Forecast skill vs execution cost (T3)")
    lines.append("")
    lines.append("`settle_vs_mid` with day-clustered CIs. This is the quantity that")
    lines.append("answers 'does the forecast beat the market', separated from what it")
    lines.append("costs to act on it. Note it is NOT achievable P&L: you cannot trade")
    lines.append("at the mid. It is an upper bound on skill and a diagnostic, not a")
    lines.append("strategy.")
    lines.append("")
    lines.append("| eligibility | snapshot | thr | trades | days | settle_vs_mid | 95% CI | cost to act | reading |")
    lines.append("|---|---|---|---|---|---|---|---|---|")
    for variant, frame in (("all", t), ("pre-event", t[t.get("pre_event", 0) == 1])):
        for h in SNAPSHOT_HOURS:
            for thr in (0.05, 0.10):
                sel = frame[(frame["h"] == h) & (frame["edge"].abs() > thr)]
                if len(sel) < 30:
                    continue
                sm = sel["settle_vs_mid"].to_numpy(dtype=float)
                gr = {d: sm[sel["event_day"].to_numpy() == d]
                      for d in pd.unique(sel["event_day"])}
                keys = [k for k in gr if k is not None]
                if len(keys) < 5:
                    continue
                rng = np.random.default_rng(0)
                bs = np.array([np.concatenate(
                    [gr[keys[j]] for j in rng.choice(len(keys), len(keys), True)]).mean()
                    for _ in range(B_BOOT)])
                lo_s, hi_s = np.percentile(bs, [2.5, 97.5])
                cost = sel["crossing"].mean() + sel["fee"].mean()
                read = ("beats the market" if lo_s > 0 else
                        "worse than the market" if hi_s < 0 else "no detectable skill")
                if lo_s > 0 and sm.mean() < cost:
                    read += ", but smaller than costs"
                lines.append(f"| {variant} | {h}h | {thr:.2f} | {len(sel)} | "
                             f"{sel['event_day'].nunique()} | {sm.mean():+.4f} | "
                             f"[{lo_s:+.4f}, {hi_s:+.4f}] | {cost:.4f} | {read} |")

    lines.append("")
    lines.append("## Entry against the ask (post-cost selection)")
    lines.append("")
    lines.append("The table above selects on |model - mid|, which over-selects wide")
    lines.append("spreads and then pays them. Here a trade is taken only when the edge")
    lines.append("survives the touch and the fee, so no known-negative bet is entered.")
    lines.append("")
    lines.append("| snapshot | min net edge | trades | days | net avg | 95% CI | verdict |")
    lines.append("|---|---|---|---|---|---|---|")
    ta = add_trades_vs_ask(snaps, model)
    ta["date"] = ta["event_day"].map(_day_to_date)
    ta = ta[ta["date"] >= HIST_CUTOFF]
    for h in SNAPSHOT_HOURS:
        for thr in (0.0, 0.02, 0.05):
            sel_a = ta[(ta["h"] == h) & (ta["edge_net"] > thr)]
            if len(sel_a) < 30:
                lines.append(f"| {h}h | {thr:.2f} | {len(sel_a)} | "
                             f"{sel_a['event_day'].nunique() if len(sel_a) else 0} | "
                             "n/a | too few trades | n/a |")
                continue
            m, lo, hi = cluster_boot(sel_a)
            v = "PASS" if lo > 0 else ("loss" if hi < 0 else "noise")
            lines.append(f"| {h}h | {thr:.2f} | {len(sel_a)} | {sel_a['event_day'].nunique()} | "
                         f"{m:+.4f} | [{lo:+.4f}, {hi:+.4f}] | {v} |")
    lines.append("")
    lines.append("A shrinking trade count as the threshold rises is the rule working:")
    lines.append("most disagreements with the market do not survive the spread.")

    lines.append("")
    lines.append("## Side pocket under both eligibility variants (T2 check)")
    lines.append("")
    lines.append("The removed filter was outcome-dependent, so the NO-side pocket was")
    lines.append("plausibly inflated. Both variants shown so the movement is visible.")
    lines.append("")
    lines.append("| eligibility | side | trades | days | net avg | 95% CI |")
    lines.append("|---|---|---|---|---|---|")
    for variant, frame in (("all", t), ("pre-event", t[t.get("pre_event", 0) == 1])):
        cell = frame[(frame["h"] == 6) & (frame["edge"].abs() > 0.10)]
        for side in ("NO", "YES"):
            g = cell[cell["side"] == side]
            if len(g) < 15:
                lines.append(f"| {variant} | {side} | {len(g)} | "
                             f"{g['event_day'].nunique() if len(g) else 0} | n/a | too few |")
                continue
            mm, ll, hh = cluster_boot(g)
            lines.append(f"| {variant} | {side} | {len(g)} | {g['event_day'].nunique()} | "
                         f"{mm:+.4f} | [{ll:+.4f}, {hh:+.4f}] |")

    # decomposition at the widest threshold, 6h (the historically ambiguous cell)
    sel = t[(t["h"] == 6) & (t["edge"].abs() > 0.10)]
    if len(sel) >= 30:
        lines.append("")
        lines.append("## Decomposition (6h, threshold 0.10)")
        for by, label in (("side", "trade side"), ("event_type", "event type"),
                          ("series", "series")):
            if by not in sel.columns:
                continue
            lines.append("")
            lines.append(f"### by {label}")
            lines.append("")
            lines.append("| group | trades | days | net avg |")
            lines.append("|---|---|---|---|")
            for g, grp in sel.groupby(by):
                lines.append(f"| {g} | {len(grp)} | {grp['event_day'].nunique()} | "
                             f"{grp['net'].mean():+.4f} |")
        if "n_prior_hits" in sel.columns:
            lines.append("")
            lines.append("### by phrase history")
            lines.append("")
            lines.append("| regime | trades | days | net avg |")
            lines.append("|---|---|---|---|")
            for name, grp in (("prior_hits < 5", sel[sel["n_prior_hits"] < 5]),
                              ("prior_hits >= 5", sel[sel["n_prior_hits"] >= 5])):
                if len(grp):
                    lines.append(f"| {name} | {len(grp)} | {grp['event_day'].nunique()} | "
                                 f"{grp['net'].mean():+.4f} |")

    lines.append("")
    lines.append("## Bottom line")
    lines.append("")
    if any_pass:
        lines.append("At least one cell passes the clustered test — inspect it above and")
        lines.append("check whether it concentrates in an economically sensible niche")
        lines.append("before believing it.")
    else:
        lines.append("No cell passes: at this information set, disagreements with the")
        lines.append("market are not profitable after fees. The path to edge is new")
        lines.append("information (Components B/C), not more thresholds.")

    manifest.append_to(lines, preds_df=preds, snaps_df=snaps)
    (out / "verdict_report.md").write_text("\n".join(lines))
    print(f"Wrote {out/'verdict_report.md'} "
          f"({'AT LEAST ONE PASS' if any_pass else 'no cell passes'})")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--snapshots", default="market_baseline/market_baseline_snapshots.csv")
    ap.add_argument("--predictions", default="component_a/component_a_predictions.csv")
    ap.add_argument("--out", default="verdict")
    ap.add_argument("--model", default="auto")
    ap.add_argument("--prereg", action="store_true",
                    help="T6 tracker: frozen rule on post-freeze days only")
    args = ap.parse_args()
    if args.prereg:
        run_prereg(args.snapshots, args.predictions, args.out)
        return
    run(args.snapshots, args.predictions, args.out, args.model)


if __name__ == "__main__":
    main()
