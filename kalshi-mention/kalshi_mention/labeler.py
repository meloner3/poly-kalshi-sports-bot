"""Occurrence labeling: join settled mention markets to transcripts and
compute whether the target phrase occurs.

    python -m kalshi_mention.labeler --db kalshi_mention.sqlite \
        --transcripts trump_transcripts_clean.parquet \
        --overrides overrides.csv

Rules enforced here (not conventions — enforced):
  * Labels are computed from text_raw (the Trump-only FactBase text), never
    from the LM-cleaned text_clean, because cleaning destroys phrase fidelity.
  * A market with no parseable phrase and no override row is SKIPPED and
    reported, never guessed.
  * Market -> transcript matching is by ET calendar date of the event (the
    event's strike_date when present, else the market close_time converted to
    ET). All same-day transcripts are labeled; the audit works on the union
    ("did any same-day document contain the phrase") plus per-doc detail.
    Day-granularity is a known limitation of the corpus (no event times).
  * Every transcript row gets an explicit available_at. Real scraped_at is
    used when the corpus has it; otherwise the documented lag assumption from
    config applies and is marked as such.

Override CSV columns: ticker,phrase,allow_plural,allow_possessive,aliases
  (aliases = |-separated alternative strings that also count as the phrase)
"""

from __future__ import annotations

import argparse
import csv
import re
import json
from datetime import datetime, timedelta, timezone
from pathlib import Path
from zoneinfo import ZoneInfo

import pandas as pd

from . import config, db
from .phrase import MatchFlags, extract_phrase, phrase_occurs, phrase_spec_occurs
from . import paths

ET = ZoneInfo(config.EVENT_TIMEZONE)


# ---------------------------------------------------------------------------
# Transcript loading
# ---------------------------------------------------------------------------

def compute_available_at(date_iso: str | None, scraped_at: str | None) -> tuple[str, str]:
    """(available_at ISO-UTC, source). Real scraped_at wins; else assumption."""
    if scraped_at:
        try:
            dt = datetime.fromisoformat(str(scraped_at).replace("Z", "+00:00"))
            if dt.tzinfo is None:
                dt = dt.replace(tzinfo=timezone.utc)
            return dt.astimezone(timezone.utc).isoformat(timespec="seconds"), "scraped_at"
        except ValueError:
            pass
    if not date_iso:
        # No date at all: treat as never-available for point-in-time purposes
        # by pushing availability far into the future; such docs also can't be
        # matched to a market date, so they are effectively excluded.
        return "9999-01-01T00:00:00+00:00", "assumed_lag"
    d = datetime.fromisoformat(str(date_iso)[:10])
    midnight_et = d.replace(tzinfo=ET)
    available = midnight_et + timedelta(hours=config.ASSUMED_TRANSCRIPT_AVAILABILITY_LAG_HOURS)
    return available.astimezone(timezone.utc).isoformat(timespec="seconds"), "assumed_lag"


def load_transcripts(conn, path: str | None = None) -> int:
    """Load the corpus into the transcripts table.

    `path` may be omitted: paths.corpus_path() then picks between
    kalshi_mention's own merged parquet and DelphiResearch's
    trump_transcripts_clean.parquet (both are emitted by
    trump_transcripts/prepare_trump_transcripts.py's schema). Whichever is
    used, the columns are checked before a single row is inserted.
    """
    p = paths.corpus_path(path)
    df = pd.read_parquet(p) if p.suffix == ".parquet" else pd.read_csv(p)
    paths.check_corpus_schema(df, p)
    print(f"corpus: {p} ({len(df)} docs)")
    has_scraped = "scraped_at" in df.columns
    n = 0
    for _, row in df.iterrows():
        date_iso = str(row["date"])[:10] if pd.notna(row["date"]) else None
        available_at, source = compute_available_at(
            date_iso, str(row["scraped_at"]) if has_scraped and pd.notna(row.get("scraped_at")) else None
        )
        db.upsert_transcript(
            conn,
            doc_id=str(row["doc_id"]),
            title=str(row["title"]) if "title" in df.columns and pd.notna(row.get("title")) else None,
            date_iso=date_iso,
            speech_type=str(row["speech_type"]) if "speech_type" in df.columns and pd.notna(row.get("speech_type")) else None,
            text_raw=str(row["text_raw"]),
            token_count=int(row["token_count"]) if "token_count" in df.columns and pd.notna(row.get("token_count")) else None,
            available_at=available_at,
            availability_source=source,
        )
        n += 1
    conn.commit()
    return n


# ---------------------------------------------------------------------------
# Market -> event-date resolution
# ---------------------------------------------------------------------------

_TICKER_DATE_RE = re.compile(r"-(\d{2})(JAN|FEB|MAR|APR|MAY|JUN|JUL|AUG|SEP|OCT|NOV|DEC)(\d{2})[A-Z]?(?:-|$)")
_MONTHS = {"JAN":1,"FEB":2,"MAR":3,"APR":4,"MAY":5,"JUN":6,"JUL":7,"AUG":8,"SEP":9,"OCT":10,"NOV":11,"DEC":12}

