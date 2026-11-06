"""Evaluate Polytaoli directional paper trades and maintain the live gate."""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import time
from datetime import datetime, timezone

import numpy as np
import pandas as pd

from . import paths, verdict

MIN_GATE_EVENT_DAYS = 60
BOOTSTRAPS = verdict.B_BOOT
SEED = 0
EVALUATION_REFRESH_SECONDS = int(os.environ.get("KALSHI_MENTION_EVALUATION_REFRESH_SECONDS", "300"))


def _finite(value: float | None) -> float | None:
    return float(value) if value is not None and np.isfinite(value) else None


def clustered_ci(
    values: np.ndarray,
    event_days: np.ndarray,
    bootstrap_count: int = BOOTSTRAPS,
    seed: int = SEED,
) -> tuple[float, float, float]:
    """Return (sample mean, 2.5th, 97.5th percentile) of the day-clustered bootstrap.

    Delegates to ``verdict.cluster_boot`` so the forward-paper gate uses exactly the
    resampling the historical verdict uses: whole event-days are drawn with
    replacement, every trade is one observation within its day, the interval is the
    two-sided 95% percentile interval, and fewer than 5 event-days yields NaN.
    """
    frame = pd.DataFrame({"net": np.asarray(values, dtype=float),
                          "event_day": np.asarray(event_days, dtype=object)})
    frame = frame[frame["event_day"].map(lambda day: day is not None and str(day) not in ("", "nan", "None"))]
    if frame.empty:
        return float("nan"), float("nan"), float("nan")
    _, low, high = verdict.cluster_boot(frame, "net", "event_day", B=bootstrap_count, seed=seed)
    return float(frame["net"].mean()), low, high


def _calibration_bins(probabilities: np.ndarray, outcomes: np.ndarray) -> list[dict]:
    bins = []
    edges = np.linspace(0, 1, 11)
    for bin_index in range(10):
        upper = edges[bin_index + 1]
        selected = (probabilities >= edges[bin_index]) & (
            probabilities <= upper if bin_index == 9 else probabilities < upper
        )
        if selected.any():
            bins.append({
                "bin_low": float(edges[bin_index]),
                "bin_high": float(upper),
                "n": int(selected.sum()),
                "mean_predicted": float(probabilities[selected].mean()),
                "fraction_won": float(outcomes[selected].mean()),
            })
    return bins


def _traded_side_arrays(rows: list[dict]) -> dict | None:
    """Re-express each resolved trade from the point of view of the side it bought.

    Trades are selected on ``p_lo`` (YES) or ``1 - p_hi`` (NO), so scoring only
    ``p_hat`` as a YES probability hides the selection effect. Here every probability
    is the model's claim that *the bought side wins*, and ``won`` is whether it did.
    """
    usable = [
        row for row in rows
        if row.get("side") in ("yes", "no")
        and row.get("outcome_yes") is not None
        and row.get("p_hat") is not None
        and row.get("entry_vwap") is not None
        and int(row.get("contracts") or 0) > 0
    ]
    if not usable:
        return None
    yes = np.array([row["side"] == "yes" for row in usable])
    outcome_yes = np.array([float(row["outcome_yes"]) for row in usable])
    p_hat = np.array([float(row["p_hat"]) for row in usable])
    has_interval = all(row.get("p_lo") is not None and row.get("p_hi") is not None for row in usable)
    p_lo = np.array([float(row["p_lo"]) if row.get("p_lo") is not None else np.nan for row in usable])
    p_hi = np.array([float(row["p_hi"]) if row.get("p_hi") is not None else np.nan for row in usable])
    price = np.array([float(row["entry_vwap"]) for row in usable])
    contracts = np.array([int(row["contracts"]) for row in usable], dtype=float)
    entry_fee = np.array([float(row.get("entry_fees") or 0.0) for row in usable]) / contracts
    return {
        "rows": usable,
        "p_side_hat": np.where(yes, p_hat, 1.0 - p_hat),
        "p_side_lo": np.where(yes, p_lo, 1.0 - p_hi) if has_interval else None,
        "won": np.where(yes, outcome_yes, 1.0 - outcome_yes),
        "price": price,
        "entry_fee": entry_fee,
        "days": np.array([row["event_day"] for row in usable], dtype=object),
    }


