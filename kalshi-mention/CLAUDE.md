> **VENDORED COPY — read [VENDORED.md](VENDORED.md) first.**
>
> This file is upstream's institutional memory, reproduced verbatim. Its
> HARD RULES 1 and 2 give paths in the DelphiResearch repo, which are **not**
> the paths here. In this repo:
>
> | | upstream (as written below) | here |
> |---|---|---|
> | venv | `~/Desktop/Git/DelphiResearch/.venv` | `kalshi-mention/.venv` |
> | code | `DelphiResearch/kalshi_mention/` | `kalshi-mention/kalshi_mention/` |
> | data | `DelphiResearch/data/kalshi_mention/` | `<repo root>/data/kalshi_mention/` |
> | tests | `pytest kalshi_mention/tests` | `pytest kalshi-mention/tests` |
> | run from | DelphiResearch repo root | anywhere (package is `pip install -e`'d) |
>
> The substance of every rule still applies — especially Rule 1 (never the
> anaconda base env), Rule 4 (this repo's data copy is not a backup), and
> Rule 8 (never trust a good backtest on first sight). Everything below this
> banner is upstream's text, unedited.

---

# CLAUDE.md — kalshi_mention project

Research system that forecasts P(Trump says phrase X during event Y), tests it
against Kalshi's own market prices, and renders a statistically honest verdict
on tradeability. Built iteratively (passes 0-4 + components B/C + unified
model); this file is the institutional memory. Read it before changing anything.

## HARD RULES (violations have burned this project before)

1. **Always work inside the venv.** `source .venv/bin/activate` (the repo venv
   at `~/Desktop/Git/DelphiResearch/.venv`, Python 3.11) before ANY python
   command. The anaconda `(base)` env has a broken NumPy 1/2 mix and a
   different Python; commands run there produce `_ARRAY_API not found` spam or
   import the wrong package state. The prompt must show `(.venv)`.
2. **The project now lives in the DelphiResearch repo** (moved 2026-09-10).
   Code: `~/Desktop/Git/DelphiResearch/kalshi_mention/` (git-tracked).
   Data: `~/Desktop/Git/DelphiResearch/data/kalshi_mention/` (gitignored) —
   sqlite, corpus, and every output directory. Run commands from the repo
   root. `~/Desktop/kalshi_mention` is now an empty shell holding only the
   dead 3.9 venv; any `kalshi_mention 2/3/4` elsewhere is download debris.
   **Never commit data**: `.gitignore` covers `data/`, `*.sqlite`, `*.parquet`
   and GitHub rejects files over 100MB.
3. **Paths never resolve against the working directory.** `paths.py` resolves
   everything against the data root, overridable with
   `$KALSHI_MENTION_DATA_DIR`. New code must go through `paths.data_path()` /
   `paths.out_dir()` / `paths.corpus_path()`, never a bare relative filename.
4. **Back up after every big data run:**
   `cp data/kalshi_mention/kalshi_mention.sqlite data/kalshi_mention/trump_transcripts_merged.parquet ~/Desktop/kalshi_data/`
   The golden backup at ~/Desktop/kalshi_data/ has already saved the project
   twice. An off-machine copy is still pending — encourage it. Being in git
   does NOT back up the data; only the code is versioned.
5. **Point-in-time discipline is sacred.** Every feature/label/fit must use
   only information available strictly before each market's open_time.
   Walk-forward refits use resolved-only outcomes. Any new feature must
   respect this or the evaluation is worthless.
6. **No comparison without an interval.** Effective n is EVENT-DAYS (228),
   not markets (8,260) — a median of 36 markets share one transcript and one
   realisation of the world. Every model comparison goes through `cluster.py`;
   a bare logloss difference is not a finding. Differences below ~0.01 are
   not resolvable at this sample size. This rule exists because the project
   clustered its P&L correctly for two years while comparing model gates as
   if they were exact, and recorded a coin flip as an established result.
7. **Every report carries a sample manifest, and no number is quoted without
   one.** `manifest.py` stamps db path/size/mtime, per-series counts,
   exclusions, event-days and snapshot coverage onto every report. Two
   reports are comparable only if their dataset identity lines match. This
   rule exists because four different market counts were in circulation
   (8,952 / 8,260 / 7,550 / 7,682) and one of them no query reproduces.
8. **Never trust a good backtest result on first sight.** This project's
   verdict apparatus caught four false edges (see Research Log). Suspicious
   signatures: one trade side dominating, hit rate ≈ base yes-rate, market
   logloss far from ~0.54, profit concentrated in a few days.

## Environment & data

- Python 3.11 repo venv at `~/Desktop/Git/DelphiResearch/.venv`
  (repo requirements.txt; includes scikit-learn, beautifulsoup4, and
  torch+transformers for Component C). The old 3.9 venv in
  ~/Desktop/kalshi_mention is dead — do not use it.
- `kalshi_mention.sqlite` (~214MB+): markets (8,952 settled), events,
  candlesticks (~500k rows, hourly), transcripts (4,926 docs, 2004→2026-07),
  labels, phrase_doc_counts (437k rows, 1,237 specs), news_volume (GDELT
  cache, ~840 phrases fetched / ~390 failed — rerun news module to sweep).
- `trump_transcripts_merged.parquet`: the corpus (Trump-only text per doc).
- Mongo was abandoned at project start; everything is sqlite+parquet.
- Kalshi public API: https://api.elections.kalshi.com/trade-api/v2. Markets
  settled >~3 months ago live in a separate historical tier
  (`/historical/markets`, `/historical/markets/{t}/candlesticks`); cutoff
  endpoint `/historical/cutoff` (boundary ~2026-05-27, rolls forward daily).
- GDELT DOC API rate-limits hard (429s); news.py has backoff + circuit
  breaker + proper-noun gating. Slow is normal.

## How to run

All commands run from the repo root (`~/Desktop/Git/DelphiResearch`):

```bash
python -m kalshi_mention.pipeline                 # everything (scrape→verdict)
python -m kalshi_mention.pipeline --only analyze  # recompute, no network
python -m kalshi_mention.predict --phrase "China / Chinese" --event-type interview
python -m kalshi_mention.predict --all-open       # score open Kalshi markets
python -m kalshi_mention.encompass                 # does the model beat the PRICE?
python -m kalshi_mention.verdict --prereg         # frozen v2 tracker (T6)
python -m kalshi_mention.audit --export-sample 200   # corpus review sample (T7a)
python -m kalshi_mention.review --export-taxonomy 150 # blind type sample (T7b)
python analysis/build_verification_report.py      # regenerate T0 verification
python -m pytest kalshi_mention/tests -q          # 104 tests, all must pass
```

All tests pass. `test_predict_phrase_end_to_end` used to fail: it pinned its
synthetic corpus to fixed calendar dates while `predict_phrase` measures
document age from `now()`, so with a 30-day half-life the corpus decayed as
real time passed and a never-observed phrase drifted 0.012 -> 0.174. Now
anchored to now(). That drift is a LIVE RISK, not just a test artifact: a
stale corpus pulls every probability toward 0.5 with no error raised (130
days stale -> 0.16; 250 -> 0.44). `predict.py` warns at 45 days and says the
numbers should not be used at 120.

Pipeline steps: scrape, collect, historical, map, label, audit, counts,
model (Component A), news, unified, baseline, verdict. All idempotent and
resumable. Outputs to read, in order, under `data/kalshi_mention/`:
audit/audit_summary.md,
component_a/component_a_report.md, unified/unified_report.md,
market_baseline/market_baseline_report.md, verdict/verdict_report.md.

## Module map

| module | role |
|---|---|
| collect / historical | Kalshi markets+events+candles (live + historical tier, resumable) |
| scrape_transcripts | incremental FactBase corpus updates |
| phrase / counts | Kalshi spec parsing (variants, "(N+ times)") + exact token matching |
| event_mapper / labeler / audit | market→transcript mapping, occurrence labels, agreement report |
| component_a | 24 base-rate variants (empirical/poisson/negbin × hl30/90 × typed × Platt-cal), walk-forward |
| news | GDELT per-phrase daily volume cache |
| component_b | walk-forward logistic context layer (superseded by unified; kept for ablation) |
| component_c | LM Monte Carlo simulator (needs torch; leakage guard vs checkpoint cutoff) |
| finetune_lm | GPT-2 fine-tune on corpus with --cutoff-date discipline |
| unified | THE model: gradient-boosted walk-forward ensemble over everything + co-occurrence engine |
| market_baseline | model vs market mid; divergence backtest at the touch, net of fees |
| verdict | day-clustered bootstrap significance + decomposition (live-era quotes only) |
| predict | ask a phrase+event-type, get calibrated P + evidence |
| pipeline | orchestrates all of the above |
| paths | data-root + corpus resolution; the only module that knows locations |
| cluster | day-clustered CIs; every model comparison must go through it |
| manifest | sample manifest stamped onto every report (Hard Rule 7) |
| prereg | the frozen v2 pre-registration, as executable constants |
| review | T7 corpus + taxonomy audit tooling (export/import round trips) |
| encompass | M0/M1 encompassing test: does the model add to the price? |

## Place in the DelphiResearch repo

Sibling projects share the corpus and the LM stack, so changes there can
affect results here.

- `trump_transcripts/prepare_trump_transcripts.py` builds
  `trump_transcripts_clean.parquet` from Mongo. `scrape_transcripts.py` builds
  `trump_transcripts_merged.parquet` from FactBase. Same schema; the merged
  one is a continuation of the clean one (4,955 docs to 2026-08-07 vs 4,729 to
  2026-03-13) and is what `paths.corpus_path()` picks by default. The clean
  parquet is the automatic fallback.
- Labels are counted on `text_raw`, never `text_clean` — `check_corpus_schema`
  enforces the column's presence. If anyone changes the cleaning in
  prepare_trump_transcripts.py, labels are unaffected; if anyone drops
  `text_raw` from the export, this project breaks loudly rather than silently.
- `trump_transcripts/gpt2_finetune.py` fine-tunes GPT-2 on the same corpus
  that `component_c.py` samples from. Any checkpoint fed to Component C must
  respect the leakage cutoff (2026-05-27) — a checkpoint trained on post-cutoff
  text invalidates the Component C evaluation, and nothing in the repo enforces
  that automatically.
- Mongo is DelphiResearch's store, not this project's. kalshi_mention is
  sqlite+parquet only; it reads Mongo output as files, never connects.


## Research log — established findings (do not re-litigate without new data)

**Data facts.** Ticker-encoded dates are authoritative (close_time unreliable:
markets close early on occurrence). `yes_sub_title` is a variant list +
optional "(N+ times)" threshold. Audit agreement: 78.1% overall (93.1%
KXPRESMENTION best; 69.8% KXTRUMPMENTIONB = same-day-union artifact at scale;
~3% corpus false-negative floor — STALE: that figure came from a
mostly-live-era sample; on the current expanded corpus the measured
`kalshi_yes_transcript_no` rate is 10.5%, reflecting worse FactBase coverage
in the older era. Either way, features and labels disagree ~21% of the time,
which caps achievable logloss regardless of model). 119 unmapped multi-doc events await manual
override CSV (mapper --list-unmapped writes the template). KXTRUMPSAY series
is excluded from labeling/modeling. "Event does not qualify" is a meta-strike,
should eventually be excluded from the phrase universe.

**Component A: validated forecaster.** Holdout logloss ~0.631 vs pooled base
~0.674 (PASS) across 8,260 markets. Best in low-history phrases (0.43 vs
0.61) and pressconf. Known bias: underpredicts "(3+ times)" markets
(discovered via Component B weight +0.494; unified gets correction features).

**CRITICAL data corruption, handled: historical-tier candle quotes are
placeholder junk.** Pre-2026-05-27 candles encode empty books as
0.0000-dollar strings (median ask 0.6c, market logloss 2.25 vs live-era
0.544). They produced two spectacular false edges (+$0.33/contract) before
being diagnosed via era-split. quote_ok() filters degenerate quotes;
verdict.py quarantines all pre-HIST_CUTOFF trades. Historical markets'
OUTCOMES are fine (audit vouches) and power Component A; only their PRICES
are untrusted. Decoding them correctly (compare stored candles vs kalshi.com
charts) would un-quarantine 162 event-days — open side quest.

**Candle payload formats** (all handled in market_baseline.parse_candle):
nested cents, flat cents, dollar-strings ("close_dollars": "0.3500"), and
volume as decimal string or volume_fp.

**Final verdict — REWRITTEN 2026-09-10 after two external reviews and the
WORKORDER remediation (T0-T9). Several previously recorded conclusions did
not survive. Read this whole block before citing any of them; every number
below has a script in `analysis/` or a report under `data/kalshi_mention/`.**

*NOT TRADEABLE — that part stands, under both entry rules.* No cell passes,
whether selecting on |model - mid| or on executable EV against the touch. But
the claim is BOUNDED, not absolute: the best-powered cell has an 80%-power
MDE of **~5.6c/contract on 40 qualifying event-days**. An edge below that is
invisible here. Note the project previously quoted CI half-widths as if they
were detection limits; the 80%-power MDE is 43% larger.

*Passive accumulation is not a plan.* Reaching 2c resolution needs ~308
qualifying event-days (~11 months); 1c needs 1,166-2,242 depending on the
cell. (The ~2,262 figure that circulated matches only the weakest cell, 24h
at a high threshold — quote the cell with the figure.) And qualifying days
accrue slower than calendar days, since not every event-day produces a
trade. If an edge is worth chasing it needs a more powerful design — paired
tests against the price, more markets per day, a lower-variance endpoint —
not more waiting.

*The 1h "significant LOSS" was execution cost, not forecast failure.* T3
decomposes every cell exactly: `net = settle_vs_mid - crossing - fee`. At 1h,
settle_vs_mid is +0.002 (CI spans zero) while crossing averages 0.103 and fee
0.018. The forecast is flat there, not anti-predictive. "Never lift fresh
offers" remains correct operationally, and there IS genuine adverse selection
at 1h - requiring positive post-cost EV still loses 5.8c - but blame
selection and the spread, not the forecast.

*Both ensemble gates FAIL, and one now fails decisively.* Day-clustered,
paired, on 78 holdout event-days:

| comparison | diff | 95% CI | reading |
|---|---|---|---|
| best A - base_pooled | -0.0428 | [-0.0559, -0.0293] | A genuinely better |
| model_b - best A | +0.0042 | [-0.0011, +0.0096] | cannot tell |
| model_u - best A (44 feats) | +0.0065 | [-0.0034, +0.0174] | cannot tell |
| model_u - best A (19 feats, T8d) | +0.0136 | [+0.0025, +0.0256] | measurably WORSE |

Component A's win over the pooled base is the project's only forecasting
claim that survives clustering. The earlier finding that "context adds
~nothing / the signal isn't there" was never supported: at 44 features the
interval spans zero, which means the sample cannot tell, not that the signal
is absent.

*T8(d) is recorded as a negative result.* Cutting U's A-columns from 24 to 3
on "variance farming" grounds made U measurably worse (+0.0060 of the
+0.0136). The 24 correlated columns were carrying information the 3 do not.
Implemented as specified; the rationale is contradicted by measurement.

*listing_load: leak found, fixed, finding SURVIVED.* It was computed as the
event-day's FINAL market count - not knowable when an early market opens (59%
of days stagger opens by >1h; ~24% of the feature was future information).
Now an inclusive point-in-time rank. After the fix its permutation importance
ROSE (+0.0184 -> +0.0292 -> +0.0357 at 19 features) and it is still #1.
Kalshi's own listing intensity really is the most informative context
feature. Permutation importances are in-sample; treat as diagnostic only.

*Forecast encompassing (T5): NULL, and it retracts a claim made during the
review itself.* M0 = recalibrated market, M1 = M0 + frozen A column,
expanding windows, resolved-only, paired OOS log loss:

    all-eligible  M1-M0 = +0.0029  CI [-0.0118, +0.0163]  913 rows, 32 days
    pre-event     M1-M0 = +0.0595  CI [-0.0156, +0.1380]  149 rows,  7 days

A `beta_A = +0.505, CI [+0.135, +0.924]` was reported mid-review as a
surviving finding. It does not survive. It was IN-SAMPLE, and it was computed
on the PRE-T2 sample that still contained the outcome-dependent close-buffer
selection; on the corrected sample the in-sample coefficient falls to +0.367
with a CI spanning zero, and out-of-sample the effect is gone. The look-ahead
removed in T2 was partly manufacturing the result found in the same session.
The machinery is validated on synthetic worlds (planted signal -> CI excludes
zero; market-plus-noise -> it does not), so the null is informative.

*RETRACTED: "public information means we cannot win by construction."* Kalshi
does set strikes near 50/50 from the same public history, and that is a useful
prior about where edge will NOT be found. It is not a proof. It says nothing
about execution-driven or timing-driven edges, and an unfalsifiable framing
is worse than a null result. Treat it as intuition, not a finding.

*RETRACTED: the k>1 mispricing hint.* Kalshi's "(3+ times)" strikes resolving
YES 67% of the time (72% for k=3 alone, n=57) is NOT evidence of
mis-set strikes. Testing that requires CONTEMPORANEOUS PRICES on those
markets, which the project does not have. Backlog item, not a claim. The
related Component B "+0.494 threshold weight" rests on 73 markets across 55
event-days and comes from a model whose overall improvement is undetectable;
it is now replaced by a single partially-pooled correction (T8c).

**Component C (narrowed conclusion).** Base GPT-2 standalone logloss 2.31.
The honest statement is narrow: *this simulator, at N=30 samples of 400
tokens, performed poorly*. That is a statement about an underpowered sampling
scheme for rare phrases - 30 draws cannot resolve a 5% probability, the
estimator's own granularity being ~3% - not a general verdict on language
models for this task. `finetune_lm.py` and its pre-registered criteria stay in
the tree; running it is NOT on the backlog, because the sample-count problem
is a property of the method rather than of training.


**Pre-registration: v1 is VOID; v2 is frozen.** See `PREREGISTRATION.md`.
v1 died for two independently sufficient reasons: the entry rule changed to
executable EV, and post-2026-08-11 outcomes may already have been observed
during routine pipeline runs, so its virgin-data promise cannot be honoured.
Its prior also weakened - T2 showed one of its four supporting structures
(NO-side +0.05) REVERSES SIGN under decision-time-legal eligibility
(all-eligible NO +0.0670; pre-event NO -0.0722) - and four models sharing ~24
collinear features on one 228-day sample were never replication in the first
place. v2 freezes `empirical_hl30_cal` at 6h, pre-event eligibility, EV >
0.02, max spread 0.10, quote age <= 1h, one contract, with a 320-day plan,
O'Brien-Fleming looks at 80/160/240/320 and a three-way SUCCESS / COMMERCIAL
FUTILITY / CONTINUE decision. Track it with
`python -m kalshi_mention.verdict --prereg`. It reports 0 eligible days today
and will for months; that is the expected state.

## Incident log — false edges and leaks

Kept because the pattern matters more than any individual bug: every one of
these produced a plausible, encouraging number first.

1-2. **Historical-tier placeholder quotes** (self-caught, era-split).
   Pre-2026-05-27 candles encode empty books as 0.0000-dollar strings; median
   ask 0.6c, market logloss 2.25 vs 0.544 live. Produced two spectacular false
   edges (+$0.33/contract). `quote_ok()` filters them; `verdict.py`
   quarantines all pre-cutoff trades. Their OUTCOMES are fine and still power
   Component A - only their PRICES are untrusted.
3-4. **Two further divergence pockets** (self-caught, day-clustering).
   Apparent edges that were one or two lucky event-days; killed by clustering
   the bootstrap and by the best-3-days-removed check.
5. **Close-buffer look-ahead** (2026-09-10) — **found by external review, not
   self-caught.** `pick_snapshot` dropped any snapshot within 30 minutes of a
   market's close. That reads as a contamination guard and is the opposite:
   these markets close early WHEN THE PHRASE OCCURS, so close_ts is a function
   of the outcome and filtering on it selected the backtest sample on the
   answer. It removed 6,502 of 14,946 snapshot rows, 68.5% of NO markets at
   24h, and flipped the sign of the NO-side pocket that the pre-registration
   was built on. Nothing downstream could have detected it; the tests actively
   asserted the buggy behaviour. Now: eligibility is decision-time only, and a
   test proves it is invariant to close time and settlement.
6. **listing_load future-count** (2026-09-10, external review). Fixed; the
   underlying finding survived and strengthened. See the research log.

The lesson from 5 and 6 is not "cluster harder" — the project already did
that. It is that a filter which *sounds* conservative can be the leak, and
that a test asserting current behaviour locks the bug in. Point-in-time
invariance is now checked by construction (T8f), not by inspection.

## Workflow gotchas (Mac-specific, recurring)

- Safari may auto-extract downloaded zips and delete them (folders appear as
  "name 2/3"). Fix: Safari Settings → General → untick "Open safe files".
- Long heredoc pastes garble in VS Code's terminal — write scripts to files.
- The user's terminal pastes into chat sometimes arrive empty; prefer files.
- Two hostnames (Mac / Julias-MacBook-Pro-53) may be the same laptop.

## Backlog — REPRIORITISED 2026-09-10 (post-WORKORDER)

Tooling for 1 and 3 now exists (T7); what is missing is the human hours.

1. **Corpus/label quality — the binding constraint.** Features and labels
   disagree ~21% of the time, and conditional on Kalshi resolving YES the
   corpus misses the phrase **24.7%** of the time. No model can tunnel under
   that. Run `audit --export-sample 200`, review by hand, then
   `audit --import-review` to get the decomposition (wrong_mapping /
   incomplete_transcript / attribution / matcher / qualification_rule). Only
   that tells you whether to fix the scraper, the 119 event-mapping
   overrides, the matcher, or nothing (a qualification_rule majority means
   agreement simply cannot go higher).
2. **Event-type taxonomy validation.** `review --export-taxonomy 150`, blind,
   then `--import-taxonomy`. Two classifiers (Kalshi titles vs FactBase
   speech_type) are assumed commensurable by the 0.15 off-type weight and
   have never been checked against each other. 18% of markets land in `other`,
   60% in `meeting`.
3. **Weekly `python -m kalshi_mention.pipeline`.** Now has a defined target
   and a defined endpoint: 320 qualifying event-days under the v2 freeze.
   But see the power note — passive accumulation alone will not resolve a 2c
   edge on a sensible horizon, so treat this as maintaining the option, not
   as the experiment.
4. **A more powerful design, if the edge is worth chasing.** Paired tests
   against the price, more markets per qualifying day, or a lower-variance
   endpoint. More calendar time is the weakest available lever.
5. Off-machine backup of ~/Desktop/kalshi_data/ (git holds CODE only; the
   data is still single-copy).
6. Sweep ~390 failed news phrases (rerun news module; resumable).
7. Contemporaneous prices for k>1 markets — the ONLY way to test the
   "Kalshi mis-sets (3+ times) strikes" hint. Until then it is not a claim.
8. Historical-quote decoding (would un-quarantine 162 event-days and
   materially help the power problem — worth more than it looks). Deferred
   by the work order: needs archived raw responses, matched pre/post-migration
   observations, independent trade records, and acceptance criteria fixed
   without consulting profitability. None available today.
9. Exclude the "Event does not qualify" meta-phrase from modeling.
10. Revisit T8(d): cutting U's A-columns to 3 measurably hurt (+0.0060).
    Either restore the 24 or find a principled reduction that does not.

**Do NOT do:** run `finetune_lm` (the sample-count problem is a property of
the method, not of training); restore historical quotes; add context features
beyond T5's frozen set; live trading integration.
