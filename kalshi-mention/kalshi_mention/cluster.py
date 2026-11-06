"""Day-clustered inference — the project's own standard, applied everywhere.

Markets on one event-day share a single transcript and a single realisation of
the world. They are one observation wearing many hats: 8,260 settled markets
are 228 event-days, a median of 36 markets per day. `verdict.py` has always
clustered its P&L bootstrap for exactly this reason. The model-selection gates
in `component_a.py` and `unified.py` did not, and reported bare holdout
loglosses as if they were exact — which is how a +0.0043 difference with a
95% CI of [-0.004, +0.013] came to be recorded as an established finding.

This module is the shared implementation so no comparison in the project is
made without an interval again.
"""

from __future__ import annotations

import numpy as np
import pandas as pd

EPS = 1e-6
B_BOOT = 4000


def event_day(tickers: pd.Series) -> pd.Series:
    """Event-day code from the ticker (the authoritative date; see CLAUDE.md)."""
    return tickers.str.extract(r"-([A-Z]*\d{2}[A-Z]{3}\d{2})")[0]


def logloss_vec(y: np.ndarray, p: np.ndarray) -> np.ndarray:
    """Per-observation log loss (not averaged — clustering needs the raw terms)."""
    p = np.clip(np.asarray(p, dtype=float), EPS, 1 - EPS)
    y = np.asarray(y, dtype=float)
    return -(y * np.log(p) + (1 - y) * np.log(1 - p))


def cluster_ci(values: np.ndarray, days: np.ndarray, B: int = B_BOOT,
               seed: int = 0) -> tuple[float, float, float]:
    """Day-clustered bootstrap mean and 95% CI.

    Resamples event-days with replacement, not rows: the unit of independence
    is the day. Returns (mean, lo, hi); NaNs when fewer than 5 clusters.
    """
    values = np.asarray(values, dtype=float)
    grouped: dict[object, np.ndarray] = {}
    for d in pd.unique(days):
        if d is None or (isinstance(d, float) and np.isnan(d)):
            continue
        grouped[d] = values[days == d]
    keys = np.array(list(grouped.keys()), dtype=object)
    if len(keys) < 5:
        return float(values.mean()), float("nan"), float("nan")
    rng = np.random.default_rng(seed)
    means = np.empty(B)
    for i in range(B):
        pick = rng.choice(len(keys), size=len(keys), replace=True)
        means[i] = np.concatenate([grouped[keys[j]] for j in pick]).mean()
    return float(values.mean()), float(np.percentile(means, 2.5)), float(np.percentile(means, 97.5))


def paired_logloss_ci(y: np.ndarray, p_a: np.ndarray, p_b: np.ndarray,
                      days: np.ndarray, B: int = B_BOOT, seed: int = 0
                      ) -> tuple[float, float, float]:
    """Day-clustered CI on the PAIRED logloss difference (model A - model B).

    Paired, because both models score the identical markets: the per-market
    difference removes the (large) shared variance from market difficulty and
    is far more powerful than comparing two separately-bootstrapped means.
    Negative means A is better.
    """
    return cluster_ci(logloss_vec(y, p_a) - logloss_vec(y, p_b), days, B, seed)


def verdict_word(lo: float, hi: float, lower_is_better: bool = True) -> str:
    """How to describe a comparison honestly given its interval."""
    if not np.isfinite(lo) or not np.isfinite(hi):
        return "too few clusters to judge"
    if lo <= 0.0 <= hi:
        return "no detectable difference at this power"
    better = (hi < 0) if lower_is_better else (lo > 0)
    return "better (CI excludes zero)" if better else "worse (CI excludes zero)"


def clustered_ll_diff(y, p1, p2, days, B: int = B_BOOT, seed: int = 0):
    """WORKORDER T4(b) entry point. Paired day-clustered log-loss difference.

    Alias of `paired_logloss_ci` under the name the work order specifies, so
    the contract is greppable. Returns (mean_diff, lo, hi); negative means p1
    is the better forecast.
    """
    return paired_logloss_ci(y, p1, p2, days, B=B, seed=seed)
