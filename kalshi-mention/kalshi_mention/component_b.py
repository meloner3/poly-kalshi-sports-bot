"""Component B — walk-forward context model (news + momentum + schedule).

    python -m kalshi_mention.component_b --db kalshi_mention.sqlite --out component_b

Stacks a small L2-regularized logistic layer on top of Component A. Inputs per
market, all computable strictly before its open:

  logit_pA        Component A's calibrated probability (the backbone)
  news_mom        log ratio of 3-day vs 60-day GDELT news volume for the
                  phrase (0 when no cache; gated by proper_noun)
  proper_noun     1 if the phrase looks like a named entity (news is
                  meaningful) rather than a common word
  phrase_mom      log ratio of the phrase's 30-day vs 365-day per-doc hit rate
  days_since_hit  log(1 + days since the phrase last appeared), capped
  listing_load    log(1 + number of markets Kalshi listed for the same event
                  day) — the exchange's own salience signal
  k_threshold     1 if the market needs 3+ occurrences
  event-type dummies (speech / gaggle / pressconf / interview / vlog vs meeting)

Training is walk-forward: refit every REFIT_EVERY newly RESOLVED markets using
only outcomes already settled at each decision time (identical discipline to
the calibration layer — leak-free by construction). Before MIN_RESOLVED
outcomes exist, Component B passes Component A through unchanged.

Output: component_b/predictions_ab.csv — Component A's predictions file plus a
`model_b` column, consumable directly by market_baseline and verdict:

    python -m kalshi_mention.market_baseline --predictions component_b/predictions_ab.csv \
        --models model_b,poisson_hl30_cal --db kalshi_mention.sqlite --out market_baseline
"""

from __future__ import annotations

import argparse
import json
import math
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import pandas as pd

from . import config, db
from .phrase import extract_phrase, parse_phrase_spec, spec_key
from . import paths
from . import manifest
from . import cluster

REFIT_EVERY = 100
MIN_RESOLVED = 300
L2 = 3.0
EPS = 1e-6
EVENT_DUMMIES = ["speech", "gaggle", "pressconf", "interview", "vlog"]

FEATURES = (["logit_pA", "news_mom", "proper_noun", "phrase_mom",
             "days_since_hit", "listing_load", "k_threshold"]
            + [f"et_{e}" for e in EVENT_DUMMIES])


def clip(p):
    return np.clip(p, EPS, 1 - EPS)


def logit(p):
    p = clip(np.asarray(p, dtype=float))
    return np.log(p / (1 - p))


def sigmoid(z):
    return 1.0 / (1.0 + np.exp(-np.clip(z, -30, 30)))


def fit_logistic(X: np.ndarray, y: np.ndarray, l2: float = L2) -> np.ndarray:
    """Damped-Newton L2 logistic with intercept; prior pulls the logit_pA
    coefficient toward 1 and everything else toward 0 (Component A is the
    trusted backbone; context must earn its weight)."""
    n, d = X.shape
    Xb = np.hstack([np.ones((n, 1)), X])
    w = np.zeros(d + 1)
    w[1] = 1.0  # start at "pass A through"
    prior = np.zeros(d + 1)
    prior[1] = 1.0

    def nll(wv):
        p = clip(sigmoid(Xb @ wv))
        return float(-(y * np.log(p) + (1 - y) * np.log(1 - p)).sum()
                     + 0.5 * l2 * ((wv - prior) ** 2).sum())

    best = nll(w)
    for _ in range(60):
        p = sigmoid(Xb @ w)
        g = np.maximum(p * (1 - p), 1e-6)
        grad = Xb.T @ (p - y) + l2 * (w - prior)
        H = (Xb * g[:, None]).T @ Xb + l2 * np.eye(d + 1)
        try:
            step = np.linalg.solve(H, grad)
        except np.linalg.LinAlgError:
            break
        s, improved = 1.0, False
        for _bt in range(20):
            cand = w - s * step
            v = nll(cand)
            if v < best - 1e-9:
                w, best, improved = cand, v, True
                break
            s *= 0.5
        if not improved or float(np.abs(s * step).sum()) < 1e-8:
            break
    return w


def predict_logistic(w: np.ndarray, X: np.ndarray) -> np.ndarray:
    Xb = np.hstack([np.ones((X.shape[0], 1)), X])
    return clip(sigmoid(Xb @ w))


