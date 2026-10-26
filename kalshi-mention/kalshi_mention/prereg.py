"""T6 — the frozen pre-registration (v2), as executable constants.

PREREGISTRATION.md is the human-readable freeze; this module is the same
freeze in code, so the document and the tracker cannot drift apart. Changing
anything here invalidates v2 and requires a v3 with a new start date.

Why v1 is void (both reasons independently sufficient):

  1. The entry rule changed. v1 traded on |model - mid| > threshold; v2 trades
     on executable EV against the touch. Those select different markets --
     the old rule mechanically over-selects wide spreads, because a wide
     spread inflates |model - mid| whether or not the forecast is any good.
  2. Post-2026-08-11 outcomes may already have been observed during ordinary
     pipeline runs, so v1's "evaluate only on data after 2026-08-11" can no
     longer be honoured. Evidence that would have been virgin is not.

Additionally, T2 showed one of v1's four supporting structures (NO-side
+0.05) reverses sign under decision-time-legal eligibility, so the prior that
motivated v1 is itself unreliable.
"""

from __future__ import annotations

import math

VERSION = "v2"

# --- what is frozen --------------------------------------------------------
# Forecast. Component A's selected calibrated column: the ONLY forecaster in
# the project whose advantage survives day-clustered inference (vs base_pooled
# -0.0428, CI [-0.0559, -0.0293]). model_u and model_b both fail their gates
# with intervals spanning zero, so using either would add complexity and
# researcher degrees of freedom for no demonstrated gain.
MODEL_COLUMN = "empirical_hl30_cal"

SNAPSHOT_HOURS = 6          # 1h is dominated by crossing cost; 24h is thin
ELIGIBILITY = "pre_event"   # T2's decision-time-legal conservative variant

# Entry: executable expected value against the price actually paid.
#   EV_yes = p - ask - fee(ask)
#   EV_no  = bid - p - fee(1 - bid)
# Trade the better side, and only if it clears the threshold.
EV_THRESHOLD = 0.02         # dollars per contract

# Liquidity and staleness filters, both decision-time observable.
MAX_SPREAD = 0.10           # skip books wider than 10c (p90 at 6h is 0.16)
MAX_QUOTE_AGE_S = 3600      # candle timestamp within 1h of the snapshot

CONTRACTS_PER_SIGNAL = 1    # no sizing rule is being tested

# prior_hits, frozen definition: documents whose occurrence count for this
# market's phrase spec reaches its threshold k, of the SAME classified event
# type, available strictly before the market's open_time.
PRIOR_HITS_DEFINITION = (
    "count(documents where spec_count >= k AND doc_type == market_type "
    "AND available_at < open_time)")

# Fee: Kalshi's published model fee, cent-aligned for a one-contract
# whole-cent fill. Verified 2026-09-10 against kalshi.com/docs/
# kalshi-fee-schedule.pdf and docs.kalshi.com/getting_started/fee_rounding.
FEE_RULE = "ceil_to_cent(0.07 * P * (1 - P)) per contract"

# --- sequential decision ---------------------------------------------------
# Endpoint: mean net P&L per contract, day-clustered.
PLANNED_MAX_DAYS = 320      # from T0.3: ~2c resolution at the best-powered cell
INTERIM_LOOKS_DAYS = (80, 160, 240, 320)
ALPHA = 0.05
COMMERCIAL_FUTILITY_BOUND = 0.02   # upper CI bound below this -> stop, not worth it

# O'Brien-Fleming boundary (Lan-DeMets form). At look k with information
# fraction t_k = days_k / PLANNED_MAX_DAYS, the two-sided critical value is
#
#     Z_k = z_(1 - ALPHA/2) / sqrt(t_k)
#
# so early looks demand near-overwhelming evidence and the final look is
# close to the nominal 1.96. Cumulative alpha spent by t is
# alpha*(t) = 2 - 2*Phi(z_(1-ALPHA/2) / sqrt(t)).
Z_ALPHA_2 = 1.959963985


def obf_boundary(days_observed: int, planned_max: int = PLANNED_MAX_DAYS) -> float:
    """Two-sided O'Brien-Fleming critical Z at this information fraction."""
    t = max(min(days_observed / float(planned_max), 1.0), 1e-6)
    return Z_ALPHA_2 / math.sqrt(t)


def alpha_spent(days_observed: int, planned_max: int = PLANNED_MAX_DAYS) -> float:
    """Cumulative two-sided alpha spent by this look."""
    from math import erf, sqrt
    z = obf_boundary(days_observed, planned_max)
    phi = 0.5 * (1.0 + erf(z / sqrt(2.0)))
    return 2.0 - 2.0 * phi


