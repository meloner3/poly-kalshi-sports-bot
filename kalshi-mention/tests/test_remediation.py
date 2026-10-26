"""Regression tests for the post-review remediation (WORKORDER T0-T8).

Each test pins a defect that a review found, so it cannot silently return.
"""

from __future__ import annotations

import numpy as np
import pandas as pd
import pytest

from kalshi_mention import cluster, db as kdb


# ---------------------------------------------------------------------------
# Corpus staleness (found while fixing the time-dependent predict test)
# ---------------------------------------------------------------------------

def _mkconn(tmp_path, last_doc_days_ago: int):
    conn = kdb.connect(str(tmp_path / "s.sqlite"))
    d = (pd.Timestamp.now(tz="UTC") - pd.Timedelta(days=last_doc_days_ago)).date().isoformat()
    conn.execute(
        "INSERT INTO transcripts (doc_id,title,date,speech_type,text_raw,token_count,"
        "available_at,availability_source) VALUES (?,?,?,?,?,?,?,?)",
        ("d0", "t", d, "Speech", "hello world", 2, d + "T23:00:00+00:00", "t"))
    conn.commit()
    return conn


def test_corpus_staleness_measured(tmp_path):
    from kalshi_mention.predict import corpus_staleness
    conn = _mkconn(tmp_path, 100)
    age = corpus_staleness(conn)
    assert 99 <= age <= 101


def test_stale_corpus_warns(tmp_path, capsys):
    """A stale corpus must announce itself: predictions silently drift to 0.5."""
    from kalshi_mention.predict import warn_if_stale
    conn = _mkconn(tmp_path, 200)
    warn_if_stale(conn)
    assert "STALE" in capsys.readouterr().out


def test_fresh_corpus_silent(tmp_path, capsys):
    from kalshi_mention.predict import warn_if_stale
    conn = _mkconn(tmp_path, 3)
    warn_if_stale(conn)
    assert capsys.readouterr().out.strip() == ""


def test_prediction_corpus_freshness_abstains_after_configured_age(tmp_path):
    from kalshi_mention.predict import require_fresh_corpus
    conn = _mkconn(tmp_path, 15)
    with pytest.raises(ValueError, match="prediction abstained"):
        require_fresh_corpus(conn, max_age_days=14)


def test_prediction_corpus_freshness_accepts_within_configured_age(tmp_path):
    from kalshi_mention.predict import require_fresh_corpus
    conn = _mkconn(tmp_path, 13)
    assert require_fresh_corpus(conn, max_age_days=14) <= 14


def test_prediction_corpus_freshness_abstains_when_no_corpus(tmp_path):
    from kalshi_mention.predict import require_fresh_corpus
    conn = kdb.connect(str(tmp_path / "empty.sqlite"))
    with pytest.raises(ValueError, match="no usable dates"):
        require_fresh_corpus(conn, max_age_days=14)


def test_future_unavailable_transcript_does_not_hide_stale_corpus(tmp_path):
    from kalshi_mention.predict import require_fresh_corpus

    conn = _mkconn(tmp_path, 15)
    future_date = (pd.Timestamp.now(tz="UTC") + pd.Timedelta(days=1)).date().isoformat()
    conn.execute(
        "INSERT INTO transcripts (doc_id,title,date,speech_type,text_raw,token_count,"
        "available_at,availability_source) VALUES (?,?,?,?,?,?,?,?)",
        ("future", "future", future_date, "Speech", "not available", 2,
         (pd.Timestamp.now(tz="UTC") + pd.Timedelta(days=1)).isoformat(), "assumed_lag"),
    )
    conn.commit()
    with pytest.raises(ValueError, match="prediction abstained"):
        require_fresh_corpus(conn, max_age_days=14)


def test_corpus_refresh_syncs_parquet_rows_into_predictor_database(tmp_path):
    from kalshi_mention.refresh_corpus import sync_corpus_to_db

    corpus = tmp_path / "corpus.csv"
    pd.DataFrame([{
        "doc_id": "fresh-doc",
        "title": "Fresh transcript",
        "date": pd.Timestamp.now(tz="UTC").date().isoformat(),
        "speech_type": "Speech",
        "text_raw": "fresh corpus content",
        "token_count": 3,
    }]).to_csv(corpus, index=False)
    database = tmp_path / "predictor.sqlite"

    assert sync_corpus_to_db(str(database), str(corpus)) == 1
    conn = kdb.connect(str(database))
    row = conn.execute("SELECT doc_id, text_raw FROM transcripts").fetchone()
    assert (row["doc_id"], row["text_raw"]) == ("fresh-doc", "fresh corpus content")
    conn.close()


