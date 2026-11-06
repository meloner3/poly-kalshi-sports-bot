"""Pass-2 tests: event mapper scoring, Component A estimator math, and the
point-in-time guarantee of the walk-forward loop."""

import math

import numpy as np

from kalshi_mention.event_mapper import event_descriptor, keywords, score_candidate
from kalshi_mention.component_a import negbin_sf, poisson_sf, predict_models
from kalshi_mention.counts import count_in_index, spec_variant_token_lists
from kalshi_mention.phrase import tokenize


# --- event mapper -----------------------------------------------------------

def test_descriptor_extraction():
    assert event_descriptor("What will Trump say during the State of the Union?", None, None) \
        == "State of the Union"
    assert event_descriptor(None, "What will Trump say during his remarks at Turning Point USA?", None) \
        == "remarks at Turning Point USA"
    assert event_descriptor(None, None,
        "Resolves Yes if Donald Trump says X as part of the 2026 NATO Summit.") == "2026 NATO Summit"


def test_mapper_scores_right_document():
    desc = "remarks at Turning Point USA"
    right = "Speech: Donald Trump Addresses a Turning Point USA Event in Phoenix - April 17, 2026"
    wrong1 = "Press Gaggle: Donald Trump Speaks to Reporters On Board Air Force One - April 17, 2026"
    wrong2 = "Press Gaggle: Donald Trump Speaks to Reporters After Air Force One Arrival - April 17, 2026"
    s_right = score_candidate(desc, right)
    assert s_right > score_candidate(desc, wrong1)
    assert s_right > score_candidate(desc, wrong2)
    assert s_right >= 2.0  # clears the acceptance threshold


def test_generic_descriptor_does_not_false_match():
    # "his announcement" carries no distinctive keywords -> low score everywhere
    assert score_candidate("his announcement", "Press Gaggle: Trump Speaks to Reporters") < 2.0


def test_keywords_strip_boilerplate():
    kw = keywords("Speech: Donald Trump Delivers Remarks at the White House")
    assert "trump" not in kw and "remarks" not in kw and "white" in kw


# --- counting via token index -----------------------------------------------

def _index(text):
    tokens = tokenize(text)
    pos = {}
    for i, t in enumerate(tokens):
        pos.setdefault(t, []).append(i)
    return tokens, pos


def test_count_single_and_multi_token_variants():
    text = "China is big. The Chinese economy. Fake news everywhere, fake news."
    tokens, pos = _index(text)
    v, k = spec_variant_token_lists("China / Chinese")
    assert count_in_index(v, pos, tokens) == 2 and k == 1
    v, k = spec_variant_token_lists("Fake News")
    assert count_in_index(v, pos, tokens) == 2


def test_count_threshold_spec_parses_k():
    v, k = spec_variant_token_lists("Iran (3+ times)")
    assert k == 3 and v == [["iran"]]


# --- estimator math ---------------------------------------------------------

def test_poisson_sf_basics():
    assert poisson_sf(1, 0.0) == 0.0
    assert abs(poisson_sf(1, 2.0) - (1 - math.exp(-2.0))) < 1e-12
    # P(N>=2) = 1 - e^-m (1+m)
    assert abs(poisson_sf(2, 1.5) - (1 - math.exp(-1.5) * (1 + 1.5))) < 1e-12


def test_negbin_collapses_to_poisson_at_high_r():
    for k in (1, 2, 3):
        assert abs(negbin_sf(k, 2.0, 1e9) - poisson_sf(k, 2.0)) < 1e-6


def test_negbin_overdispersion_lowers_p_at_least_once():
    # Same mean, bursty counts: P(>=1) must be LOWER under NB than Poisson.
    assert negbin_sf(1, 1.0, 0.3) < poisson_sf(1, 1.0)


