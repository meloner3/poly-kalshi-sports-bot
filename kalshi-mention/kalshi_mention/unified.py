"""Unified model ("U") — the project's best single forecaster.

    python -m kalshi_mention.unified --db kalshi_mention.sqlite --out unified

One learned model consuming every validated signal in the system, built from
the project's own experimental findings:

  1. ALL Component A variants as features (every empirical/poisson/negbin x
     half-life x typed x calibrated column) — per-market model selection,
     learned, instead of one global pick.
  2. The k-threshold correction: Component B's walk-forward fit exposed a
     large systematic bias on "(3+ times)" markets (weight +0.49); U gets the
     raw ingredients (k, hits-at-k, hits-at-1) to learn the correction.
  3. PHRASE CO-OCCURRENCE CONTEXT — the "what goes together" signal Component
     C was meant to capture, computed exactly instead of simulated: from all
     transcripts available at decision time, learn PMI(p, q) between phrases,
     then score each market by the recent activity of its phrase's historical
     companions. NATO vocabulary heating up lifts "Article 5" before any base
     rate can see it. Fully point-in-time: co-occurrence counts advance with
     an incremental engine that only ever sees past documents.
  4. Component B's context features (news salience when cached, momentum,
     recency, listing load, event types) and Component C's p_c when present.

Learner: gradient-boosted trees (scikit-learn HistGradientBoosting) refit
walk-forward on resolved-only outcomes, with the project's proven logistic as
automatic fallback when sklearn is unavailable. Output probabilities get the
same walk-forward Platt calibration Component A uses. Gate: beat the best
single Component A column on holdout log loss.

Output: unified/predictions_unified.csv with column `model_u`, consumed
automatically by market_baseline and verdict via the pipeline.
"""

from __future__ import annotations

import argparse
import math
from collections import defaultdict
from pathlib import Path

import numpy as np
import pandas as pd

from . import config, db
from .component_a import walk_forward_calibrate
from .component_b import (FEATURES as B_FEATURES, build_features, clip,
                          fit_logistic, logit, predict_logistic)
from . import cluster, paths
from . import manifest

MIN_RESOLVED = 300
REFIT_EVERY = 150
HOLDOUT_FRAC = 0.4
COOC_HALFLIFE_D = 14.0
COOC_WINDOW_D = 30
COOC_TOPK = 50


# ---------------------------------------------------------------------------
# Point-in-time co-occurrence engine
# ---------------------------------------------------------------------------

class CoocEngine:
    """Incremental phrase co-occurrence over documents ordered by date.

    advance(date) ingests all documents strictly BEFORE date; signal(spec, date)
    then scores how strongly the spec's historical companions have been active
    in the trailing window — using only ingested (past) documents. The engine
    can only move forward in time, which structurally enforces point-in-time.
    """

    def __init__(self, doc_dates: list[str], doc_specs: list[set[str]]):
        order = np.argsort(np.array(doc_dates))
        self.dates = [doc_dates[i] for i in order]
        self.specsets = [doc_specs[i] for i in order]
        self.ptr = 0
        self.n_docs = 0
        self.c: dict[str, int] = defaultdict(int)
        self.cpq: dict[tuple[str, str], int] = defaultdict(int)
        self.recent: list[tuple[str, set[str]]] = []  # (date, specset) window

    def advance(self, date: str) -> None:
        while self.ptr < len(self.dates) and self.dates[self.ptr] < date:
            specs = self.specsets[self.ptr]
            self.n_docs += 1
            sl = sorted(specs)
            for i, a in enumerate(sl):
                self.c[a] += 1
                for b in sl[i + 1:]:
                    self.cpq[(a, b)] += 1
            self.recent.append((self.dates[self.ptr], specs))
            self.ptr += 1
        # trim trailing window
        cutoff = _shift_date(date, -COOC_WINDOW_D)
        while self.recent and self.recent[0][0] < cutoff:
            self.recent.pop(0)

    def pmi(self, a: str, b: str) -> float:
        if self.n_docs < 50:
            return 0.0
        key = (a, b) if a < b else (b, a)
        cab = self.cpq.get(key, 0)
        ca, cb = self.c.get(a, 0), self.c.get(b, 0)
        if ca == 0 or cb == 0:
            return 0.0
        return math.log(((cab + 0.5) * self.n_docs) / ((ca + 0.5) * (cb + 0.5)))

    def signal(self, spec: str, date: str) -> float:
        """Activity-weighted mean PMI of the spec with recently-active specs."""
        if not self.recent:
            return 0.0
        act: dict[str, float] = defaultdict(float)
        for d, specs in self.recent:
            age = _days_between(d, date)
            w = 0.5 ** (age / COOC_HALFLIFE_D)
            for q in specs:
                if q != spec:
                    act[q] += w
        if not act:
            return 0.0
        top = sorted(act.items(), key=lambda kv: -kv[1])[:COOC_TOPK]
        num = sum(w * self.pmi(spec, q) for q, w in top)
        den = sum(w for _q, w in top)
        return num / den if den > 0 else 0.0