def test_scheduled_refresh_rebuilds_counts_only_when_scrape_adds_rows(monkeypatch):
    from kalshi_mention import refresh_corpus

    calls = []
    monkeypatch.setattr(refresh_corpus, "scrape_transcripts", lambda path: calls.append(("scrape", path)) or 1)
    monkeypatch.setattr(refresh_corpus, "sync_corpus_to_db", lambda db_path, corpus: calls.append(("sync", db_path, corpus)) or 12)
    monkeypatch.setattr(refresh_corpus.counts, "update_missing_doc_counts", lambda db_path: calls.append(("counts", db_path)) or 1)

    assert refresh_corpus.run("test-db", "test-corpus.parquet") == 12
    assert calls == [
        ("scrape", "test-corpus.parquet"),
        ("sync", "test-db", str(refresh_corpus.paths.data_path("test-corpus.parquet"))),
        ("counts", "test-db"),
    ]


def test_missing_phrase_counts_are_computed_atomically(tmp_path):
    conn = kdb.connect(str(tmp_path / "incremental.sqlite"))
    kdb.upsert_market(conn, {
        "ticker": "KXTRUMPMENTION-26OCT01-CHINA",
        "event_ticker": "KXTRUMPMENTION-26OCT01",
        "series_ticker": "KXTRUMPMENTION",
        "title": "What will Trump say during a rally?",
        "yes_sub_title": "China",
        "result": "yes",
        "status": "settled",
    })
    kdb.upsert_transcript(
        conn, "new-doc", "Speech", "2026-10-01", "Speech", "China economy",
        2, "2026-10-01T12:00:00+00:00", "scraped_at",
    )
    conn.commit()
    conn.close()

    from kalshi_mention.counts import update_missing_doc_counts
    assert update_missing_doc_counts(str(tmp_path / "incremental.sqlite")) == 1
    assert update_missing_doc_counts(str(tmp_path / "incremental.sqlite")) == 0
    conn = kdb.connect(str(tmp_path / "incremental.sqlite"))
    assert conn.execute("SELECT COUNT(*) FROM phrase_doc_counts").fetchone()[0] == 1
    assert conn.execute("SELECT COUNT(*) FROM phrase_doc_counts_status").fetchone()[0] == 1
    conn.close()


# ---------------------------------------------------------------------------
# Day-clustered inference (T4b)
# ---------------------------------------------------------------------------

def test_event_day_extraction():
    s = pd.Series(["KXTRUMPMENTION-26JUL24-UFC", "KXPRESMENTION-26JUL24-CLIN",
                   "KXTRUMPMENTIONB-26AUG02-IRAN"])
    assert list(cluster.event_day(s)) == ["26JUL24", "26JUL24", "26AUG02"]


def test_clustered_ci_wider_than_naive():
    """Clustering must widen intervals when observations repeat within a day."""
    rng = np.random.default_rng(0)
    days, vals = [], []
    for d in range(40):                       # 40 days, 30 correlated obs each
        shock = rng.normal(0, 1.0)
        for _ in range(30):
            days.append(f"D{d:03d}")
            vals.append(shock + rng.normal(0, 0.1))
    vals = np.array(vals)
    days = np.array(days)
    _m, lo, hi = cluster.cluster_ci(vals, days, B=800)
    clustered_half = (hi - lo) / 2
    naive_half = 1.96 * vals.std(ddof=1) / np.sqrt(len(vals))
    assert clustered_half > 3 * naive_half


def test_paired_logloss_ci_detects_real_difference():
    rng = np.random.default_rng(1)
    n = 2000
    days = np.array([f"D{i//25:03d}" for i in range(n)])
    y = rng.binomial(1, 0.5, n).astype(float)
    good = np.where(y == 1, 0.8, 0.2)          # informative
    bad = np.full(n, 0.5)                      # uninformative
    m, lo, hi = cluster.paired_logloss_ci(y, good, bad, days, B=500)
    assert m < 0 and hi < 0                    # 'good' better, CI excludes zero


def test_verdict_word():
    assert "no detectable" in cluster.verdict_word(-0.01, 0.01)
    assert "better" in cluster.verdict_word(-0.05, -0.01)
    assert "worse" in cluster.verdict_word(0.01, 0.05)


# ---------------------------------------------------------------------------
# Sample manifests (T1)
# ---------------------------------------------------------------------------