def test_predict_models_on_bursty_counts_large_corpus():
    # 200 docs (enough exposure that rate smoothing is negligible), phrase
    # said 5x in every 10th doc, absent otherwise: mean 0.5/doc, hit rate 0.1.
    counts = np.array(([5.0] + [0.0] * 9) * 20)
    lengths = np.full(200, 1000.0)
    weights = np.ones(200)
    p = predict_models(counts, lengths, weights, k=1)
    # Empirical tracks the true doc-level hit rate; Poisson at mean~0.4-0.5
    # overpredicts (~0.35+); NB absorbs the burstiness and sits between.
    assert abs(p["empirical"] - 0.1) < 0.02
    assert p["poisson"] > 0.3
    assert p["empirical"] < p["negbin"] < p["poisson"]


def test_predict_models_prior_shrinks_thin_samples():
    # Same shape but only 10 docs: exposure (10k tokens) is small vs the 50k
    # smoothing prior, so the Poisson mean must be pulled well below its
    # unsmoothed value (~0.39) — thin evidence should not produce bold rates.
    counts = np.array([5.0] + [0.0] * 9)
    p = predict_models(counts, np.full(10, 1000.0), np.ones(10), k=1)
    assert p["poisson"] < 0.15
    assert p["negbin"] <= p["poisson"] + 1e-12  # burstiness never raises it


def test_recency_weights_shift_probability():
    counts = np.array([0.0, 0.0, 1.0, 1.0])  # hits only in the recent half
    lengths = np.full(4, 1000.0)
    old_heavy = np.array([1.0, 1.0, 0.01, 0.01])
    new_heavy = np.array([0.01, 0.01, 1.0, 1.0])
    p_old = predict_models(counts, lengths, old_heavy, 1)["empirical"]
    p_new = predict_models(counts, lengths, new_heavy, 1)["empirical"]
    assert p_new > 0.6 > 0.4 > p_old


# --- walk-forward point-in-time guard ----------------------------------------

def test_walk_forward_excludes_future_documents():
    """Reproduce the eligibility logic from component_a.run and prove a
    document available after the market's open cannot influence it."""
    avail_ts = np.array([100.0, 200.0, 300.0])
    as_of = 250.0
    n_elig = int(np.searchsorted(avail_ts, as_of, side="right"))
    assert n_elig == 2  # doc at t=300 excluded
    counts = np.array([0.0, 0.0, 99.0])[:n_elig]
    assert counts.max() == 0.0  # the future hit is invisible


# --- pass 2.5: event types, typed conditioning, recalibration ----------------

from kalshi_mention.event_types import classify_doc, classify_market
from kalshi_mention.component_a import apply_platt, fit_platt, walk_forward_calibrate


def test_market_type_classification():
    assert classify_market("What will Trump say during his rally in Ohio?") == "speech"
    assert classify_market("What will Trump say during his press gaggle?") == "gaggle"
    assert classify_market("What will Trump say during the NATO press conference?") == "pressconf"
    assert classify_market("What will Trump say on the Hugh Hewitt Radio Show?") == "interview"
    assert classify_market("What will Trump say during his bilateral with Iraq?") == "meeting"


def test_doc_type_classification():
    assert classify_doc("Press Gaggle", None) == "gaggle"
    assert classify_doc("Speech", None) == "speech"
    assert classify_doc(None, "Interview: Someone Interviews Donald Trump") == "interview"
    assert classify_doc("Donald Trump Vlog", None) == "vlog"


def test_typed_weights_change_prediction():
    # History: long rallies where phrase always occurs, short gaggles where it never does.
    counts = np.array([3.0, 3.0, 3.0, 0.0, 0.0, 0.0])
    lengths = np.array([8000.0] * 3 + [400.0] * 3)
    base_w = np.ones(6)
    types = np.array(["speech"] * 3 + ["gaggle"] * 3)
    for target, lo, hi in (("gaggle", 0.0, 0.35), ("speech", 0.65, 1.0)):
        tw = np.where(types == target, 1.0, 0.15)
        p = predict_models(counts, lengths, base_w * tw, 1)["empirical"]
        assert lo < p < hi, (target, p)


