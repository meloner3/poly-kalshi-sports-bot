"""Historical-tier collection (pass 4a).

    python -m kalshi_mention.historical --db kalshi_mention.sqlite

Kalshi partitions exchange data: markets settled more than ~3 months ago move
from GET /markets to GET /historical/markets, and their candlesticks move to
GET /historical/markets/{ticker}/candlesticks (docs: Historical Data). This is
why the Jan-Apr 2026 mention markets were invisible to the pass-1 collector
and why 68 candle fetches 404'd on markets that settled just before the
rolling cutoff.

This module: prints the current cutoff, pages the historical markets for each
mention series into the same `markets` table (upsert, snapshot-preserving),
then fetches candlesticks for every settled market that has none stored —
trying the live endpoint first and falling back to the historical one on 404.
Fully resumable: rerun after interruption and it skips what it has.
"""

from __future__ import annotations

import argparse
import sys

from . import config, db
from .kalshi_client import KalshiPublicClient
from .collect import _parse_iso_ts


def run(db_path: str, period_minutes: int = 60, base_url: str | None = None) -> None:
    client = KalshiPublicClient(base_url=base_url) if base_url else KalshiPublicClient()
    conn = db.connect(db_path)

    try:
        cutoff = client.get_historical_cutoff()
        print(f"Historical cutoff payload: {cutoff}")
    except RuntimeError as exc:
        print(f"cutoff endpoint failed ({exc}) — continuing anyway", file=sys.stderr)

    # --- markets ------------------------------------------------------------
    total_new = 0
    for st in config.MENTION_SERIES:
        before = conn.execute(
            "SELECT COUNT(*) c FROM markets WHERE series_ticker = ?", (st,)
        ).fetchone()["c"]
        n = 0
        try:
            for m in client.iter_historical_markets(st):
                m.setdefault("series_ticker", st)
                db.upsert_market(conn, m)
                n += 1
        except RuntimeError as exc:
            print(f"  ! {st}: historical markets error -> {exc}", file=sys.stderr)
        conn.commit()
        after = conn.execute(
            "SELECT COUNT(*) c FROM markets WHERE series_ticker = ?", (st,)
        ).fetchone()["c"]
        total_new += after - before
        print(f"{st}: {n} historical markets fetched ({after - before} new; total now {after})")

    settled = conn.execute(
        "SELECT COUNT(*) c FROM markets WHERE result IN ('yes','no')"
    ).fetchone()["c"]
    print(f"\nSettled markets in DB: {settled} (+{total_new} from historical tier)")

    # --- candlesticks for any settled market without them -------------------
    rows = conn.execute(
        """SELECT m.ticker, m.series_ticker, m.open_time, m.close_time
           FROM markets m
           WHERE m.result IN ('yes','no')
             AND m.ticker NOT IN (
                 SELECT DISTINCT market_ticker FROM candlesticks WHERE period_minutes = ?
             )""",
        (period_minutes,),
    ).fetchall()
    print(f"Candlesticks missing for {len(rows)} settled markets — fetching "
          f"(live endpoint with historical fallback)...")
    ok = failed = 0
    for r in rows:
        start = _parse_iso_ts(r["open_time"])
        end = _parse_iso_ts(r["close_time"])
        if start is None or end is None or end <= start:
            failed += 1
            continue
        candles = None
        try:
            candles = client.get_candlesticks(
                r["series_ticker"], r["ticker"], start, end, period_minutes)
        except RuntimeError:
            try:
                candles = client.get_historical_candlesticks(
                    r["ticker"], start, end, period_minutes)
            except RuntimeError as exc:
                failed += 1
                print(f"  ! {r['ticker']}: both endpoints failed -> {exc}", file=sys.stderr)
        if candles is not None:
            db.insert_candlesticks(conn, r["ticker"], period_minutes, candles)
            ok += 1
        if (ok + failed) % 50 == 0 and (ok + failed) > 0:
            conn.commit()
            print(f"  progress: {ok} ok / {failed} failed")
    conn.commit()
    print(f"  candlesticks: {ok} ok, {failed} failed")
    conn.close()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--candle-period", type=int, default=60, choices=[1, 60, 1440])
    args = ap.parse_args()
    run(args.db, args.candle_period)


if __name__ == "__main__":
    main()
