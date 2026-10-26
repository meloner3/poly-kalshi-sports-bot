from __future__ import annotations

import numpy as np

from kalshi_mention.directional_evaluation import score_rows


def _rows(day_count: int, model: str = "v1", net: float = 0.02):
    rows = []
    for day in range(day_count):
        rows.append({
            "model_version": model,
            "event_day": f"D{day}",
            "outcome_yes": 1,
            "net_pnl": net,
            "contracts": 1,
            "p_hat": 0.8,
            "predicted_edge": 0.03,
            "slippage": 0.01,
        })
    return rows


def test_gate_requires_minimum_days_and_positive_lower_bound():
    blocked = score_rows(_rows(59), "v1", bootstrap_count=300)
    assert blocked["status"] == "blocked"
    assert blocked["n_days"] == 59

    passed = score_rows(_rows(60), "v1", bootstrap_count=300)
    assert passed["status"] == "passed"
    assert passed["live_gate_passed"] is True
    assert passed["ci_low"] > 0


def test_gate_never_combines_model_versions():
    rows = _rows(60, model="v1") + _rows(60, model="v2", net=-0.02)
    result = score_rows(rows, "v1", bootstrap_count=300)
    assert result["n_days"] == 60
    assert result["edge_estimate"] > 0

def test_ci_matches_the_verdict_modules_day_clustered_bootstrap():
    import pandas as pd
    from kalshi_mention import verdict
    from kalshi_mention.directional_evaluation import clustered_ci

    rng = np.random.default_rng(3)
    values = rng.normal(0.01, 0.1, 80)
    days = np.array([f"D{index // 4}" for index in range(80)], dtype=object)
    mean, low, high = clustered_ci(values, days, bootstrap_count=400)
    frame = pd.DataFrame({"net": values, "event_day": days})
    _, expected_low, expected_high = verdict.cluster_boot(frame, "net", "event_day", B=400, seed=0)
    assert (low, high) == (expected_low, expected_high)
    assert mean == values.mean()
    assert low < mean < high


def test_realized_early_exit_counts_for_the_gate_before_its_market_resolves():
    rows = _rows(60)
    for row in rows[:10]:
        row["outcome_yes"] = None
    result = score_rows(rows, "v1", bootstrap_count=300)
    assert result["n_days"] == 60
    assert result["n_trades"] == 60
    assert result["n_trades_with_outcome"] == 50
    assert result["n_trades_pending_outcome"] == 10


def test_traded_side_calibration_scores_the_side_that_was_bought():
    # NO bought at 0.30 because p_hat(YES)=0.20; market resolved NO, so the NO side won.
    rows = []
    for day in range(6):
        rows.append({
            "model_version": "v1", "event_day": f"D{day}", "side": "no",
            "outcome_yes": 0, "net_pnl": 0.6, "contracts": 1,
            "p_hat": 0.2, "p_lo": 0.1, "p_hi": 0.3,
            "entry_vwap": 0.3, "entry_fees": 0.02, "exit_fees": 0.0,
            "slippage": 0.0, "predicted_edge": 0.4,
        })
    result = score_rows(rows, "v1", bootstrap_count=100)
    side = result["traded_side"]
    assert side["n"] == 6
    assert side["win_rate"] == 1.0
    assert abs(side["mean_p_side_hat"] - 0.8) < 1e-12
    assert abs(side["mean_p_side_lo"] - 0.7) < 1e-12  # 1 - p_hi
    assert abs(side["brier_model"] - 0.04) < 1e-12
    assert abs(side["brier_market_entry_price"] - 0.49) < 1e-12
    assert side["brier_skill_vs_market"] > 0
    # YES-oriented Brier on the same rows is computed on p_hat directly.
    assert abs(result["brier_score"] - 0.04) < 1e-12
    assert result["calibration_bins"][0]["fraction_won"] == 1.0