def looks_proper_noun(raw_phrase: str) -> int:
    alts, _ = parse_phrase_spec(raw_phrase)
    head = alts[0].strip()
    words = head.split()
    caps = sum(1 for wd in words if wd[:1].isupper())
    stop = {
        "great", "beautiful", "tremendous", "amazing", "incredible", "huge",
        "fake", "witch", "hoax", "genius", "perfect", "disaster"}
    if any(wd.lower() in stop for wd in words):
        return 0
    return 1 if caps == len(words) else 0


def build_features(db_path: str, preds: pd.DataFrame, backbone: str) -> pd.DataFrame:
    conn = db.connect(db_path)

    # phrase spec per market (for news + proper-noun + momentum keys)
    placeholders = ",".join("?" * len(config.LABELING_EXCLUDED_SERIES)) or "''"
    raw = {r["ticker"]: r["raw_json"] for r in conn.execute(
        f"""SELECT ticker, raw_json FROM markets
            WHERE result IN ('yes','no') AND series_ticker NOT IN ({placeholders})""",
        list(config.LABELING_EXCLUDED_SERIES))}
    sk_map, pn_map = {}, {}
    for tick, rj in raw.items():
        phrase, _ = extract_phrase(json.loads(rj))
        if phrase:
            sk_map[tick] = spec_key(phrase)
            pn_map[tick] = looks_proper_noun(phrase)

    # per-spec hit dates from counts x transcripts (for momentum / recency)
    hits: dict[str, np.ndarray] = {}
    q = conn.execute("""SELECT pdc.spec_key, t.date FROM phrase_doc_counts pdc
                        JOIN transcripts t ON t.doc_id = pdc.doc_id""")
    def _iso(x):
        s = str(x)[:10]
        return s if len(s) == 10 and s[4] == "-" else None

    tmp: dict[str, list] = {}
    for r in q:
        d = _iso(r["date"])
        if d:
            tmp.setdefault(r["spec_key"], []).append(d)
    for k, v in tmp.items():
        hits[k] = np.array(sorted(v))
    doc_dates = np.array(sorted(
        d for d in (_iso(r["date"]) for r in conn.execute("SELECT date FROM transcripts")) if d))

    # news cache
    news: dict[str, dict[str, float]] = {}
    for r in conn.execute("SELECT spec_key, date, vol FROM news_volume"):
        news.setdefault(r["spec_key"], {})[r["date"]] = r["vol"]
    print(f"news cache: {len(news)} phrases")

    # listing load: markets on this event day ALREADY LISTED at decision time.
    #
    # This was previously day_codes.value_counts() — the whole day's final
    # total, which is not knowable when an early market opens. Measured on the
    # settled set: 59% of event-days list markets more than an hour apart
    # (median stagger 5.8h, p90 64h), and 67% of markets had day-totals that
    # included ~11 of ~46 listings that did not exist yet — about 24% of the
    # feature was future information. It was also the #1 feature by
    # permutation importance, so every ranking computed before this fix is
    # suspect. Point-in-time discipline is Hard Rule #4; this is it applied.
    #
    # rank(method="max") counts markets with open_time <= this one's, so
    # simultaneously-listed markets correctly see each other (ties included).
    day_codes = preds["ticker"].str.extract(r"-([A-Z]*\d{2}[A-Z]{3}\d{2})")[0]
    _ld = pd.DataFrame({"day": day_codes,
                        "ts": preds["as_of_ts"].to_numpy(dtype=float)})
    load = _ld.groupby("day")["ts"].rank(method="max")

    def as_date(ts):
        return datetime.fromtimestamp(float(ts), tz=timezone.utc).date().isoformat()

    rows = []
    for i, r in preds.iterrows():
        tick = r["ticker"]
        sk = sk_map.get(tick)
        open_date = as_date(r["as_of_ts"])
        f = {"ticker": tick, "logit_pA": float(logit(r[backbone])[()])}
        # news momentum: mean vol last 3 days vs last 60, strictly before open
        nm = 0.0
        pn = pn_map.get(tick, 0)
        if sk in news and pn:
            ser = news[sk]
            d3 = [ser.get(d, 0.0) for d in _days_before(open_date, 3)]
            d60 = [ser.get(d, 0.0) for d in _days_before(open_date, 60)]
            if any(v > 0 for v in d60):
                nm = math.log((np.mean(d3) + 1e-3) / (np.mean(d60) + 1e-3))
        f["news_mom"] = nm
        f["proper_noun"] = pn
        # phrase momentum + recency from transcript hits strictly before open
        h = hits.get(sk, np.empty(0, dtype="<U10"))
        h_before = h[h < open_date]
        d_before = doc_dates[doc_dates < open_date]
        r30 = _rate(h_before, d_before, open_date, 30)
        r365 = _rate(h_before, d_before, open_date, 365)
        f["phrase_mom"] = math.log((r30 + 1e-3) / (r365 + 1e-3))
        if len(h_before):
            gap = (datetime.fromisoformat(open_date) - datetime.fromisoformat(str(h_before[-1]))).days
        else:
            gap = 730
        f["days_since_hit"] = math.log1p(min(gap, 730))
        f["listing_load"] = math.log1p(float(load.iloc[i]) if pd.notna(load.iloc[i]) else 0.0)
        f["k_threshold"] = 1.0 if int(r["k"]) > 1 else 0.0
        et = str(r.get("event_type", "meeting"))
        for e in EVENT_DUMMIES:
            f[f"et_{e}"] = 1.0 if et == e else 0.0
        rows.append(f)
    conn.close()
    return pd.DataFrame(rows)


