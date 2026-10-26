"""Minimal Kalshi public market-data client.

Only unauthenticated endpoints are used: /series, /events, /markets, and
per-market candlesticks. No credentials, no trading capability — by design
for this pass.

Endpoint shapes verified against Kalshi's public docs (docs.kalshi.com,
"Quick Start: Market Data"). If Kalshi changes response shapes, the parsing
here fails loudly rather than silently storing partial objects.
"""

from __future__ import annotations

import time
from typing import Iterator, Optional

import requests

from . import config


class KalshiPublicClient:
    def __init__(
        self,
        base_url: str = config.KALSHI_API_BASE,
        min_interval: float = config.MIN_REQUEST_INTERVAL_SECONDS,
        session: Optional[requests.Session] = None,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.min_interval = min_interval
        self.session = session or requests.Session()
        self.session.headers.update({"Accept": "application/json"})
        self._last_request_ts = 0.0

    # -- low-level ----------------------------------------------------------

    def _throttle(self) -> None:
        elapsed = time.monotonic() - self._last_request_ts
        if elapsed < self.min_interval:
            time.sleep(self.min_interval - elapsed)

    def _get(self, path: str, params: Optional[dict] = None) -> dict:
        url = f"{self.base_url}{path}"
        last_exc: Optional[Exception] = None
        for attempt in range(config.MAX_RETRIES):
            self._throttle()
            try:
                resp = self.session.get(url, params=params, timeout=config.REQUEST_TIMEOUT_SECONDS)
                self._last_request_ts = time.monotonic()
            except requests.RequestException as exc:  # network-level failure
                last_exc = exc
                time.sleep(config.RETRY_BACKOFF_BASE_SECONDS * (2**attempt))
                continue

            if resp.status_code == 200:
                return resp.json()
            if resp.status_code in (429, 500, 502, 503, 504):
                retry_after = resp.headers.get("Retry-After")
                delay = (
                    float(retry_after)
                    if retry_after and retry_after.replace(".", "", 1).isdigit()
                    else config.RETRY_BACKOFF_BASE_SECONDS * (2**attempt)
                )
                time.sleep(delay)
                last_exc = RuntimeError(f"HTTP {resp.status_code} from {url}")
                continue
            # 4xx other than 429: not retryable, fail loudly with body context
            raise RuntimeError(f"HTTP {resp.status_code} from {url}: {resp.text[:500]}")
        raise RuntimeError(f"Giving up on {url} after {config.MAX_RETRIES} attempts: {last_exc}")

    # -- endpoints ----------------------------------------------------------

    def get_series(self, series_ticker: str) -> dict:
        payload = self._get(f"/series/{series_ticker}")
        obj = payload.get("series")
        if not isinstance(obj, dict):
            raise RuntimeError(f"Unexpected /series payload keys: {list(payload)}")
        return obj

    def iter_events(self, series_ticker: str, status: Optional[str] = None) -> Iterator[dict]:
        cursor: Optional[str] = None
        while True:
            params: dict = {"series_ticker": series_ticker, "limit": 200}
            if status:
                params["status"] = status
            if cursor:
                params["cursor"] = cursor
            payload = self._get("/events", params)
            events = payload.get("events")
            if events is None:
                raise RuntimeError(f"Unexpected /events payload keys: {list(payload)}")
            yield from events
            cursor = payload.get("cursor")
            if not cursor:
                return

    def iter_markets(self, series_ticker: str, status: Optional[str] = None) -> Iterator[dict]:
        cursor: Optional[str] = None
        while True:
            params: dict = {"series_ticker": series_ticker, "limit": 1000}
            if status:
                params["status"] = status
            if cursor:
                params["cursor"] = cursor
            payload = self._get("/markets", params)
            markets = payload.get("markets")
            if markets is None:
                raise RuntimeError(f"Unexpected /markets payload keys: {list(payload)}")
            yield from markets
            cursor = payload.get("cursor")
            if not cursor:
                return

    def get_candlesticks(
        self,
        series_ticker: str,
        market_ticker: str,
        start_ts: int,
        end_ts: int,
        period_interval_minutes: int = 60,
    ) -> list[dict]:
        payload = self._get(
            f"/series/{series_ticker}/markets/{market_ticker}/candlesticks",
            {
                "start_ts": start_ts,
                "end_ts": end_ts,
                "period_interval": period_interval_minutes,
            },
        )
        candles = payload.get("candlesticks")
        if candles is None:
            raise RuntimeError(f"Unexpected candlesticks payload keys: {list(payload)}")
        return candles

    # --- historical tier (markets settled before the rolling cutoff) --------

    def get_historical_cutoff(self) -> dict:
        return self._get("/historical/cutoff")

    def iter_historical_markets(self, series_ticker: str):
        """Settled markets older than the cutoff. Same cursor pagination as
        the live endpoint; series_ticker passed through and also enforced
        client-side by ticker prefix in case the param is ignored."""
        cursor = None
        while True:
            params = {"series_ticker": series_ticker, "limit": 1000}
            if cursor:
                params["cursor"] = cursor
            payload = self._get("/historical/markets", params)
            markets = payload.get("markets") or []
            for m in markets:
                if str(m.get("ticker", "")).startswith(series_ticker + "-"):
                    yield m
            cursor = payload.get("cursor")
            if not cursor or not markets:
                break

    def get_historical_candlesticks(
        self, market_ticker: str, start_ts: int, end_ts: int,
        period_interval_minutes: int = 60,
    ) -> list[dict]:
        payload = self._get(
            f"/historical/markets/{market_ticker}/candlesticks",
            {"start_ts": start_ts, "end_ts": end_ts,
             "period_interval": period_interval_minutes},
        )
        candles = payload.get("candlesticks")
        if candles is None:
            raise RuntimeError(f"Unexpected historical candlesticks payload: {list(payload)}")
        return candles
