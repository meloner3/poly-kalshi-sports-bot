"""News salience via GDELT (Component B's external signal).

    python -m kalshi_mention.news --db kalshi_mention.sqlite

For every distinct phrase Kalshi has listed, fetches a daily article-volume
timeline from the free GDELT DOC 2.0 API (one request per phrase, covering the
whole market history) and caches it in the news_volume table. Component B then
reads ONLY days strictly before each market's open — the cache stores the full
series, but point-in-time discipline is enforced at feature time.

Notes:
- Volume is GDELT's normalized share of global coverage mentioning the query.
- Generic phrases ("Great", "Beautiful") produce meaningless salience; the
  downstream model learns near-zero weight for them, and a proper-noun flag is
  provided so it can gate the feature.
- Failures are per-phrase and non-fatal; rerun to fill gaps (cached phrases
  are skipped unless --refresh).
"""

from __future__ import annotations

import argparse
import json
import time
from datetime import datetime, timedelta, timezone

import requests

from . import config, db
from .phrase import extract_phrase, parse_phrase_spec, spec_key
from .component_b import looks_proper_noun

GDELT_URL = "https://api.gdeltproject.org/api/v2/doc/doc"
UA = "kalshi-mention-research/1.0"


def phrase_query(raw_phrase: str) -> str:
    alts, _k = parse_phrase_spec(raw_phrase)
    q = alts[0].strip()
    return f'"{q}"' if " " in q else q


def fetch_timeline(query: str, start: str, end: str, timeout: int = 30) -> dict[str, float]:
    """Daily volume series {YYYY-MM-DD: value} from GDELT timelinevol."""
    r = requests.get(GDELT_URL, params={
        "query": query, "mode": "timelinevol", "format": "json",
        "startdatetime": start.replace("-", "") + "000000",
        "enddatetime": end.replace("-", "") + "235959",
    }, headers={"User-Agent": UA}, timeout=timeout)
    r.raise_for_status()
    payload = r.json()
    out: dict[str, float] = {}
    for series in payload.get("timeline", []):
        for pt in series.get("data", []):
            d = str(pt.get("date", ""))[:8]
            if len(d) == 8:
                out[f"{d[:4]}-{d[4:6]}-{d[6:8]}"] = float(pt.get("value", 0.0))
    return out


def run(db_path: str, start: str | None = None, refresh: bool = False) -> None:
    conn = db.connect(db_path)
    placeholders = ",".join("?" * len(config.LABELING_EXCLUDED_SERIES)) or "''"
    markets = conn.execute(
        f"""SELECT raw_json FROM markets
            WHERE result IN ('yes','no') AND series_ticker NOT IN ({placeholders})""",
        list(config.LABELING_EXCLUDED_SERIES),
    ).fetchall()
    specs: dict[str, str] = {}
    for m in markets:
        phrase, _ = extract_phrase(json.loads(m["raw_json"]))
        if phrase:
            specs.setdefault(spec_key(phrase), phrase)
    print(f"Distinct phrases: {len(specs)}")

    if start is None:
        row = conn.execute("SELECT MIN(open_time) m FROM markets WHERE result IN ('yes','no')").fetchone()
        start = (str(row["m"])[:10] if row["m"] else "2025-10-01")
        start = (datetime.fromisoformat(start) - timedelta(days=120)).date().isoformat()
    end = datetime.now(timezone.utc).date().isoformat()

    have = {r["spec_key"] for r in conn.execute("SELECT DISTINCT spec_key FROM news_volume")}
    todo = [(sk, ph) for sk, ph in specs.items()
            if (refresh or sk not in have) and looks_proper_noun(ph)]
    skipped_common = sum(1 for ph in specs.values() if not looks_proper_noun(ph))
    print(f"Skipping {skipped_common} common-word phrases (news feature is proper-noun gated)")
    print(f"Fetching {len(todo)} phrase timelines ({start} -> {end}); cached: {len(have)}")

    ok = failed = 0
    backoff, streak = 6.0, 0
    for i, (sk, ph) in enumerate(todo):
        q = phrase_query(ph)
        series = None
        for attempt in range(6):
            try:
                series = fetch_timeline(q, start, end)
                backoff = max(6.0, backoff * 0.7)
                break
            except requests.HTTPError as e:
                if e.response is not None and e.response.status_code == 429:
                    wait = backoff * (2 ** attempt)
                    print(f"  429 on {q!r} — backing off {wait:.0f}s (attempt {attempt + 1}/6)")
                    time.sleep(wait)
                    continue
                print(f"  ! {q!r}: {e}")
                break
            except (requests.RequestException, ValueError) as e:
                print(f"  ! {q!r}: {e}")
                time.sleep(5.0)
                break
        if series is None:
            failed += 1
            streak += 1
            if streak >= 3:
                print("  3 phrases blocked in a row — sleeping 30 min before continuing")
                time.sleep(1800)
                streak = 0
            continue
        streak = 0
        conn.execute("DELETE FROM news_volume WHERE spec_key = ?", (sk,))
        conn.executemany("INSERT INTO news_volume VALUES (?,?,?)",
                         [(sk, d, v) for d, v in series.items()])
        ok += 1
        if (i + 1) % 20 == 0:
            conn.commit()
            print(f"  {i + 1}/{len(todo)} ({ok} ok, {failed} failed)")
        time.sleep(20.0)
    conn.commit()
    n = conn.execute("SELECT COUNT(*) c FROM news_volume").fetchone()["c"]
    print(f"Done: {ok} fetched, {failed} failed; {n} (phrase, day) rows cached.")
    conn.close()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--start", default=None, help="YYYY-MM-DD (default: 120d before first market)")
    ap.add_argument("--refresh", action="store_true", help="refetch cached phrases too")
    args = ap.parse_args()
    run(args.db, args.start, args.refresh)


if __name__ == "__main__":
    main()
