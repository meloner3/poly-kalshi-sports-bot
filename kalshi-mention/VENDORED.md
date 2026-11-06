# Vendored: kalshi_mention

This directory is a copy of the `kalshi_mention` research package, taken from
its home repo and brought into this one so the arbitrage bot can consume its
forecasts directly.

| | |
|---|---|
| **Upstream repo** | `~/Desktop/Git/DelphiResearch` |
| **Upstream path** | `DelphiResearch/kalshi_mention/` |
| **Commit** | `88f9fb61f8c2b93a6382a3b32651426b04e0e086` |
| **Commit date** | 2026-09-10 — *T9: rewrite CLAUDE.md conclusions to match what is actually defensible* |
| **Vendored on** | 2026-09-10 |
| **Upstream tree state** | clean (no uncommitted changes to `kalshi_mention/`) |

Upstream remains the place this code is *developed*. This copy exists to be
*run*. Changes made here should be carried back upstream, or they will be lost
the next time the package is re-vendored.

## What the package is

It forecasts P(Trump says phrase X during event Y) and tests that forecast
against Kalshi's own market prices. Read [CLAUDE.md](CLAUDE.md) before changing
anything — it is the institutional memory, and it records four false edges the
project's own verdict apparatus has already caught.

## Read this before trusting a number

The package's recorded verdict is **NOT TRADEABLE**. No cell passes under
either entry rule, and the best-powered cell has an 80%-power minimum
detectable effect of **~5.6¢/contract on 40 qualifying event-days** — an edge
smaller than that is invisible to this apparatus. `CLAUDE.md` additionally
lists "live trading integration" under **Do NOT do**.

This repo wires the forecast into a live execution path anyway. That was a
deliberate decision by the repo owner, made with the above on the record. The
UI surfaces the staleness and the MDE caveat next to every number for the same
reason.

## Changes made during vendoring

Kept to the minimum needed to run outside DelphiResearch. Everything else is
byte-identical to upstream.

1. **`kalshi_mention/paths.py`** — the package used to sit directly under its
   repo root, so `REPO_ROOT = PACKAGE_DIR.parent`. Here it is one level
   deeper. Added `VENDOR_ROOT` (this directory) and repointed `REPO_ROOT` at
   the actual repo root, so `DEFAULT_DATA_DIR` still resolves to
   `<repo root>/data/kalshi_mention`.

2. **`kalshi_mention/verdict.py`** — `prereg_start_date()` derived the
   pre-registration's first evaluable day from `git log PREREGISTRATION.md`.
   Vendoring gives that file a new git history, so deriving from it would have
   silently **re-frozen a frozen experiment at the vendoring date**. The
   upstream freeze date (2026-09-10, first evaluable day 2026-09-11) is now
   pinned as `PREREG_FROZEN_DATE`; git is consulted only to detect an earlier
   edit in another lineage.

3. **Layout** — `tests/`, `scripts/`, and the markdown docs were moved from
   inside the importable package up to this directory, which is where they
   already logically belonged. No file contents changed.

4. **`conftest.py`** — new. Puts this directory on `sys.path` so
   `import kalshi_mention` resolves; upstream got that free from pytest's
   rootdir.

5. **`requirements.txt`** — new, trimmed from the upstream repo-wide file.
   Dropped `jupyterlab` and `pymongo` (unused by this package — it is
   sqlite+parquet only). `torch`/`transformers` stay commented out, as upstream.

6. **`PREREGISTRATION.md`** — copied in from the DelphiResearch repo root,
   since `verdict.py --prereg` refers to it and it is part of this project
   rather than of its old host repo.

### Known degradation

`paths.corpus_path()` has a fallback chain ending at
`<repo root>/trump_transcripts_clean.parquet`, a DelphiResearch sibling
artifact that does not exist here. Harmless — the primary candidate,
`data/kalshi_mention/trump_transcripts_merged.parquet`, is present — but a
corpus rebuild in this repo has one fewer fallback than upstream.

## Data

Code is vendored; data is not committed. The data root is
`<repo root>/data/kalshi_mention/`, gitignored, overridable with
`$KALSHI_MENTION_DATA_DIR`. See the [README](README.md) for what lives there.

The sqlite database represents many hours of rate-limited API calls. Copies
exist at `DelphiResearch/data/kalshi_mention/` and `~/Desktop/kalshi_data/`.
This repo holds a third. None of them is off-machine.

## Running it

```bash
python -m pytest kalshi-mention/tests -q          # from the repo root
python -m kalshi_mention.predict --all-open       # needs kalshi-mention/ on sys.path
```

## Verification performed at vendoring (2026-09-10)

| check | result |
|---|---|
| Test suite | **104 passed** — matches the count upstream's CLAUDE.md records |
| Data copy integrity | SHA-256 match on all 4 data files; 26/26 files present; source tree untouched |
| Path resolution | `data_dir()` → `<repo root>/data/kalshi_mention`, `db_path().exists()` → True |
| End-to-end | `predict --phrase "China / Chinese" --event-type interview` → calibration fitted on **8,260** resolved markets, matching CLAUDE.md |
| **Output parity vs upstream** | **identical**, at a pinned clock, across 4 phrase/event-type cases + the calibration fit |
| Git hygiene | data root ignored; largest tracked file 29 KB |

Parity is re-runnable — re-run it after any re-vendoring:

```bash
kalshi-mention/.venv/bin/python kalshi-mention/scripts/vendor_parity.py
```

The clock has to be pinned because `predict_phrase()` measures document age
from `now()`. Two unpinned runs seconds apart differ in the last decimal
places (a 43-second gap moved `p` by 4e-8), which is upstream behaviour, not
drift introduced here.

### Dataset identity at vendoring

Hard Rule 7 says two reports are comparable only if their dataset identity
matches. This copy's:

```
kalshi_mention.sqlite   726,581,248 bytes   mtime 2026-08-18
  transcripts         4,955 rows
  markets             9,835 rows
  events                367 rows
  phrase_doc_counts 443,830 rows
corpus  trump_transcripts_merged.parquet   50,129,950 bytes
```

### Corpus staleness — watch this

**35.1 days** at vendoring. Recency weights are already at **44.5%** of fresh.
`predict.py` warns at 45 days and says the numbers should not be used at 120;
this corpus crosses the warn threshold around **2026-09-20** without a scrape.

A stale corpus degrades silently — every probability drifts toward 0.5 with no
error raised. Nothing about the output looks wrong while it happens. Refresh
with:

```bash
kalshi-mention/.venv/bin/python -m kalshi_mention.pipeline --only scrape
```
