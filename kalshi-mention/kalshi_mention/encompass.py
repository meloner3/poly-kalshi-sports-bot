"""T5 — Forecast encompassing: the master information test.

    python -m kalshi_mention.encompass

The question the project exists to answer, in its highest-power form. Two
nested models over the same rows:

    M0:  logit(q) = alpha + beta_m * logit(mid)              market only
    M1:  logit(q) = alpha + beta_m * logit(mid) + beta_A * logit(p_A)

M0 is the market price RECALIBRATED — not the raw mid. That matters: if the
mid is systematically mis-scaled, a raw comparison would credit the model for
fixing the market's calibration rather than for adding information. Giving M0
its own alpha and beta_m removes that.

**Primary endpoint**: paired out-of-sample log-loss difference M1 - M0, with a
day-clustered CI. Out-of-sample via expanding chronological windows refit on
resolved-only markets, so no fit ever sees an outcome that had not settled at
decision time. Paired because both models score identical rows, which removes
the (large) shared variance from market difficulty.

**Secondary**: beta_A with a clustered CI, Brier, and calibration bins.

Interpretation, stated in the report and repeated here because it is the
thing most likely to be over-read:

  * A significant beta_A (or a negative M1-M0) means the model carries
    incremental information UNDER THIS FUNCTIONAL FORM. It is NOT
    profitability. The residual still has to clear the spread and the fee —
    which `verdict.py` measures, and which it currently does not clear.
  * A null means no evidence of incremental information under this form. It
    is NOT proof there is none: a different form, or more data, could find
    some. With ~40 live event-days this test resolves only large effects.

Eligibility follows T2: decision-time-legal rows only, defaulting to the
conservative pre-event variant.
"""

from __future__ import annotations

import argparse

import numpy as np
import pandas as pd

from . import cluster, manifest, paths

# One shared clipping rule everywhere in this module (T5 requirement).
CLIP = 1e-4
DEFAULT_H = 6
MIN_FIT_ROWS = 150          # below this an expanding-window fit is noise
REFIT_EVERY = 50
B_BOOT = 2000
HIST_CUTOFF = "2026-05-27"


def clip(p):
    return np.clip(np.asarray(p, dtype=float), CLIP, 1 - CLIP)


def logit(p):
    p = clip(p)
    return np.log(p / (1 - p))


def sigmoid(z):
    return 1.0 / (1.0 + np.exp(-np.clip(z, -30.0, 30.0)))


def logloss(y, p):
    p = clip(p)
    y = np.asarray(y, dtype=float)
    return -(y * np.log(p) + (1 - y) * np.log(1 - p))


def brier(y, p):
    return float(((clip(p) - np.asarray(y, dtype=float)) ** 2).mean())


# ---------------------------------------------------------------------------
# Fitting
# ---------------------------------------------------------------------------

def fit_logistic(X: np.ndarray, y: np.ndarray, l2: float = 1e-4,
                 iters: int = 100) -> np.ndarray:
    """Logistic MLE by damped Newton. X must already include an intercept."""
    w = np.zeros(X.shape[1])
    for _ in range(iters):
        p = sigmoid(X @ w)
        g = X.T @ (p - y) + l2 * w
        W = np.maximum(p * (1 - p), 1e-6)
        H = (X * W[:, None]).T @ X + l2 * np.eye(X.shape[1])
        try:
            step = np.linalg.solve(H, g)
        except np.linalg.LinAlgError:
            break
        new = w - step
        if not np.all(np.isfinite(new)):
            break
        if np.max(np.abs(new - w)) < 1e-10:
            return new
        w = new
    return w


def design(mid, p_model=None):
    """[1, logit(mid)] for M0; adds logit(p_model) for M1."""
    cols = [np.ones(len(mid)), logit(mid)]
    if p_model is not None:
        cols.append(logit(p_model))
    return np.column_stack(cols)