def score_rows(rows: list[dict], model_version: str, bootstrap_count: int = BOOTSTRAPS) -> dict:
    """Score one strategy version's closed/settled paper trades.

    Gate statistics use every realized trade (``net_pnl`` known), so an early exit
    is not held back until its market resolves. Calibration statistics use only the
    subset whose market outcome is known.
    """
    realized = [
        row for row in rows
        if row["model_version"] == model_version
        and row["net_pnl"] is not None
        and int(row["contracts"] or 0) > 0
    ]
    days = np.array([row["event_day"] for row in realized], dtype=object)
    per_contract = np.array([
        float(row["net_pnl"]) / int(row["contracts"]) for row in realized
    ], dtype=float)
    mean, ci_low, ci_high = clustered_ci(per_contract, days, bootstrap_count)
    event_days = len({day for day in days if day is not None and str(day) not in ("", "nan", "None")})

    resolved = [row for row in realized if row["outcome_yes"] is not None]
    outcomes = np.array([float(row["outcome_yes"]) for row in resolved], dtype=float)
    probabilities = np.array([float(row["p_hat"]) for row in resolved], dtype=float)
    brier = float(np.mean((probabilities - outcomes) ** 2)) if resolved else None

    predicted = np.array([
        float(row["predicted_edge"]) if row.get("predicted_edge") is not None else np.nan
        for row in realized
    ], dtype=float)
    has_predicted = np.isfinite(predicted)
    gap = per_contract[has_predicted] - predicted[has_predicted]

    traded = _traded_side_arrays(resolved)
    side_report: dict = {"n": 0}
    if traded is not None:
        won, p_side = traded["won"], traded["p_side_hat"]
        price, fee = traded["price"], traded["entry_fee"]
        model_sq = (p_side - won) ** 2
        market_sq = (price - won) ** 2
        skill = market_sq - model_sq
        _, skill_low, skill_high = clustered_ci(skill, traded["days"], bootstrap_count)
        expected_edge_hat = p_side - price - fee
        realized_edge = np.array([
            float(row["net_pnl"]) / int(row["contracts"]) for row in traded["rows"]
        ])
        _, hat_gap_low, hat_gap_high = clustered_ci(
            realized_edge - expected_edge_hat, traded["days"], bootstrap_count)
        side_report = {
            "n": int(len(won)),
            "win_rate": float(won.mean()),
            "mean_p_side_hat": float(p_side.mean()),
            "mean_p_side_lo": (
                float(np.nanmean(traded["p_side_lo"])) if traded["p_side_lo"] is not None else None
            ),
            "mean_entry_price": float(price.mean()),
            "brier_model": float(model_sq.mean()),
            "brier_market_entry_price": float(market_sq.mean()),
            "brier_skill_vs_market": float(skill.mean()),
            "brier_skill_ci_low": _finite(skill_low),
            "brier_skill_ci_high": _finite(skill_high),
            "mean_expected_edge_p_hat": float(expected_edge_hat.mean()),
            "mean_realized_minus_expected_p_hat": float((realized_edge - expected_edge_hat).mean()),
            "realized_minus_expected_p_hat_ci_low": _finite(hat_gap_low),
            "realized_minus_expected_p_hat_ci_high": _finite(hat_gap_high),
        }

    by_day = []
    for day in sorted({str(day) for day in days}):
        mask = np.array([str(item) == day for item in days])
        by_day.append({
            "event_day": day,
            "n_trades": int(mask.sum()),
            "net_pnl": float(sum(float(row["net_pnl"]) for row, keep in zip(realized, mask) if keep)),
            "mean_net_per_contract": float(per_contract[mask].mean()),
        })

    fees = [
        (float(row.get("entry_fees") or 0.0) + float(row.get("exit_fees") or 0.0)) / int(row["contracts"])
        for row in realized
    ]
    slippage = [float(row["slippage"]) for row in realized if row.get("slippage") is not None]
    passed = event_days >= MIN_GATE_EVENT_DAYS and np.isfinite(ci_low) and ci_low > 0.0
    return {
        "model_version": model_version,
        "edge_estimate": _finite(mean),
        "ci_low": _finite(ci_low),
        "ci_high": _finite(ci_high),
        "ci_method": (
            "two-sided 95% percentile interval of the day-clustered bootstrap "
            "(verdict.cluster_boot: event-days resampled with replacement, per-trade mean)"
        ),
        "bootstrap_count": bootstrap_count,
        "n_days": event_days,
        "n_trades": len(realized),
        "n_trades_with_outcome": len(resolved),
        "n_trades_pending_outcome": len(realized) - len(resolved),
        "brier_score": brier,
        "brier_score_note": "p_hat scored as a YES probability on every resolved trade; see traded_side for the side-aware view",
        "mean_predicted_net_edge": float(np.nanmean(predicted)) if has_predicted.any() else None,
        "mean_realized_net_pnl_per_contract": float(per_contract.mean()) if realized else None,
        "mean_realized_minus_predicted": float(gap.mean()) if gap.size else None,
        "predicted_edge_definition": "p_lo (or 1 - p_hi) minus VWAP and fee per contract at entry",
        "mean_fees_per_contract": float(np.mean(fees)) if fees else None,
        "mean_slippage": float(np.mean(slippage)) if slippage else None,
        "traded_side": side_report,
        "calibration_bins": (
            _calibration_bins(traded["p_side_hat"], traded["won"]) if traded is not None else []
        ),
        "calibration_bins_basis": "model probability that the bought side wins vs fraction of those trades that won",
        "calibration_bins_p_hat_yes": _calibration_bins(probabilities, outcomes) if resolved else [],
        "by_day": by_day,
        "resolved_net_pnl": float(np.sum([float(row["net_pnl"]) for row in realized])),
        "fill_assumption": (
            "paper fills are taken at the fresh Kalshi WebSocket book with no queue position or "
            "market impact, so realized edge is an upper bound on what live execution would achieve"
        ),
        "status": "passed" if passed else "blocked",
        "live_gate_passed": bool(passed),
        "as_of": datetime.now(timezone.utc).isoformat(timespec="seconds"),
    }