def _seed_markets(conn, n=6):
    import pandas as pd
    base = pd.Timestamp("2026-07-01", tz="UTC")
    for i in range(n):
        d = base + pd.Timedelta(days=i)
        tick = f"{d.strftime('%y')}JUL{d.day:02d}"
        kdb.upsert_market(conn, {
            "ticker": f"KXTRUMPMENTION-{tick}-A{i}",
            "event_ticker": f"KXTRUMPMENTION-{tick}",
            "series_ticker": "KXTRUMPMENTION", "title": "t",
            "yes_sub_title": "China", "result": "yes" if i % 2 else "no",
            "status": "settled", "open_time": d.isoformat(),
            "close_time": (d + pd.Timedelta(hours=6)).isoformat(),
            "expiration_time": (d + pd.Timedelta(hours=6)).isoformat(),
            "rules_primary": ""})
    conn.commit()


def test_manifest_has_required_fields(tmp_path):
    from kalshi_mention import manifest as M
    conn = kdb.connect(str(tmp_path / "m.sqlite"))
    _seed_markets(conn)
    m = M.manifest(conn)
    for key in ("db_path", "db_mtime", "series", "markets_all", "markets_settled",
                "excluded_series", "markets_modeled", "events", "transcripts",
                "corpus_range", "live_era_boundary"):
        assert key in m, key
    assert m["markets_settled"] == 6
    assert m["markets_modeled"] == 6          # nothing excluded in this fixture


def test_manifest_excludes_configured_series(tmp_path):
    """KXTRUMPSAY is excluded from modeling; the manifest must show the split."""
    import pandas as pd
    from kalshi_mention import manifest as M
    conn = kdb.connect(str(tmp_path / "m2.sqlite"))
    _seed_markets(conn, 4)
    d = pd.Timestamp("2026-07-20", tz="UTC")
    kdb.upsert_market(conn, {
        "ticker": "KXTRUMPSAY-26JUL20-X", "event_ticker": "KXTRUMPSAY-26JUL20",
        "series_ticker": "KXTRUMPSAY", "title": "t", "yes_sub_title": "China",
        "result": "yes", "status": "settled", "open_time": d.isoformat(),
        "close_time": d.isoformat(), "expiration_time": d.isoformat(),
        "rules_primary": ""})
    conn.commit()
    m = M.manifest(conn)
    assert m["markets_settled"] == 5
    assert m["markets_excluded"] == 1
    assert m["markets_modeled"] == 4


def test_manifest_identity_is_stable_and_rendered(tmp_path):
    """Two reports off the same db must show identical identity lines."""
    from kalshi_mention import manifest as M
    conn = kdb.connect(str(tmp_path / "m3.sqlite"))
    _seed_markets(conn)
    a, b = M.identity_line(M.manifest(conn)), M.identity_line(M.manifest(conn))
    assert a == b
    body = "\n".join(M.render(M.manifest(conn)))
    assert "## Sample manifest" in body and "Dataset identity" in body


def test_manifest_counts_event_days_not_markets(tmp_path):
    """The manifest must surface event-days, the real unit of independence."""
    from kalshi_mention import manifest as M
    conn = kdb.connect(str(tmp_path / "m4.sqlite"))
    _seed_markets(conn)
    preds = pd.DataFrame({"ticker": ["KXTRUMPMENTION-26JUL01-A",
                                     "KXTRUMPMENTION-26JUL01-B",
                                     "KXTRUMPMENTION-26JUL02-C"]})
    m = M.manifest(conn, preds_df=preds)
    assert m["preds_rows"] == 3
    assert m["event_days"] == 2


# ---------------------------------------------------------------------------
# T2 — eligibility must not depend on the outcome
# ---------------------------------------------------------------------------

def test_eligibility_invariant_to_close_and_settlement():
    """The acceptance criterion for T2.

    A snapshot's eligibility must be a function of decision-time information
    only. Altering when the market closed, or how it settled, must not change
    which candle is chosen — otherwise the backtest sample is selected on the
    outcome (these markets close early WHEN THE PHRASE OCCURS).
    """
    from kalshi_mention.market_baseline import pick_snapshot
    open_ts, eval_ts = 1000.0, 1000.0 + 6 * 3600
    candles = [{"ts": open_ts + 600, "bid": 0.4, "ask": 0.5, "trade": 0.45, "volume": 10},
               {"ts": open_ts + 3600, "bid": 0.41, "ask": 0.49, "trade": 0.45, "volume": 12},
               {"ts": eval_ts + 60, "bid": 0.9, "ask": 0.95, "trade": 0.92, "volume": 99}]
    chosen = pick_snapshot(candles, open_ts, eval_ts)
    assert chosen is not None and chosen["ts"] == open_ts + 3600  # never past eval_ts

    # pick_snapshot cannot even receive close/settlement any more.
    import inspect
    params = set(inspect.signature(pick_snapshot).parameters)
    assert "close_ts" not in params, "eligibility must not see close time"
    assert params == {"candles", "open_ts", "eval_ts"}

    # And the result is byte-identical for markets that closed at wildly
    # different times / settled differently.
    for _close, _result in ((eval_ts + 1, "yes"), (eval_ts + 10 ** 6, "no"),
                            (open_ts + 1, "yes")):
        assert pick_snapshot(candles, open_ts, eval_ts) == chosen