def market_event_date_et(market_row, event_row) -> str | None:
    """ET calendar date (ISO) of the underlying event.

    Priority: date encoded in the ticker (authoritative for mention series;
    close_time can be days later because markets can close early on
    occurrence), then event strike_date, then close/expiration time in ET.
    """
    ticker = ""
    try:
        ticker = market_row["ticker"] or ""
    except (KeyError, IndexError):
        pass
    m = _TICKER_DATE_RE.search(ticker)
    if m:
        yy, mon, dd = m.groups()
        return f"20{yy}-{_MONTHS[mon]:02d}-{int(dd):02d}"
    for candidate in (
        event_row["strike_date"] if event_row is not None else None,
        market_row["close_time"],
        market_row["expiration_time"],
    ):
        if not candidate:
            continue
        try:
            dt = datetime.fromisoformat(str(candidate).replace("Z", "+00:00"))
        except ValueError:
            continue
        if dt.tzinfo is None:
            dt = dt.replace(tzinfo=timezone.utc)
        return dt.astimezone(ET).date().isoformat()
    return None


# ---------------------------------------------------------------------------
# Overrides
# ---------------------------------------------------------------------------

def load_overrides(path: str | None) -> dict[str, tuple[str, MatchFlags]]:
    overrides: dict[str, tuple[str, MatchFlags]] = {}
    if not path:
        return overrides
    with open(path, newline="", encoding="utf-8") as f:
        for row in csv.DictReader(f):
            flags = MatchFlags(
                allow_plural=str(row.get("allow_plural", "")).strip().lower() in ("1", "true", "yes"),
                allow_possessive=str(row.get("allow_possessive", "")).strip().lower() in ("1", "true", "yes"),
                aliases=[a for a in str(row.get("aliases", "")).split("|") if a.strip()],
            )
            overrides[row["ticker"].strip()] = (row["phrase"].strip(), flags)
    return overrides


# ---------------------------------------------------------------------------
# Labeling
# ---------------------------------------------------------------------------

def run_labeling(db_path: str, transcripts_path: str, overrides_path: str | None) -> None:
    conn = db.connect(db_path)

    n = load_transcripts(conn, transcripts_path)
    print(f"Loaded {n} transcripts (available_at source counts):")
    for r in conn.execute(
        "SELECT availability_source, COUNT(*) c FROM transcripts GROUP BY availability_source"
    ):
        print(f"  {r['availability_source']}: {r['c']}")

    conn.execute("DELETE FROM labels")  # stale rows from prior runs corrupt the audit union
    overrides = load_overrides(overrides_path)
    placeholders = ",".join("?" * len(config.LABELING_EXCLUDED_SERIES)) or "''"
    markets = conn.execute(
        f"""SELECT m.*, e.strike_date AS ev_strike_date
            FROM markets m LEFT JOIN events e ON m.event_ticker = e.event_ticker
            WHERE m.result IN ('yes','no')
              AND m.series_ticker NOT IN ({placeholders})""",
        list(config.LABELING_EXCLUDED_SERIES),
    ).fetchall()
    print(f"Settled per-event markets to label: {len(markets)}")

    computed_at = db.utcnow_iso()
    skipped_no_phrase: list[str] = []
    skipped_no_transcript: list[str] = []
    labeled = 0

    for m in markets:
        market_obj = json.loads(m["raw_json"])
        if m["ticker"] in overrides:
            phrase, flags = overrides[m["ticker"]]
            phrase_source = "override_csv"
        else:
            phrase, phrase_source = extract_phrase(market_obj)
            flags = MatchFlags()
        if not phrase:
            skipped_no_phrase.append(m["ticker"])
            continue

        ev_row = {"strike_date": m["ev_strike_date"]}
        event_date = market_event_date_et(m, ev_row)
        if not event_date:
            skipped_no_transcript.append(m["ticker"])
            continue

        mapped = conn.execute(
            "SELECT doc_id FROM event_doc_map WHERE event_ticker = ?", (m["event_ticker"],)
        ).fetchone()
        if mapped:
            docs = conn.execute(
                "SELECT doc_id, text_raw FROM transcripts WHERE doc_id = ?", (mapped["doc_id"],)
            ).fetchall()
        else:
            docs = conn.execute(
                "SELECT doc_id, text_raw FROM transcripts WHERE date = ?", (event_date,)
            ).fetchall()
        if not docs:
            skipped_no_transcript.append(m["ticker"])
            continue

        for d in docs:
            matched, count = phrase_spec_occurs(d["text_raw"], phrase, flags)
            conn.execute(
                """INSERT INTO labels (market_ticker, doc_id, phrase, phrase_source,
                                       matched, match_count, match_flags, computed_at)
                   VALUES (?,?,?,?,?,?,?,?)
                   ON CONFLICT(market_ticker, doc_id) DO UPDATE SET
                     phrase=excluded.phrase, phrase_source=excluded.phrase_source,
                     matched=excluded.matched, match_count=excluded.match_count,
                     match_flags=excluded.match_flags, computed_at=excluded.computed_at""",
                (m["ticker"], d["doc_id"], phrase, phrase_source,
                 int(matched), count, flags.to_json(), computed_at),
            )
        labeled += 1

    conn.commit()
    print(f"Labeled {labeled} markets against same-day transcripts.")
    if skipped_no_phrase:
        print(f"SKIPPED {len(skipped_no_phrase)} markets with no parseable phrase "
              f"(add override rows): {skipped_no_phrase[:10]}{'...' if len(skipped_no_phrase) > 10 else ''}")
    if skipped_no_transcript:
        print(f"SKIPPED {len(skipped_no_transcript)} markets with no same-day transcript "
              f"(coverage gap): {skipped_no_transcript[:10]}{'...' if len(skipped_no_transcript) > 10 else ''}")
    conn.close()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--transcripts", required=True,
                    help="trump_transcripts_clean.parquet (or .csv) from your pipeline")
    ap.add_argument("--overrides", default=None, help="per-market phrase override CSV")
    args = ap.parse_args()
    run_labeling(args.db, args.transcripts, args.overrides)


if __name__ == "__main__":
    main()