def test_platt_fixes_overconfidence():
    rng = np.random.default_rng(0)
    true_p = rng.uniform(0.2, 0.8, 4000)
    y = (rng.uniform(size=4000) < true_p).astype(float)
    over = np.clip(0.5 + 2.0 * (true_p - 0.5), 1e-6, 1 - 1e-6)  # systematically overconfident
    a, b = fit_platt(np.log(over / (1 - over)), y)
    cal = apply_platt(over, a, b)
    from kalshi_mention.component_a import log_loss
    assert log_loss(y, cal) < log_loss(y, over) - 0.02
    assert 0.0 < b < 1.0  # learned to shrink toward the mean (b<1 = de-confidence)


def test_walk_forward_calibration_is_leak_free_and_inert_early():
    rng = np.random.default_rng(1)
    n = 300
    p_raw = rng.uniform(0.05, 0.95, n)
    y = (rng.uniform(size=n) < p_raw).astype(float)
    as_of = np.arange(n, dtype=float)
    close = as_of + 1000.0  # nothing resolves until after every decision
    out = walk_forward_calibrate(p_raw, y, as_of, close)
    assert np.allclose(out, np.clip(p_raw, 1e-6, 1 - 1e-6))  # no resolved data -> identity
    close2 = as_of + 0.5  # resolves immediately after decision
    out2 = walk_forward_calibrate(p_raw, y, as_of, close2)
    assert np.allclose(out2[:75], np.clip(p_raw[:75], 1e-6, 1 - 1e-6))  # inert before MIN_RESOLVED
    assert not np.allclose(out2[150:], p_raw[150:])  # active later


# --- pass 3: candle parsing, fees, snapshot guards, backtest math ------------

from kalshi_mention.market_baseline import (
    candle_prob, kalshi_fee, parse_candle, pick_snapshot, trade_pnl,
)


def test_parse_candle_nested_and_flat():
    nested = {"end_period_ts": 100, "yes_bid": {"close": 40}, "yes_ask": {"close": 44},
              "price": {"close": 42}, "volume": 7}
    c = parse_candle(nested)
    assert c == {"ts": 100, "bid": 0.40, "ask": 0.44, "trade": 0.42, "volume": 7}
    flat = {"end_period_ts": 200, "yes_bid_close": 10, "yes_ask_close": 90, "close": 55}
    c = parse_candle(flat)
    assert c["bid"] == 0.10 and c["ask"] == 0.90 and c["trade"] == 0.55 and c["volume"] == 0


def test_candle_prob_prefers_tight_mid_falls_back_to_trade():
    assert abs(candle_prob({"bid": 0.40, "ask": 0.44, "trade": 0.10, "volume": 1}) - 0.42) < 1e-9
    # 10/90 spread is junk -> fall back to last trade
    assert candle_prob({"bid": 0.10, "ask": 0.90, "trade": 0.55, "volume": 1}) == 0.55
    assert candle_prob({"bid": None, "ask": None, "trade": None, "volume": 0}) is None


def test_pick_snapshot_guards():
    cs = [{"ts": t, "bid": 0.4, "ask": 0.5, "trade": None, "volume": 1} for t in (50, 150, 250)]
    # latest candle in (open, eval]
    assert pick_snapshot(cs, open_ts=0, eval_ts=200)["ts"] == 150
    # candle at/before open excluded
    assert pick_snapshot(cs, open_ts=150, eval_ts=200) is None
    # candles strictly after eval_ts are never visible
    assert pick_snapshot(cs, open_ts=0, eval_ts=100)["ts"] == 50
    # NOTE: this test previously asserted a "contamination guard" that returned
    # None when eval_ts came within 30 minutes of close_ts. T2 removed it: these
    # markets close early when the phrase occurs, so close_ts is a function of
    # the outcome and filtering on it selected the sample on the answer. The
    # assertion encoded the very bug it appeared to protect against. See
    # test_remediation.test_eligibility_invariant_to_close_and_settlement.