def test_pre_event_flag_is_day_granular_and_conservative():
    from kalshi_mention.market_baseline import event_day_start_ts
    ts = event_day_start_ts("KXTRUMPMENTION-26JUL24-UFC")
    assert ts is not None
    from datetime import datetime, timezone
    d = datetime.fromtimestamp(ts, tz=timezone.utc)
    # 00:00 ET on 2026-07-24 is 04:00 or 05:00 UTC depending on DST
    assert d.year == 2026 and d.month == 7 and d.day == 24 and d.hour in (4, 5)
    assert event_day_start_ts("NOT-A-TICKER") is None


# ---------------------------------------------------------------------------
# T3 — P&L decomposition identity
# ---------------------------------------------------------------------------

def test_pnl_decomposition_identity_exact():
    """net == settle_vs_mid - crossing - fee, elementwise, to 1e-9."""
    from kalshi_mention.market_baseline import decompose
    rng = np.random.default_rng(0)
    n = 5000
    bid = rng.uniform(0.02, 0.60, n)
    ask = np.minimum(bid + rng.uniform(0.01, 0.35, n), 0.99)
    y = rng.integers(0, 2, n).astype(float)
    take_yes = rng.random(n) < 0.5
    sm, cr, fee, net = decompose(y, take_yes, bid, ask)
    assert np.abs(net - (sm - cr - fee)).max() < 1e-9


def test_decomposition_matches_legacy_trade_pnl():
    """The decomposition must not change any P&L number, only explain it."""
    from kalshi_mention.market_baseline import decompose, trade_pnl
    rng = np.random.default_rng(7)
    n = 500
    bid = rng.uniform(0.05, 0.6, n)
    ask = np.minimum(bid + rng.uniform(0.01, 0.3, n), 0.99)
    y = rng.integers(0, 2, n)
    p = rng.uniform(0.01, 0.99, n)
    take_yes = p > (bid + ask) / 2
    _sm, _cr, _fee, net = decompose(y, take_yes, bid, ask)
    ref = np.array([trade_pnl(p[i], int(y[i]), bid[i], ask[i])[1] for i in range(n)])
    assert np.abs(net - ref).max() < 1e-12


def test_crossing_equals_half_spread_both_sides():
    """Crossing cost is the half-spread whichever side is taken."""
    from kalshi_mention.market_baseline import decompose
    bid, ask = np.array([0.30, 0.30]), np.array([0.50, 0.50])
    _sm, cr, _fee, _net = decompose(np.array([1.0, 0.0]),
                                    np.array([True, False]), bid, ask)
    assert np.allclose(cr, 0.10)


def test_settle_vs_mid_is_payoff_minus_mid_price():
    """Skill credit = payoff minus the price at the mid, per side.

    NOT symmetric across sides, and it should not be: at mid 0.40 a winning
    YES stakes 0.40 to gain 0.60, while a winning NO stakes 0.60 to gain
    0.40. Equal-credit would be the bug.
    """
    from kalshi_mention.market_baseline import decompose
    bid, ask = np.array([0.30, 0.30, 0.30, 0.30]), np.array([0.50, 0.50, 0.50, 0.50])
    #                YES/win  NO/win   YES/lose  NO/lose
    y = np.array([1.0, 0.0, 0.0, 1.0])
    take_yes = np.array([True, False, True, False])
    sm, _cr, _fee, _net = decompose(y, take_yes, bid, ask)
    assert np.isclose(sm[0], +0.60)    # YES wins: 1 - mid
    assert np.isclose(sm[1], +0.40)    # NO wins:  mid - 0
    assert np.isclose(sm[2], -0.40)    # YES loses: 0 - mid
    assert np.isclose(sm[3], -0.60)    # NO loses:  mid - 1
    # a winning bet always beats the mid, a losing one never does
    assert (sm[:2] > 0).all() and (sm[2:] < 0).all()


# ---------------------------------------------------------------------------
# T4(a) — listing_load must not see the future
# ---------------------------------------------------------------------------

def _listing_load(preds: pd.DataFrame) -> np.ndarray:
    """Mirror of the production expression in component_b.build_features."""
    day = cluster.event_day(preds["ticker"])
    ld = pd.DataFrame({"day": day, "ts": preds["as_of_ts"].to_numpy(dtype=float)})
    return ld.groupby("day")["ts"].rank(method="max").to_numpy()


