"""Refresh FactBase transcripts and sync them into the predictor database."""

from __future__ import annotations

import argparse
import os

from . import config, counts, db, paths
from .labeler import load_transcripts
from .scrape_transcripts import run as scrape_transcripts


def sync_corpus_to_db(
    db_path: str = config.DEFAULT_DB_PATH,
    corpus_file: str | None = None,
) -> int:
    """Upsert the scraper's merged Parquet corpus into predictor SQLite."""
    conn = db.connect(db_path)
    try:
        corpus_path = corpus_file or str(paths.data_path(config.DEFAULT_CORPUS_NAME))
        count = load_transcripts(conn, corpus_path)
        print(f"Synced {count} transcript rows into {db_path}")
        return count
    finally:
        conn.close()


def run(
    db_path: str = config.DEFAULT_DB_PATH,
    corpus_file: str | None = None,
) -> int:
    target = corpus_file or os.environ.get(paths.ENV_CORPUS) or config.DEFAULT_CORPUS_NAME
    added = scrape_transcripts(target)
    synced = sync_corpus_to_db(db_path, str(paths.data_path(target)))
    indexed = counts.update_missing_doc_counts(db_path)
    if added or indexed:
        print(f"Indexed phrase counts for {indexed} previously unindexed transcripts")
    return synced


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", default=config.DEFAULT_DB_PATH)
    parser.add_argument("--corpus", default=None)
    args = parser.parse_args()
    run(args.db, args.corpus)


if __name__ == "__main__":
    main()