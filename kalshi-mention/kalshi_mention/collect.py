"""Phase 0 collection job.

Usage (run on a machine with internet access — Kalshi market data is public,
no credentials required):

    python -m kalshi_mention.collect --db kalshi_mention.sqlite
    python -m kalshi_mention.collect --db kalshi_mention.sqlite --candles --candle-period 60

Idempotent: re-running refreshes latest state and appends new snapshots.
"""

from __future__ import annotations

import argparse
import sys
from datetime import datetime, timezone

from . import config, db
from .kalshi_client import KalshiPublicClient


def _parse_iso_ts(value: str | None) -> int | None:
    if not value:
        return None
    try:
        return int(datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp())
    except ValueError:
        return None


def collect(
    db_path: str,
    series_tickers: list[str],
    with_candles: bool = False,
    candle_period_minutes: int = 60,
    base_url: str = config.KALSHI_API_BASE,
) -> None:
    client = KalshiPublicClient(base_url=base_url)
    conn = db.connect(db_path)

    for st in series_tickers:
        print(f"\n=== {st} ===")
        try:
            series_obj = client.get_series(st)
            db.upsert_series(conn, series_obj)
            print(f"  series: {series_obj.get('title')!r} frequency={series_obj.get('frequency')!r}")
        except RuntimeError as exc:
            print(f"  ! series fetch failed ({exc}); continuing with events/markets", file=sys.stderr)

        n_events = 0
        for ev in client.iter_events(st):
            db.upsert_event(conn, ev)
            n_events += 1
        conn.commit()
        print(f"  events:  {n_events}")

        n_markets = 0
        settled = 0
        for mkt in client.iter_markets(st):
            db.upsert_market(conn, mkt)
            n_markets += 1
            if (mkt.get("result") or "") in ("yes", "no"):
                settled += 1
        conn.commit()
        print(f"  markets: {n_markets} ({settled} with a yes/no result)")

    if with_candles:
        _collect_candles(conn, client, series_tickers, candle_period_minutes)

    conn.commit()
    conn.close()
    print("\nDone.")


def _collect_candles(conn, client: KalshiPublicClient, series_tickers, period_minutes: int) -> None:
    """Price history for settled markets — needed later for the price-aware
    backtest. Optional here because it is by far the slowest part."""
    rows = conn.execute(
        """SELECT ticker, series_ticker, open_time, close_time FROM markets
           WHERE series_ticker IN ({}) AND result IN ('yes','no')""".format(
            ",".join("?" * len(series_tickers))
        ),
        list(series_tickers),
    ).fetchall()
    print(f"\nCandlesticks for {len(rows)} settled markets (period={period_minutes}m)...")
    have = {
        r["market_ticker"]
        for r in conn.execute(
            "SELECT DISTINCT market_ticker FROM candlesticks WHERE period_minutes = ?",
            (period_minutes,),
        )
    }
    if have:
        rows = [r for r in rows if r["ticker"] not in have]
        print(f"  resume: {len(have)} markets already stored, {len(rows)} to fetch")
    ok = failed = 0
    for r in rows:
        start = _parse_iso_ts(r["open_time"])
        end = _parse_iso_ts(r["close_time"])
        if start is None or end is None or end <= start:
            failed += 1
            continue
        try:
            candles = client.get_candlesticks(
                r["series_ticker"], r["ticker"], start, end, period_minutes
            )
            db.insert_candlesticks(conn, r["ticker"], period_minutes, candles)
            ok += 1
        except RuntimeError as exc:
            failed += 1
            print(f"  ! {r['ticker']}: {exc}", file=sys.stderr)
        if (ok + failed) % 50 == 0:
            conn.commit()
            print(f"  progress: {ok} ok / {failed} failed")
    conn.commit()
    print(f"  candlesticks: {ok} markets ok, {failed} failed/skipped")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--series", nargs="*", default=config.MENTION_SERIES,
                    help="series tickers to collect (default: known mention series)")
    ap.add_argument("--base-url", default=config.KALSHI_API_BASE)
    ap.add_argument("--candles", action="store_true",
                    help="also pull candlestick price history for settled markets")
    ap.add_argument("--candle-period", type=int, default=60, choices=[1, 60, 1440],
                    help="candlestick period in minutes")
    args = ap.parse_args()
    collect(args.db, args.series, args.candles, args.candle_period, args.base_url)


if __name__ == "__main__":
    main()