def fit_encompassing(y, mid, p_model, days, B: int = B_BOOT, seed: int = 0) -> dict:
    """Full-sample fit of M0 and M1 with a day-clustered CI on beta_A.

    Used for the secondary endpoint and by the synthetic acceptance tests.
    """
    y = np.asarray(y, dtype=float)
    X0, X1 = design(mid), design(mid, p_model)
    w0, w1 = fit_logistic(X0, y), fit_logistic(X1, y)

    uniq = pd.unique(np.asarray(days))
    idx = {d: np.flatnonzero(np.asarray(days) == d) for d in uniq}
    keys = list(idx)
    rng = np.random.default_rng(seed)
    betas = []
    for _ in range(B):
        rows = np.concatenate([idx[keys[j]] for j in
                               rng.choice(len(keys), len(keys), replace=True)])
        yb = y[rows]
        if yb.min() == yb.max():
            continue
        betas.append(fit_logistic(X1[rows], yb)[2])
    betas = np.array(betas) if betas else np.array([np.nan])
    lo, hi = (np.nanpercentile(betas, [2.5, 97.5]) if np.isfinite(betas).any()
              else (np.nan, np.nan))
    return {
        "alpha": float(w1[0]), "beta_m": float(w1[1]), "beta_A": float(w1[2]),
        "beta_A_lo": float(lo), "beta_A_hi": float(hi),
        "m0_alpha": float(w0[0]), "m0_beta_m": float(w0[1]),
        "n": int(len(y)), "days": int(len(keys)),
        "insample_ll_m0": float(logloss(y, sigmoid(X0 @ w0)).mean()),
        "insample_ll_m1": float(logloss(y, sigmoid(X1 @ w1)).mean()),
    }


def walk_forward_encompass(y, mid, p_model, decision_ts, resolve_ts,
                           min_fit: int = MIN_FIT_ROWS,
                           refit_every: int = REFIT_EVERY):
    """Expanding-window out-of-sample predictions for M0 and M1.

    Row i is predicted by models fitted only on rows whose market had already
    RESOLVED at row i's decision time. Knowing which past markets have settled
    is decision-time information; knowing their own outcome is not, and never
    happens here.

    Returns (p0, p1, usable_mask).
    """
    y = np.asarray(y, dtype=float)
    n = len(y)
    order = np.argsort(decision_ts)
    resolve_order = np.argsort(resolve_ts)
    X0, X1 = design(mid), design(mid, p_model)

    p0 = np.full(n, np.nan)
    p1 = np.full(n, np.nan)
    resolved: list[int] = []
    ptr = 0
    w0 = w1 = None
    fitted_at = -1
    for i in order:
        t = decision_ts[i]
        while ptr < n and resolve_ts[resolve_order[ptr]] <= t:
            resolved.append(int(resolve_order[ptr]))
            ptr += 1
        usable = [j for j in resolved if j != i]
        if len(usable) >= min_fit and (len(usable) - fitted_at) >= refit_every:
            idx = np.array(usable)
            yy = y[idx]
            if yy.min() != yy.max():
                w0 = fit_logistic(X0[idx], yy)
                w1 = fit_logistic(X1[idx], yy)
                fitted_at = len(usable)
        if w0 is not None:
            p0[i] = float(sigmoid(X0[i:i + 1] @ w0)[0])
            p1[i] = float(sigmoid(X1[i:i + 1] @ w1)[0])
    mask = ~np.isnan(p0) & ~np.isnan(p1)
    return p0, p1, mask


# ---------------------------------------------------------------------------
# Data assembly
# ---------------------------------------------------------------------------

def pick_frozen_a_column(preds: pd.DataFrame) -> str:
    """Component A's selected calibrated column, chosen the way A chooses it.

    Selection is on the earlier (selection) period only, never the holdout,
    mirroring component_a.run so the frozen column is the same one the
    project reports.
    """
    cal = [c for c in preds.columns if c.endswith("_cal")]
    if not cal:
        raise SystemExit("no calibrated Component A columns in predictions file")
    sel = preds.iloc[:int(len(preds) * 0.6)]
    yv = sel["y"].to_numpy(dtype=float)
    return min(cal, key=lambda c: logloss(yv, sel[c].to_numpy(dtype=float)).mean())


def assemble(snapshots_path: str, predictions_path: str, h: int,
             pre_event_only: bool, live_only: bool, a_col: str | None):
    snaps = pd.read_csv(paths.data_path(snapshots_path))
    preds = pd.read_csv(paths.data_path(predictions_path))
    a_col = a_col or pick_frozen_a_column(preds)
    if a_col not in preds.columns:
        raise SystemExit(f"frozen A column {a_col!r} not in predictions file")

    keep = ["ticker", a_col, "as_of_ts", "close_ts"]
    s = snaps.merge(preds[keep], on="ticker", how="inner", suffixes=("", "_p"))
    s = s[(s["h"] == h) & s["bid"].notna() & s["ask"].notna()].copy()
    if pre_event_only and "pre_event" in s.columns:
        s = s[s["pre_event"] == 1]
    s["event_day"] = cluster.event_day(s["ticker"])
    s["date"] = s["event_day"].map(manifest.day_to_date)
    if live_only:
        s = s[s["date"] >= HIST_CUTOFF]
    s["mid"] = (s["bid"] + s["ask"]) / 2.0
    s = s.dropna(subset=["mid", a_col, "y"])
    return s, a_col


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------

