"""Component A — historical base-rate occurrence model, walk-forward evaluated.

    python -m kalshi_mention.component_a --db kalshi_mention.sqlite --out component_a

Pass 2.5 revision, driven by the pass-2 eval finding (models beat the pooled
base on Brier but lost on log loss — signal present, confidence miscalibrated,
with confident-and-wrong predictions traced to type-blind exposure):

  1. EVENT-TYPE CONDITIONING. Each market's event type is classified ex ante
     from its Kalshi title (speech / pressconf / gaggle / interview / meeting /
     vlog); each document from its FactBase speech_type. "Typed" model variants
     down-weight off-type history by TYPE_OFFMATCH_WEIGHT instead of excluding
     it, which shrinks smoothly toward the global estimate when same-type
     history is thin.
  2. WALK-FORWARD RECALIBRATION. Every model also gets a "_cal" variant:
     a one-feature logistic map on logit(p), refit periodically on ONLY the
     markets already resolved (close_time <= decision time). Leak-free by
     construction; identity until MIN_RESOLVED outcomes exist.

Selection stays honest: best model chosen on the earlier selection period,
holdout columns are the numbers that count. Gate: beat base_pooled on holdout
log loss.
"""

from __future__ import annotations

import argparse
import json
import math
from datetime import datetime, timezone

import numpy as np
import pandas as pd

from . import config, db
from .event_types import classify_doc, classify_market
from .phrase import extract_phrase, parse_phrase_spec, spec_key
from . import cluster, paths
from . import manifest

HALF_LIVES_DAYS = [30.0, 90.0]
TYPE_OFFMATCH_WEIGHT = 0.15
HOLDOUT_FRAC = 0.4
N_CALIB_BINS = 10
EPS = 1e-6

BETA_A = 0.5
BETA_B = 0.5
RATE_PRIOR_COUNT = 0.5
RATE_PRIOR_EXPOSURE = 50_000

MIN_RESOLVED = 75      # calibration inactive until this many resolved outcomes
REFIT_EVERY = 25       # refit cadence (in newly resolved markets)
CAL_L2 = 1e-3


def _ts(iso: str | None) -> float | None:
    if not iso:
        return None
    try:
        dt = datetime.fromisoformat(str(iso).replace("Z", "+00:00"))
    except ValueError:
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.timestamp()


def poisson_sf(k: int, m: float) -> float:
    if m <= 0:
        return 0.0
    cdf, term = 0.0, math.exp(-m)
    for j in range(k):
        cdf += term
        term *= m / (j + 1)
    return min(max(1.0 - cdf, 0.0), 1.0)


def negbin_sf(k: int, m: float, r: float) -> float:
    if m <= 0:
        return 0.0
    if not math.isfinite(r) or r > 1e6:
        return poisson_sf(k, m)
    p = r / (r + m)
    log_p, log_q = math.log(p), math.log(1.0 - p)
    cdf = 0.0
    for j in range(k):
        lg = math.lgamma(j + r) - math.lgamma(r) - math.lgamma(j + 1) + r * log_p + j * log_q
        cdf += math.exp(lg)
    return min(max(1.0 - cdf, 0.0), 1.0)


def predict_models(counts: np.ndarray, lengths: np.ndarray, weights: np.ndarray, k: int) -> dict[str, float]:
    w_sum = float(weights.sum())
    if w_sum <= 0:
        return {}
    hits = (counts >= k).astype(float)
    p_emp = (BETA_A + float((weights * hits).sum())) / (BETA_A + BETA_B + w_sum)

    w_count = float((weights * counts).sum())
    w_len = float((weights * lengths).sum())
    lam = (RATE_PRIOR_COUNT + w_count) / (RATE_PRIOR_EXPOSURE + w_len)
    exp_len = w_len / w_sum
    m = lam * exp_len
    p_pois = poisson_sf(k, m)

    mean_c = w_count / w_sum
    var_c = float((weights * (counts - mean_c) ** 2).sum()) / w_sum
    if var_c > mean_c > 0:
        r = mean_c**2 / (var_c - mean_c)
        p_nb = negbin_sf(k, m, max(r, 1e-3))
    else:
        p_nb = p_pois
    return {"empirical": p_emp, "poisson": p_pois, "negbin": p_nb}


