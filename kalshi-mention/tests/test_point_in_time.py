"""Point-in-time correctness tests — the silent-bug source the spec calls out."""

import json

from kalshi_mention import db
from kalshi_mention.labeler import compute_available_at


def _mem():
    return db.connect(":memory:")


def _add_doc(conn, doc_id, date_iso, available_at, source="assumed_lag"):
    db.upsert_transcript(
        conn, doc_id=doc_id, title=doc_id, date_iso=date_iso, speech_type="Rally",
        text_raw="some text", token_count=2,
        available_at=available_at, availability_source=source,
    )


def test_as_of_excludes_future_transcripts():
    conn = _mem()
    _add_doc(conn, "early", "2025-01-01", "2025-01-02T05:00:00+00:00")
    _add_doc(conn, "late", "2025-01-05", "2025-01-06T05:00:00+00:00")
    rows = db.transcripts_as_of(conn, "2025-01-03T00:00:00+00:00")
    assert [r["doc_id"] for r in rows] == ["early"]


def test_as_of_boundary_inclusive():
    conn = _mem()
    _add_doc(conn, "exact", "2025-01-01", "2025-01-02T05:00:00+00:00")
    assert len(db.transcripts_as_of(conn, "2025-01-02T05:00:00+00:00")) == 1
    assert len(db.transcripts_as_of(conn, "2025-01-02T04:59:59+00:00")) == 0


def test_assumed_lag_availability_is_after_event_date():
    # Event on 2025-06-10 (ET); default lag 24h => available 2025-06-11 00:00 ET
    available, source = compute_available_at("2025-06-10", None)
    assert source == "assumed_lag"
    assert available > "2025-06-10T23:59:59"  # strictly after event day starts, UTC ISO ordering
    # It must be a valid UTC ISO timestamp
    assert available.endswith("+00:00")


def test_real_scraped_at_wins_over_assumption():
    available, source = compute_available_at("2025-06-10", "2025-06-10T21:15:00Z")
    assert source == "scraped_at"
    assert available == "2025-06-10T21:15:00+00:00"


def test_missing_date_and_scraped_at_never_available():
    available, source = compute_available_at(None, None)
    assert available.startswith("9999-")


def test_market_snapshot_as_of_returns_correct_version():
    conn = _mem()
    v1 = {"ticker": "KXTEST-1", "status": "open", "result": ""}
    v2 = {"ticker": "KXTEST-1", "status": "settled", "result": "yes"}
    db.upsert_market(conn, v1, fetched_at="2025-01-01T00:00:00+00:00")
    db.upsert_market(conn, v2, fetched_at="2025-02-01T00:00:00+00:00")

    # latest table shows settled...
    row = conn.execute("SELECT result FROM markets WHERE ticker='KXTEST-1'").fetchone()
    assert row["result"] == "yes"
    # ...but as-of mid-January the backtest must see the open version
    snap = db.market_snapshot_as_of(conn, "KXTEST-1", "2025-01-15T00:00:00+00:00")
    assert snap["status"] == "open" and snap["result"] == ""
    # and before any fetch, nothing
    assert db.market_snapshot_as_of(conn, "KXTEST-1", "2024-12-31T00:00:00+00:00") is None


def test_snapshots_are_append_only():
    conn = _mem()
    v1 = {"ticker": "KXTEST-2", "status": "open"}
    v2 = {"ticker": "KXTEST-2", "status": "settled", "result": "no"}
    db.upsert_market(conn, v1, fetched_at="2025-01-01T00:00:00+00:00")
    db.upsert_market(conn, v2, fetched_at="2025-02-01T00:00:00+00:00")
    n = conn.execute("SELECT COUNT(*) c FROM market_snapshots WHERE ticker='KXTEST-2'").fetchone()["c"]
    assert n == 2
    raws = [json.loads(r["raw_json"]) for r in conn.execute(
        "SELECT raw_json FROM market_snapshots WHERE ticker='KXTEST-2' ORDER BY fetched_at"
    )]
    assert raws[0]["status"] == "open" and raws[1]["status"] == "settled"