def run(snapshots_path: str, predictions_path: str, out_dir: str,
        h: int = DEFAULT_H, pre_event_only: bool = True,
        live_only: bool = True, a_col: str | None = None) -> None:
    out = paths.out_dir(out_dir)
    lines = ["# T5 — Forecast encompassing: does the model add to the price?", ""]
    lines.append("```")
    lines.append("M0:  logit(q) = alpha + beta_m * logit(mid)")
    lines.append("M1:  logit(q) = alpha + beta_m * logit(mid) + beta_A * logit(p_A)")
    lines.append("```")
    lines.append("")
    lines.append("M0 is the market **recalibrated**, not the raw mid, so M1 gets no")
    lines.append("credit for fixing the market's scaling. Primary endpoint is the")
    lines.append("paired out-of-sample log-loss difference M1 - M0.")
    lines.append("")

    variants = []
    for pe in ([True, False] if pre_event_only else [False]):
        s, used_col = assemble(snapshots_path, predictions_path, h, pe, live_only, a_col)
        if len(s) < 60:
            lines.append(f"- {'pre-event' if pe else 'all-eligible'}: only {len(s)} "
                         "rows, skipped.")
            continue
        y = s["y"].to_numpy(dtype=float)
        mid = s["mid"].to_numpy(dtype=float)
        pA = s[used_col].to_numpy(dtype=float)
        days = s["event_day"].to_numpy()
        dts = s["as_of_ts"].to_numpy(dtype=float) + h * 3600.0
        rts = s["close_ts"].to_numpy(dtype=float)

        p0, p1, mask = walk_forward_encompass(y, mid, pA, dts, rts)
        res = fit_encompassing(y, mid, pA, days)
        oos = None
        if mask.sum() >= 40:
            d, lo, hi = cluster.cluster_ci(
                logloss(y[mask], p1[mask]) - logloss(y[mask], p0[mask]), days[mask])
            oos = {"n": int(mask.sum()),
                   "days": int(pd.Series(days[mask]).nunique()),
                   "ll_m0": float(logloss(y[mask], p0[mask]).mean()),
                   "ll_m1": float(logloss(y[mask], p1[mask]).mean()),
                   "diff": d, "lo": lo, "hi": hi,
                   "brier_m0": brier(y[mask], p0[mask]),
                   "brier_m1": brier(y[mask], p1[mask])}
        variants.append(("pre-event" if pe else "all-eligible", used_col, s, res, oos, mask, p0, p1))

    if not variants:
        raise SystemExit("no eligible rows for the encompassing test.")

    lines.append(f"Snapshot: {h}h. Frozen A column: `{variants[0][1]}` "
                 "(Component A's selected calibrated column, at its opening-time value).")
    lines.append(f"Clipping: {CLIP} everywhere. Live-era only: {live_only}.")
    lines.append("")
    lines.append("## Primary endpoint — paired out-of-sample log loss (M1 - M0)")
    lines.append("")
    lines.append("| eligibility | rows | days | LL(M0) | LL(M1) | M1-M0 | 95% CI | reading |")
    lines.append("|---|---|---|---|---|---|---|---|")
    for name, _c, _s, _res, oos, *_ in variants:
        if oos is None:
            lines.append(f"| {name} | — | — | — | — | — | too few OOS rows | — |")
            continue
        read = ("model ADDS information" if oos["hi"] < 0 else
                "model HURTS" if oos["lo"] > 0 else
                "no detectable added information")
        lines.append(f"| {name} | {oos['n']} | {oos['days']} | {oos['ll_m0']:.4f} | "
                     f"{oos['ll_m1']:.4f} | {oos['diff']:+.4f} | "
                     f"[{oos['lo']:+.4f}, {oos['hi']:+.4f}] | {read} |")
    lines.append("")
    lines.append("Negative M1-M0 means adding the model improved out-of-sample loss.")
    lines.append("")
    lines.append("## Secondary — coefficients, Brier, calibration")
    lines.append("")
    lines.append("| eligibility | rows | days | beta_m | beta_A | beta_A 95% CI | Brier M0 | Brier M1 |")
    lines.append("|---|---|---|---|---|---|---|---|")
    for name, _c, _s, res, oos, *_ in variants:
        b0 = f"{oos['brier_m0']:.4f}" if oos else "n/a"
        b1 = f"{oos['brier_m1']:.4f}" if oos else "n/a"
        lines.append(f"| {name} | {res['n']} | {res['days']} | {res['beta_m']:+.3f} | "
                     f"{res['beta_A']:+.3f} | [{res['beta_A_lo']:+.3f}, "
                     f"{res['beta_A_hi']:+.3f}] | {b0} | {b1} |")
    lines.append("")
    lines.append("beta_A is an in-sample coefficient with a clustered CI; the")
    lines.append("out-of-sample table above is the endpoint that decides.")

    name, used_col, s, res, oos, mask, p0, p1 = variants[0]
    if oos is not None:
        lines.append("")
        lines.append(f"## Calibration bins ({name}, out-of-sample)")
        lines.append("")
        lines.append("| bin | n | mean pred M0 | mean pred M1 | realized |")
        lines.append("|---|---|---|---|---|")
        yv = s["y"].to_numpy(dtype=float)[mask]
        edges = np.linspace(0, 1, 11)
        rows_cal = []
        for i in range(10):
            m = (p1[mask] >= edges[i]) & (p1[mask] < edges[i + 1] if i < 9 else p1[mask] <= 1)
            if m.sum() < 5:
                continue
            lines.append(f"| {edges[i]:.1f}-{edges[i+1]:.1f} | {int(m.sum())} | "
                         f"{p0[mask][m].mean():.3f} | {p1[mask][m].mean():.3f} | "
                         f"{yv[m].mean():.3f} |")
            rows_cal.append({"bin_low": edges[i], "n": int(m.sum()),
                             "pred_m0": float(p0[mask][m].mean()),
                             "pred_m1": float(p1[mask][m].mean()),
                             "realized": float(yv[m].mean())})
        pd.DataFrame(rows_cal).to_csv(out / "encompass_calibration.csv", index=False)

    lines.append("")
    lines.append("## What this does and does not establish")
    lines.append("")
    lines.append("- A significant result means incremental information **under this")
    lines.append("  functional form**. It is NOT profitability: the residual still has")
    lines.append("  to clear the spread and the fee. See verdict_report.md, where it")
    lines.append("  currently does not.")
    lines.append("- A null means **no evidence under this form**, not proof of none.")
    lines.append("  At this day count only large effects are resolvable (see")
    lines.append("  analysis/verification_report.md, T0.3).")
    lines.append("- M0 is recalibrated, so this asks whether the model adds to a")
    lines.append("  fairly-scaled market, which is the harder and correct question.")

    manifest.append_to(lines, preds_df=pd.read_csv(paths.data_path(predictions_path)))
    (out / "encompassing_report.md").write_text("\n".join(lines))
    pd.DataFrame([{"eligibility": n, **r,
                   **({f"oos_{k}": v for k, v in (o or {}).items()})}
                  for n, _c, _s, r, o, *_ in variants]).to_csv(
        out / "encompassing.csv", index=False)
    for n, _c, _s, r, o, *_ in variants:
        if o:
            print(f"  {n}: M1-M0 = {o['diff']:+.4f} CI [{o['lo']:+.4f}, {o['hi']:+.4f}] "
                  f"({o['n']} rows, {o['days']} days); beta_A={r['beta_A']:+.3f} "
                  f"[{r['beta_A_lo']:+.3f}, {r['beta_A_hi']:+.3f}]")
    print(f"Wrote {out}/encompassing_report.md")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--snapshots", default="market_baseline/market_baseline_snapshots.csv")
    ap.add_argument("--predictions", default="component_a/component_a_predictions.csv")
    ap.add_argument("--out", default="encompass")
    ap.add_argument("--h", type=int, default=DEFAULT_H)
    ap.add_argument("--a-col", default=None, help="frozen Component A column")
    ap.add_argument("--all-eras", action="store_true")
    args = ap.parse_args()
    run(args.snapshots, args.predictions, args.out, h=args.h,
        pre_event_only=True, live_only=not args.all_eras, a_col=args.a_col)


if __name__ == "__main__":
    main()