def test_listing_load_is_point_in_time_with_staggered_opens():
    """Staggered opens: each market may only count listings that already exist.

    59% of real event-days stagger opens by more than an hour (median 5.8h),
    so the old frame-wide value_counts gave every market the day's FINAL
    count -- on average 11 of 46 listings that did not yet exist.
    """
    preds = pd.DataFrame({
        "ticker": [f"KXTRUMPMENTION-26JUL24-M{i}" for i in range(4)],
        "as_of_ts": [1000.0, 2000.0, 3000.0, 4000.0],   # 4 staggered opens
    })
    load = _listing_load(preds)
    assert list(load) == [1.0, 2.0, 3.0, 4.0]
    # the old behaviour would have been [4,4,4,4]
    assert not (load == 4).all()


def test_listing_load_ties_see_each_other():
    """Simultaneously-listed markets DO all exist at each other's open."""
    preds = pd.DataFrame({
        "ticker": [f"KXTRUMPMENTION-26JUL24-M{i}" for i in range(3)],
        "as_of_ts": [5000.0, 5000.0, 5000.0],
    })
    assert list(_listing_load(preds)) == [3.0, 3.0, 3.0]


def test_listing_load_is_per_event_day():
    preds = pd.DataFrame({
        "ticker": ["KXTRUMPMENTION-26JUL24-A", "KXTRUMPMENTION-26JUL24-B",
                   "KXTRUMPMENTION-26JUL25-C"],
        "as_of_ts": [1000.0, 2000.0, 3000.0],
    })
    assert list(_listing_load(preds)) == [1.0, 2.0, 1.0]


def test_listing_load_unaffected_by_later_appended_markets():
    """Appending markets that open LATER must not change earlier rows."""
    base = pd.DataFrame({
        "ticker": ["KXTRUMPMENTION-26JUL24-A", "KXTRUMPMENTION-26JUL24-B"],
        "as_of_ts": [1000.0, 2000.0]})
    before = _listing_load(base)
    grown = pd.concat([base, pd.DataFrame({
        "ticker": ["KXTRUMPMENTION-26JUL24-C", "KXTRUMPMENTION-26JUL24-D"],
        "as_of_ts": [9000.0, 9500.0]})], ignore_index=True)
    after = _listing_load(grown)
    assert list(after[:2]) == list(before)


# ---------------------------------------------------------------------------
# T5 — encompassing: synthetic worlds with known answers
# ---------------------------------------------------------------------------

def _synth(n_days=60, per_day=25, signal=0.0, seed=0):
    """Build a world where truth = market belief + `signal` * private info.

    The model observes the private component; the market does not. signal=0
    means the model is the market plus noise and carries nothing extra.
    """
    rng = np.random.default_rng(seed)
    days, mids, models, ys = [], [], [], []
    for d in range(n_days):
        for _ in range(per_day):
            m_logit = rng.normal(0, 1.0)              # market's belief
            private = rng.normal(0, 1.0)              # extra information
            true_logit = m_logit + signal * private
            p = 1 / (1 + np.exp(-true_logit))
            mid = 1 / (1 + np.exp(-m_logit))
            # model sees the market plus (when signal>0) the private part
            model_logit = m_logit + (private if signal > 0 else rng.normal(0, 1.0))
            days.append(f"D{d:03d}")
            mids.append(np.clip(mid, 1e-3, 1 - 1e-3))
            models.append(1 / (1 + np.exp(-model_logit)))
            ys.append(rng.random() < p)
    return (np.array(ys, dtype=float), np.array(mids),
            np.array(models), np.array(days))


def test_encompass_detects_real_added_information():
    """Acceptance: a model with genuine private signal must show beta_A > 0."""
    from kalshi_mention.encompass import fit_encompassing
    y, mid, pm, days = _synth(signal=1.0, seed=1)
    r = fit_encompassing(y, mid, pm, days, B=400)
    assert r["beta_A"] > 0
    assert r["beta_A_lo"] > 0, f"CI {r['beta_A_lo']:+.3f}..{r['beta_A_hi']:+.3f}"
    assert r["insample_ll_m1"] < r["insample_ll_m0"]


def test_encompass_null_when_model_is_market_plus_noise():
    """Acceptance: a model carrying nothing extra must NOT show beta_A > 0."""
    from kalshi_mention.encompass import fit_encompassing
    y, mid, pm, days = _synth(signal=0.0, seed=2)
    r = fit_encompassing(y, mid, pm, days, B=400)
    assert r["beta_A_lo"] <= 0 <= r["beta_A_hi"], \
        f"false positive: CI {r['beta_A_lo']:+.3f}..{r['beta_A_hi']:+.3f}"


