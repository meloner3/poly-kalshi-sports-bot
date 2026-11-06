# kalshi_mention

Forecasts P(Trump says phrase X during event Y), tests the forecast against
Kalshi's own market prices, and renders a statistically honest verdict on
tradeability.

Before changing anything, read [CLAUDE.md](CLAUDE.md) — the hard rules, the
research log, and the four false edges this project's verdict apparatus has
already caught.

## Where things live

Code is in git. Data is not.

```
DelphiResearch/
  kalshi_mention/              <- this package (tracked)
    paths.py                   <- the only place that knows about locations
    tests/                     <- pytest suite
    scripts/                   <- one-off patch/analysis scripts kept for history
  data/kalshi_mention/         <- DATA ROOT (gitignored)
    kalshi_mention.sqlite              ~700MB: markets, events, candles,
                                       transcripts, labels, counts, news
    trump_transcripts_merged.parquet   the corpus
    recent_transcripts.jsonl           scraper staging
    audit/ component_a/ component_b/ component_c/
    unified/ market_baseline/ verdict/ <- every report and predictions CSV
```

Nothing resolves against the current working directory. `paths.py` resolves
every path against the data root, so the commands below give identical
results from anywhere in the repo.

Override the data root with `KALSHI_MENTION_DATA_DIR`:

```bash
export KALSHI_MENTION_DATA_DIR=~/Desktop/kalshi_data     # e.g. run off the backup
```

## Restoring the committed data

The corpus and database are committed under `data/kalshi_mention/` (force-added,
because `data/` is gitignored): `trump_transcripts_merged.parquet` and the
xz-compressed `kalshi_mention.sqlite.xz` (702MB raw, 46MB compressed). Restore
the database from the repository root with:

```sh
xz -dk data/kalshi_mention/kalshi_mention.sqlite.xz
(cd data/kalshi_mention && shasum -a 256 -c kalshi_mention.sqlite.sha256)
```

The decompressed `.sqlite` stays gitignored. Do not commit it: GitHub rejects
files over 100MB. Re-compress with `xz -9e -T0 -k` if the committed copy is
refreshed, and update the `.sha256` file.

## Running it

Use the repo venv (Python 3.11) from the repo root — **not** anaconda `base`,
which has a broken NumPy 1/2 mix:

```bash
cd ~/Desktop/Git/DelphiResearch
source .venv/bin/activate
pip install -r requirements.txt        # first time only

python -m kalshi_mention.pipeline                 # everything (scrape -> verdict)
python -m kalshi_mention.pipeline --only analyze  # recompute, no network
python -m kalshi_mention.refresh_corpus           # scrape and sync predictor SQLite
python -m kalshi_mention.signal_service           # localhost signal HTTP service
python -m kalshi_mention.predict --phrase "China / Chinese" --event-type interview
python -m kalshi_mention.predict --all-open       # score open Kalshi markets
python -m pytest kalshi_mention/tests -q
```

On macOS, install the daily 06:00 local-time corpus refresh and the 06:15
directional paper-gate evaluation with
`kalshi-mention/.venv/bin/python kalshi-mention/scripts/install_scraper_launch_agent.py`.
Use `--hour 0..23` to choose another local hour, `--print` to inspect the
LaunchAgent definition without installing, or `--uninstall` to remove it.
The job refreshes FactBase transcripts, syncs them into SQLite, and atomically
indexes phrase counts for any newly available documents. The later evaluator
updates only `directional_evaluation/`; neither job rewrites frozen verdict
reports. Prediction abstains
when that SQLite corpus is missing or older than 14 days. Override the limit
with `KALSHI_MENTION_MAX_CORPUS_AGE_DAYS`.

The standalone signal service binds only to `127.0.0.1:8765`. Its `GET /signals`
response is a list of per-market records; `GET /health` reports predictor
health and the latest forward-paper gate artifact. Until that gate artifact is
produced, the verdict fields are null and the service remains degraded. Signal
evidence and interval settings can be overridden with `KALSHI_MENTION_SIGNAL_*`
environment variables; the Rust client should treat all abstentions and
unreachable responses as unavailable signals.
The live-gate evaluator reads the Rust database and writes only
`data/kalshi_mention/directional_evaluation/`. Run
`python -m kalshi_mention.directional_evaluation --watch` during paper trading
to refresh the gate every five minutes; the installed LaunchAgent runs a
one-shot report refresh daily.giv
The evaluator opens the Rust database read-only. The gate interval is the
two-sided 95% day-clustered bootstrap from `verdict.cluster_boot` (the same
method as the historical verdict, and stricter than a one-sided bound); the
gate requires `n_days >= 60` and `ci_low > 0`. `report.json` also carries
traded-side calibration bins and Brier scores, `by_day` results, and a
`fill_assumption` caveat: paper fills assume the fresh book with no queue or
impact, so realized edge is an upper bound.

Pipeline steps, in order: `scrape, collect, historical, map, label, audit,
counts, model, news, unified, baseline, verdict`. All idempotent and
resumable; `--skip` and `--only` take step names.

Reports to read, in order, under `data/kalshi_mention/`:
`audit/audit_summary.md`, `component_a/component_a_report.md`,
`unified/unified_report.md`, `market_baseline/market_baseline_report.md`,
`verdict/verdict_report.md`.

## How it connects to the rest of DelphiResearch

The two projects share a corpus. `trump_transcripts/prepare_trump_transcripts.py`
builds `trump_transcripts_clean.parquet` from Mongo; `kalshi_mention`'s own
`scrape_transcripts.py` grows `trump_transcripts_merged.parquet` incrementally
from FactBase. They emit the same schema, so either can feed the labeler.

`paths.corpus_path()` picks one, in this order:

1. an explicit `--transcripts` argument
2. `$KALSHI_MENTION_CORPUS`
3. `data/kalshi_mention/trump_transcripts_merged.parquet`
4. `trump_transcripts_clean.parquet` at the repo root
5. the `.csv` equivalents of 3 and 4

Whichever is chosen, `paths.check_corpus_schema()` verifies
`doc_id, text_raw, date` are present before a single row is inserted. The
`text_raw` requirement is not stylistic: occurrence labels are counted on raw
text, never on `text_clean`, or the counts stop matching what Kalshi resolved
against.

The two corpora are the same lineage — the merged parquet is the clean parquet
carried forward (4,729 docs to 2026-03-13, vs 4,955 docs to 2026-08-07), so
the merged one is the default.

The other shared surface is Component C: `component_c.py` runs a GPT-2 Monte
Carlo simulator over the same corpus that `trump_transcripts/gpt2_finetune.py`
fine-tunes on. Both need `torch` and `transformers` (already in the repo venv,
commented out of `requirements.txt` because they are heavy).

## Data safety

The data root is the only copy of a ~700MB database that took many hours of
rate-limited API calls to build. Back it up after every big run:

```bash
cp data/kalshi_mention/kalshi_mention.sqlite \
   data/kalshi_mention/trump_transcripts_merged.parquet \
   ~/Desktop/kalshi_data/
```

`.gitignore` covers `data/`, `*.sqlite` and `*.parquet` repo-wide. Do not
remove those rules — GitHub rejects files over 100MB, and a committed 700MB
blob is not easy to undo.
