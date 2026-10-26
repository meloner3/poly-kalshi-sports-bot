"""Per-event transcript mapping (pass 2, workstream 1).

Maps each Kalshi mention event to the specific same-day FactBase document it
refers to, replacing the same-day-union join that produced ~14% false
kalshi_no_transcript_yes disagreements in the pass-1 audit (209/211 of those
were on multi-document days).

    python -m kalshi_mention.event_mapper --db kalshi_mention.sqlite \
        --overrides event_overrides.csv        # optional

Mapping strategy, in order of trust:
  1. override CSV (event_ticker,doc_id) — always wins.
  2. only_doc  — exactly one transcript exists that day.
  3. auto_title — keyword overlap between the event's descriptor (the "X" in
     "What will Trump say during X?") and transcript titles; accepted only
     with a clear score and margin, never guessed.
Unmapped events stay on the union path in the labeler (explicit fallback,
reported, not silent).
"""

from __future__ import annotations

import argparse
import csv
import re
from collections import defaultdict

from . import config, db
from .labeler import market_event_date_et
from . import paths

_STOP = {
    "the", "a", "an", "his", "her", "their", "at", "in", "on", "of", "for",
    "to", "with", "and", "or", "will", "trump", "donald", "say", "during",
    "what", "remarks", "speech", "event", "press", "conference", "gaggle",
    "interview", "speaks", "delivers", "holds", "attends", "addresses",
    "reporters", "before", "after", "2026", "2025",
}

_DURING_RE = re.compile(r"during\s+(?:his\s+|her\s+|the\s+|a\s+|an\s+)?(.+?)\s*\??$", re.IGNORECASE)
_ASPART_RE = re.compile(r"as part of\s+(?:the\s+|a\s+|an\s+)?(.+?)\s*[.?]", re.IGNORECASE)
_WORD_RE = re.compile(r"[a-z0-9']+")


def keywords(text: str) -> set[str]:
    return {w for w in _WORD_RE.findall(text.casefold()) if w not in _STOP and len(w) > 1}


def event_descriptor(event_title: str | None, market_title: str | None, rules: str | None) -> str:
    for source in (event_title, market_title):
        if source:
            m = _DURING_RE.search(source)
            if m:
                return m.group(1)
    if rules:
        m = _ASPART_RE.search(rules)
        if m:
            return m.group(1)
    return ""


def score_candidate(descriptor: str, doc_title: str) -> float:
    dkw = keywords(descriptor)
    tkw = keywords(doc_title)
    if not dkw:
        return 0.0
    overlap = len(dkw & tkw)
    score = float(overlap)
    if descriptor and descriptor.casefold() in doc_title.casefold():
        score += 2.0
    return score


def build_mapping(db_path: str, overrides_path: str | None = None,
                  transcripts_path: str | None = None) -> None:
    conn = db.connect(db_path)
    n_docs = conn.execute("SELECT COUNT(*) c FROM transcripts").fetchone()["c"]
    if n_docs == 0:
        # Empty table: load a corpus rather than failing. With no explicit
        # --transcripts, paths.corpus_path() finds one (and reports the search
        # list if it can't).
        from .labeler import load_transcripts
        load_transcripts(conn, transcripts_path)
    computed_at = db.utcnow_iso()
    conn.execute("DELETE FROM event_doc_map")

    overrides: dict[str, str] = {}
    if overrides_path:
        with open(overrides_path, newline="", encoding="utf-8") as f:
            for row in csv.DictReader(f):
                overrides[row["event_ticker"].strip()] = row["doc_id"].strip()

    # One representative market per event gives us ticker-encoded date + titles.
    rows = conn.execute(
        f"""SELECT m.event_ticker, MIN(m.ticker) AS ticker, MIN(m.close_time) AS close_time,
                   MIN(m.expiration_time) AS expiration_time,
                   MIN(m.title) AS market_title, MIN(m.rules_primary) AS rules_primary,
                   e.title AS event_title, e.strike_date
            FROM markets m LEFT JOIN events e ON m.event_ticker = e.event_ticker
            WHERE m.result IN ('yes','no')
              AND m.series_ticker NOT IN ({','.join('?' * len(config.LABELING_EXCLUDED_SERIES)) or "''"})
            GROUP BY m.event_ticker""",
        list(config.LABELING_EXCLUDED_SERIES),
    ).fetchall()

    stats = defaultdict(int)
    unmapped_multi: list[tuple[str, str, int]] = []

    for ev in rows:
        event_date = market_event_date_et(ev, {"strike_date": ev["strike_date"]})
        if not event_date:
            stats["no_event_date"] += 1
            continue
        docs = conn.execute(
            "SELECT doc_id, title FROM transcripts WHERE date = ?", (event_date,)
        ).fetchall()
        if not docs:
            stats["no_same_day_doc"] += 1
            continue

        if ev["event_ticker"] in overrides:
            conn.execute(
                "INSERT INTO event_doc_map VALUES (?,?,?,?,?)",
                (ev["event_ticker"], overrides[ev["event_ticker"]], "override", None, computed_at),
            )
            stats["override"] += 1
            continue

        if len(docs) == 1:
            conn.execute(
                "INSERT INTO event_doc_map VALUES (?,?,?,?,?)",
                (ev["event_ticker"], docs[0]["doc_id"], "only_doc", None, computed_at),
            )
            stats["only_doc"] += 1
            continue

        descriptor = event_descriptor(ev["event_title"], ev["market_title"], ev["rules_primary"])
        scored = sorted(
            ((score_candidate(descriptor, d["title"] or ""), d["doc_id"]) for d in docs),
            reverse=True,
        )
        best_score, best_doc = scored[0]
        second_score = scored[1][0] if len(scored) > 1 else 0.0
        # Accept only with clear signal AND clear margin — never guess.
        if best_score >= 2.0 and best_score > second_score:
            conn.execute(
                "INSERT INTO event_doc_map VALUES (?,?,?,?,?)",
                (ev["event_ticker"], best_doc, "auto_title", best_score, computed_at),
            )
            stats["auto_title"] += 1
        else:
            stats["unmapped_multi_doc"] += 1
            unmapped_multi.append((ev["event_ticker"], descriptor, len(docs)))

    conn.commit()

    total = len(rows)
    print(f"Events considered: {total}")
    for k in ("override", "only_doc", "auto_title", "unmapped_multi_doc", "no_same_day_doc", "no_event_date"):
        if stats[k]:
            print(f"  {k}: {stats[k]}")
    if unmapped_multi:
        print("\nUnmapped multi-doc events (labeler falls back to same-day union;")
        print("add rows to the override CSV to map them):")
        for et, desc, n in unmapped_multi[:15]:
            print(f"  {et}  descriptor={desc!r}  candidates={n}")
        if len(unmapped_multi) > 15:
            print(f"  ... and {len(unmapped_multi) - 15} more")
    conn.close()