def test_encompass_walk_forward_is_out_of_sample():
    """No fit may use a market that had not resolved at the decision time."""
    from kalshi_mention.encompass import walk_forward_encompass
    y, mid, pm, _days = _synth(n_days=40, per_day=20, signal=1.0, seed=3)
    n = len(y)
    decision = np.arange(n, dtype=float) * 100.0
    resolve = decision + 50.0                      # each resolves before the next
    p0, p1, mask = walk_forward_encompass(y, mid, pm, decision, resolve,
                                          min_fit=100, refit_every=50)
    assert mask.sum() > 0
    # the first min_fit rows cannot be predicted: nothing had resolved yet
    assert not mask[:100].any()
    assert np.all((p0[mask] > 0) & (p0[mask] < 1))


def test_encompass_shared_clipping_rule():
    from kalshi_mention.encompass import CLIP, clip
    assert CLIP == 1e-4
    assert clip(np.array([0.0, 1.0])).tolist() == [CLIP, 1 - CLIP]


def test_pick_frozen_a_column_uses_selection_period_only():
    """The frozen column must be chosen without consulting the holdout."""
    from kalshi_mention.encompass import pick_frozen_a_column
    n = 100
    y = np.array([1.0] * 60 + [0.0] * 40)
    df = pd.DataFrame({
        "y": y,
        # good on the first 60% (the selection period), terrible after
        "empirical_hl30_cal": np.where(np.arange(n) < 60, 0.9, 0.9),
        # bad on selection, perfect after -- must NOT be chosen
        "poisson_hl90_cal": np.where(np.arange(n) < 60, 0.1, 0.02),
    })
    assert pick_frozen_a_column(df) == "empirical_hl30_cal"


# ---------------------------------------------------------------------------
# T7 — review tooling round-trips
# ---------------------------------------------------------------------------

def _review_db(tmp_path):
    """Synthetic db with labels spanning both eras and all agreement cells."""
    conn = kdb.connect(str(tmp_path / "r.sqlite"))
    rows = [("26JUL01", "live", "yes", 1), ("26JUL02", "live", "no", 0),
            ("26JUL03", "live", "yes", 0), ("26JUL04", "live", "no", 1),
            ("25NOV01", "hist", "yes", 1), ("25NOV02", "hist", "yes", 0),
            ("25NOV03", "hist", "no", 1), ("25NOV04", "hist", "no", 0)]
    for i, (tick, _era, result, matched) in enumerate(rows):
        yy = "20" + tick[:2]
        mon = {"JUL": "07", "NOV": "11"}[tick[2:5]]
        date = f"{yy}-{mon}-{tick[5:7]}"
        doc = f"doc{i}"
        conn.execute(
            "INSERT INTO transcripts (doc_id,title,date,speech_type,text_raw,"
            "token_count,available_at,availability_source) VALUES (?,?,?,?,?,?,?,?)",
            (doc, f"Remarks Donald Trump Holds a Meeting - {date}", date, "Remarks",
             "china " * 50, 50, date + "T23:00:00+00:00", "t"))
        kdb.upsert_market(conn, {
            "ticker": f"KXTRUMPMENTION-{tick}-CHIN",
            "event_ticker": f"KXTRUMPMENTION-{tick}",
            "series_ticker": "KXTRUMPMENTION",
            "title": "What will Trump say during his speech?",
            "yes_sub_title": "China", "result": result, "status": "settled",
            "open_time": date + "T12:00:00Z", "close_time": date + "T18:00:00Z",
            "expiration_time": date + "T18:00:00Z", "rules_primary": ""})
        conn.execute(
            "INSERT INTO labels (market_ticker,doc_id,phrase,phrase_source,matched,"
            "match_count,match_flags,computed_at) VALUES (?,?,?,?,?,?,?,?)",
            (f"KXTRUMPMENTION-{tick}-CHIN", doc, "China", "yes_sub_title",
             matched, matched, "{}", "2026-01-01T00:00:00+00:00"))
        conn.execute(
            "INSERT OR REPLACE INTO event_doc_map (event_ticker,doc_id,method,"
            "score,computed_at) VALUES (?,?,?,?,?)",
            (f"KXTRUMPMENTION-{tick}", doc, "only_doc", 1.0,
             "2026-01-01T00:00:00+00:00"))
    conn.commit()
    return conn, str(tmp_path / "r.sqlite")


