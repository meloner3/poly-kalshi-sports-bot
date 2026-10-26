"""SQLite storage with point-in-time discipline.

Design rules:
  * Kalshi objects are stored append-only in *_snapshots tables (full raw JSON
    + fetched_at). The convenience tables (series/events/markets) hold only
    the latest snapshot's parsed fields. Backtests must read snapshots (or use
    as-of helpers), never assume the latest row was known historically.
  * Transcripts carry an explicit `available_at` timestamp. Historical rows
    without a real scraped_at get a documented assumption (config).
  * All timestamps stored as ISO-8601 UTC strings.
"""

from __future__ import annotations

import json
import sqlite3
from datetime import datetime, timezone
from typing import Any, Iterable, Optional
from . import paths


SCHEMA = """
CREATE TABLE IF NOT EXISTS series_snapshots (
    ticker      TEXT NOT NULL,
    fetched_at  TEXT NOT NULL,
    raw_json    TEXT NOT NULL,
    PRIMARY KEY (ticker, fetched_at)
);

CREATE TABLE IF NOT EXISTS event_snapshots (
    event_ticker TEXT NOT NULL,
    fetched_at   TEXT NOT NULL,
    raw_json     TEXT NOT NULL,
    PRIMARY KEY (event_ticker, fetched_at)
);

CREATE TABLE IF NOT EXISTS market_snapshots (
    ticker      TEXT NOT NULL,
    fetched_at  TEXT NOT NULL,
    raw_json    TEXT NOT NULL,
    PRIMARY KEY (ticker, fetched_at)
);

CREATE TABLE IF NOT EXISTS series (
    ticker      TEXT PRIMARY KEY,
    title       TEXT,
    category    TEXT,
    frequency   TEXT,
    raw_json    TEXT,
    fetched_at  TEXT
);

CREATE TABLE IF NOT EXISTS events (
    event_ticker  TEXT PRIMARY KEY,
    series_ticker TEXT,
    title         TEXT,
    sub_title     TEXT,
    strike_date   TEXT,
    raw_json      TEXT,
    fetched_at    TEXT
);

CREATE TABLE IF NOT EXISTS markets (
    ticker          TEXT PRIMARY KEY,
    event_ticker    TEXT,
    series_ticker   TEXT,
    market_type     TEXT,
    title           TEXT,
    subtitle        TEXT,
    yes_sub_title   TEXT,
    no_sub_title    TEXT,
    rules_primary   TEXT,
    rules_secondary TEXT,
    open_time       TEXT,
    close_time      TEXT,
    expiration_time TEXT,
    status          TEXT,
    result          TEXT,
    last_price      INTEGER,
    volume          INTEGER,
    liquidity       INTEGER,
    raw_json        TEXT,
    fetched_at      TEXT
);

CREATE INDEX IF NOT EXISTS idx_markets_series ON markets (series_ticker);
CREATE INDEX IF NOT EXISTS idx_markets_event  ON markets (event_ticker);
CREATE INDEX IF NOT EXISTS idx_markets_status ON markets (status);

CREATE TABLE IF NOT EXISTS candlesticks (
    market_ticker   TEXT NOT NULL,
    period_minutes  INTEGER NOT NULL,
    end_period_ts   INTEGER NOT NULL,
    raw_json        TEXT NOT NULL,
    fetched_at      TEXT NOT NULL,
    PRIMARY KEY (market_ticker, period_minutes, end_period_ts)
);

CREATE TABLE IF NOT EXISTS transcripts (
    doc_id       TEXT PRIMARY KEY,
    title        TEXT,
    date         TEXT,          -- event calendar date, ISO date (ET calendar)
    speech_type  TEXT,
    text_raw     TEXT,
    token_count  INTEGER,
    available_at TEXT NOT NULL, -- ISO UTC: real scraped_at or documented assumption
    availability_source TEXT NOT NULL,  -- 'scraped_at' | 'assumed_lag'
    source       TEXT
);

CREATE INDEX IF NOT EXISTS idx_transcripts_date ON transcripts (date);

CREATE TABLE IF NOT EXISTS labels (
    market_ticker TEXT NOT NULL,
    doc_id        TEXT NOT NULL,
    phrase        TEXT,
    phrase_source TEXT,          -- how the phrase was parsed from the market
    matched       INTEGER,       -- 0/1 for this document
    match_count   INTEGER,
    match_flags   TEXT,          -- JSON of matcher flags used
    computed_at   TEXT NOT NULL,
    PRIMARY KEY (market_ticker, doc_id)
);
CREATE TABLE IF NOT EXISTS event_doc_map (
    event_ticker TEXT PRIMARY KEY,
    doc_id       TEXT NOT NULL,
    method       TEXT NOT NULL,   -- 'override' | 'auto_title' | 'only_doc'
    score        REAL,
    computed_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS phrase_doc_counts (
    spec_key TEXT NOT NULL,
    doc_id   TEXT NOT NULL,
    count    INTEGER NOT NULL,
    PRIMARY KEY (spec_key, doc_id)
);

CREATE TABLE IF NOT EXISTS phrase_doc_counts_status (
    doc_id       TEXT PRIMARY KEY,
    computed_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS news_volume (
    spec_key TEXT NOT NULL,
    date     TEXT NOT NULL,
    vol      REAL NOT NULL,
    PRIMARY KEY (spec_key, date)
);

"""


