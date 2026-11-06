"""Market-price baseline + divergence backtest (pass 3).

    python -m kalshi_mention.market_baseline --db kalshi_mention.sqlite \
        --predictions component_a/component_a_predictions.csv --out market_baseline

Component A passed the naive-baseline gate; this module runs the comparison
that decides tradeability: the market's own price. Two analyses:

1. FORECAST COMPARISON. At fixed snapshots (hours after market open), score
   the market's mid price against the model's prediction on the identical
   market subset. Honest framing: the model predicts once at open; the market
   at later snapshots has strictly more information, so later snapshots are
   biased toward the market. Beating the market at h=1 would be remarkable;
   losing at h=24 is expected and uninformative.

2. DIVERGENCE BACKTEST. Where |model - market| exceeds a threshold, simulate
   the trade at the touch (buy YES at the ask / buy NO at 1 - bid), hold to
   settlement, subtract Kalshi's trading fee. Average net P&L per contract is
   the number that speaks P&L. No slippage, queue, or size effects modeled —
   fills at the candle's closing quotes are an approximation, stated plainly.

Contamination guards: a snapshot is only used if it falls strictly before the
market's close (early-close markets otherwise leak the outcome into the
price), and only candles timestamped after open and at/before the snapshot
are eligible.
"""

from __future__ import annotations

import argparse
import json
import math
import re
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import pandas as pd

from . import config, db
from . import paths
from . import manifest

try:
    from zoneinfo import ZoneInfo
    _ET = ZoneInfo("America/New_York")
except Exception:                                    # pragma: no cover
    _ET = timezone.utc

