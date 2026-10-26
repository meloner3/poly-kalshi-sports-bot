"""Fixture-based tests: realistic Kalshi market payloads flow through
db.upsert_market and the labeler's event-date logic correctly.
(Live API calls are intentionally not made in tests.)"""

import json

from kalshi_mention import db
from kalshi_mention.labeler import market_event_date_et
from kalshi_mention.phrase import extract_phrase

FIXTURE_MARKET = {
    "ticker": "KXTRUMPMENTION-26FEB28-250",
    "event_ticker": "KXTRUMPMENTION-26FEB28",
    "market_type": "binary",
    "title": "What will Trump say during the State of the Union?",
    "yes_sub_title": "250",
    "no_sub_title": "Not 250",
    "rules_primary": "Resolves Yes if Donald Trump says 250 as part of 2026 State of the Union.",
    "open_time": "2026-02-20T15:00:00Z",
    "close_time": "2026-02-25T04:22:00Z",   # 11:22pm EST Feb 24 == 04:22 UTC Feb 25
    "expiration_time": "2026-02-25T05:00:00Z",
    "status": "settled",
    "result": "yes",
    "last_price": 97,
    "volume": 12345,
    "liquidity": 55555,
}


def test_upsert_and_series_derivation():
    conn = db.connect(":memory:")
    db.upsert_market(conn, FIXTURE_MARKET, fetched_at="2026-03-01T00:00:00+00:00")
    row = conn.execute("SELECT * FROM markets").fetchone()
    assert row["ticker"] == "KXTRUMPMENTION-26FEB28-250"
    assert row["series_ticker"] == "KXTRUMPMENTION"  # derived from event ticker
    assert row["result"] == "yes"
    assert json.loads(row["raw_json"])["yes_sub_title"] == "250"


def test_phrase_extraction_from_fixture():
    phrase, method = extract_phrase(FIXTURE_MARKET)
    assert phrase == "250" and method == "yes_sub_title"


def test_event_date_uses_strike_date_when_present():
    market_row = {"close_time": FIXTURE_MARKET["close_time"],
                  "expiration_time": FIXTURE_MARKET["expiration_time"]}
    event_row = {"strike_date": "2026-02-24T21:00:00Z"}  # 4pm ET Feb 24
    assert market_event_date_et(market_row, event_row) == "2026-02-24"


def test_event_date_falls_back_to_close_time_in_et():
    # close 04:22 UTC Feb 25 == 11:22pm ET Feb 24 -> ET calendar date must be Feb 24,
    # matching how FactBase dates the transcript. This is the exact off-by-one-day
    # bug the ET conversion exists to prevent.
    market_row = {"close_time": FIXTURE_MARKET["close_time"],
                  "expiration_time": FIXTURE_MARKET["expiration_time"]}
    event_row = {"strike_date": None}
    assert market_event_date_et(market_row, event_row) == "2026-02-24"


def test_event_date_none_when_nothing_parseable():
    assert market_event_date_et(
        {"close_time": None, "expiration_time": None}, {"strike_date": None}
    ) is None