def utcnow_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def connect(path: str) -> sqlite3.Connection:
    # Resolved through paths so a bare "kalshi_mention.sqlite" lands in the
    # data dir rather than wherever the process happens to be running.
    # ":memory:" and sqlite URIs are sqlite directives, not filesystem paths.
    if path != ":memory:" and not str(path).startswith("file:"):
        path = str(paths.resolve(path))
    conn = sqlite3.connect(path)
    conn.row_factory = sqlite3.Row
    conn.executescript(SCHEMA)
    return conn


# ---------------------------------------------------------------------------
# Upserts (snapshot + latest)
# ---------------------------------------------------------------------------

def upsert_series(conn: sqlite3.Connection, obj: dict, fetched_at: Optional[str] = None) -> None:
    fetched_at = fetched_at or utcnow_iso()
    raw = json.dumps(obj, sort_keys=True)
    ticker = obj.get("ticker") or obj.get("series_ticker")
    if not ticker:
        raise ValueError(f"series object missing ticker: {list(obj)}")
    conn.execute(
        "INSERT OR IGNORE INTO series_snapshots (ticker, fetched_at, raw_json) VALUES (?,?,?)",
        (ticker, fetched_at, raw),
    )
    conn.execute(
        """INSERT INTO series (ticker, title, category, frequency, raw_json, fetched_at)
           VALUES (?,?,?,?,?,?)
           ON CONFLICT(ticker) DO UPDATE SET
             title=excluded.title, category=excluded.category,
             frequency=excluded.frequency, raw_json=excluded.raw_json,
             fetched_at=excluded.fetched_at""",
        (ticker, obj.get("title"), obj.get("category"), obj.get("frequency"), raw, fetched_at),
    )


def upsert_event(conn: sqlite3.Connection, obj: dict, fetched_at: Optional[str] = None) -> None:
    fetched_at = fetched_at or utcnow_iso()
    raw = json.dumps(obj, sort_keys=True)
    et = obj.get("event_ticker")
    if not et:
        raise ValueError(f"event object missing event_ticker: {list(obj)}")
    conn.execute(
        "INSERT OR IGNORE INTO event_snapshots (event_ticker, fetched_at, raw_json) VALUES (?,?,?)",
        (et, fetched_at, raw),
    )
    conn.execute(
        """INSERT INTO events (event_ticker, series_ticker, title, sub_title, strike_date, raw_json, fetched_at)
           VALUES (?,?,?,?,?,?,?)
           ON CONFLICT(event_ticker) DO UPDATE SET
             series_ticker=excluded.series_ticker, title=excluded.title,
             sub_title=excluded.sub_title, strike_date=excluded.strike_date,
             raw_json=excluded.raw_json, fetched_at=excluded.fetched_at""",
        (
            et,
            obj.get("series_ticker"),
            obj.get("title"),
            obj.get("sub_title"),
            obj.get("strike_date"),
            raw,
            fetched_at,
        ),
    )