_TICKER_DAY_RE = re.compile(r"-[A-Z]*(\d{2})([A-Z]{3})(\d{2})(?:-|$)")
_MONTH_NUM = {m: i + 1 for i, m in enumerate(
    ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"])}

SNAPSHOT_HOURS = [1, 6, 24]
EDGE_THRESHOLDS = [0.05, 0.10, 0.15]
# CLOSE_BUFFER_S removed in T2: filtering on close_ts is outcome-dependent
# selection, because these markets close early when the phrase occurs.
EPS = 1e-6


# --- candle parsing ----------------------------------------------------------

def _cents(obj, *keys):
    """Extract a cents value from nested ({'close': 43}) or flat (43) forms."""
    cur = obj
    for k in keys:
        if cur is None:
            return None
        if isinstance(cur, dict):
            cur = cur.get(k)
        else:
            break
    if isinstance(cur, dict):
        return None
    try:
        v = float(cur)
    except (TypeError, ValueError):
        return None
    return v


def _dollars(obj, outer, inner="close_dollars"):
    """Dollar-string form: {"yes_bid": {"close_dollars": "0.3500"}} -> 0.35"""
    cur = obj.get(outer)
    if isinstance(cur, dict):
        cur = cur.get(inner)
    try:
        return float(cur)
    except (TypeError, ValueError):
        return None


def parse_candle(raw: dict) -> dict | None:
    """Normalize one stored candle to {ts, bid, ask, trade, volume} (prices in
    dollars, None where absent). Handles Kalshi's dollar-string format
    ("close_dollars": "0.3500") plus nested/flat cents formats."""
    ts = raw.get("end_period_ts")
    if ts is None:
        return None
    to_p = lambda c: None if c is None else max(0.0, min(1.0, c / 100.0))
    bid = _dollars(raw, "yes_bid")
    if bid is None:
        bid = to_p(_cents(raw, "yes_bid", "close"))
    if bid is None:
        bid = to_p(_cents(raw, "yes_bid_close"))
    ask = _dollars(raw, "yes_ask")
    if ask is None:
        ask = to_p(_cents(raw, "yes_ask", "close"))
    if ask is None:
        ask = to_p(_cents(raw, "yes_ask_close"))
    trade = _dollars(raw, "price")
    if trade is None:
        trade = to_p(_cents(raw, "price", "close"))
    if trade is None:
        trade = to_p(_cents(raw, "close"))
    vol = raw.get("volume")
    if vol is None:
        vol = raw.get("volume_fp")
    try:
        vol = int(float(vol))
    except (TypeError, ValueError):
        vol = 0
    return {"ts": int(ts), "bid": bid, "ask": ask,
            "trade": trade, "volume": vol}


def quote_ok(c: dict) -> bool:
    """A quote is real only if both sides exist strictly inside (0,1) and the
    book saw volume; historical candles encode an empty book as 0.0000."""
    b, a = c.get("bid"), c.get("ask")
    return (b is not None and a is not None and 0.0 < b < 1.0 and 0.0 < a < 1.0
            and a >= b and c.get("volume", 0) > 0)


def candle_prob(c: dict) -> float | None:
    """Best probability estimate from one candle: mid of the touch, else last
    trade. Degenerate/empty quotes yield None."""
    b, a = c["bid"], c["ask"]
    if quote_ok(c) and (a - b) <= 0.30:
        return (a + b) / 2.0
    if c["trade"] is not None:
        return c["trade"]
    return None


def pick_snapshot(candles: list[dict], open_ts: float, eval_ts: float) -> dict | None:
    """Latest candle with open_ts < ts <= eval_ts. Decision-time information only.

    This function DELIBERATELY does not see close_ts or the settlement. It
    used to drop any snapshot within CLOSE_BUFFER_S of the market's close,
    which sounds like a contamination guard but is the opposite: these
    markets close early WHEN THE PHRASE OCCURS, so close_ts is a function of
    the outcome, and filtering on it is outcome-dependent sample selection.

    Measured on the settled set, the old buffer dropped 12.8% of NO markets
    versus 10.2% of YES markets at h=6 (1.1% vs 0.4% at h=1) — modest, but
    a look-ahead is a look-ahead, and nothing downstream could have detected
    it. A trader standing at eval_ts knows the candle exists; they do not
    know when the market will close.

    Eligibility is therefore: a quote was observably on the tape at or before
    the decision time. Nothing else.
    """
    best = None
    for c in candles:
        if open_ts < c["ts"] <= eval_ts:
            if best is None or c["ts"] > best["ts"]:
                best = c
    return best


def event_day_start_ts(ticker: str) -> float | None:
    """UTC timestamp of 00:00 America/New_York on the ticker's event day.

    Ticker date codes are day-granular and Kalshi gives no event start time,
    so the only decision-time-legal statement available is "before the event
    day began". That is conservative: an appearance at 14:00 ET counts the
    whole preceding day as during-event. Documented as a limitation, not a
    precise pre-event boundary.
    """
    m = _TICKER_DAY_RE.search(ticker or "")
    if not m:
        return None
    yy, mon, dd = m.group(1), m.group(2), m.group(3)
    try:
        d = datetime(2000 + int(yy), _MONTH_NUM[mon], int(dd), tzinfo=_ET)
    except (KeyError, ValueError):
        return None
    return d.timestamp()


# --- economics ---------------------------------------------------------------

# Kalshi fee schedule, verified 2026-09-10 against:
#   https://kalshi.com/docs/kalshi-fee-schedule.pdf  ("Fee Schedule for July
#     2026 - 7.7.26 Update")   -> fees = roundup(M * 0.07 * C * P * (1-P))
#   https://docs.kalshi.com/getting_started/fee_rounding
#     -> the MODEL fee is rounded up to $0.000001 (a centicent), NOT to the
#        cent. Separately, a standard member's balance change (position cost
#        PLUS fee) is floored to the $0.01 grid, and the shortfall is charged
#        as a "rounding fee".
FEE_FACTOR = 0.07
FEE_SCHEDULE_VERIFIED = "2026-09-10"


def fee_model(price: float, contracts: int = 1, multiplier: float = 1.0,
              factor: float = FEE_FACTOR) -> float:
    """Kalshi's published model fee: roundup(M * factor * C * P * (1-P)).

    Rounded up to $0.000001 per the API docs, which is the exchange's actual
    granularity. This is the general formula; `kalshi_fee` below is what a
    standard member actually pays under this project's conditions.
    """
    raw = multiplier * factor * contracts * price * (1.0 - price)
    return math.ceil(raw * 1e6) / 1e6


def kalshi_fee(price: float, factor: float = FEE_FACTOR) -> float:
    """Total fee a standard member pays for ONE contract at a whole-cent price.

    Equals ceil-to-the-cent of the model fee. That is NOT the general rule --
    it is the special case that applies here, and it is worth stating why,
    because the two rules disagree in general:

      Kalshi floors (position cost + fee) to the $0.01 grid and charges the
      shortfall as a rounding fee. Our prices come from candlesticks and are
      always whole cents, and the pre-registered rule trades exactly one
      contract, so the position cost is already cent-aligned. The alignment
      therefore falls entirely on the fee, making the total exactly the model
      fee rounded up to the next cent.

      Worked check: cost 0.43, model fee 0.07*0.43*0.57 = 0.017157.
      Balance change -0.447157 -> floored to -0.45 -> total fee 0.02, which
      is ceil(1.7157)/100 = 0.02. Agrees.

    For multi-contract or fractional-cent fills this would overstate the fee;
    use `fee_model` plus explicit alignment there. Overstating is the safe
    direction for a backtest, so no result here is flattered by it.
    """
    return math.ceil(100.0 * factor * price * (1.0 - price)) / 100.0


def decompose(y, take_yes, bid, ask):
    """Split per-contract P&L into its three economically distinct parts.

    Vectorised. Returns (settle_vs_mid, crossing, fee, net) with the exact
    identity

        net == settle_vs_mid - crossing - fee

    holding elementwise. The three pieces answer three different questions
    and the project has repeatedly conflated them:

      settle_vs_mid  did the forecast beat the market's own belief?
                     (YES: y - mid;  NO: (1-y) - (1-mid) = mid - y)
      crossing       what did it cost to get filled at the touch?
                     (YES: ask - mid;  NO: (1-bid) - (1-mid) = mid - bid;
                      both equal the half-spread)
      fee            Kalshi's charge on the entry price.

    A cell can show a large negative `net` purely through `crossing` while
    `settle_vs_mid` is flat -- which is exactly what the 1h cells do. Judging
    the forecast by `net` alone reads execution cost as forecast failure.
    """
    y = np.asarray(y, dtype=float)
    take_yes = np.asarray(take_yes, dtype=bool)
    bid = np.asarray(bid, dtype=float)
    ask = np.asarray(ask, dtype=float)
    mid = (bid + ask) / 2.0
    settle_vs_mid = np.where(take_yes, y - mid, mid - y)
    crossing = np.where(take_yes, ask - mid, mid - bid)
    cost = np.where(take_yes, ask, 1.0 - bid)
    fee = np.array([kalshi_fee(float(c)) for c in cost])
    net = settle_vs_mid - crossing - fee
    return settle_vs_mid, crossing, fee, net


def trade_pnl(p_model: float, y: int, bid: float, ask: float) -> tuple[float, float]:
    """(gross, net) P&L per contract for the divergence trade implied by the
    model against the touch: buy YES at ask if model > mid, else buy NO at
    1 - bid. Settlement pays 1 to the correct side."""
    mid = (bid + ask) / 2.0
    if p_model > mid:
        cost = ask
        gross = float(y) - cost
    else:
        cost = 1.0 - bid
        gross = float(1 - y) - cost
    return gross, gross - kalshi_fee(cost)


# --- metrics -----------------------------------------------------------------

def log_loss(y, p):
    p = np.clip(np.asarray(p, dtype=float), EPS, 1 - EPS)
    y = np.asarray(y, dtype=float)
    return float(-(y * np.log(p) + (1 - y) * np.log(1 - p)).mean())


def brier(y, p):
    return float(((np.asarray(p, float) - np.asarray(y, float)) ** 2).mean())


# --- main --------------------------------------------------------------------

def run(db_path: str, predictions_path: str, out_dir: str, models: list[str]) -> None:
    conn = db.connect(db_path)
    out = paths.out_dir(out_dir)

    preds = pd.read_csv(paths.data_path(predictions_path))
    missing = [m for m in models if m not in preds.columns]
    if missing:
        raise SystemExit(f"Models not in predictions file: {missing}. "
                         f"Available: {[c for c in preds.columns if '_' in c][:10]}...")
    primary = models[0]

    n_candle_markets = conn.execute(
        "SELECT COUNT(DISTINCT market_ticker) c FROM candlesticks").fetchone()["c"]
    if n_candle_markets == 0:
        raise SystemExit("candlesticks table is empty — run: "
                         "python -m kalshi_mention.collect --db ... --candles")

    candles_by_market: dict[str, list[dict]] = {}
    for row in conn.execute("SELECT market_ticker, raw_json FROM candlesticks"):
        c = parse_candle(json.loads(row["raw_json"]))
        if c is not None:
            candles_by_market.setdefault(row["market_ticker"], []).append(c)
    for v in candles_by_market.values():
        v.sort(key=lambda c: c["ts"])
    print(f"Candles loaded for {len(candles_by_market)} markets "
          f"({sum(len(v) for v in candles_by_market.values())} candles)")

    # --- build snapshot rows -------------------------------------------------
    snap_rows = []
    for _, r in preds.iterrows():
        cs = candles_by_market.get(r["ticker"])
        if not cs:
            continue
        _eds = event_day_start_ts(str(r["ticker"]))
        for h in SNAPSHOT_HOURS:
            eval_ts = float(r["as_of_ts"]) + h * 3600.0
            c = pick_snapshot(cs, float(r["as_of_ts"]), eval_ts)
            if c is None:
                continue
            p_mkt = candle_prob(c)
            if p_mkt is None:
                continue
            tradeable = quote_ok(c)
            row = {"ticker": r["ticker"], "h": h, "y": int(r["y"]),
                   "p_market": p_mkt,
                   "bid": c["bid"] if tradeable else None,
                   "ask": c["ask"] if tradeable else None,
                   "volume": c["volume"], "base_pooled": float(r["base_pooled"]),
                   "candle_ts": c["ts"], "eval_ts": eval_ts,
                   "pre_event": (1 if (_eds is not None and eval_ts < _eds) else 0)}
            for m in models:
                row[m] = float(r[m])
            snap_rows.append(row)
    snaps = pd.DataFrame(snap_rows)
    if snaps.empty:
        raise SystemExit("No usable snapshots — check candle coverage and close-time guards.")
    snaps.to_csv(out / "market_baseline_snapshots.csv", index=False)

    lines = ["# Market-Price Baseline & Divergence Backtest (pass 3)", ""]
    lines.append(f"Predictions: {len(preds)} markets; candle coverage: "
                 f"{len(candles_by_market)} markets; usable snapshot rows: {len(snaps)}.")
    lines.append(f"Primary model: `{primary}`. Snapshots are hours after market open;")
    lines.append("the model predicts once at open, so later snapshots structurally favor")
    lines.append("the market (it has more information). The trading question is the")
    lines.append("divergence table, not the forecast table.")
    lines.append("")
    lines.append("## Forecast comparison (identical market subset per row)")
    lines.append("")
    lines.append("| snapshot | n | market logloss | market brier | " +
                 " | ".join(f"{m} logloss" for m in models) + " | base logloss |")
    lines.append("|---" * (5 + len(models)) + "|")
    for h in SNAPSHOT_HOURS:
        g = snaps[snaps["h"] == h]
        if len(g) < 20:
            continue
        y = g["y"].to_numpy()
        cells = [str(h) + "h", str(len(g)),
                 f"{log_loss(y, g['p_market']):.4f}", f"{brier(y, g['p_market']):.4f}"]
        cells += [f"{log_loss(y, g[m]):.4f}" for m in models]
        cells += [f"{log_loss(y, g['base_pooled']):.4f}"]
        lines.append("| " + " | ".join(cells) + " |")

    lines.append("")
    lines.append(f"## Divergence backtest — `{primary}` vs the touch")
    lines.append("")
    lines.append("Trade when |model − market mid| > threshold: buy YES at ask / NO at")
    lines.append("1−bid, hold to settlement, net of Kalshi fee (7% · P·(1−P), ceil to cent).")
    lines.append("")
    lines.append("| snapshot | threshold | trades | hit rate | gross avg | net avg | net total |")
    lines.append("|---|---|---|---|---|---|---|")
    tradeable = snaps[(snaps["bid"].notna()) & (snaps["ask"].notna())]
    for h in SNAPSHOT_HOURS:
        g = tradeable[tradeable["h"] == h]
        for thr in EDGE_THRESHOLDS:
            sel = g[(g[primary] - g["p_market"]).abs() > thr]
            if len(sel) < 5:
                continue
            pnl = [trade_pnl(r[primary], r["y"], r["bid"], r["ask"]) for _, r in sel.iterrows()]
            gross = np.array([p[0] for p in pnl])
            net = np.array([p[1] for p in pnl])
            hit = float((gross > 0).mean())
            lines.append(f"| {h}h | {thr:.2f} | {len(sel)} | {hit:.2f} | "
                         f"{gross.mean():+.4f} | {net.mean():+.4f} | {net.sum():+.2f} |")

    lines.append("")
    lines.append("## Caveats (read before believing any number above)")
    lines.append("")
    lines.append("- Fills at the hourly candle's closing quotes; real fills face spread")
    lines.append("  movement, queue position, and size limits. Treat net averages as an")
    lines.append("  upper bound on realizable edge at small size.")
    lines.append("- Later snapshots hand the market an information advantage over the")
    lines.append("  open-time model; a positive net at h=1 with volume is the strongest signal.")
    lines.append("- Wide or missing quotes were excluded; that filters toward liquid")
    lines.append("  markets, which is also where fills are most realistic.")
    lines.append("- One-sided position sizing (1 contract per signal), no compounding,")
    lines.append("  no early exit on early-close markets.")

    manifest.append_to(lines, conn=conn, preds_df=preds, snaps_df=snaps)
    (out / "market_baseline_report.md").write_text("\n".join(lines))
    print(f"Wrote {out/'market_baseline_report.md'} and market_baseline_snapshots.csv")
    conn.close()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--predictions", default="component_a/component_a_predictions.csv")
    ap.add_argument("--out", default="market_baseline")
    ap.add_argument("--models", default="poisson_hl30_cal,poissonT_hl90_cal",
                    help="comma-separated model columns; first is primary for the backtest")
    args = ap.parse_args()
    run(args.db, args.predictions, args.out, [m.strip() for m in args.models.split(",")])


if __name__ == "__main__":
    main()