def test_review_export_import_roundtrip(tmp_path):
    """T7a acceptance: export -> label -> import must round-trip."""
    from kalshi_mention.review import ERROR_KINDS, export_sample, import_review
    _conn, dbp = _review_db(tmp_path)
    out = tmp_path / "sample.csv"
    sample = export_sample(dbp, str(out), n=8)
    assert len(sample) > 0
    assert "review_error_kind" in sample.columns
    assert set(sample["cell"]) <= {"agree", "kalshi_yes_transcript_no",
                                   "kalshi_no_transcript_yes"}
    # a human fills the column
    df = pd.read_csv(out)
    df["review_error_kind"] = [ERROR_KINDS[i % len(ERROR_KINDS)] for i in range(len(df))]
    reviewed = tmp_path / "reviewed.csv"
    df.to_csv(reviewed, index=False)
    lines = import_review(str(reviewed), str(tmp_path / "outdir"))
    body = "\n".join(lines)
    assert "error decomposition" in body.lower()
    assert (tmp_path / "outdir" / "review_decomposition.md").exists()


def test_review_import_handles_unlabelled_file(tmp_path):
    """An untouched export must not crash the importer."""
    from kalshi_mention.review import export_sample, import_review
    _conn, dbp = _review_db(tmp_path)
    out = tmp_path / "s2.csv"
    export_sample(dbp, str(out), n=8)
    lines = import_review(str(out), str(tmp_path / "o2"))
    assert "nothing to decompose" in "\n".join(lines).lower()


def test_taxonomy_export_is_blind_and_roundtrips(tmp_path):
    """T7b acceptance: classifier labels withheld, key separate, import works."""
    from kalshi_mention.review import export_taxonomy, import_taxonomy
    _conn, dbp = _review_db(tmp_path)
    out = tmp_path / "tax.csv"
    blind = export_taxonomy(dbp, str(out), n=8)
    assert "_market_type" not in blind.columns and "_doc_type" not in blind.columns
    key = tmp_path / "tax_key.csv"
    assert key.exists()
    assert {"_market_type", "_doc_type"} <= set(pd.read_csv(key).columns)

    df = pd.read_csv(out)
    df["review_event_type"] = "meeting"
    df.to_csv(out, index=False)
    lines = import_taxonomy(str(out), str(tmp_path / "o3"))
    body = "\n".join(lines)
    assert "classify_market" in body and "classify_doc" in body
    assert "agree with EACH OTHER" in body


def test_stratified_sample_keeps_rare_strata():
    """Rare cells are the interesting ones and must not be sampled away."""
    from kalshi_mention.review import stratified_sample
    df = pd.DataFrame({"cell": ["agree"] * 200 + ["rare"] * 2,
                       "v": range(202)})
    s = stratified_sample(df, 20, ["cell"])
    assert (s["cell"] == "rare").any()


# ---------------------------------------------------------------------------
# T8(f) — three permanent review diagnostics
# ---------------------------------------------------------------------------

def _tiny_corpus_db(tmp_path, name="inv.sqlite", n_days=60):
    """Small but complete db: transcripts, markets, counts."""
    import pandas as pd
    from kalshi_mention.counts import build_counts
    dbp = str(tmp_path / name)
    conn = kdb.connect(dbp)
    MON = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN",
           "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"]
    start = pd.Timestamp("2026-01-01")
    for i in range(n_days):
        d = start + pd.Timedelta(days=i)
        ds = d.date().isoformat()
        said = (i % 3) != 0
        conn.execute(
            "INSERT INTO transcripts (doc_id,title,date,speech_type,text_raw,"
            "token_count,available_at,availability_source) VALUES (?,?,?,?,?,?,?,?)",
            (f"d{i}", f"Speech Donald Trump Holds a Rally - {ds}", ds, "Speech",
             ("china " * 40) if said else ("economy " * 40), 40,
             ds + "T23:00:00+00:00", "t"))
        tick = f"{d.strftime('%y')}{MON[d.month-1]}{d.day:02d}"
        kdb.upsert_market(conn, {
            "ticker": f"KXTRUMPMENTION-{tick}-CHIN",
            "event_ticker": f"KXTRUMPMENTION-{tick}",
            "series_ticker": "KXTRUMPMENTION",
            "title": "What will Trump say during his speech?",
            "yes_sub_title": "China", "result": "yes" if said else "no",
            "status": "settled", "open_time": ds + "T12:00:00Z",
            "close_time": ds + "T20:00:00Z", "expiration_time": ds + "T20:00:00Z",
            "rules_primary": ""})
    conn.commit()
    build_counts(dbp, None)
    return conn, dbp