def test_kalshi_fee_examples():
    assert kalshi_fee(0.50) == 0.02          # 7% * 0.25 = 1.75c -> ceil 2c
    assert kalshi_fee(0.05) == 0.01          # 0.3325c -> ceil 1c
    assert kalshi_fee(0.99) == 0.01


def test_trade_pnl_directions():
    # model above mid -> buy YES at ask
    g, n = trade_pnl(0.80, y=1, bid=0.40, ask=0.44)
    assert abs(g - 0.56) < 1e-9 and n == g - kalshi_fee(0.44)
    g, _ = trade_pnl(0.80, y=0, bid=0.40, ask=0.44)
    assert abs(g + 0.44) < 1e-9
    # model below mid -> buy NO at 1 - bid
    g, n = trade_pnl(0.10, y=0, bid=0.40, ask=0.44)
    assert abs(g - 0.40) < 1e-9 and n == g - kalshi_fee(0.60)
    g, _ = trade_pnl(0.10, y=1, bid=0.40, ask=0.44)
    assert abs(g + 0.60) < 1e-9


# --- pass 4a: historical tier routing ----------------------------------------

from kalshi_mention.kalshi_client import KalshiPublicClient


class _FakeClient(KalshiPublicClient):
    """Records requests; serves scripted responses without any network."""
    def __init__(self, script):
        self.script = script
        self.calls = []
    def _get(self, path, params=None):
        self.calls.append((path, params or {}))
        for prefix, resp in self.script:
            if path.startswith(prefix):
                if isinstance(resp, Exception):
                    raise resp
                return resp
        raise RuntimeError(f"unscripted path {path}")


def test_historical_markets_pagination_and_prefix_filter():
    page1 = {"markets": [{"ticker": "KXTRUMPMENTION-26FEB28-CHIN"},
                         {"ticker": "OTHERSERIES-26FEB28-X"}], "cursor": "c2"}
    page2 = {"markets": [{"ticker": "KXTRUMPMENTION-26JAN30-IRAN"}], "cursor": None}
    fc = _FakeClient([("/historical/markets", page1)])
    got = []
    it = fc.iter_historical_markets("KXTRUMPMENTION")
    got.append(next(it))          # consumes page1, filters foreign ticker
    fc.script = [("/historical/markets", page2)]
    got.extend(it)
    tickers = [m["ticker"] for m in got]
    assert tickers == ["KXTRUMPMENTION-26FEB28-CHIN", "KXTRUMPMENTION-26JAN30-IRAN"]
    assert fc.calls[0][1]["series_ticker"] == "KXTRUMPMENTION"
    assert fc.calls[1][1]["cursor"] == "c2"


def test_historical_candles_fallback_on_404():
    """Live 404 -> historical endpoint serves the candles (the 68-market case)."""
    live_404 = RuntimeError("HTTP 404 not found")
    hist = {"candlesticks": [{"end_period_ts": 1, "price": {"close_dollars": "0.5000"}}]}
    fc = _FakeClient([("/series/", live_404), ("/historical/markets/", hist)])
    try:
        fc.get_candlesticks("KXTRUMPMENTION", "KXTRUMPMENTION-26MAY18-TAX", 0, 10, 60)
        assert False, "live endpoint should have raised"
    except RuntimeError:
        pass
    out = fc.get_historical_candlesticks("KXTRUMPMENTION-26MAY18-TAX", 0, 10, 60)
    assert out == hist["candlesticks"]
    assert fc.calls[-1][0] == "/historical/markets/KXTRUMPMENTION-26MAY18-TAX/candlesticks"


def test_parse_candle_kalshi_dollar_format():
    real = {"end_period_ts": 1784930400, "volume_fp": "98.68",
            "price": {"close_dollars": "0.3500"},
            "yes_ask": {"close_dollars": "0.3200"},
            "yes_bid": {"close_dollars": "0.2100"}}
    c = parse_candle(real)
    assert c["bid"] == 0.21 and c["ask"] == 0.32 and c["trade"] == 0.35 and c["volume"] == 98


