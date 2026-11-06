"""Local HTTP service exposing mention-market forecasts and health."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import threading
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Callable
from urllib.parse import urlsplit

import numpy as np

from . import component_a as component_a_module
from . import config, counts as counts_module, db, event_types as event_types_module
from . import paths, phrase as phrase_module, predict
from . import component_a
from .component_a import fit_platt, logit, predict_models
from .counts import spec_variant_token_lists
from .event_types import classify_doc, classify_market
from .kalshi_client import KalshiPublicClient
from .phrase import extract_phrase

_EVENT_DAY_RE = re.compile(r"-([A-Z]*\d{2}[A-Z]{3}\d{2})(?:-|$)")
_EPS = 1e-6


@dataclass(frozen=True)
class CorpusView:
    docs: list
    lengths: np.ndarray
    event_ts: np.ndarray
    types: np.ndarray
    latest_update: str | None
    freshness_sec: float | None


@dataclass(frozen=True)
class CalibrationFit:
    coefficients: tuple[float, float]
    bootstrap_coefficients: tuple[tuple[float, float], ...]
    event_type_base_rates: dict[str, float]
    overall_base_rate: float
    market_count: int
    model_version: str


def event_day(ticker: str, event_ticker: str | None = None) -> str | None:
    match = _EVENT_DAY_RE.search(ticker or "")
    if match:
        return match.group(1)
    match = _EVENT_DAY_RE.search(event_ticker or "")
    return match.group(1) if match else None


def prepare_corpus(docs: list, now: datetime | None = None) -> CorpusView:
    now = now or datetime.now(timezone.utc)
    now_ts = now.timestamp()
    docs = [
        doc for doc in docs
        if (available_at := predict._ts(doc["available_at"])) is not None
        and available_at <= now_ts
    ]
    lengths = np.array([float(doc["token_count"] or 0) for doc in docs])
    event_ts = np.array([
        predict._ts(str(doc["date"])[:10] + "T12:00:00+00:00") or 0.0
        for doc in docs
    ])
    types = np.array([classify_doc(doc["speech_type"], doc["title"]) for doc in docs])
    latest = max((doc["available_at"] for doc in docs if doc["available_at"]), default=None)
    latest_ts = predict._ts(latest)
    freshness = max(0.0, now_ts - latest_ts) if latest_ts is not None else None
    return CorpusView(docs, lengths, event_ts, types, latest, freshness)


def calibration_observations(conn) -> list[dict]:
    """Build resolved-market calibration points using only pre-open transcripts."""
    placeholders = ",".join("?" * len(config.LABELING_EXCLUDED_SERIES)) or "''"
    markets = conn.execute(
        f"""SELECT m.ticker, m.event_ticker, m.result, m.open_time, m.title AS mtitle,
                   m.raw_json, e.title AS etitle
            FROM markets m LEFT JOIN events e ON m.event_ticker = e.event_ticker
            WHERE m.result IN ('yes','no') AND m.series_ticker NOT IN ({placeholders})""",
        list(config.LABELING_EXCLUDED_SERIES),
    ).fetchall()
    type_day_outcomes: dict[str, dict[str, list[float]]] = {}
    day_outcomes: dict[str, list[float]] = {}
    for market in markets:
        day = event_day(market["ticker"], market["event_ticker"])
        if day is None:
            continue
        market_type = classify_market(market["etitle"] or market["mtitle"])
        outcome = 1.0 if market["result"] == "yes" else 0.0
        type_day_outcomes.setdefault(market_type, {}).setdefault(day, []).append(outcome)
        day_outcomes.setdefault(day, []).append(outcome)
    base_rates = {
        event_type: float(np.mean([np.mean(values) for values in days.values()]))
        for event_type, days in type_day_outcomes.items()
    }
    overall_base_rate = float(np.mean([np.mean(values) for values in day_outcomes.values()]))
    docs = conn.execute(
        "SELECT doc_id, date, available_at, token_count, speech_type, title "
        "FROM transcripts WHERE token_count IS NOT NULL ORDER BY available_at"
    ).fetchall()
    available = np.array([predict._ts(doc["available_at"]) or math.inf for doc in docs])
    lengths = np.array([float(doc["token_count"] or 0) for doc in docs])
    event_ts = np.array([
        predict._ts(str(doc["date"])[:10] + "T12:00:00+00:00") or 0.0
        for doc in docs
    ])
    doc_types = np.array([classify_doc(doc["speech_type"], doc["title"]) for doc in docs])
    doc_index = {doc["doc_id"]: idx for idx, doc in enumerate(docs)}
    counts_by_spec: dict[str, np.ndarray] = {}
    for row in conn.execute("SELECT spec_key, doc_id, count FROM phrase_doc_counts"):
        counts = counts_by_spec.setdefault(row["spec_key"], np.zeros(len(docs)))
        idx = doc_index.get(row["doc_id"])
        if idx is not None:
            counts[idx] = row["count"]

    zero_counts = np.zeros(len(docs))
    observations = []
    for market in markets:
        phrase, _ = extract_phrase(json.loads(market["raw_json"]))
        as_of = predict._ts(market["open_time"])
        day = event_day(market["ticker"], market["event_ticker"])
        if not phrase or as_of is None or day is None:
            continue
        eligible = int(np.searchsorted(available, as_of, side="right"))
        if eligible < 30:
            continue
        market_type = classify_market(market["etitle"] or market["mtitle"])
        ages = np.maximum((as_of - event_ts[:eligible]) / 86400.0, 0.0)
        weights = np.power(0.5, ages / 30.0) * np.where(
            doc_types[:eligible] == market_type, 1.0, 0.15
        )
        phrase_counts = counts_by_spec.get(predict.spec_key(phrase), zero_counts)[:eligible]
        threshold = predict.parse_phrase_spec(phrase)[1]
        hit_values = (phrase_counts >= threshold).astype(float)
        weight_sum = float(weights.sum())
        squared_weight_sum = float(np.dot(weights, weights))
        if weight_sum <= 0 or squared_weight_sum <= 0:
            continue
        effective_n = weight_sum * weight_sum / squared_weight_sum
        weighted_rate = float(np.dot(weights, hit_values) / weight_sum)
        base_rate = base_rates.get(market_type, overall_base_rate)
        alpha = config.SIGNAL_PRIOR_STRENGTH * base_rate + weighted_rate * effective_n
        beta = config.SIGNAL_PRIOR_STRENGTH * (1.0 - base_rate) + (1.0 - weighted_rate) * effective_n
        raw = alpha / (alpha + beta)
        if math.isfinite(float(raw)):
            observations.append({
                "p_raw": float(raw),
                "outcome": 1.0 if market["result"] == "yes" else 0.0,
                "event_day": day,
                "event_type": market_type,
                "base_rate": base_rate,
            })
    return observations


def _model_version(observations: list[dict], coefficients: tuple[float, float]) -> str:
    digest = hashlib.sha256()
    for path in (
        Path(predict.__file__), Path(component_a_module.__file__),
        Path(counts_module.__file__), Path(event_types_module.__file__),
        Path(phrase_module.__file__), Path(__file__),
    ):
        digest.update(path.read_bytes())
    payload = {
        "calibration": [round(coefficients[0], 12), round(coefficients[1], 12)],
        "calibration_rows": [
            [round(row["p_raw"], 12), row["outcome"], row["event_day"], row["event_type"]]
            for row in observations
        ],
        "signal_config": {
            "minimum_effective_n": config.SIGNAL_MIN_EFFECTIVE_N,
            "minimum_calibration_markets": config.SIGNAL_MIN_CALIBRATION_MARKETS,
            "prior_strength": config.SIGNAL_PRIOR_STRENGTH,
            "bootstrap_count": config.SIGNAL_CALIBRATION_BOOTSTRAPS,
            "interval": [config.SIGNAL_INTERVAL_LOW, config.SIGNAL_INTERVAL_HIGH],
            "max_corpus_age_days": config.PREDICT_MAX_CORPUS_AGE_DAYS,
        },
    }
    digest.update(json.dumps(payload, sort_keys=True, separators=(",", ":")).encode())
    return digest.hexdigest()


def fit_clustered_calibration(
    observations: list[dict],
    bootstrap_count: int = config.SIGNAL_CALIBRATION_BOOTSTRAPS,
    minimum_markets: int = config.SIGNAL_MIN_CALIBRATION_MARKETS,
    minimum_valid_bootstraps: int = config.SIGNAL_MIN_VALID_BOOTSTRAPS,
    seed: int = 0,
) -> CalibrationFit | None:
    if len(observations) < minimum_markets:
        return None
    raw = np.array([row["p_raw"] for row in observations], dtype=float)
    outcomes = np.array([row["outcome"] for row in observations], dtype=float)
    days = np.array([row["event_day"] for row in observations], dtype=object)
    if len(set(days)) < 5 or float(np.std(logit(raw))) < 0.5:
        return None
    coefficients = fit_platt(logit(raw), outcomes)
    if not (math.isfinite(coefficients[0]) and 0.2 <= coefficients[1] <= 5.0):
        return None

    grouped = {day: np.flatnonzero(days == day) for day in sorted(set(days))}
    day_keys = np.array(list(grouped), dtype=object)
    rng = np.random.default_rng(seed)
    bootstrap_coefficients = []
    for _ in range(bootstrap_count):
        chosen_days = rng.choice(day_keys, size=len(day_keys), replace=True)
        sample = np.concatenate([grouped[day] for day in chosen_days])
        if outcomes[sample].min() == outcomes[sample].max():
            continue
        a, b = fit_platt(logit(raw[sample]), outcomes[sample])
        if math.isfinite(a) and math.isfinite(b) and 0.2 <= b <= 5.0:
            bootstrap_coefficients.append((float(a), float(b)))
    if len(bootstrap_coefficients) < minimum_valid_bootstraps:
        return None

    by_type_day: dict[str, dict[str, list[float]]] = {}
    by_day: dict[str, list[float]] = {}
    for row in observations:
        by_type_day.setdefault(row["event_type"], {}).setdefault(
            row["event_day"], []
        ).append(row["outcome"])
        by_day.setdefault(row["event_day"], []).append(row["outcome"])
    base_rates = {
        event_type: float(np.mean([row.get("base_rate", row["outcome"]) for row in observations
                                   if row["event_type"] == event_type]))
        for event_type in by_type_day
    }
    overall_base_rate = float(np.mean([np.mean(values) for values in by_day.values()]))
    return CalibrationFit(
        (float(coefficients[0]), float(coefficients[1])),
        tuple(bootstrap_coefficients),
        base_rates,
        overall_base_rate,
        len(observations),
        _model_version(observations, coefficients),
    )


def _apply_platt(probability: float, coefficients: tuple[float, float]) -> float:
    a, b = coefficients
    clipped = min(max(probability, _EPS), 1.0 - _EPS)
    log_odds = math.log(clipped / (1.0 - clipped))
    return 1.0 / (1.0 + math.exp(-max(min(a + b * log_odds, 30.0), -30.0)))


def beta_parameters(hit_values: np.ndarray, weights: np.ndarray, base_rate: float) -> tuple[float, float, float]:
    weight_sum = float(weights.sum())
    squared_weight_sum = float(np.dot(weights, weights))
    if weight_sum <= 0 or squared_weight_sum <= 0:
        return 0.0, 0.0, 0.0
    effective_n = weight_sum * weight_sum / squared_weight_sum
    weighted_rate = float(np.dot(weights, hit_values) / weight_sum)
    base_rate = min(max(base_rate, _EPS), 1.0 - _EPS)
    alpha = config.SIGNAL_PRIOR_STRENGTH * base_rate + weighted_rate * effective_n
    beta = config.SIGNAL_PRIOR_STRENGTH * (1.0 - base_rate) + (1.0 - weighted_rate) * effective_n
    return alpha, beta, effective_n


def abstain_signal(
    market: dict,
    phrase: str | None,
    event_type: str,
    calibration_n: int,
    features_as_of: str,
    freshness_sec: float | None,
    reason: str,
    model_version: str | None = None,
) -> dict:
    return {
        "market_ticker": market.get("ticker"),
        "event_id": market.get("event_ticker"),
        "event_day": event_day(market.get("ticker", ""), market.get("event_ticker")),
        "close_time": market.get("close_time"),
        "phrase": phrase,
        "p_hat": None,
        "p_lo": None,
        "p_hi": None,
        "model_version": model_version,
        "features_as_of": features_as_of,
        "data_freshness_sec": freshness_sec,
        "calibration_n": calibration_n,
        "status": "abstain",
        "reason": reason,
        "event_type": event_type,
    }


def signal_for_market(
    market: dict,
    corpus: CorpusView,
    calibration: CalibrationFit | None,
    features_as_of: datetime | None = None,
    global_abstain_reason: str | None = None,
) -> dict:
    now = features_as_of or datetime.now(timezone.utc)
    now_text = now.astimezone(timezone.utc).isoformat(timespec="seconds")
    phrase, _ = extract_phrase(market)
    event_text = market.get("title") or market.get("event_title")
    event_type = classify_market(event_text)
    calibration_n = calibration.market_count if calibration else 0
    version = calibration.model_version if calibration else None

    reason = global_abstain_reason
    if reason is None and not market.get("event_ticker"):
        reason = "missing_event_context"
    if reason is None and not event_text:
        reason = "missing_event_context"
    if reason is None and not phrase:
        reason = "unparseable_phrase"
    if reason is None and calibration is None:
        reason = "insufficient_or_unstable_calibration"
    if reason is not None:
        return abstain_signal(
            market, phrase, event_type, calibration_n, now_text,
            corpus.freshness_sec, reason, version,
        )

    counts = predict.count_spec_in_docs(corpus.docs, phrase)
    if not np.any(counts > 0):
        return abstain_signal(
            market, phrase, event_type, calibration_n, now_text,
            corpus.freshness_sec, "phrase_never_seen", version,
        )

    threshold = predict.parse_phrase_spec(phrase)[1]
    ages = np.maximum((now.timestamp() - corpus.event_ts) / 86400.0, 0.0)
    type_weights = np.where(corpus.types == event_type, 1.0, 0.15)
    weights = np.power(0.5, ages / 30.0) * type_weights
    hit_values = (counts >= threshold).astype(float)
    base_rate = calibration.event_type_base_rates.get(
        event_type, calibration.overall_base_rate
    )
    alpha, beta, effective_n = beta_parameters(hit_values, weights, base_rate)
    if alpha <= 0 or beta <= 0:
        return abstain_signal(
            market, phrase, event_type, calibration_n, now_text,
            corpus.freshness_sec, "insufficient_effective_evidence", version,
        )

    if effective_n < config.SIGNAL_MIN_EFFECTIVE_N:
        return abstain_signal(
            market, phrase, event_type, calibration_n, now_text,
            corpus.freshness_sec, "insufficient_effective_evidence", version,
        )

    assert calibration is not None
    q_hat = alpha / (alpha + beta)
    p_hat = _apply_platt(q_hat, calibration.coefficients)

    seed_material = f"{market.get('ticker')}:{calibration.model_version}".encode()
    seed = int.from_bytes(hashlib.sha256(seed_material).digest()[:8], "big")
    rng = np.random.default_rng(seed)
    q_draws = rng.beta(alpha, beta, size=config.SIGNAL_CALIBRATION_BOOTSTRAPS)
    bootstrap_indices = rng.integers(
        0, len(calibration.bootstrap_coefficients), size=len(q_draws)
    )
    p_draws = np.array([
        _apply_platt(float(q), calibration.bootstrap_coefficients[int(index)])
        for q, index in zip(q_draws, bootstrap_indices)
    ])
    p_lo, p_hi = np.quantile(
        p_draws, [config.SIGNAL_INTERVAL_LOW, config.SIGNAL_INTERVAL_HIGH]
    )
    p_lo = min(float(p_lo), p_hat)
    p_hi = max(float(p_hi), p_hat)

    return {
        "market_ticker": market.get("ticker"),
        "event_id": market.get("event_ticker"),
        "event_day": event_day(market.get("ticker", ""), market.get("event_ticker")),
        "close_time": market.get("close_time"),
        "phrase": phrase,
        "p_hat": float(p_hat),
        "p_lo": float(p_lo),
        "p_hi": float(p_hi),
        "model_version": calibration.model_version,
        "features_as_of": now_text,
        "data_freshness_sec": corpus.freshness_sec,
        "calibration_n": calibration.market_count,
        "status": "ok",
        "event_type": event_type,
    }


def open_mention_markets() -> list[dict]:
    client = KalshiPublicClient()
    markets = []
    for series in config.MENTION_SERIES:
        if series in config.LABELING_EXCLUDED_SERIES:
            continue
        markets.extend(client.iter_markets(series, status="open"))
    return markets


def latest_verdict() -> dict:
    artifact = paths.data_path("directional_evaluation/gate.json")
    if not artifact.exists():
        return {
            "edge_estimate": None,
            "ci_low": None,
            "ci_high": None,
            "n_days": 0,
            "as_of": None,
            "status": "unavailable",
            "reason": "forward-paper gate artifact has not been produced",
        }
    try:
        result = json.loads(artifact.read_text(encoding="utf-8"))
        required = ("edge_estimate", "ci_low", "ci_high", "n_days", "as_of")
        if any(key not in result for key in required):
            raise ValueError("missing required gate fields")
        return {key: result[key] for key in required} | {
            "status": result.get("status", "available"),
            "model_version": result.get("model_version"),
            "live_gate_passed": result.get("live_gate_passed", False),
        }
    except (OSError, json.JSONDecodeError, ValueError) as exc:
        return {
            "edge_estimate": None,
            "ci_low": None,
            "ci_high": None,
            "n_days": 0,
            "as_of": None,
            "status": "invalid",
            "reason": str(exc),
        }


class SignalEngine:
    def __init__(
        self,
        db_path: str = config.DEFAULT_DB_PATH,
        market_provider: Callable[[], list[dict]] = open_mention_markets,
        refresh_seconds: int = config.SIGNAL_REFRESH_SECONDS,
    ) -> None:
        self.db_path = db_path
        self.market_provider = market_provider
        self.refresh_seconds = refresh_seconds
        self._lock = threading.RLock()
        self._refreshed_at = 0.0
        self._snapshot: dict = {
            "signals": [],
            "error": "signal cache has not been initialized",
            "corpus": None,
            "calibration": None,
        }

    def refresh(self) -> None:
        with self._lock:
            now = datetime.now(timezone.utc)
            corpus = None
            calibration = None
            try:
                conn = db.connect(self.db_path)
                try:
                    stale_reason = None
                    try:
                        predict.require_fresh_corpus(conn)
                    except ValueError as exc:
                        stale_reason = str(exc)
                    docs = predict.load_docs(conn)[0]
                    corpus = prepare_corpus(docs, now)
                    observations = calibration_observations(conn) if stale_reason is None else []
                    calibration = fit_clustered_calibration(observations) if observations else None
                    markets = self.market_provider()
                finally:
                    conn.close()

                global_reason = stale_reason
                if global_reason is None and calibration is None:
                    global_reason = "insufficient_or_unstable_calibration"
                signals = [
                    signal_for_market(
                        market, corpus, calibration, now, global_reason
                    )
                    for market in markets
                ]
                self._snapshot = {
                    "signals": signals,
                    "error": None,
                    "corpus": corpus,
                    "calibration": calibration,
                    "updated_at": now.isoformat(timespec="seconds"),
                }
            except Exception as exc:
                self._snapshot = {
                    "signals": [],
                    "error": str(exc),
                    "corpus": corpus,
                    "calibration": calibration,
                    "updated_at": now.isoformat(timespec="seconds"),
                }
            self._refreshed_at = time.monotonic()

    def _current(self) -> dict:
        with self._lock:
            if time.monotonic() - self._refreshed_at >= self.refresh_seconds:
                self.refresh()
            return self._snapshot

    def get_signals(self) -> list[dict]:
        return list(self._current()["signals"])

    def get_health(self) -> dict:
        snapshot = self._current()
        corpus = snapshot.get("corpus")
        calibration = snapshot.get("calibration")
        healthy = snapshot.get("error") is None and calibration is not None
        return {
            "status": "ok" if healthy else "degraded",
            "model_version": calibration.model_version if calibration else None,
            "last_corpus_update": corpus.latest_update if corpus else None,
            "latest_verdict": latest_verdict(),
            "error": snapshot.get("error"),
        }


def request_handler(engine: SignalEngine):
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            path = urlsplit(self.path).path
            if path == "/signals":
                body = engine.get_signals()
            elif path == "/health":
                body = engine.get_health()
            else:
                self.send_error(404)
                return
            encoded = json.dumps(body, allow_nan=False).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)

        def log_message(self, _format: str, *_args) -> None:
            return

    return Handler


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", default=config.DEFAULT_DB_PATH)
    parser.add_argument("--port", type=int, default=config.SIGNAL_PORT)
    parser.add_argument("--refresh-seconds", type=int, default=config.SIGNAL_REFRESH_SECONDS)
    args = parser.parse_args()
    if not (1 <= args.port <= 65535) or args.refresh_seconds < 1:
        parser.error("port and refresh-seconds must be positive and in range")
    engine = SignalEngine(args.db, refresh_seconds=args.refresh_seconds)
    server = ThreadingHTTPServer(("127.0.0.1", args.port), request_handler(engine))
    print(f"Mention signal service listening on http://127.0.0.1:{args.port}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()