def test_predictions_invariant_to_future_dated_records(tmp_path):
    """Diagnostic 1: appending FUTURE records must not move past predictions.

    The strongest available check that point-in-time discipline holds end to
    end. If any feature or fit reached forward in time, adding documents and
    markets dated after every existing market would perturb their forecasts.
    """
    import pandas as pd
    from kalshi_mention.component_a import run as a_run
    conn, dbp = _tiny_corpus_db(tmp_path)
    out1 = tmp_path / "a1"
    a_run(dbp, str(out1))
    before = pd.read_csv(out1 / "component_a_predictions.csv").set_index("ticker")

    # append documents and markets dated well AFTER everything above
    MON = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN",
           "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"]
    future = pd.Timestamp("2027-06-01")
    for i in range(15):
        d = future + pd.Timedelta(days=i)
        ds = d.date().isoformat()
        conn.execute(
            "INSERT INTO transcripts (doc_id,title,date,speech_type,text_raw,"
            "token_count,available_at,availability_source) VALUES (?,?,?,?,?,?,?,?)",
            (f"future{i}", f"Speech Donald Trump Holds a Rally - {ds}", ds,
             "Speech", "china " * 200, 200, ds + "T23:00:00+00:00", "t"))
        tick = f"{d.strftime('%y')}{MON[d.month-1]}{d.day:02d}"
        kdb.upsert_market(conn, {
            "ticker": f"KXTRUMPMENTION-{tick}-CHIN",
            "event_ticker": f"KXTRUMPMENTION-{tick}",
            "series_ticker": "KXTRUMPMENTION",
            "title": "What will Trump say during his speech?",
            "yes_sub_title": "China", "result": "yes", "status": "settled",
            "open_time": ds + "T12:00:00Z", "close_time": ds + "T20:00:00Z",
            "expiration_time": ds + "T20:00:00Z", "rules_primary": ""})
    conn.commit()
    from kalshi_mention.counts import build_counts
    build_counts(dbp, None)

    out2 = tmp_path / "a2"
    a_run(dbp, str(out2))
    after = pd.read_csv(out2 / "component_a_predictions.csv").set_index("ticker")

    shared = before.index.intersection(after.index)
    assert len(shared) >= 50
    # RAW model columns must be bit-identical: they depend only on the past.
    raw_cols = [c for c in before.columns
                if any(c.startswith(p) for p in ("empirical", "poisson", "negbin"))
                and not c.endswith("_cal")]
    assert raw_cols
    for c in raw_cols:
        delta = (before.loc[shared, c] - after.loc[shared, c]).abs().max()
        assert delta < 1e-12, f"{c} moved by {delta} when future records were added"


def test_holdout_untouched_by_calibration_fit():
    """Diagnostic 3 (canary): later outcomes must not alter earlier forecasts.

    walk_forward_calibrate may only use markets that RESOLVED before each
    decision time. Flipping every outcome that resolves later is the canary:
    if any of them leaked into an earlier fit, that earlier prediction moves.
    """
    from kalshi_mention.component_a import walk_forward_calibrate
    rng = np.random.default_rng(0)
    n = 600
    as_of = np.arange(n, dtype=float) * 1000.0
    close = as_of + 500.0
    p_raw = np.clip(rng.beta(2, 2, n), 1e-6, 1 - 1e-6)
    y = (rng.random(n) < p_raw).astype(float)

    out_a = walk_forward_calibrate(p_raw.copy(), y.copy(), as_of, close)
    y_flipped = y.copy()
    cut = 400
    y_flipped[cut:] = 1.0 - y_flipped[cut:]        # corrupt only the future
    out_b = walk_forward_calibrate(p_raw.copy(), y_flipped, as_of, close)

    # everything decided before the corrupted region must be untouched
    assert np.abs(out_a[:cut] - out_b[:cut]).max() < 1e-12
    # and the corruption must actually be capable of moving something,
    # otherwise the test would pass vacuously
    assert np.abs(out_a[cut:] - out_b[cut:]).max() > 0


def test_k_correction_is_point_in_time():
    """T8(c): the pooled threshold offset may only use resolved markets."""
    from kalshi_mention.unified import k_correction
    n = 200
    as_of = np.arange(n, dtype=float) * 100.0
    close = as_of + 50.0
    k = np.where(np.arange(n) % 10 == 0, 3.0, 1.0)
    y = np.zeros(n)
    y[k > 1] = 1.0                       # every threshold market resolves YES
    corr = k_correction(k, y, as_of, close)
    assert corr[0] == 0.0                # nothing resolved yet -> no offset
    later = corr[k > 1][-1]
    assert later > 0                     # evidence accumulates
    # flipping outcomes that resolve later must not move an earlier value
    y2 = y.copy()
    y2[150:] = 1.0 - y2[150:]
    corr2 = k_correction(k, y2, as_of, close)
    assert np.abs(corr[:150] - corr2[:150]).max() < 1e-12