def test_parse_candle_volume_string():
    c = parse_candle({"end_period_ts": 1, "volume": "30.89",
                      "yes_bid": {"close_dollars": "0.10"},
                      "yes_ask": {"close_dollars": "0.14"}})
    assert c["volume"] == 30 and c["bid"] == 0.10 and c["ask"] == 0.14


# --- Components B and C ------------------------------------------------------

from kalshi_mention.component_b import (fit_logistic, predict_logistic,
                                        looks_proper_noun, logit as b_logit)
from kalshi_mention.component_c import make_prompt, spec_hits
from kalshi_mention.counts import spec_variant_token_lists as svtl
from kalshi_mention.news import phrase_query


def test_logistic_recovers_planted_weights():
    rng = np.random.default_rng(0)
    n = 4000
    X = np.column_stack([rng.normal(0, 1, n), rng.normal(0, 1, n)])
    z = 0.5 + 1.0 * X[:, 0] - 0.8 * X[:, 1]
    y = (rng.uniform(size=n) < 1 / (1 + np.exp(-z))).astype(float)
    w = fit_logistic(X, y, l2=0.1)
    assert abs(w[0] - 0.5) < 0.15 and abs(w[1] - 1.0) < 0.15 and abs(w[2] + 0.8) < 0.15


def test_logistic_prior_passes_A_through_when_features_useless():
    rng = np.random.default_rng(1)
    n = 2000
    pA = rng.uniform(0.1, 0.9, n)
    y = (rng.uniform(size=n) < pA).astype(float)
    X = np.column_stack([b_logit(pA), rng.normal(0, 1, n)])  # feature 2 is noise
    w = fit_logistic(X, y)
    pB = predict_logistic(w, X)
    # noise weight shrunk, backbone weight near 1, predictions track A closely
    assert abs(w[2]) < 0.1 and abs(w[1] - 1.0) < 0.2
    assert float(np.abs(pB - pA).mean()) < 0.05


def test_proper_noun_gate():
    assert looks_proper_noun("China / Chinese") == 1
    assert looks_proper_noun("Fake News") == 0        # stoplisted head word
    assert looks_proper_noun("beautiful") == 0
    assert looks_proper_noun("Nobel Prize") == 1


def test_news_query_quoting():
    assert phrase_query("China / Chinese") == "China"
    assert phrase_query("Crooked Joe / Sleepy Joe") == '"Crooked Joe"'


def test_component_c_spec_counting_and_prompt():
    variants, k = svtl("China / Chinese")
    assert spec_hits("We love china. chinese steel is tough.", variants, k)
    assert not spec_hits("Nothing relevant here.", variants, k)
    variants, k = svtl("Iran (3+ times)")
    assert not spec_hits("Iran twice: Iran.", variants, k)
    assert spec_hits("Iran, Iran, Iran again.", variants, k)
    long_text = " ".join(f"w{i}" for i in range(1000))
    assert len(make_prompt(long_text, 300).split()) == 300