def _shift_date(d: str, days: int) -> str:
    from datetime import date, timedelta
    return (date.fromisoformat(d) + timedelta(days=days)).isoformat()


def _days_between(d0: str, d1: str) -> float:
    from datetime import date
    return float((date.fromisoformat(d1) - date.fromisoformat(d0)).days)



# T8(d): the three A columns fed to U, in preference order.
A_FEATURE_COLUMNS = ["empirical_hl30_cal", "poisson_hl90_cal", "empiricalT_hl30_cal"]

BETA_PRIOR = 2.0   # Beta(2,2): shrinks a thin k>1 sample toward the base rate


def k_correction(k: np.ndarray, y: np.ndarray, as_of: np.ndarray,
                 close: np.ndarray) -> np.ndarray:
    """Point-in-time, partially-pooled log-odds offset for threshold markets.

    For each market i, using ONLY markets resolved strictly before as_of[i]:

        p_thr  = Beta-smoothed YES rate among resolved k>1 markets
        p_base = Beta-smoothed YES rate among resolved k=1 markets
        offset = is_thresh_i * (logit(p_thr) - logit(p_base))

    With no resolved k>1 markets yet the Beta prior pins p_thr at 0.5 and the
    offset is whatever the pooled evidence supports -- which early on is
    almost nothing. That is the partial pooling: the feature earns its
    magnitude rather than being handed five free parameters.
    """
    n = len(k)
    out = np.zeros(n)
    order = np.argsort(as_of)
    resolve_order = np.argsort(close)
    ptr = 0
    s_thr = n_thr = s_base = n_base = 0.0
    a = b = BETA_PRIOR
    for i in order:
        while ptr < n and close[resolve_order[ptr]] <= as_of[i]:
            j = resolve_order[ptr]
            if k[j] > 1:
                s_thr += y[j]
                n_thr += 1
            else:
                s_base += y[j]
                n_base += 1
            ptr += 1
        if k[i] > 1 and n_base > 0:
            p_thr = (a + s_thr) / (a + b + n_thr)
            p_base = (a + s_base) / (a + b + n_base)
            out[i] = float(logit(np.array([p_thr]))[0] - logit(np.array([p_base]))[0])
    return out

# ---------------------------------------------------------------------------
# Learner (boosted trees with logistic fallback)
# ---------------------------------------------------------------------------

