from __future__ import annotations

from datetime import datetime, timezone
from threading import Thread
from urllib.request import urlopen
from http.server import ThreadingHTTPServer
import json

import numpy as np

from kalshi_mention import config
from kalshi_mention.signal_service import (
    CalibrationFit,
    CorpusView,
    fit_clustered_calibration,
    signal_for_market,
)


def _calibration() -> CalibrationFit:
    return CalibrationFit(
        coefficients=(0.0, 1.0),
        bootstrap_coefficients=tuple((0.0, 1.0) for _ in range(50)),
        event_type_base_rates={"speech": 0.5},
        overall_base_rate=0.5,
        market_count=400,
        model_version="test-model-version",
    )


def _corpus(texts: list[str], now: datetime) -> CorpusView:
    docs = [{
        "doc_id": f"d{index}",
        "date": now.date().isoformat(),
        "available_at": now.isoformat(),
        "token_count": len(text.split()),
        "speech_type": "Speech",
        "title": "Speech",
        "text_raw": text,
    } for index, text in enumerate(texts)]
    return CorpusView(
        docs=docs,
        lengths=np.array([doc["token_count"] for doc in docs], dtype=float),
        event_ts=np.array([now.timestamp()] * len(docs)),
        types=np.array(["speech"] * len(docs)),
        latest_update=now.isoformat(),
        freshness_sec=0.0,
    )


def test_prepare_corpus_excludes_transcripts_not_yet_available():
    now = datetime.now(timezone.utc)
    corpus = _corpus(["available now", "future material"], now)
    docs = list(corpus.docs)
    docs[1]["available_at"] = (now.replace(year=now.year + 1)).isoformat()

    from kalshi_mention.signal_service import prepare_corpus
    filtered = prepare_corpus(docs, now)

    assert [doc["text_raw"] for doc in filtered.docs] == ["available now"]


def test_signal_contract_contains_required_fields_and_conservative_interval():
    now = datetime.now(timezone.utc)
    market = {
        "ticker": "KXTRUMPMENTION-26OCT01-CHINA",
        "event_ticker": "KXTRUMPMENTION-26OCT01",
        "title": "What will Trump say during his rally?",
        "yes_sub_title": "China",
    }
    corpus = _corpus(["China economy" if i % 2 else "economy" for i in range(40)], now)

    signal = signal_for_market(market, corpus, _calibration(), now)

    assert set((
        "market_ticker", "event_id", "phrase", "p_hat", "p_lo", "p_hi",
        "model_version", "features_as_of", "data_freshness_sec", "calibration_n", "status",
    )) <= signal.keys()
    assert signal["status"] == "ok"
    assert signal["model_version"] == "test-model-version"
    assert 0 <= signal["p_lo"] <= signal["p_hat"] <= signal["p_hi"] <= 1
    assert signal["calibration_n"] == 400


def test_signal_abstains_for_unseen_phrase_without_probabilities():
    now = datetime.now(timezone.utc)
    market = {
        "ticker": "KXTRUMPMENTION-26OCT01-NEVER",
        "event_ticker": "KXTRUMPMENTION-26OCT01",
        "title": "What will Trump say during his rally?",
        "yes_sub_title": "Unseenphrase",
    }
    signal = signal_for_market(
        market, _corpus(["China economy"] * 40, now), _calibration(), now
    )
    assert signal["status"] == "abstain"
    assert signal["reason"] == "phrase_never_seen"
    assert signal["p_hat"] is signal["p_lo"] is signal["p_hi"] is None


def test_signal_abstains_for_stale_corpus():
    now = datetime.now(timezone.utc)
    market = {
        "ticker": "KXTRUMPMENTION-26OCT01-CHINA",
        "event_ticker": "KXTRUMPMENTION-26OCT01",
        "title": "What will Trump say during his rally?",
        "yes_sub_title": "China",
    }
    signal = signal_for_market(
        market, _corpus(["China economy"] * 40, now), _calibration(), now,
        global_abstain_reason="corpus_stale",
    )
    assert signal["status"] == "abstain"
    assert signal["p_hat"] is signal["p_lo"] is signal["p_hi"] is None


def test_signal_abstains_for_low_effective_sample_and_missing_calibration():
    now = datetime.now(timezone.utc)
    market = {
        "ticker": "KXTRUMPMENTION-26OCT01-CHINA",
        "event_ticker": "KXTRUMPMENTION-26OCT01",
        "title": "What will Trump say during his rally?",
        "yes_sub_title": "China",
    }
    small = _corpus(["China economy" if i % 2 else "economy" for i in range(8)], now)
    low_n = signal_for_market(market, small, _calibration(), now)
    no_cal = signal_for_market(
        market, _corpus(["China economy", "economy"] * 20, now), None, now
    )
    assert low_n["status"] == "abstain"
    assert low_n["reason"] == "insufficient_effective_evidence"
    assert no_cal["status"] == "abstain"
    assert no_cal["reason"] == "insufficient_or_unstable_calibration"


def test_clustered_platt_bootstrap_groups_by_event_day_and_is_reproducible():
    rng = np.random.default_rng(3)
    observations = []
    for day in range(12):
        for within_day in range(30):
            p = 0.15 + 0.7 * within_day / 29
            outcome = float(rng.random() < p)
            observations.append({
                "p_raw": p,
                "outcome": outcome,
                "event_day": f"D{day}",
                "event_type": "speech",
            })
    first = fit_clustered_calibration(
        observations, bootstrap_count=30, minimum_valid_bootstraps=20
    )
    second = fit_clustered_calibration(
        observations, bootstrap_count=30, minimum_valid_bootstraps=20
    )
    assert first is not None and first == second
    assert len(first.bootstrap_coefficients) >= 20


def test_http_signals_returns_a_list_and_health_has_verdict_contract():
    class StubEngine:
        def get_signals(self):
            return [{"market_ticker": "KXTEST", "status": "abstain"}]

        def get_health(self):
            return {
                "model_version": "test-version",
                "last_corpus_update": "2026-10-01T00:00:00+00:00",
                "latest_verdict": {
                    "edge_estimate": None, "ci_low": None, "ci_high": None,
                    "n_days": 0, "as_of": None,
                },
            }

    from kalshi_mention.signal_service import request_handler

    server = ThreadingHTTPServer(("127.0.0.1", 0), request_handler(StubEngine()))
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base = f"http://127.0.0.1:{server.server_port}"
        with urlopen(f"{base}/signals") as response:
            signals = json.loads(response.read())
        with urlopen(f"{base}/health") as response:
            health = json.loads(response.read())
        assert isinstance(signals, list) and signals[0]["market_ticker"] == "KXTEST"
        assert health["latest_verdict"]["n_days"] == 0
        assert health["latest_verdict"]["ci_low"] is None
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)