def _days_before(open_date: str, n: int) -> list[str]:
    from datetime import date, timedelta
    d0 = date.fromisoformat(open_date)
    return [(d0 - timedelta(days=j)).isoformat() for j in range(1, n + 1)]


def _rate(hit_dates: np.ndarray, all_dates: np.ndarray, open_date: str, window: int) -> float:
    from datetime import date, timedelta
    lo = (date.fromisoformat(open_date) - timedelta(days=window)).isoformat()
    n_hit = int(((hit_dates >= lo) & (hit_dates < open_date)).sum())
    n_doc = int(((all_dates >= lo) & (all_dates < open_date)).sum())
    return n_hit / n_doc if n_doc else 0.0


def run(db_path: str, predictions_path: str, out_dir: str, backbone: str,
        c_file: str | None = None, min_resolved: int = MIN_RESOLVED,
        refit_every: int = REFIT_EVERY) -> None:
    out = paths.out_dir(out_dir)
    preds = pd.read_csv(paths.data_path(predictions_path))
    if backbone == "auto":
        cands = [c for c in preds.columns if c.endswith("_cal")]
        if not cands:
            raise SystemExit("no *_cal backbone column found in predictions")
        backbone = cands[0]
    print(f"Backbone: {backbone}; markets: {len(preds)}")

    feats = build_features(db_path, preds, backbone)
    features = list(FEATURES)
    if c_file:
        cdf = pd.read_csv(paths.data_path(c_file))[["ticker", "p_c"]]
        feats = feats.merge(cdf, on="ticker", how="left")
        feats["logit_pC"] = np.where(feats["p_c"].notna(), logit(feats["p_c"].fillna(0.5)), 0.0)
        feats["has_pC"] = feats["p_c"].notna().astype(float)
        features += ["logit_pC", "has_pC"]
        print(f"Component C feature merged: {int(feats['has_pC'].sum())} markets covered")
    X_all = feats[features].to_numpy(dtype=float)
    # standardize non-logit features on an expanding basis is overkill; fixed
    # scale-free transforms above keep magnitudes ~O(1) already.
    y = preds["y"].to_numpy(dtype=float)
    as_of = preds["as_of_ts"].to_numpy(dtype=float)
    close = preds["close_ts"].to_numpy(dtype=float)
    pA = preds[backbone].to_numpy(dtype=float)

    n = len(preds)
    pB = pA.copy()
    resolve_order = np.argsort(close)
    ptr, resolved, w, fitted_at = 0, [], None, -1
    for i in range(n):
        while ptr < n and close[resolve_order[ptr]] <= as_of[i]:
            resolved.append(resolve_order[ptr])
            ptr += 1
        if len(resolved) >= min_resolved and len(resolved) - fitted_at >= refit_every:
            idx = np.array(resolved)
            w = fit_logistic(X_all[idx], y[idx])
            fitted_at = len(resolved)
        if w is not None:
            pB[i] = float(predict_logistic(w, X_all[i:i + 1])[0])
    preds["model_b"] = clip(pB)

    # evaluation on the same chronological holdout convention as Component A
    split = int(n * 0.6)
    hold = slice(split, n)

    def ll(p):
        pv = clip(np.asarray(p, float)[hold])
        yv = y[hold]
        return float(-(yv * np.log(pv) + (1 - yv) * np.log(1 - pv)).mean())

    llB, llA = ll(preds["model_b"]), ll(pA)
    llBase = ll(preds["base_pooled"]) if "base_pooled" in preds else float("nan")
    gate = "PASS" if llB < llA else "FAIL"

    preds.to_csv(out / "predictions_ab.csv", index=False)
    lines = ["# Component B — walk-forward context model", ""]
    lines.append(f"Backbone `{backbone}`; features: {', '.join(features)}.")
    lines.append(f"Refit every {refit_every} resolved markets (min {min_resolved}); "
                 f"L2={L2} with prior = pass-A-through.")
    lines.append("")
    lines.append("| model | holdout logloss |")
    lines.append("|---|---|")
    lines.append(f"| model_b (A + context) | {llB:.4f} |")
    lines.append(f"| {backbone} (A alone) | {llA:.4f} |")
    lines.append(f"| base_pooled | {llBase:.4f} |")
    lines.append("")
    lines.append(f"Gate (B must beat A on holdout log loss): **{gate}**")
    lines.append("")
    # T4(b): no model-vs-model claim without a day-clustered interval.
    yh = np.asarray(y, dtype=float)[hold]
    days_h = cluster.event_day(preds["ticker"].iloc[hold]).to_numpy()
    pB_h = clip(np.asarray(preds["model_b"], dtype=float))[hold]
    pA_h = clip(np.asarray(pA, dtype=float))[hold]
    d1, l1, h1 = cluster.clustered_ll_diff(yh, pB_h, pA_h, days_h)
    nd = int(pd.Series(days_h).nunique())
    lines.append(f"Paired, day-clustered over {nd} event-days ({len(yh)} markets):")
    lines.append("")
    lines.append("| comparison | diff | 95% CI | reading |")
    lines.append("|---|---|---|---|")
    lines.append(f"| model_b - {backbone} | {d1:+.4f} | [{l1:+.4f}, {h1:+.4f}] | "
                 f"{cluster.verdict_word(l1, h1)} |")
    if "base_pooled" in preds:
        pBase_h = clip(np.asarray(preds["base_pooled"], dtype=float))[hold]
        d2, l2, h2 = cluster.clustered_ll_diff(yh, pA_h, pBase_h, days_h)
        lines.append(f"| {backbone} - base_pooled | {d2:+.4f} | [{l2:+.4f}, {h2:+.4f}] | "
                     f"{cluster.verdict_word(l2, h2)} |")
    lines.append("")
    lines.append("Bare holdout log losses above are point estimates on ~78")
    lines.append("independent event-days; only these intervals support a claim.")
    if w is not None:
        lines.append("")
        lines.append("## Final fitted weights (last refit)")
        lines.append("")
        lines.append("| feature | weight |")
        lines.append("|---|---|")
        lines.append(f"| intercept | {w[0]:+.3f} |")
        for name, wt in zip(features, w[1:]):
            lines.append(f"| {name} | {wt:+.3f} |")
    lines.append("")
    lines.append("Next: feed component_b/predictions_ab.csv into market_baseline with")
    lines.append("--models model_b,... then verdict — the same heat map judges B.")
    manifest.append_to(lines, db_path=db_path, preds_df=preds)
    (out / "component_b_report.md").write_text("\n".join(lines))
    print(f"holdout logloss: B={llB:.4f} vs A={llA:.4f} ({gate}); wrote {out}/")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--predictions", default="component_a/component_a_predictions.csv")
    ap.add_argument("--out", default="component_b")
    ap.add_argument("--backbone", default="auto")
    ap.add_argument("--c-file", default=None,
                    help="component_c predictions CSV to include as a feature")
    ap.add_argument("--min-resolved", type=int, default=MIN_RESOLVED)
    ap.add_argument("--refit-every", type=int, default=REFIT_EVERY)
    args = ap.parse_args()
    run(args.db, args.predictions, args.out, args.backbone, args.c_file,
        args.min_resolved, args.refit_every)


if __name__ == "__main__":
    main()