def make_learner(force_logistic: bool = False):
    if not force_logistic:
        try:
            from sklearn.ensemble import HistGradientBoostingClassifier

            def fit(X, y):
                m = HistGradientBoostingClassifier(
                    max_depth=3, max_iter=150, learning_rate=0.06,
                    l2_regularization=1.0, min_samples_leaf=40,
                    early_stopping=False, random_state=0)
                m.fit(X, y)
                return m

            def predict(m, X):
                return clip(m.predict_proba(X)[:, 1])

            return fit, predict, "hist_gradient_boosting"
        except ImportError:
            pass

    def fit(X, y):
        return fit_logistic(X, y, l2=3.0)

    def predict(w, X):
        return predict_logistic(w, X)

    return fit, predict, "logistic_fallback"


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def run(db_path: str, predictions_path: str, out_dir: str,
        c_file: str | None = None, min_resolved: int = MIN_RESOLVED,
        refit_every: int = REFIT_EVERY, force_logistic: bool = False) -> None:
    out = paths.out_dir(out_dir)
    preds = pd.read_csv(paths.data_path(predictions_path))
    n = len(preds)
    y = preds["y"].to_numpy(dtype=float)
    as_of = preds["as_of_ts"].to_numpy(dtype=float)
    close = preds["close_ts"].to_numpy(dtype=float)

    # -- feature block 1: every Component A model column, as logits ----------
    # T8(d): three columns representing DISTINCT mechanisms, not 24 near-copies.
    # Feeding a depth-3 GBM two dozen correlated versions of one number, on an
    # effective sample of ~228 independent event-days, is variance farming.
    # Kept: one empirical (short half-life, untyped), one count model (long
    # half-life), one typed. That spans empirical-vs-count, short-vs-long, and
    # typed-vs-untyped; the rest are interpolations between them.
    preferred = [c for c in A_FEATURE_COLUMNS if c in preds.columns]
    all_a = [c for c in preds.columns if any(
        c.startswith(p) for p in ("empirical", "poisson", "negbin"))]
    a_cols = preferred or all_a[:3]
    print(f"A-model feature columns: {len(a_cols)} of {len(all_a)} available "
          f"({', '.join(a_cols)})")
    A_logits = np.column_stack([logit(preds[c].to_numpy(dtype=float)) for c in a_cols])
    backbone = next((c for c in a_cols if c.endswith("_cal")),
                    next((c for c in all_a if c.endswith("_cal")), a_cols[0]))

    # -- feature block 2: Component B context features -----------------------
    feats = build_features(db_path, preds, backbone)
    b_names = list(B_FEATURES)
    if c_file:
        cdf = pd.read_csv(paths.data_path(c_file))[["ticker", "p_c"]]
        feats = feats.merge(cdf, on="ticker", how="left")
        feats["logit_pC"] = np.where(feats["p_c"].notna(),
                                     logit(feats["p_c"].fillna(0.5)), 0.0)
        feats["has_pC"] = feats["p_c"].notna().astype(float)
        b_names += ["logit_pC", "has_pC"]
        print(f"Component C merged: {int(feats['has_pC'].sum())} markets")
    B_mat = feats[b_names].to_numpy(dtype=float)

    # -- feature block 3: threshold-correction ingredients -------------------
    # T8(c): ONE partially-pooled correction, not five threshold features.
    #
    # The five features existed to learn Component B's +0.494 "(3+ times)"
    # bias. That weight rests on 73 markets out of 8,260. Five free parameters
    # for 73 observations is decoration, and the correction it encodes is not
    # separable from noise at that n.
    #
    # Replaced by a single Beta-smoothed, point-in-time log-odds offset: how
    # much more often k>1 markets resolved YES than k=1 markets, among markets
    # ALREADY RESOLVED at this market's open. The Beta prior does the partial
    # pooling -- with few k>1 resolutions the offset shrinks to zero on its
    # own, and it only grows as evidence accumulates.
    kk = preds["k"].to_numpy(dtype=float)
    T_mat = np.column_stack([k_correction(kk, y, as_of, close)])
    t_names = ["k_correction"]

    # -- feature block 4: co-occurrence context ------------------------------
    conn = db.connect(db_path)
    doc_rows = conn.execute(
        """SELECT t.doc_id, t.date FROM transcripts t
           WHERE EXISTS (SELECT 1 FROM phrase_doc_counts p WHERE p.doc_id = t.doc_id)"""
    ).fetchall()
    spec_by_doc: dict[str, set[str]] = defaultdict(set)
    for r in conn.execute("SELECT spec_key, doc_id FROM phrase_doc_counts"):
        spec_by_doc[r["doc_id"]].add(r["spec_key"])
    doc_dates, doc_specs = [], []
    for r in doc_rows:
        d = str(r["date"])[:10]
        if len(d) == 10 and d[4] == "-":
            doc_dates.append(d)
            doc_specs.append(spec_by_doc.get(r["doc_id"], set()))
    engine = CoocEngine(doc_dates, doc_specs)

    import json as _json
    from datetime import datetime, timezone
    from .phrase import extract_phrase, spec_key
    sk_map = {}
    for r in conn.execute("SELECT ticker, raw_json FROM markets WHERE result IN ('yes','no')"):
        ph, _ = extract_phrase(_json.loads(r["raw_json"]))
        if ph:
            sk_map[r["ticker"]] = spec_key(ph)
    conn.close()

    order = np.argsort(as_of)
    cooc = np.zeros(n)
    for idx in order:
        open_date = datetime.fromtimestamp(as_of[idx], tz=timezone.utc).date().isoformat()
        engine.advance(open_date)
        sk = sk_map.get(preds["ticker"].iloc[idx])
        cooc[idx] = engine.signal(sk, open_date) if sk else 0.0
    print(f"co-occurrence signal computed ({engine.n_docs} docs ingested)")

    X_all = np.column_stack([A_logits, B_mat, T_mat, cooc])
    feat_names = a_cols + b_names + t_names + ["cooc_context"]

    # -- walk-forward fit ----------------------------------------------------
    fit, predict, learner_name = make_learner(force_logistic)
    print(f"learner: {learner_name}; features: {X_all.shape[1]}")
    pU_raw = preds[backbone].to_numpy(dtype=float).copy()
    resolve_order = np.argsort(close)
    ptr, resolved, model, fitted_at = 0, [], None, -1
    for i in range(n):
        while ptr < n and close[resolve_order[ptr]] <= as_of[i]:
            resolved.append(resolve_order[ptr])
            ptr += 1
        if len(resolved) >= min_resolved and len(resolved) - fitted_at >= refit_every:
            idx = np.array(resolved)
            model = fit(X_all[idx], y[idx])
            fitted_at = len(resolved)
        if model is not None:
            pU_raw[i] = float(predict(model, X_all[i:i + 1])[0])
    pU = walk_forward_calibrate(clip(pU_raw), y, as_of, close)
    preds["model_u"] = clip(pU)

    # -- evaluation ----------------------------------------------------------
    split = int(n * (1 - HOLDOUT_FRAC))
    hold = slice(split, n)

    def ll(col_vals):
        pv = clip(np.asarray(col_vals, float)[hold])
        yv = y[hold]
        return float(-(yv * np.log(pv) + (1 - yv) * np.log(1 - pv)).mean())

    a_hold = {c: ll(preds[c]) for c in a_cols}
    best_a = min(a_hold, key=a_hold.get)
    llU = ll(preds["model_u"])
    gate = "PASS" if llU < a_hold[best_a] else "FAIL"

    # The point difference alone is not a finding. Markets on one event-day
    # share a transcript, so the holdout's 3,304 markets are ~78 independent
    # days; a bare comparison of 0.6348 vs 0.6305 overstates precision by an
    # order of magnitude. Paired, day-clustered CI on the difference.
    hold_days = cluster.event_day(preds["ticker"].iloc[hold]).to_numpy()
    d_mean, d_lo, d_hi = cluster.paired_logloss_ci(
        y[hold], np.asarray(preds["model_u"])[hold],
        np.asarray(preds[best_a])[hold], hold_days)
    gate_reading = cluster.verdict_word(d_lo, d_hi)
    n_days = len(pd.unique(hold_days[pd.notna(hold_days)]))

    # permutation importance on the resolved set with the final model
    importances = []
    if model is not None and len(resolved) > 200:
        idx = np.array(resolved)
        base_p = predict(model, X_all[idx])
        base_ll = float(-(y[idx] * np.log(base_p) + (1 - y[idx]) * np.log(1 - base_p)).mean())
        rng = np.random.default_rng(0)
        for j, name in enumerate(feat_names):
            Xp = X_all[idx].copy()
            rng.shuffle(Xp[:, j])
            pp = predict(model, Xp)
            d = float(-(y[idx] * np.log(pp) + (1 - y[idx]) * np.log(1 - pp)).mean()) - base_ll
            importances.append((name, d))
        importances.sort(key=lambda kv: -kv[1])

    preds.to_csv(out / "predictions_unified.csv", index=False)
    lines = ["# Unified model (U) — walk-forward ensemble", ""]
    lines.append(f"Learner: {learner_name}; {X_all.shape[1]} features "
                 f"({len(a_cols)} A-model logits, {len(b_names)} context, "
                 f"{len(t_names)} threshold, 1 co-occurrence).")
    lines.append(f"Refit every {refit_every} resolved (min {min_resolved}); "
                 "Platt walk-forward calibration on output.")
    lines.append("")
    lines.append("| model | holdout logloss |")
    lines.append("|---|---|")
    lines.append(f"| **model_u** | **{llU:.4f}** |")
    lines.append(f"| best single A ({best_a}) | {a_hold[best_a]:.4f} |")
    if "base_pooled" in preds:
        lines.append(f"| base_pooled | {ll(preds['base_pooled']):.4f} |")
    lines.append("")
    lines.append(f"Gate (U must beat the best single A column): **{gate}**")
    lines.append("")
    lines.append(f"Paired difference (U - {best_a}) on the holdout: "
                 f"**{d_mean:+.4f}**, day-clustered 95% CI "
                 f"[{d_lo:+.4f}, {d_hi:+.4f}] over {n_days} event-days "
                 f"({len(preds.iloc[hold])} markets).")
    lines.append("")
    lines.append(f"Reading: **{gate_reading}**.")
    lines.append("")
    if d_lo <= 0 <= d_hi:
        lines.append("The gate outcome still governs adoption — a more complex model "
                     "that shows no improvement over its nested baseline should not "
                     "be adopted — but a FAIL whose interval spans zero is *not* "
                     "evidence that the added signals are worthless. It is evidence "
                     "that this sample cannot tell.")
    elif d_lo > 0:
        lines.append("The interval excludes zero on the wrong side: U is measurably "
                     "WORSE than the single A column, not merely unproven. That is a "
                     "stronger statement than a bare FAIL and it does not need more "
                     "data to be believed.")
    else:
        lines.append("The interval excludes zero in U's favour. Note the gate and the "
                     "interval now disagree with the PASS/FAIL text above; re-read "
                     "both before drawing a conclusion.")
    if importances:
        lines.append("")
        lines.append("## Permutation importance (top 15, resolved set)")
        lines.append("")
        lines.append("*In-sample* (computed on the fitted set), so these rank "
                     "what the model leans on, not what generalises. Treat as "
                     "diagnostic, never as evidence a signal is real.")
        lines.append("")
        lines.append("| feature | logloss increase when shuffled |")
        lines.append("|---|---|")
        for name, d in importances[:15]:
            lines.append(f"| {name} | {d:+.4f} |")
    lines.append("")
    lines.append("The heat map judges U next: pipeline --only baseline,verdict")
    manifest.append_to(lines, db_path=db_path, preds_df=preds)
    (out / "unified_report.md").write_text("\n".join(lines))
    print(f"holdout logloss: U={llU:.4f} vs best A ({best_a})={a_hold[best_a]:.4f} ({gate})")
    print(f"Wrote {out}/unified_report.md and predictions_unified.csv")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--predictions", default="component_a/component_a_predictions.csv")
    ap.add_argument("--out", default="unified")
    ap.add_argument("--c-file", default=None)
    ap.add_argument("--min-resolved", type=int, default=MIN_RESOLVED)
    ap.add_argument("--refit-every", type=int, default=REFIT_EVERY)
    ap.add_argument("--force-logistic", action="store_true")
    args = ap.parse_args()
    run(args.db, args.predictions, args.out, args.c_file,
        args.min_resolved, args.refit_every, args.force_logistic)


if __name__ == "__main__":
    main()