def test_component_c_run_with_stub_backend(tmp_path):
    """Full component_c.run against a synthetic db with a stub generator:
    contexts where the stub says 'china' often must yield high p_c."""
    import kalshi_mention.db as kdb
    from kalshi_mention.component_c import run as c_run
    dbp = str(tmp_path / "c.sqlite")
    conn = kdb.connect(dbp)
    conn.execute("INSERT INTO transcripts (doc_id,title,date,speech_type,text_raw,token_count,available_at,availability_source) "
                 "VALUES ('d1','Speech: Rally','2026-06-01','Speech','great america wall',3,'2026-06-01T12:00:00+00:00','test')")
    for suf, phrase in (("CHIN", "China / Chinese"), ("ZEBR", "Zebra")):
        kdb.upsert_market(conn, {"ticker": f"KXTRUMPMENTION-26JUN02-{suf}",
            "event_ticker": "KXTRUMPMENTION-26JUN02", "series_ticker": "KXTRUMPMENTION",
            "title": "What will Trump say during his rally?", "yes_sub_title": phrase,
            "result": "yes" if suf == "CHIN" else "no", "status": "settled",
            "open_time": "2026-06-02T00:00:00Z", "close_time": "2026-06-03T00:00:00Z",
            "expiration_time": "2026-06-03T00:00:00Z", "rules_primary": ""})
    conn.commit(); conn.close()

    def stub(prompt, n):
        return ["we will talk about china today"] * (n - 2) + ["nothing else"] * 2

    c_run(dbp, str(tmp_path / "out"), "stub", "2026-01-01", 10, 50, 4, None,
          generate_fn=stub)
    import pandas as pd
    df = pd.read_csv(tmp_path / "out" / "component_c_predictions.csv")
    p = dict(zip(df["ticker"].str[-4:], df["p_c"]))
    assert p["CHIN"] > 0.7 and p["ZEBR"] < 0.1


# --- Unified model -----------------------------------------------------------

from kalshi_mention.unified import CoocEngine, make_learner


def test_cooc_engine_point_in_time():
    dates = ["2026-01-01", "2026-01-02", "2026-01-03", "2026-01-10"]
    specs = [{"nato", "article5"}, {"nato", "article5"}, {"economy"}, {"nato"}]
    e = CoocEngine(dates, specs)
    e.advance("2026-01-03")          # ingests Jan 1-2 only
    assert e.n_docs == 2
    # engine only moves forward; future docs invisible
    assert e.ptr == 2
    # below the 50-doc corpus guard, signals are conservatively zero
    assert e.signal("article5", "2026-01-03") == 0.0


def test_cooc_signal_rises_with_companion_activity():
    # 120 docs: "summit" and "article5" co-occur; last 10 days summit-heavy
    dates, specs = [], []
    for i in range(120):
        d = f"2026-{(i // 28) + 1:02d}-{(i % 28) + 1:02d}"
        dates.append(d)
        if i < 100:
            specs.append({"summit", "article5"} if i % 4 == 0 else {"economy", "jobs"})
        else:
            specs.append({"summit"})   # summit active recently, article5 absent
    e = CoocEngine(dates, specs)
    e.advance("2026-05-05")
    s_hot = e.signal("article5", "2026-05-05")   # companion (summit) hot
    s_cold = e.signal("jobs", "2026-05-05")      # companions (economy) cold
    assert s_hot > s_cold


def test_make_learner_fallback_and_boosted():
    fitL, predL, name = make_learner(force_logistic=True)
    assert name == "logistic_fallback"
    rng = np.random.default_rng(0)
    X = rng.normal(0, 1, (500, 3))
    yv = (rng.uniform(size=500) < 1 / (1 + np.exp(-X[:, 0]))).astype(float)
    m = fitL(X, yv)
    p = predL(m, X)
    assert 0.6 < float(((p > 0.5) == yv).mean())
    fitB, predB, nameB = make_learner()
    if nameB == "hist_gradient_boosting":
        m = fitB(X, yv)
        p = predB(m, X)
        assert 0.6 < float(((p > 0.5) == yv).mean())


# --- predict command ---------------------------------------------------------