def load_rows(database: str) -> tuple[list[dict], str | None]:
    # The database belongs to the Rust bot: open it read-only so this process can neither
    # create the kalshi-mention schema inside it nor hold a write lock against the bot.
    if not os.path.exists(database):
        return [], None
    conn = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
    conn.row_factory = sqlite3.Row
    try:
        names = {row[0] for row in conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table'"
        )}
        if "directional_paper_positions" not in names:
            return [], None
        current_version = None
        if "directional_state" in names:
            state = conn.execute(
                "SELECT current_model_version FROM directional_state WHERE id=1"
            ).fetchone()
            current_version = state[0] if state else None
        rows = [dict(row) for row in conn.execute(
            """SELECT model_version, event_day, side, outcome_yes, net_pnl, contracts,
                      p_hat, p_lo, p_hi, entry_vwap, entry_fees, exit_fees,
                      slippage, predicted_edge
               FROM directional_paper_positions
             WHERE mode = 'paper' AND status IN ('closed','settled')
             ORDER BY updated_at DESC"""
        )]
        return rows, current_version
    finally:
        conn.close()


def write_evaluation(database: str | None = None) -> dict:
    database = database or str(paths.REPO_ROOT / "rust-backend" / "arbitrage_history.db")
    rows,current_version = load_rows(database)
    version = current_version or (rows[0]["model_version"] if rows else "")
    result = score_rows(rows, version) if version else score_rows([], "")
    output = paths.out_dir("directional_evaluation")
    gate = {key: result[key] for key in (
        "edge_estimate", "ci_low", "ci_high", "n_days", "as_of", "model_version",
        "status", "live_gate_passed", "brier_score", "n_trades",
    )}
    (output / "gate.json").write_text(json.dumps(gate, indent=2, sort_keys=True) + "\n")
    (output / "report.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    lines = [
        "# Directional Paper Evaluation", "",
        f"Model version: `{version or 'none'}`",
        f"Resolved paper trades: {result['n_trades']} across {result['n_days']} event-days",
        f"Mean net P&L/contract: {result['edge_estimate']}",
        f"Two-sided 95% day-clustered bootstrap CI (verdict method): "
        f"[{result['ci_low']}, {result['ci_high']}]",
        f"Trades with a known outcome: {result['n_trades_with_outcome']} "
        f"({result['n_trades_pending_outcome']} realized but awaiting resolution)",
        f"Brier score (p_hat as YES probability): {result['brier_score']}",
        f"Traded-side calibration: {json.dumps(result['traded_side'], sort_keys=True)}",
        f"Predicted net edge/contract (p_lo basis): {result['mean_predicted_net_edge']}",
        f"Realized net P&L/contract (net of fees and fill slippage): {result['mean_realized_net_pnl_per_contract']}",
        f"Realized minus predicted: {result['mean_realized_minus_predicted']}",
        f"Mean fees/contract: {result['mean_fees_per_contract']}",
        "",
        f"Caveat: {result['fill_assumption']}.",
        f"Live gate: **{result['status'].upper()}** (requires >= {MIN_GATE_EVENT_DAYS} days and ci_low > 0)",
        "",
        "Calibration bins and per-day results: `report.json`; CI/gate contract: `gate.json`.",
    ]
    (output / "report.md").write_text("\n".join(lines) + "\n")
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    default_database = str(paths.REPO_ROOT / "rust-backend" / "arbitrage_history.db")
    parser.add_argument("--db", default=default_database)
    parser.add_argument("--watch", action="store_true", help="refresh gate artifact periodically")
    parser.add_argument("--interval-seconds", type=int, default=EVALUATION_REFRESH_SECONDS)
    args = parser.parse_args()
    if args.watch and args.interval_seconds < 1:
        parser.error("interval-seconds must be positive")
    while True:
        result = write_evaluation(args.db)
        print(f"Directional paper gate: {result['status']} ({result['n_days']} event-days)")
        if not args.watch:
            break
        time.sleep(args.interval_seconds)


def run_once() -> dict:
    return write_evaluation()


if __name__ == "__main__":
    main()