def upsert_market(conn: sqlite3.Connection, obj: dict, fetched_at: Optional[str] = None) -> None:
    fetched_at = fetched_at or utcnow_iso()
    raw = json.dumps(obj, sort_keys=True)
    ticker = obj.get("ticker")
    if not ticker:
        raise ValueError(f"market object missing ticker: {list(obj)}")
    # Kalshi market tickers are SERIES-EVENTSUFFIX-MARKETSUFFIX; derive series
    # from the event ticker's first segment when not provided directly.
    event_ticker = obj.get("event_ticker") or ""
    series_ticker = obj.get("series_ticker") or (event_ticker.split("-")[0] if event_ticker else ticker.split("-")[0])
    conn.execute(
        "INSERT OR IGNORE INTO market_snapshots (ticker, fetched_at, raw_json) VALUES (?,?,?)",
        (ticker, fetched_at, raw),
    )
    conn.execute(
        """INSERT INTO markets (ticker, event_ticker, series_ticker, market_type, title, subtitle,
                                yes_sub_title, no_sub_title, rules_primary, rules_secondary,
                                open_time, close_time, expiration_time, status, result,
                                last_price, volume, liquidity, raw_json, fetched_at)
           VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
           ON CONFLICT(ticker) DO UPDATE SET
             event_ticker=excluded.event_ticker, series_ticker=excluded.series_ticker,
             market_type=excluded.market_type, title=excluded.title, subtitle=excluded.subtitle,
             yes_sub_title=excluded.yes_sub_title, no_sub_title=excluded.no_sub_title,
             rules_primary=excluded.rules_primary, rules_secondary=excluded.rules_secondary,
             open_time=excluded.open_time, close_time=excluded.close_time,
             expiration_time=excluded.expiration_time, status=excluded.status,
             result=excluded.result, last_price=excluded.last_price,
             volume=excluded.volume, liquidity=excluded.liquidity,
             raw_json=excluded.raw_json, fetched_at=excluded.fetched_at""",
        (
            ticker,
            event_ticker,
            series_ticker,
            obj.get("market_type"),
            obj.get("title"),
            obj.get("subtitle"),
            obj.get("yes_sub_title"),
            obj.get("no_sub_title"),
            obj.get("rules_primary"),
            obj.get("rules_secondary"),
            obj.get("open_time"),
            obj.get("close_time"),
            obj.get("expiration_time"),
            obj.get("status"),
            obj.get("result"),
            obj.get("last_price"),
            obj.get("volume"),
            obj.get("liquidity"),
            raw,
            fetched_at,
        ),
    )


def insert_candlesticks(
    conn: sqlite3.Connection,
    market_ticker: str,
    period_minutes: int,
    candles: Iterable[dict],
    fetched_at: Optional[str] = None,
) -> int:
    fetched_at = fetched_at or utcnow_iso()
    n = 0
    for c in candles:
        end_ts = c.get("end_period_ts")
        if end_ts is None:
            continue
        conn.execute(
            """INSERT OR IGNORE INTO candlesticks
               (market_ticker, period_minutes, end_period_ts, raw_json, fetched_at)
               VALUES (?,?,?,?,?)""",
            (market_ticker, period_minutes, int(end_ts), json.dumps(c, sort_keys=True), fetched_at),
        )
        n += 1
    return n


def upsert_transcript(
    conn: sqlite3.Connection,
    doc_id: str,
    title: Optional[str],
    date_iso: Optional[str],
    speech_type: Optional[str],
    text_raw: str,
    token_count: Optional[int],
    available_at: str,
    availability_source: str,
    source: str = "factbase",
) -> None:
    conn.execute(
        """INSERT INTO transcripts
           (doc_id, title, date, speech_type, text_raw, token_count,
            available_at, availability_source, source)
           VALUES (?,?,?,?,?,?,?,?,?)
           ON CONFLICT(doc_id) DO UPDATE SET
             title=excluded.title, date=excluded.date, speech_type=excluded.speech_type,
             text_raw=excluded.text_raw, token_count=excluded.token_count,
             available_at=excluded.available_at,
             availability_source=excluded.availability_source, source=excluded.source""",
        (doc_id, title, date_iso, speech_type, text_raw, token_count,
         available_at, availability_source, source),
    )


# ---------------------------------------------------------------------------
# Point-in-time queries
# ---------------------------------------------------------------------------

def transcripts_as_of(conn: sqlite3.Connection, as_of_utc_iso: str) -> list[sqlite3.Row]:
    """All transcripts whose availability timestamp is <= as_of.

    This is THE access path any backtest must use. Reading the transcripts
    table without the available_at filter is a point-in-time bug by definition.
    """
    return conn.execute(
        "SELECT * FROM transcripts WHERE available_at <= ? ORDER BY available_at",
        (as_of_utc_iso,),
    ).fetchall()


def market_snapshot_as_of(
    conn: sqlite3.Connection, ticker: str, as_of_utc_iso: str
) -> Optional[dict[str, Any]]:
    """Latest snapshot of a market fetched at or before as_of (None if unseen)."""
    row = conn.execute(
        """SELECT raw_json FROM market_snapshots
           WHERE ticker = ? AND fetched_at <= ?
           ORDER BY fetched_at DESC LIMIT 1""",
        (ticker, as_of_utc_iso),
    ).fetchone()
    return json.loads(row["raw_json"]) if row else None