def decide(mean: float, lo: float, hi: float, days: int,
           planned_max: int = PLANNED_MAX_DAYS) -> tuple[str, str]:
    """The three-way sequential decision. Returns (decision, rationale).

    (i)   SUCCESS  — lower confidence bound above zero at the OBF boundary
    (ii)  FUTILITY — upper bound below the commercial floor; a real edge, if
                     any, is too small to be worth trading
    (iii) CONTINUE — otherwise
    """
    if days < 5:
        return "CONTINUE", f"only {days} eligible days; need >=5 clusters to bound"
    b = obf_boundary(days, planned_max)
    scale = b / Z_ALPHA_2                      # widen the 95% CI to the OBF width
    half = (hi - lo) / 2.0
    lo_adj, hi_adj = mean - half * scale, mean + half * scale
    if lo_adj > 0:
        return "SUCCESS", (f"OBF-adjusted lower bound {lo_adj:+.4f} > 0 "
                           f"at Z={b:.2f} (t={days}/{planned_max})")
    if hi_adj < COMMERCIAL_FUTILITY_BOUND:
        return "FUTILITY", (f"OBF-adjusted upper bound {hi_adj:+.4f} < "
                            f"{COMMERCIAL_FUTILITY_BOUND:.2f}/contract — any real "
                            "edge is too small to trade")
    return "CONTINUE", (f"OBF-adjusted CI [{lo_adj:+.4f}, {hi_adj:+.4f}] at "
                        f"Z={b:.2f}; neither bound decisive")


def select_trades(snaps, model_column: str = MODEL_COLUMN):
    """Apply the frozen entry rule. Returns the traded rows with net P&L.

    Every filter here is decision-time observable. Nothing references
    close_ts, settlement, or any later listing.
    """
    import numpy as np
    from .market_baseline import kalshi_fee

    t = snaps[snaps["bid"].notna() & snaps["ask"].notna()].copy()
    t = t[t["h"] == SNAPSHOT_HOURS]
    if ELIGIBILITY == "pre_event" and "pre_event" in t.columns:
        t = t[t["pre_event"] == 1]
    t = t[(t["ask"] - t["bid"]) <= MAX_SPREAD]
    if "candle_ts" in t.columns and "eval_ts" in t.columns:
        t = t[(t["eval_ts"] - t["candle_ts"]) <= MAX_QUOTE_AGE_S]
    if not len(t):
        return t.assign(side=[], net=[], ev=[])

    ask = t["ask"].to_numpy(dtype=float)
    bid = t["bid"].to_numpy(dtype=float)
    p = t[model_column].to_numpy(dtype=float)
    fee_y = np.array([kalshi_fee(c) for c in ask])
    fee_n = np.array([kalshi_fee(c) for c in 1.0 - bid])
    ev_y = p - ask - fee_y
    ev_n = bid - p - fee_n
    take_yes = ev_y >= ev_n
    ev = np.where(take_yes, ev_y, ev_n)
    t = t.assign(ev=ev, side=np.where(take_yes, "YES", "NO"))
    t = t[t["ev"] > EV_THRESHOLD].copy()
    if not len(t):
        return t

    ty = (t["side"] == "YES").to_numpy()
    y = t["y"].to_numpy(dtype=float)
    cost = np.where(ty, t["ask"], 1.0 - t["bid"])
    gross = np.where(ty, y - cost, (1 - y) - cost)
    t["net"] = (gross - np.array([kalshi_fee(c) for c in cost])) * CONTRACTS_PER_SIGNAL
    return t


def summary() -> list[str]:
    """The frozen parameter block, for embedding in reports."""
    return [
        f"- version: **{VERSION}**",
        f"- forecast column: `{MODEL_COLUMN}`",
        f"- snapshot: {SNAPSHOT_HOURS}h after market open",
        f"- eligibility: {ELIGIBILITY} (T2 decision-time-legal)",
        f"- entry: executable EV > {EV_THRESHOLD:.2f}/contract "
        "(EV_yes = p - ask - fee; EV_no = bid - p - fee)",
        f"- max spread: {MAX_SPREAD:.2f}; max quote age: {MAX_QUOTE_AGE_S}s",
        f"- size: {CONTRACTS_PER_SIGNAL} contract per signal",
        f"- fee: {FEE_RULE}",
        f"- planned max: {PLANNED_MAX_DAYS} eligible event-days; "
        f"interim looks at {INTERIM_LOOKS_DAYS}",
        f"- boundary: O'Brien-Fleming, two-sided alpha={ALPHA}; "
        f"futility if upper bound < {COMMERCIAL_FUTILITY_BOUND:.2f}",
    ]
