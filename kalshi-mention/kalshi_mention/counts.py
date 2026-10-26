"""Precompute phrase-spec occurrence counts for every (spec, document) pair.

    python -m kalshi_mention.counts --db kalshi_mention.sqlite \
        --transcripts trump_transcripts_merged.parquet

Component A needs, for every phrase Kalshi has ever listed, its occurrence
count in every historical document. Doing this naively (full token-sequence
scan per spec per doc) is O(specs x docs x doc_length); instead each document
is tokenized once into a token->positions index, making single-token variants
O(1) lookups and multi-token variants O(occurrences of first token).

Only nonzero counts are stored; absent rows mean zero.
"""

from __future__ import annotations

import argparse
import json
from collections import defaultdict

import pandas as pd

from . import config, db
from .phrase import (
    MatchFlags,
    _last_token_alternatives,
    _phrase_token_variants,
    extract_phrase,
    parse_phrase_spec,
    spec_key,
    tokenize,
)


def spec_variant_token_lists(raw_phrase: str) -> tuple[list[list[str]], int]:
    """All token-sequence variants that count for this spec, plus threshold."""
    alts, k = parse_phrase_spec(raw_phrase)
    variants: list[list[str]] = []
    seen: set[tuple[str, ...]] = set()
    for alt in alts:
        for v in _phrase_token_variants(alt):
            t = tuple(v)
            if t not in seen:
                seen.add(t)
                variants.append(v)
    return variants, k


def count_in_index(
    variants: list[list[str]],
    positions: dict[str, list[int]],
    tokens: list[str],
) -> int:
    total = 0
    flags = MatchFlags()
    for v in variants:
        if len(v) == 1:
            for alt in _last_token_alternatives(v[0], flags):
                total += len(positions.get(alt, ()))
        else:
            head, last_alts = v[:-1], _last_token_alternatives(v[-1], flags)
            n = len(v)
            for i in positions.get(v[0], ()):
                if i + n <= len(tokens) and tokens[i + 1 : i + n - 1] == head[1:] and tokens[i + n - 1] in last_alts:
                    total += 1
    return total


def build_counts(db_path: str, transcripts_path: str | None = None) -> None:
    conn = db.connect(db_path)

    # Ensure transcripts are loaded (idempotent if labeler already ran).
    n_docs = conn.execute("SELECT COUNT(*) c FROM transcripts").fetchone()["c"]
    if n_docs == 0:
        from .labeler import load_transcripts
        n_docs = load_transcripts(conn, transcripts_path)
    print(f"Transcripts: {n_docs}")

    # Distinct specs across all settled per-event markets.
    placeholders = ",".join("?" * len(config.LABELING_EXCLUDED_SERIES)) or "''"
    markets = conn.execute(
        f"""SELECT ticker, raw_json FROM markets
            WHERE result IN ('yes','no') AND series_ticker NOT IN ({placeholders})""",
        list(config.LABELING_EXCLUDED_SERIES),
    ).fetchall()

    specs: dict[str, tuple[list[list[str]], int, str]] = {}
    for m in markets:
        market_obj = json.loads(m["raw_json"])
        phrase, method = extract_phrase(market_obj)
        if not phrase:
            continue
        sk = spec_key(phrase)
        if sk not in specs:
            variants, k = spec_variant_token_lists(phrase)
            specs[sk] = (variants, k, phrase)
    print(f"Distinct phrase specs: {len(specs)} (from {len(markets)} settled markets)")

    conn.execute("DELETE FROM phrase_doc_counts")
    conn.execute("DELETE FROM phrase_doc_counts_status")
    docs = conn.execute("SELECT doc_id, text_raw FROM transcripts").fetchall()
    inserted = 0
    for di, d in enumerate(docs):
        tokens = tokenize(d["text_raw"])
        positions: dict[str, list[int]] = defaultdict(list)
        for i, t in enumerate(tokens):
            positions[t].append(i)
        rows = []
        for sk, (variants, _k, _p) in specs.items():
            c = count_in_index(variants, positions, tokens)
            if c > 0:
                rows.append((sk, d["doc_id"], c))
        if rows:
            conn.executemany("INSERT INTO phrase_doc_counts VALUES (?,?,?)", rows)
            inserted += len(rows)
        conn.execute(
            "INSERT INTO phrase_doc_counts_status (doc_id, computed_at) VALUES (?,?)",
            (d["doc_id"], db.utcnow_iso()),
        )
        if (di + 1) % 500 == 0:
            conn.commit()
            print(f"  {di + 1}/{len(docs)} docs ({inserted} nonzero counts)")
    conn.commit()
    print(f"Done: {inserted} nonzero (spec, doc) counts stored.")
    conn.close()


def update_missing_doc_counts(db_path: str) -> int:
    """Atomically compute phrase counts for transcript rows not yet indexed."""
    conn = db.connect(db_path)
    try:
        conn.execute("BEGIN")
        # Existing count rows prove those documents were processed by the
        # pre-status-table full builder. Documents with zero hits are safely
        # recomputed once because they have no such rows to migrate.
        conn.execute(
            """INSERT OR IGNORE INTO phrase_doc_counts_status (doc_id, computed_at)
               SELECT DISTINCT doc_id, ? FROM phrase_doc_counts""",
            (db.utcnow_iso(),),
        )
        docs = conn.execute(
            """SELECT t.doc_id, t.text_raw FROM transcripts t
               LEFT JOIN phrase_doc_counts_status s ON s.doc_id = t.doc_id
               WHERE s.doc_id IS NULL ORDER BY t.doc_id"""
        ).fetchall()
        if not docs:
            conn.commit()
            return 0

        placeholders = ",".join("?" * len(config.LABELING_EXCLUDED_SERIES)) or "''"
        markets = conn.execute(
            f"""SELECT raw_json FROM markets
                WHERE result IN ('yes','no') AND series_ticker NOT IN ({placeholders})""",
            list(config.LABELING_EXCLUDED_SERIES),
        ).fetchall()
        specs: dict[str, tuple[list[list[str]], int]] = {}
        for market in markets:
            phrase, _ = extract_phrase(json.loads(market["raw_json"]))
            if phrase:
                key = spec_key(phrase)
                if key not in specs:
                    variants, threshold = spec_variant_token_lists(phrase)
                    specs[key] = (variants, threshold)

        computed_at = db.utcnow_iso()
        for doc in docs:
            tokens = tokenize(doc["text_raw"])
            positions: dict[str, list[int]] = defaultdict(list)
            for index, token in enumerate(tokens):
                positions[token].append(index)
            rows = [
                (key, doc["doc_id"], count)
                for key, (variants, _threshold) in specs.items()
                if (count := count_in_index(variants, positions, tokens)) > 0
            ]
            if rows:
                conn.executemany("INSERT OR REPLACE INTO phrase_doc_counts VALUES (?,?,?)", rows)
            conn.execute(
                "INSERT INTO phrase_doc_counts_status (doc_id, computed_at) VALUES (?,?)",
                (doc["doc_id"], computed_at),
            )
        conn.commit()
        return len(docs)
    except Exception:
        conn.rollback()
        raise
    finally:
        conn.close()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--transcripts", default=None,
                    help="corpus parquet/CSV; default: auto-resolved (see paths.py)")
    args = ap.parse_args()
    build_counts(args.db, args.transcripts)


if __name__ == "__main__":
    main()
