"""Filesystem layout: where code ends, where data begins.

This package lives inside the DelphiResearch git repo. Its data does not —
`kalshi_mention.sqlite` is ~700MB and the corpus parquet ~50MB, so both live
in a gitignored data directory and every path in the package resolves against
it instead of against the current working directory.

Resolution rules (`resolve`):

  absolute path        -> used as-is
  existing CWD-relative path -> used as-is (so old habits keep working: you can
                          still cd into a data directory and run the CLI there)
  anything else        -> <data_dir>/<name>

`data_dir()` is, in order of precedence:

  1. $KALSHI_MENTION_DATA_DIR
  2. <repo root>/data/kalshi_mention          (the default in this repo)

The corpus gets its own resolver because two projects in this repo produce
one: kalshi_mention's own `trump_transcripts_merged.parquet` (incrementally
grown by scrape_transcripts.py) and DelphiResearch's
`trump_transcripts_clean.parquet` (built from Mongo by
trump_transcripts/prepare_trump_transcripts.py). They share a schema, so
either can feed the labeler; see `corpus_path` for the search order.
"""

from __future__ import annotations

import os
from pathlib import Path

# Vendored into poly-kalshi-sports-bot (see ../VENDORED.md). Upstream this
# package sat directly under the DelphiResearch repo root, so REPO_ROOT was
# one level up. Here it sits one level deeper:
#   kalshi_mention/paths.py -> kalshi_mention/ -> kalshi-mention/ -> repo root
PACKAGE_DIR = Path(__file__).resolve().parent
VENDOR_ROOT = PACKAGE_DIR.parent
REPO_ROOT = VENDOR_ROOT.parent

ENV_DATA_DIR = "KALSHI_MENTION_DATA_DIR"
ENV_CORPUS = "KALSHI_MENTION_CORPUS"

DEFAULT_DATA_DIR = REPO_ROOT / "data" / "kalshi_mention"

# Columns the labeler needs from whichever corpus is used. `text_raw` is
# non-negotiable: labels are counted on raw text, never on cleaned text, or
# the occurrence counts stop matching what Kalshi resolved against.
REQUIRED_CORPUS_COLUMNS = ["doc_id", "text_raw", "date"]


def data_dir(create: bool = True) -> Path:
    """The directory holding the sqlite DB, the corpus and all run outputs."""
    env = os.environ.get(ENV_DATA_DIR)
    d = Path(env).expanduser() if env else DEFAULT_DATA_DIR
    if create:
        d.mkdir(parents=True, exist_ok=True)
    return d


def resolve(p: str | os.PathLike) -> Path:
    """Turn a path argument into a concrete location under the data dir."""
    q = Path(p).expanduser()
    if q.is_absolute():
        return q
    if q.exists():          # CWD-relative and already there: respect it
        return q.resolve()
    return data_dir() / q


def data_path(name: str | os.PathLike) -> Path:
    """Resolve an input file (DB, corpus, a prior step's predictions CSV)."""
    return resolve(name)


def out_dir(name: str | os.PathLike) -> Path:
    """Resolve an output directory and create it."""
    d = resolve(name)
    d.mkdir(parents=True, exist_ok=True)
    return d


def db_path() -> Path:
    """Default location of the sqlite database."""
    return data_dir() / "kalshi_mention.sqlite"


# ---------------------------------------------------------------------------
# Corpus resolution (the DelphiResearch integration point)
# ---------------------------------------------------------------------------

def corpus_candidates() -> list[Path]:
    """Every corpus this repo knows how to produce, best first.

    1. kalshi_mention's own merged parquet — the one scrape_transcripts.py
       grows incrementally and the one the research log's numbers were
       computed on.
    2/3. DelphiResearch's cleaned corpus from
       trump_transcripts/prepare_trump_transcripts.py, parquet then CSV.
    """
    d = data_dir(create=False)
    return [
        d / "trump_transcripts_merged.parquet",
        REPO_ROOT / "trump_transcripts_clean.parquet",
        d / "trump_transcripts_clean.parquet",
        REPO_ROOT / "trump_transcripts_clean.csv",
        d / "trump_transcripts_clean.csv",
    ]


def corpus_path(explicit: str | os.PathLike | None = None) -> Path:
    """Locate the transcript corpus.

    Order: explicit argument, then $KALSHI_MENTION_CORPUS, then the known
    candidates. Raises with the full search list rather than failing deep
    inside pandas.
    """
    if explicit:
        p = resolve(explicit)
        if not p.exists():
            raise SystemExit(f"corpus not found: {p}")
        return p

    env = os.environ.get(ENV_CORPUS)
    if env:
        p = Path(env).expanduser()
        if not p.exists():
            raise SystemExit(f"${ENV_CORPUS} points at a missing file: {p}")
        return p

    for c in corpus_candidates():
        if c.exists():
            return c

    listing = "\n  ".join(str(c) for c in corpus_candidates())
    raise SystemExit(
        "No transcript corpus found. Looked for:\n  " + listing +
        "\n\nProduce one with either:\n"
        "  python -m kalshi_mention.scrape_transcripts   (grows the merged parquet)\n"
        "  python trump_transcripts/prepare_trump_transcripts.py   (rebuilds from Mongo)\n"
        f"or point ${ENV_CORPUS} at an existing file."
    )


def check_corpus_schema(df, source: str | os.PathLike) -> None:
    """Fail loudly and usefully when a corpus is missing required columns.

    Both corpus producers in this repo emit these columns; a miss usually
    means someone passed a train/val split or a text_clean-only export.
    """
    missing = [c for c in REQUIRED_CORPUS_COLUMNS if c not in df.columns]
    if missing:
        raise SystemExit(
            f"Corpus {source} is missing columns {missing} "
            f"(need {REQUIRED_CORPUS_COLUMNS}).\n"
            "Expected the output of trump_transcripts/prepare_trump_transcripts.py "
            "or kalshi_mention/scrape_transcripts.py. Labels are counted on "
            "text_raw, never text_clean, so a cleaned-only export cannot be used."
        )
