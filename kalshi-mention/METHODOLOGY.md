# kalshi_mention — methodology and project log

Forecasts P(Trump says phrase X during event E), tests it against Kalshi's
own prices, and renders a statistically honest verdict on tradeability.

Setup, paths and commands now live in [README.md](README.md); the hard rules
and research findings live in [CLAUDE.md](CLAUDE.md). This file is the
methodology reference and the pass-by-pass project log.

Note: command lines below predate the move into DelphiResearch. They still
work, but file arguments like `kalshi_mention.sqlite` or
`component_a/component_a_predictions.csv` now resolve against
`data/kalshi_mention/` rather than the working directory (see `paths.py`), and
in most cases can simply be omitted.

## Reading the outputs (in order)

1. audit/audit_summary.md — is ground truth healthy? Watch the by-series
   table; a series with outlier agreement should be excluded from modeling.
2. component_a/component_a_report.md — does the forecaster beat the naive
   baseline? (Header prints PASS/FAIL.)
3. market_baseline/market_baseline_report.md — forecast comparison vs the
   market + raw divergence backtest.
4. verdict/verdict_report.md — THE answer. Day-clustered bootstrap CIs; a
   cell only PASSES if the CI clears zero and survives best-day removal.

## Module map

| module | role |
|---|---|
| scrape_transcripts | incremental FactBase corpus updates (no Mongo) |
| collect | live Kalshi markets/events/candles |
| historical | pre-cutoff markets + candle fallback |
| event_mapper | market -> specific transcript (overrides: --overrides CSV) |
| labeler / audit | occurrence labels; agreement report |
| counts | phrase-per-document occurrence matrix |
| component_a | walk-forward base-rate models + calibration |
| market_baseline | model vs market; divergence backtest |
| verdict | clustered-bootstrap significance + decomposition |
| pipeline | orchestrates all of the above |

## Asking for a number (predict)

```bash
python -m kalshi_mention.predict --phrase "China / Chinese" --event-type interview
python -m kalshi_mention.predict --phrase "Iran (3+ times)" --event-type pressconf
python -m kalshi_mention.predict --all-open        # score every open Kalshi mention market
python -m kalshi_mention.predict --phrase "Tariffs" --event-type speech --json
```

Returns the calibrated probability plus the evidence (prior same-type events,
hit count, last-90d rate, last occurrence). Event types: speech, pressconf,
gaggle, interview, meeting, vlog, any. Phrases accept Kalshi's exact format.
Calibration is refit from the resolved-markets database on each call, with a
degeneracy guard that falls back to raw rates if the fit is untrustworthy.

## The unified model (U) — current best forecaster

One learned model over every signal in the system: all 24 Component A variants
(per-market model selection), Component B context features + news cache,
Component C's p_c when present, the k-threshold correction, and a point-in-time
phrase co-occurrence engine (which phrases travel together x what's been active
lately). Gradient-boosted trees walk-forward on resolved-only outcomes, Platt
calibration on top, logistic fallback if sklearn is missing.

```bash
pip install scikit-learn
python -m kalshi_mention.unified --db kalshi_mention.sqlite
python -m kalshi_mention.pipeline --only baseline,verdict   # heat map judges U
```

The pipeline's `--only analyze` runs unified automatically after component_a
and news; baseline/verdict prefer model_u whenever unified/ exists. Gate: beat
the best single Component A column on holdout log loss.

## Components B and C

Component B (context model — news salience, phrase momentum, schedule):

```bash
python -m kalshi_mention.news --db kalshi_mention.sqlite     # ~6 min, one-time + incremental
python -m kalshi_mention.component_b --db kalshi_mention.sqlite
python -m kalshi_mention.pipeline --only baseline,verdict     # judge B on the heat map
```

The pipeline now runs news -> modelb automatically inside `--only analyze`;
baseline and verdict auto-consume component_b/predictions_ab.csv (model_b)
when it exists. B's gate: beat Component A on holdout log loss.

Component C (LM Monte Carlo simulator; compute-heavy, run deliberately):

```bash
pip install torch transformers                     # one-time, ~2GB download
python -m kalshi_mention.component_c --db kalshi_mention.sqlite --limit 40   # smoke first
python -m kalshi_mention.component_c --db kalshi_mention.sqlite              # full (hours)
python -m kalshi_mention.component_b --db kalshi_mention.sqlite \
    --c-file component_c/component_c_predictions.csv          # C joins the ensemble
python -m kalshi_mention.pipeline --only baseline,verdict
```

Use --model /path/to/checkpoint for a fine-tuned GPT-2; the report documents
the training-cutoff leakage caveat (base gpt2 is leakage-clean but weak).