def clip(p):
    return np.clip(p, EPS, 1 - EPS)


def logit(p):
    p = clip(p)
    return np.log(p / (1 - p))


def _sigmoid(z: np.ndarray) -> np.ndarray:
    return 1.0 / (1.0 + np.exp(-np.clip(z, -30.0, 30.0)))


def _nll(x: np.ndarray, y: np.ndarray, a: float, b: float) -> float:
    p = np.clip(_sigmoid(a + b * x), EPS, 1 - EPS)
    return float(-(y * np.log(p) + (1 - y) * np.log(1 - p)).sum()) \
        + 0.5 * CAL_L2 * (a * a + (b - 1.0) ** 2)


def fit_platt(x: np.ndarray, y: np.ndarray) -> tuple[float, float]:
    """Logistic y ~ sigmoid(a + b x) via damped Newton with light L2 and
    backtracking; x = logit(p_raw). Guaranteed non-divergent."""
    a, b = 0.0, 1.0
    best = _nll(x, y, a, b)
    for _ in range(50):
        p = _sigmoid(a + b * x)
        g = np.maximum(p * (1 - p), 1e-6)
        grad_a = float((p - y).sum()) + CAL_L2 * a
        grad_b = float(((p - y) * x).sum()) + CAL_L2 * (b - 1.0)
        h_aa = float(g.sum()) + CAL_L2
        h_ab = float((g * x).sum())
        h_bb = float((g * x * x).sum()) + CAL_L2
        det = h_aa * h_bb - h_ab * h_ab
        if det <= 1e-12:
            break
        da = (h_bb * grad_a - h_ab * grad_b) / det
        db = (h_aa * grad_b - h_ab * grad_a) / det
        step = 1.0
        improved = False
        for _bt in range(20):
            na, nb = a - step * da, b - step * db
            val = _nll(x, y, na, nb)
            if val < best - 1e-10:
                a, b, best = na, nb, val
                improved = True
                break
            step *= 0.5
        if not improved or abs(step * da) + abs(step * db) < 1e-8:
            break
    return a, b


def apply_platt(p: np.ndarray, a: float, b: float) -> np.ndarray:
    return clip(_sigmoid(a + b * logit(np.asarray(p, dtype=float))))


def walk_forward_calibrate(p_raw: np.ndarray, y: np.ndarray, as_of: np.ndarray, close_ts: np.ndarray) -> np.ndarray:
    """Recalibrate each prediction using only markets resolved before it.

    Markets arrive in as_of order. A market j is usable for calibrating market
    i iff close_ts[j] <= as_of[i]. Refit every REFIT_EVERY newly resolved
    outcomes; identity until MIN_RESOLVED.
    """
    n = len(p_raw)
    out = p_raw.copy()
    resolve_order = np.argsort(close_ts)
    ptr = 0
    resolved: list[int] = []
    a, b = 0.0, 1.0
    fitted_at = -1
    x_all = logit(p_raw)
    for i in range(n):
        while ptr < n and close_ts[resolve_order[ptr]] <= as_of[i]:
            resolved.append(resolve_order[ptr])
            ptr += 1
        if len(resolved) >= MIN_RESOLVED and len(resolved) - fitted_at >= REFIT_EVERY:
            idx = np.array(resolved)
            a, b = fit_platt(x_all[idx], y[idx])
            fitted_at = len(resolved)
        if fitted_at > 0:
            out[i] = float(apply_platt(np.array([p_raw[i]]), a, b)[0])
    return clip(out)


def log_loss(y, p):
    p = clip(np.asarray(p, dtype=float))
    y = np.asarray(y, dtype=float)
    return float(-(y * np.log(p) + (1 - y) * np.log(1 - p)).mean())


def brier(y, p):
    return float(((np.asarray(p, float) - np.asarray(y, float)) ** 2).mean())


