"""One-command pipeline: update data, rebuild labels and models, render the verdict.

    python -m kalshi_mention.pipeline                     # everything
    python -m kalshi_mention.pipeline --skip scrape       # offline corpus
    python -m kalshi_mention.pipeline --only analyze      # recompute from stored data

Steps, in order:
  scrape    - pull new FactBase transcripts into the merged parquet
  collect   - live Kalshi markets/events + candlesticks (resumable)
  historical- historical-tier markets + missing candlesticks (resumable)
  map       - per-event transcript mapping
  label     - occurrence labels (per-event scope, union fallback)
  audit     - transcript-vs-Kalshi agreement report
  counts    - phrase-per-document counts
  model     - Component A walk-forward evaluation
  baseline  - market-price comparison + divergence backtest
  verdict   - day-clustered bootstrap verdict
  encompass - does the model add information to the market price?

Groups: --only data = scrape..historical; --only analyze = map..verdict.
Each step is idempotent; rerunning the pipeline is always safe.
"""

from __future__ import annotations

import argparse
import time
import traceback

from . import config, paths

DATA_STEPS = ["scrape", "collect", "historical"]
ANALYZE_STEPS = ["map", "label", "audit", "counts", "model", "news", "unified", "baseline", "verdict", "encompass"]
ALL_STEPS = DATA_STEPS + ANALYZE_STEPS


def run_pipeline(db: str, transcripts: str | None, steps: list[str], model: str) -> None:
    done, t0 = [], time.time()
    # Every artifact below is named relative to the data dir, never to the
    # current working directory, so the pipeline gives identical results from
    # anywhere in the repo. See paths.py.
    data = paths.data_dir()
    print(f"data dir: {data}")

    def art(rel: str) -> str:
        """Path to a pipeline artifact inside the data dir."""
        return str(data / rel)

    def step(name):
        return name in steps

    if step("scrape"):
        from .scrape_transcripts import run as scrape_run
        scrape_run(transcripts)
        done.append("scrape")
    if step("collect"):
        from .collect import collect as collect_run
        collect_run(db, config.MENTION_SERIES, with_candles=True)
        done.append("collect")
    if step("historical"):
        from .historical import run as hist_run
        hist_run(db)
        done.append("historical")
    if step("map"):
        from .event_mapper import build_mapping
        build_mapping(db, transcripts_path=transcripts)
        done.append("map")
    if step("label"):
        from .labeler import run_labeling
        run_labeling(db, transcripts, None)
        done.append("label")
    if step("audit"):
        from .audit import run_audit
        run_audit(db, art("audit"))
        done.append("audit")
    if step("counts"):
        from .counts import build_counts
        build_counts(db, transcripts)
        done.append("counts")
    if step("model"):
        from .component_a import run as model_run
        model_run(db, art("component_a"))
        done.append("model")
    if step("news"):
        try:
            from .news import run as news_run
            news_run(db)
        except SystemExit:
            raise
        except Exception as e:
            print(f"news step failed ({e}) — continuing without fresh salience")
        done.append("news")
    if step("unified") or step("modelb"):
        from .unified import run as u_run
        import os
        c_file = art("component_c/component_c_predictions.csv")
        c_file = c_file if os.path.exists(c_file) else None
        u_run(db, art("component_a/component_a_predictions.csv"), art("unified"), c_file)
        done.append("unified")
    if step("baseline"):
        from .market_baseline import run as baseline_run
        import os
        if os.path.exists(art("unified/predictions_unified.csv")):
            pred_file = art("unified/predictions_unified.csv")
        elif os.path.exists(art("component_b/predictions_ab.csv")):
            pred_file = art("component_b/predictions_ab.csv")
        else:
            pred_file = art("component_a/component_a_predictions.csv")
        stage = os.path.basename(os.path.dirname(pred_file))
        if model != "auto":
            models = [model]
        elif stage == "unified":
            models = ["model_u"]
        elif stage == "component_b":
            models = ["model_b"]
        else:
            models = ["poisson_hl30_cal"]
        try:
            baseline_run(db, pred_file, art("market_baseline"), models)
        except SystemExit as e:
            # model column may differ per run; retry with any *_cal column
            import pandas as pd
            cols = [c for c in pd.read_csv(
                art("component_a/component_a_predictions.csv"), nrows=1).columns
                if c.endswith("_cal")]
            if not cols:
                raise
            print(f"({e}) retrying with {cols[0]}")
            baseline_run(db, pred_file, art("market_baseline"), [cols[0]])
        done.append("baseline")
    if step("verdict"):
        from .verdict import run as verdict_run
        import os
        if os.path.exists(art("unified/predictions_unified.csv")):
            vp = art("unified/predictions_unified.csv")
        elif os.path.exists(art("component_b/predictions_ab.csv")):
            vp = art("component_b/predictions_ab.csv")
        else:
            vp = art("component_a/component_a_predictions.csv")
        verdict_run(art("market_baseline/market_baseline_snapshots.csv"), vp,
                    art("verdict"), "auto")
        done.append("verdict")

    if step("encompass"):
        from .encompass import run as enc_run
        try:
            enc_run(art("market_baseline/market_baseline_snapshots.csv"),
                    art("component_a/component_a_predictions.csv"),
                    art("encompass"))
        except SystemExit as e:
            print(f"encompass skipped ({e})")
        done.append("encompass")

    mins = (time.time() - t0) / 60
    print(f"\nPipeline done ({', '.join(done)}) in {mins:.1f} min.")
    print(f"Read, under {data}:")
    print("  audit/audit_summary.md, component_a/component_a_report.md,")
    print("  unified/unified_report.md, market_baseline/market_baseline_report.md,")
    print("  verdict/verdict_report.md")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--transcripts", default=None,
                    help="corpus parquet/CSV; default: auto-resolved (see paths.py)")
    ap.add_argument("--skip", default="", help="comma-separated steps to skip")
    ap.add_argument("--only", default="", help="'data', 'analyze', or comma-separated steps")
    ap.add_argument("--model", default="auto", help="model column for the backtest")
    args = ap.parse_args()

    if args.only == "data":
        steps = list(DATA_STEPS)
    elif args.only == "analyze":
        steps = list(ANALYZE_STEPS)
    elif args.only:
        steps = [s.strip() for s in args.only.split(",") if s.strip()]
    else:
        steps = list(ALL_STEPS)
    for s in (x.strip() for x in args.skip.split(",") if x.strip()):
        if s in steps:
            steps.remove(s)
    bad = [s for s in steps if s not in ALL_STEPS]
    if bad:
        raise SystemExit(f"unknown steps {bad}; valid: {ALL_STEPS}")

    print(f"Running steps: {steps}")
    try:
        run_pipeline(args.db, args.transcripts, steps, args.model)
    except KeyboardInterrupt:
        print("\nInterrupted — every step is resumable; rerun the same command.")
    except Exception:
        traceback.print_exc()
        raise SystemExit("\nPipeline stopped at the step above. Fix or --skip it and rerun.")


if __name__ == "__main__":
    main()