def test_load_rows_reads_only_paper_realized_rows_read_only(tmp_path):
    import sqlite3
    from kalshi_mention.directional_evaluation import load_rows

    path = tmp_path / "bot.db"
    conn = sqlite3.connect(path)
    conn.executescript("""
        CREATE TABLE directional_state (id INTEGER PRIMARY KEY, current_model_version TEXT);
        INSERT INTO directional_state VALUES (1, 'v1');
        CREATE TABLE directional_paper_positions (
            model_version TEXT, event_day TEXT, side TEXT, outcome_yes INTEGER, net_pnl REAL,
            contracts INTEGER, p_hat REAL, p_lo REAL, p_hi REAL, entry_vwap REAL,
            entry_fees REAL, exit_fees REAL, slippage REAL, predicted_edge REAL,
            mode TEXT, status TEXT, updated_at TEXT);
        INSERT INTO directional_paper_positions VALUES
          ('v1','D1','yes',1,0.5,1,.6,.5,.7,.4,.01,0,0,.05,'paper','settled','2026-01-02'),
          ('v1','D2','yes',NULL,0.1,1,.6,.5,.7,.4,.01,0,0,.05,'paper','closed','2026-01-03'),
          ('v1','D3','yes',NULL,NULL,1,.6,.5,.7,.4,.01,0,0,.05,'paper','open','2026-01-04'),
          ('v1','D4','yes',1,0.5,1,.6,.5,.7,.4,.01,0,0,.05,'live','settled','2026-01-05');
    """)
    conn.commit()
    conn.close()
    rows, version = load_rows(str(path))
    assert version == "v1"
    assert sorted(row["event_day"] for row in rows) == ["D1", "D2"]
    # Nothing may be created inside the bot's database.
    conn = sqlite3.connect(path)
    tables = {row[0] for row in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    conn.close()
    assert tables == {"directional_state", "directional_paper_positions"}
    assert load_rows(str(tmp_path / "missing.db")) == ([], None)


def test_write_evaluation_produces_the_gate_contract_the_signal_service_reads(tmp_path, monkeypatch):
    import json
    import sqlite3
    from kalshi_mention import directional_evaluation, paths, signal_service

    database = tmp_path / "bot.db"
    conn = sqlite3.connect(database)
    conn.executescript("""
        CREATE TABLE directional_state (id INTEGER PRIMARY KEY, current_model_version TEXT);
        INSERT INTO directional_state VALUES (1, 'v1');
        CREATE TABLE directional_paper_positions (
            model_version TEXT, event_day TEXT, side TEXT, outcome_yes INTEGER, net_pnl REAL,
            contracts INTEGER, p_hat REAL, p_lo REAL, p_hi REAL, entry_vwap REAL,
            entry_fees REAL, exit_fees REAL, slippage REAL, predicted_edge REAL,
            mode TEXT, status TEXT, updated_at TEXT);
    """)
    for day in range(8):
        conn.execute(
            "INSERT INTO directional_paper_positions VALUES "
            "('v1',?, 'yes',1,0.4,2,.7,.6,.8,.5,.02,0,0.01,.08,'paper','settled','2026-01-01')",
            (f"26JAN{day + 1:02d}",))
    conn.commit()
    conn.close()

    monkeypatch.setattr(paths, "out_dir", lambda name: _ensure(tmp_path / name))
    monkeypatch.setattr(paths, "data_path", lambda name: tmp_path / name)
    result = directional_evaluation.write_evaluation(str(database))
    gate = json.loads((tmp_path / "directional_evaluation" / "gate.json").read_text())
    assert gate["model_version"] == "v1"
    assert gate["n_days"] == 8
    assert gate["ci_low"] is not None and gate["ci_high"] is not None
    assert gate["live_gate_passed"] is False  # fewer than 60 event-days
    assert (tmp_path / "directional_evaluation" / "report.md").exists()
    verdict_view = signal_service.latest_verdict()
    assert verdict_view["n_days"] == 8 and verdict_view["model_version"] == "v1"
    assert result["traded_side"]["n"] == 8


def _ensure(path):
    path.mkdir(parents=True, exist_ok=True)
    return path
