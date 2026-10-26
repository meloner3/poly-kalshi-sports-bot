"""Resolution-mismatch audit — the number this whole pass exists to produce.

For every settled per-event mention market with at least one same-day
transcript, compare:

    transcript_label = OR over same-day docs of "phrase occurs in text_raw"
    kalshi_result    = the market's actual settlement

and report the agreement rate, split by disagreement direction:

    kalshi=yes, transcript=no  -> transcript false negatives (missed words,
                                  dropped speaker blocks, transcription error,
                                  or Kalshi counting variants we don't)
    kalshi=no,  transcript=yes -> matcher too loose (matching across
                                  interruptions/variants Kalshi doesn't count)
                                  or transcript includes remarks outside the
                                  market's event window (same-day union is
                                  broader than one event)

    python -m kalshi_mention.audit --db kalshi_mention.sqlite --out audit
"""

from __future__ import annotations

import argparse
from pathlib import Path

import pandas as pd

from . import config, db
from . import paths
from . import manifest


def run_audit(db_path: str, out_dir: str) -> None:
    conn = db.connect(db_path)
    out = paths.out_dir(out_dir)

    q = """
        SELECT m.ticker, m.series_ticker, m.event_ticker, m.result AS kalshi_result,
               m.close_time, m.volume,
               l.phrase, l.phrase_source,
               MAX(l.matched)      AS transcript_label,
               SUM(l.match_count)  AS total_match_count,
               COUNT(l.doc_id)     AS n_docs
        FROM markets m
        JOIN labels l ON l.market_ticker = m.ticker
        WHERE m.result IN ('yes','no')
        GROUP BY m.ticker
    """
    df = pd.read_sql_query(q, conn)
    conn.close()
    if df.empty:
        raise SystemExit("No labeled settled markets — run collect and labeler first.")

    df["kalshi_yes"] = (df["kalshi_result"] == "yes").astype(int)
    df["agree"] = (df["kalshi_yes"] == df["transcript_label"]).astype(int)
    df["disagreement"] = "agree"
    df.loc[(df["kalshi_yes"] == 1) & (df["transcript_label"] == 0), "disagreement"] = "kalshi_yes_transcript_no"
    df.loc[(df["kalshi_yes"] == 0) & (df["transcript_label"] == 1), "disagreement"] = "kalshi_no_transcript_yes"

    df.sort_values(["disagreement", "series_ticker", "ticker"]).to_csv(
        out / "audit_per_market.csv", index=False
    )
    disagreements = df[df["agree"] == 0]
    disagreements.to_csv(out / "audit_disagreements.csv", index=False)

    lines = ["# Transcript-vs-Kalshi Resolution Agreement", ""]
    n = len(df)
    lines.append(f"Settled per-event markets with >=1 same-day transcript: **{n}**")
    lines.append(f"Agreement rate: **{df['agree'].mean():.1%}**")
    lines.append("")
    lines.append("| direction | count | share |")
    lines.append("|---|---|---|")
    for name, grp in df.groupby("disagreement"):
        lines.append(f"| {name} | {len(grp)} | {len(grp)/n:.1%} |")
    lines.append("")
    lines.append("## By series")
    lines.append("")
    lines.append("| series | n | agreement |")
    lines.append("|---|---|---|")
    for st, grp in df.groupby("series_ticker"):
        lines.append(f"| {st} | {len(grp)} | {grp['agree'].mean():.1%} |")
    lines.append("")
    lines.append("## Interpretation guardrails")
    lines.append("")
    lines.append("- `kalshi_yes_transcript_no` above a few percent means the FactBase corpus")
    lines.append("  systematically misses spoken words (dropped speaker labels, transcription")
    lines.append("  gaps) — a direct bias on any base-rate model trained on it. Investigate the")
    lines.append("  rows in audit_disagreements.csv before building Component A.")
    lines.append("- `kalshi_no_transcript_yes` can also reflect the same-day-union limitation:")
    lines.append("  the phrase occurred in a *different* same-day appearance than the market's")
    lines.append("  event. Day-granularity dates make this unavoidable in v1; if this bucket is")
    lines.append("  large, per-event transcript mapping (manual or via event titles) is the fix.")
    lines.append("- Do not tune matcher flags to maximize agreement blindly: flag changes must")
    lines.append("  be justified by reading actual disagreement cases, then re-run.")

    manifest.append_to(lines, db_path=db_path)
    (out / "audit_summary.md").write_text("\n".join(lines))
    print(f"Wrote {out/'audit_summary.md'} — agreement {df['agree'].mean():.1%} over {n} markets")
    print(f"Disagreements: {len(disagreements)} (see audit_disagreements.csv)")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--out", default="audit")
    ap.add_argument("--export-sample", type=int, metavar="N",
                    help="T7a: write a stratified sample for manual review")
    ap.add_argument("--import-review", metavar="FILE",
                    help="T7a: ingest reviewed labels, decompose the error")
    ap.add_argument("--file", default="audit/review_sample.csv",
                    help="path for --export-sample")
    args = ap.parse_args()
    if args.export_sample:
        from .review import export_sample
        export_sample(args.db, args.file, args.export_sample)
    elif args.import_review:
        from .review import import_review
        import_review(args.import_review, args.out)
    else:
        run_audit(args.db, args.out)


if __name__ == "__main__":
    main()