def list_unmapped(db_path: str) -> None:
    """Print every unmapped multi-doc event with its candidate documents and
    write event_overrides_template.csv for hand-filling."""
    conn = db.connect(db_path)
    rows = conn.execute(
        f"""SELECT m.event_ticker, MIN(m.ticker) AS ticker, MIN(m.close_time) AS close_time,
                   MIN(m.expiration_time) AS expiration_time,
                   MIN(m.title) AS market_title, MIN(m.rules_primary) AS rules_primary,
                   e.title AS event_title, e.strike_date
            FROM markets m LEFT JOIN events e ON m.event_ticker = e.event_ticker
            WHERE m.result IN ('yes','no')
              AND m.series_ticker NOT IN ({','.join('?' * len(config.LABELING_EXCLUDED_SERIES)) or "''"})
              AND m.event_ticker NOT IN (SELECT event_ticker FROM event_doc_map)
            GROUP BY m.event_ticker""",
        list(config.LABELING_EXCLUDED_SERIES),
    ).fetchall()
    template = []
    for ev in rows:
        event_date = market_event_date_et(ev, {"strike_date": ev["strike_date"]})
        if not event_date:
            continue
        docs = conn.execute("SELECT doc_id, title FROM transcripts WHERE date = ?", (event_date,)).fetchall()
        if len(docs) < 2:
            continue
        descriptor = event_descriptor(ev["event_title"], ev["market_title"], ev["rules_primary"])
        print(f"\n{ev['event_ticker']}  ({event_date})  descriptor={descriptor!r}")
        for d in docs:
            print(f"    {d['doc_id'][:12]}...  {d['title'][:90]}")
        template.append((ev["event_ticker"], docs))
    template_path = paths.data_path("event_overrides_template.csv")
    with open(template_path, "w", newline="", encoding="utf-8") as f:
        w = csv.writer(f)
        w.writerow(["event_ticker", "doc_id"])
        for et, docs in template:
            w.writerow([et, ""])
    print(f"\nWrote {template_path} ({len(template)} rows).")
    print("Fill doc_id with the FULL id of the correct doc (query it with:")
    print(f"  sqlite3 {paths.db_path()} \"SELECT doc_id,title FROM transcripts WHERE date='YYYY-MM-DD'\")")
    print("then rerun: event_mapper --db ... --overrides event_overrides_template.csv")
    conn.close()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--overrides", default=None, help="CSV: event_ticker,doc_id")
    ap.add_argument("--transcripts", default=None, help="parquet, only needed if transcripts not yet loaded")
    ap.add_argument("--list-unmapped", action="store_true",
                    help="print unmapped multi-doc events with candidate docs; write override template")
    args = ap.parse_args()
    if args.list_unmapped:
        list_unmapped(args.db)
    else:
        build_mapping(args.db, args.overrides, args.transcripts)


if __name__ == "__main__":
    main()