## Project log (what was learned when)

## Pass 2: per-event mapping + Component A

Run inside the project venv (`python3 -m venv .venv && source .venv/bin/activate
&& pip install -r requirements.txt`) — Component A imports numpy/pandas heavily.

```bash
# 1. Map each event to its specific transcript (only_doc / auto_title / override)
python -m kalshi_mention.event_mapper --db kalshi_mention.sqlite
#    unmapped multi-doc events are printed; map stragglers via:
#    python -m kalshi_mention.event_mapper --db ... --overrides event_overrides.csv
#    (CSV columns: event_ticker,doc_id)

# 2. Relabel with per-event scope (mapped events use their doc; others use the
#    same-day union fallback), then re-audit — agreement should rise materially
python -m kalshi_mention.labeler --db kalshi_mention.sqlite --transcripts trump_transcripts_merged.parquet
python -m kalshi_mention.audit   --db kalshi_mention.sqlite --out audit

# 3. Precompute phrase-per-document counts across the whole corpus (one-time,
#    a few minutes; token-index based so it scales)
python -m kalshi_mention.counts --db kalshi_mention.sqlite --transcripts trump_transcripts_merged.parquet

# 4. Walk-forward Component A evaluation
python -m kalshi_mention.component_a --db kalshi_mention.sqlite --out component_a
cat component_a/component_a_report.md
```

Reading the report: the holdout columns are the honest numbers (models are
selected on the earlier selection period only). The gate: the selected model
must beat `base_pooled` on holdout log loss. The market-price baseline is
deferred until price history collection (fast-follow). Point-in-time is
enforced structurally: every prediction uses only transcripts whose
`available_at` precedes the market's `open_time`.


## Pass 2.5: event-type conditioning + walk-forward recalibration

Component A now also produces, for every base model:
- `...T_...` typed variants — history reweighted toward documents of the
  market's ex-ante event type (classified from the Kalshi title: speech /
  pressconf / gaggle / interview / meeting / vlog); off-type docs keep 15%
  weight so thin types shrink smoothly toward the global estimate.
- `..._cal` variants — walk-forward Platt recalibration refit only on markets
  already resolved at each decision time (identity until 75 resolved).

The report gains a holdout-by-event-type table. To hand-map the remaining
unmapped multi-doc events (worth doing — mapping alone moved audit agreement
83.0% -> 88.3%):

```bash
python -m kalshi_mention.event_mapper --db kalshi_mention.sqlite --list-unmapped
# fill doc_id column in event_overrides_template.csv, then:
python -m kalshi_mention.event_mapper --db kalshi_mention.sqlite --overrides event_overrides_template.csv
# and re-run labeler + audit
```


## Pass 3: price history + market baseline + divergence backtest

```bash
# 1. Pull hourly candlesticks for all settled markets (~5-10 min; resumable —
#    rerun the same command if interrupted and it skips what it already has)
python -m kalshi_mention.collect --db kalshi_mention.sqlite --candles

# 2. Compare model vs market and run the divergence backtest
python -m kalshi_mention.market_baseline --db kalshi_mention.sqlite \
    --predictions component_a/component_a_predictions.csv --out market_baseline
cat market_baseline/market_baseline_report.md
```

Reading order inside the report: the forecast table is context (later
snapshots structurally favor the market); the divergence table is the
decision. Positive net average P&L at the 1h snapshot with a meaningful trade
count is the signal that Component A alone carries tradeable edge; a negative
one means the edge must come from mapping improvements or Components B/C.


## Pass 4a: historical-tier markets (Jan-Apr recovery)

Kalshi moves markets settled >~3 months ago to a historical API tier; this
recovers them plus their candlesticks (with live->historical fallback for the
markets that 404'd near the cutoff), then the standard chain re-runs on the
enlarged sample:

```bash
python -m kalshi_mention.historical --db kalshi_mention.sqlite
python -m kalshi_mention.event_mapper --db kalshi_mention.sqlite
python -m kalshi_mention.labeler --db kalshi_mention.sqlite --transcripts trump_transcripts_merged.parquet
python -m kalshi_mention.audit   --db kalshi_mention.sqlite --out audit
python -m kalshi_mention.counts  --db kalshi_mention.sqlite --transcripts trump_transcripts_merged.parquet
python -m kalshi_mention.component_a --db kalshi_mention.sqlite --out component_a
python -m kalshi_mention.market_baseline --db kalshi_mention.sqlite \
    --predictions component_a/component_a_predictions.csv --out market_baseline
```

The point: the pass-3 backtest rested on ~39 independent event-days. Doubling
the day count is the difference between suggestive and statistical.