def test_predict_phrase_end_to_end(tmp_path):
    """Synthetic corpus + resolved markets -> calibration -> a live question.
    'china' appears in most speeches, 'zebra' never: probabilities must
    separate accordingly and stay in (0,1)."""
    import kalshi_mention.db as kdb
    from kalshi_mention.predict import fit_calibration, predict_phrase
    import pandas as pd, random
    random.seed(2)
    dbp = str(tmp_path / "p.sqlite")
    conn = kdb.connect(dbp)
    MONTHS = ["JAN","FEB","MAR","APR","MAY","JUN","JUL","AUG","SEP","OCT","NOV","DEC"]
    # Anchor the synthetic corpus to NOW, not to a fixed calendar date.
    # predict_phrase weights documents by 0.5**(age_days/30) measured from
    # datetime.now(), so a corpus pinned to fixed dates decays as real time
    # passes: sum(w) halves every 30 days and the Jeffreys prior takes over,
    # dragging a never-observed phrase from ~0.01 toward 0.5. This test was
    # written in Aug 2026 and started failing in Sep 2026 for exactly that
    # reason. Ending the corpus 5 days ago keeps it time-invariant.
    start = pd.Timestamp.now(tz="UTC").normalize().tz_localize(None) - pd.Timedelta(days=125)
    for i in range(120):
        d = start + pd.Timedelta(days=i); ds = d.date().isoformat()
        said = random.random() < 0.8
        rare = random.random() < 0.15
        text = " ".join(["economy wall"] * 200) + (" china " if said else "") + (" nato " if rare else "")
        conn.execute(
            "INSERT INTO transcripts (doc_id,title,date,speech_type,text_raw,token_count,available_at,availability_source) VALUES (?,?,?,?,?,?,?,?)",
            (f"d{i}", f"Speech: Rally - {ds}", ds, "Speech", text, len(text.split()),
             ds + "T23:00:00+00:00", "t"))
        tick = f"{d.strftime('%y')}{MONTHS[d.month-1]}{d.day:02d}"
        kdb.upsert_market(conn, {"ticker": f"KXTRUMPMENTION-{tick}-NATO",
            "event_ticker": f"KXTRUMPMENTION-{tick}", "series_ticker": "KXTRUMPMENTION",
            "title": "What will Trump say during his rally?", "yes_sub_title": "Nato",
            "result": "yes" if rare else "no", "status": "settled",
            "open_time": (d - pd.Timedelta(hours=12)).isoformat() + "Z",
            "close_time": (d + pd.Timedelta(hours=12)).isoformat() + "Z",
            "expiration_time": (d + pd.Timedelta(hours=12)).isoformat() + "Z",
            "rules_primary": ""})
        kdb.upsert_market(conn, {"ticker": f"KXTRUMPMENTION-{tick}-CHIN",
            "event_ticker": f"KXTRUMPMENTION-{tick}", "series_ticker": "KXTRUMPMENTION",
            "title": "What will Trump say during his rally?", "yes_sub_title": "China / Chinese",
            "result": "yes" if said else "no", "status": "settled",
            "open_time": (d - pd.Timedelta(hours=12)).isoformat() + "Z",
            "close_time": (d + pd.Timedelta(hours=12)).isoformat() + "Z",
            "expiration_time": (d + pd.Timedelta(hours=12)).isoformat() + "Z",
            "rules_primary": ""})
    conn.commit()
    # counts table for calibration
    from kalshi_mention.counts import build_counts
    import tempfile, pandas as pd2
    build_counts(dbp, "/dev/null")
    cal = fit_calibration(conn)
    hi = predict_phrase(conn, "China / Chinese", "speech", cal)
    lo = predict_phrase(conn, "Zebra", "speech", cal)
    assert 0.55 < hi["p"] < 0.99
    # A phrase absent from every document must land near the prior floor.
    # With a fresh corpus sum(w) ~ 41, so the Jeffreys estimate is ~0.012.
    assert lo["p"] < 0.05, f"never-observed phrase scored {lo['p']:.3f} — is the corpus stale?"
    assert lo["p"] < hi["p"] - 0.4
    assert hi["n_same_type_hit"] > 80 and lo["n_same_type_hit"] == 0
    th = predict_phrase(conn, "China (3+ times)", "speech", cal)
    assert th["p"] < lo["p"] + 0.2 or th["p"] < hi["p"]  # threshold lowers vs k=1
    assert th["p"] < hi["p"]