def run(db_path: str, out_dir: str) -> None:
    from pathlib import Path

    conn = db.connect(db_path)
    out = paths.out_dir(out_dir)

    if conn.execute("SELECT COUNT(*) c FROM phrase_doc_counts").fetchone()["c"] == 0:
        raise SystemExit("phrase_doc_counts is empty — run kalshi_mention.counts first.")

    docs = conn.execute(
        "SELECT doc_id, date, available_at, token_count, speech_type, title FROM transcripts "
        "WHERE token_count IS NOT NULL ORDER BY available_at"
    ).fetchall()
    doc_ids = [d["doc_id"] for d in docs]
    doc_index = {d: i for i, d in enumerate(doc_ids)}
    avail_ts = np.array([_ts(d["available_at"]) or math.inf for d in docs])
    event_ts = np.array([_ts(str(d["date"]) + "T12:00:00+00:00") or 0.0 for d in docs])
    lengths = np.array([float(d["token_count"] or 0) for d in docs])
    doc_types = np.array([classify_doc(d["speech_type"], d["title"]) for d in docs])

    counts_by_spec: dict[str, np.ndarray] = {}
    for row in conn.execute("SELECT spec_key, doc_id, count FROM phrase_doc_counts"):
        arr = counts_by_spec.setdefault(row["spec_key"], np.zeros(len(doc_ids)))
        i = doc_index.get(row["doc_id"])
        if i is not None:
            arr[i] = row["count"]
    zero_counts = np.zeros(len(doc_ids))

    placeholders = ",".join("?" * len(config.LABELING_EXCLUDED_SERIES)) or "''"
    markets = conn.execute(
        f"""SELECT m.ticker, m.result, m.open_time, m.close_time, m.title AS mtitle,
                   m.raw_json, e.title AS etitle
            FROM markets m LEFT JOIN events e ON m.event_ticker = e.event_ticker
            WHERE m.result IN ('yes','no') AND m.series_ticker NOT IN ({placeholders})""",
        list(config.LABELING_EXCLUDED_SERIES),
    ).fetchall()

    records = []
    for mkt in markets:
        obj = json.loads(mkt["raw_json"])
        phrase, _ = extract_phrase(obj)
        if not phrase:
            continue
        sk = spec_key(phrase)
        _alts, k = parse_phrase_spec(phrase)
        as_of = _ts(mkt["open_time"]) or _ts(mkt["close_time"])
        close = _ts(mkt["close_time"]) or as_of
        if as_of is None:
            continue
        mtype = classify_market(mkt["etitle"] or mkt["mtitle"])
        records.append((as_of, close, mkt["ticker"], sk, k, phrase, mtype,
                        1 if mkt["result"] == "yes" else 0))
    records.sort()
    print(f"Markets to predict: {len(records)}")

    rows = []
    for as_of, close, ticker, sk, k, phrase, mtype, y in records:
        n_elig = int(np.searchsorted(avail_ts, as_of, side="right"))
        if n_elig == 0:
            continue
        c = counts_by_spec.get(sk, zero_counts)[:n_elig]
        L = lengths[:n_elig]
        age_days = np.maximum((as_of - event_ts[:n_elig]) / 86400.0, 0.0)
        type_match = (doc_types[:n_elig] == mtype).astype(float)
        type_w = np.where(type_match > 0, 1.0, TYPE_OFFMATCH_WEIGHT)
        row = {
            "ticker": ticker, "phrase": phrase, "k": k, "y": y,
            "event_type": mtype, "as_of_ts": as_of, "close_ts": close,
            "n_prior_docs": n_elig,
            "n_prior_hits": int((c >= k).sum()),
            "n_prior_same_type": int(type_match.sum()),
        }
        for hl in HALF_LIVES_DAYS:
            w = np.power(0.5, age_days / hl)
            tag = f"hl{int(hl)}"
            for name, p in predict_models(c, L, w, k).items():
                row[f"{name}_{tag}"] = float(clip(p))
            for name, p in predict_models(c, L, w * type_w, k).items():
                row[f"{name}T_{tag}"] = float(clip(p))
        rows.append(row)

    pred_df = pd.DataFrame(rows)
    if pred_df.empty:
        raise SystemExit("No predictions produced.")

    y_arr = pred_df["y"].to_numpy(dtype=float)
    prior_sum = np.concatenate([[0.0], np.cumsum(y_arr)[:-1]])
    prior_n = np.arange(len(y_arr), dtype=float)
    pred_df["base_pooled"] = np.clip((0.5 + prior_sum) / (1.0 + prior_n), EPS, 1 - EPS)

    raw_cols = [c for c in pred_df.columns if any(
        c.startswith(p) for p in ("empirical", "poisson", "negbin"))]
    as_of_a = pred_df["as_of_ts"].to_numpy(dtype=float)
    close_a = pred_df["close_ts"].to_numpy(dtype=float)
    for col in raw_cols:
        pred_df[col + "_cal"] = walk_forward_calibrate(
            pred_df[col].to_numpy(dtype=float), y_arr, as_of_a, close_a)

    pred_df.to_csv(out / "component_a_predictions.csv", index=False)

    n = len(pred_df)
    split = int(n * (1 - HOLDOUT_FRAC))
    sel, hold = pred_df.iloc[:split], pred_df.iloc[split:]
    model_cols = raw_cols + [c + "_cal" for c in raw_cols] + ["base_pooled"]

    def metrics(frame):
        yv = frame["y"].to_numpy(dtype=float)
        return pd.DataFrame([
            {"model": c, "log_loss": log_loss(yv, frame[c]), "brier": brier(yv, frame[c])}
            for c in model_cols
        ])

    sel_m = metrics(sel).rename(columns={"log_loss": "sel_log_loss", "brier": "sel_brier"})
    hold_m = metrics(hold).rename(columns={"log_loss": "hold_log_loss", "brier": "hold_brier"})
    summary = sel_m.merge(hold_m, on="model").sort_values("sel_log_loss")
    selected = summary.iloc[0]["model"]
    base_hold = float(summary.loc[summary.model == "base_pooled", "hold_log_loss"].iloc[0])
    sel_hold = float(summary.iloc[0]["hold_log_loss"])

    yv = hold["y"].to_numpy(dtype=float)
    pv = hold[selected].to_numpy(dtype=float)
    bins = np.linspace(0, 1, N_CALIB_BINS + 1)
    calib = []
    for i in range(N_CALIB_BINS):
        mask = (pv >= bins[i]) & ((pv < bins[i + 1]) if i < N_CALIB_BINS - 1 else (pv <= bins[i + 1]))
        if mask.sum():
            calib.append({"bin_low": bins[i], "bin_high": bins[i + 1], "n": int(mask.sum()),
                          "mean_pred": float(pv[mask].mean()), "frac_yes": float(yv[mask].mean())})
    calib_df = pd.DataFrame(calib)
    calib_df.to_csv(out / "calibration_bins.csv", index=False)
    ece = float(sum(r["n"] * abs(r["mean_pred"] - r["frac_yes"]) for _, r in calib_df.iterrows()) / len(hold))

    lines = ["# Component A — Walk-Forward Evaluation (pass 2.5)", ""]
    lines.append(f"Markets: {n} (selection {len(sel)}, chronological holdout {len(hold)}).")
    lines.append(f"Base yes-rate: {pred_df['y'].mean():.3f}. Typed variants (suffix `T`)")
    lines.append(f"condition on ex-ante event type; `_cal` variants are walk-forward")
    lines.append(f"recalibrated on resolved-only outcomes. Selected on selection log loss:")
    lines.append(f"**{selected}** — holdout {sel_hold:.4f} vs base_pooled {base_hold:.4f} "
                 f"(**{'PASS' if sel_hold < base_hold else 'FAIL'}** on the gate).")
    lines.append("")
    # Effective n is event-days, not markets: one transcript resolves every
    # market on a day. Report the gate with the interval that reflects that.
    hold_days = cluster.event_day(hold["ticker"]).to_numpy()
    g_mean, g_lo, g_hi = cluster.paired_logloss_ci(
        yv_gate := hold["y"].to_numpy(dtype=float),
        hold[selected].to_numpy(dtype=float),
        hold["base_pooled"].to_numpy(dtype=float), hold_days)
    n_days = int(pd.Series(hold_days).nunique())
    lines.append(f"Paired difference vs base_pooled: **{g_mean:+.4f}**, "
                 f"day-clustered 95% CI [{g_lo:+.4f}, {g_hi:+.4f}] over "
                 f"{n_days} event-days ({len(hold)} markets — a median of "
                 f"{len(hold)//max(n_days,1)} markets share each transcript).")
    lines.append(f"Reading: **{cluster.verdict_word(g_lo, g_hi)}**.")
    lines.append("")
    lines.append("| model | sel logloss | sel brier | holdout logloss | holdout brier |")
    lines.append("|---|---|---|---|---|")
    for _, r in summary.iterrows():
        lines.append(f"| {r['model']} | {r['sel_log_loss']:.4f} | {r['sel_brier']:.4f} | "
                     f"{r['hold_log_loss']:.4f} | {r['hold_brier']:.4f} |")
    lines.append("")
    lines.append(f"## Calibration of `{selected}` on holdout")
    lines.append("")
    lines.append(f"ECE: **{ece:.4f}**")
    lines.append("")
    lines.append("| bin | n | mean pred | realized yes |")
    lines.append("|---|---|---|---|")
    for _, r in calib_df.iterrows():
        lines.append(f"| {r['bin_low']:.1f}-{r['bin_high']:.1f} | {int(r['n'])} | "
                     f"{r['mean_pred']:.3f} | {r['frac_yes']:.3f} |")
    lines.append("")
    lines.append("## Holdout log loss by event type (selected vs base)")
    lines.append("")
    lines.append("| event type | n | selected | base_pooled |")
    lines.append("|---|---|---|---|")
    for et, grp in hold.groupby("event_type"):
        if len(grp) < 5:
            continue
        yg = grp["y"].to_numpy(dtype=float)
        lines.append(f"| {et} | {len(grp)} | {log_loss(yg, grp[selected]):.4f} | "
                     f"{log_loss(yg, grp['base_pooled']):.4f} |")
    lines.append("")
    lines.append("## Low-history regime (holdout)")
    lines.append("")
    lines.append("| regime | n | selected | base_pooled |")
    lines.append("|---|---|---|---|")
    for name, frame in (("prior_hits < 5", hold[hold["n_prior_hits"] < 5]),
                        ("prior_hits >= 5", hold[hold["n_prior_hits"] >= 5])):
        if len(frame):
            yg = frame["y"].to_numpy(dtype=float)
            lines.append(f"| {name} | {len(frame)} | {log_loss(yg, frame[selected]):.4f} | "
                         f"{log_loss(yg, frame['base_pooled']):.4f} |")
    lines.append("")
    lines.append("## Reading guardrails")
    lines.append("")
    lines.append("- Holdout columns are the honest numbers; selection was blind to them.")
    lines.append("- Log losses in the table above are point estimates on ~78 independent")
    lines.append("  event-days. Differences smaller than ~0.01 are not distinguishable;")
    lines.append("  only the clustered interval above supports a claim.")
    lines.append("- The market-price baseline still awaits price collection (deferred).")
    lines.append("- Residual event-mapping noise attenuates all models equally.")

    manifest.append_to(lines, conn=conn, preds_df=pred_df)
    (out / "component_a_report.md").write_text("\n".join(lines))
    print(f"Selected: {selected} | holdout logloss {sel_hold:.4f} vs base {base_hold:.4f} "
          f"({'PASS' if sel_hold < base_hold else 'FAIL'})")
    print(f"Wrote {out/'component_a_report.md'}, component_a_predictions.csv, calibration_bins.csv")
    conn.close()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--out", default="component_a")
    args = ap.parse_args()
    run(args.db, args.out)


if __name__ == "__main__":
    main()
