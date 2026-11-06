"""T7 — human-review tooling for corpus quality and event-type taxonomy.

Measurement first, no modelling changes. Two independent audits, each a
stratified export -> human labelling -> import -> decomposition round trip.

Why this is the highest-value work in the queue: features and labels disagree
about 21% of the time (10.5% `kalshi_yes_transcript_no`, 11.4% the other
way), and conditional on Kalshi resolving YES the corpus misses the phrase
24.7% of the time. That is a hard ceiling on achievable log loss which no
ensemble can tunnel under. Until the disagreement is DECOMPOSED — how much is
bad event mapping, how much is missing transcript text, how much is the
matcher, how much is Kalshi's own qualification rules — there is no way to
know which fix is worth doing.

Both audits are blind where blindness matters: the taxonomy export hides both
classifiers' labels so a reviewer cannot anchor on them.

    python -m kalshi_mention.audit --export-sample 200
    #   ... fill in the review columns by hand ...
    python -m kalshi_mention.audit --import-review reviewed.csv

    python -m kalshi_mention.review --export-taxonomy 150
    python -m kalshi_mention.review --import-taxonomy reviewed_types.csv
"""

from __future__ import annotations

import argparse

import numpy as np
import pandas as pd

from . import config, db, paths
from .event_types import TYPES, classify_doc, classify_market

HIST_CUTOFF = "2026-05-27"

# The error taxonomy a reviewer assigns to each disagreement. Chosen so the
# categories map onto DIFFERENT fixes -- there is no point distinguishing
# errors that would be repaired the same way.
ERROR_KINDS = [
    "wrong_mapping",        # market matched to the wrong document
    "incomplete_transcript",  # phrase was said, corpus text is missing it
    "attribution",          # text present but attributed to the wrong speaker
    "matcher",              # our token matching disagrees with Kalshi's reading
    "qualification_rule",   # Kalshi's own rules exclude/include unusually
    "kalshi_error",         # the settlement itself looks wrong
    "unclear",              # cannot determine from the available evidence
]

REVIEW_COLUMNS = ["review_error_kind", "review_notes"]
TAXONOMY_COLUMNS = ["review_event_type", "review_notes"]


def _era(date_str) -> str:
    if not isinstance(date_str, str) or len(date_str) < 10:
        return "unknown"
    return "live" if date_str >= HIST_CUTOFF else "historical"


def stratified_sample(df: pd.DataFrame, n: int, strata: list[str],
                      seed: int = 0) -> pd.DataFrame:
    """Proportional-ish stratified sample with a floor of 1 per stratum.

    Small strata are the interesting ones (rare disagreement cells), so they
    are guaranteed representation rather than being sampled away.
    """
    present = [c for c in strata if c in df.columns]
    if not present or df.empty:
        return df.sample(min(n, len(df)), random_state=seed)
    groups = list(df.groupby(present, dropna=False))
    if not groups:
        return df.head(0)
    per = max(1, n // len(groups))
    picks = [g.sample(min(per, len(g)), random_state=seed) for _, g in groups]
    out = pd.concat(picks) if picks else df.head(0)
    if len(out) < n:                       # top up at random from the remainder
        rest = df.drop(index=out.index, errors="ignore")
        if len(rest):
            out = pd.concat([out, rest.sample(min(n - len(out), len(rest)),
                                              random_state=seed)])
    return out.sample(frac=1.0, random_state=seed).reset_index(drop=True)


# ---------------------------------------------------------------------------
# (a) Corpus / resolution audit
# ---------------------------------------------------------------------------

def export_sample(db_path: str, out_path, n: int = 200, seed: int = 0) -> pd.DataFrame:
    """Stratified sample of market-event-doc triples for manual review."""
    conn = db.connect(db_path)
    q = """
        SELECT m.ticker, m.series_ticker, m.event_ticker,
               m.result AS kalshi_result, m.title AS market_title,
               l.phrase, l.doc_id, l.matched, l.match_count,
               t.title AS doc_title, t.date AS doc_date,
               t.speech_type AS doc_speech_type, t.source_url AS doc_url
        FROM markets m
        JOIN labels l ON l.market_ticker = m.ticker
        LEFT JOIN transcripts t ON t.doc_id = l.doc_id
        WHERE m.result IN ('yes','no')
    """
    try:
        df = pd.read_sql_query(q, conn)
    except Exception:                       # source_url may not exist in older dbs
        df = pd.read_sql_query(q.replace(", t.source_url AS doc_url", ""), conn)
        df["doc_url"] = None
    conn.close()
    if df.empty:
        raise SystemExit("no labelled markets — run the labeler first")

    df["kalshi_yes"] = (df["kalshi_result"] == "yes").astype(int)
    df["agree"] = (df["kalshi_yes"] == df["matched"].fillna(0).astype(int)).astype(int)
    df["cell"] = np.where(df["agree"] == 1, "agree",
                          np.where(df["kalshi_yes"] == 1,
                                   "kalshi_yes_transcript_no",
                                   "kalshi_no_transcript_yes"))
    df["era"] = df["doc_date"].astype(str).str[:10].map(_era)

    sample = stratified_sample(df, n, ["era", "series_ticker", "cell"], seed)
    for c in REVIEW_COLUMNS:
        sample[c] = ""
    sample.attrs["error_kinds"] = ERROR_KINDS
    path = paths.data_path(out_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    sample.to_csv(path, index=False)
    print(f"Wrote {path} — {len(sample)} rows across "
          f"{sample['cell'].nunique()} cells, {sample['era'].nunique()} eras.")
    print(f"Fill `review_error_kind` with one of: {', '.join(ERROR_KINDS)}")
    return sample


def import_review(review_path, out_dir) -> list[str]:
    """Ingest human labels and decompose the disagreement."""
    df = pd.read_csv(paths.data_path(review_path))
    missing = [c for c in ("cell", "review_error_kind") if c not in df.columns]
    if missing:
        raise SystemExit(f"review file missing columns {missing}; "
                         "is this an --export-sample file?")
    df["review_error_kind"] = df["review_error_kind"].fillna("").astype(str).str.strip()
    labelled = df[df["review_error_kind"] != ""]

    lines = ["# Corpus / resolution error decomposition (T7a)", ""]
    lines.append(f"Reviewed {len(labelled)} of {len(df)} exported rows.")
    if labelled.empty:
        lines.append("")
        lines.append("No rows carry a `review_error_kind` yet — nothing to decompose.")
        out = paths.out_dir(out_dir)
        (out / "review_decomposition.md").write_text("\n".join(lines))
        return lines

    unknown = sorted(set(labelled["review_error_kind"]) - set(ERROR_KINDS))
    if unknown:
        lines.append("")
        lines.append(f"**Warning** — unrecognised error kinds: {unknown}")
    lines.append("")
    lines.append("## Error kind by disagreement cell")
    lines.append("")
    tab = pd.crosstab(labelled["review_error_kind"], labelled["cell"])
    lines.append("| error kind | " + " | ".join(tab.columns) + " | total |")
    lines.append("|" + "---|" * (len(tab.columns) + 2))
    for kind, row in tab.iterrows():
        lines.append(f"| {kind} | " + " | ".join(str(int(v)) for v in row) +
                     f" | {int(row.sum())} |")
    lines.append("")

    dis = labelled[labelled["cell"] != "agree"]
    if len(dis):
        lines.append("## Share of DISAGREEMENTS by cause")
        lines.append("")
        lines.append("| error kind | n | share |")
        lines.append("|---|---|---|")
        vc = dis["review_error_kind"].value_counts()
        for kind, cnt in vc.items():
            lines.append(f"| {kind} | {cnt} | {cnt/len(dis):.1%} |")
        lines.append("")
        lines.append("This is the number that decides what to fix. A corpus")
        lines.append("dominated by `incomplete_transcript` needs a better scraper;")
        lines.append("one dominated by `wrong_mapping` needs the override CSV;")
        lines.append("one dominated by `matcher` needs the phrase rules; one")
        lines.append("dominated by `qualification_rule` is not an error at all and")
        lines.append("caps how high agreement can ever go.")
        if "era" in dis.columns:
            lines.append("")
            lines.append("## By era")
            lines.append("")
            t2 = pd.crosstab(dis["review_error_kind"], dis["era"])
            lines.append("| error kind | " + " | ".join(t2.columns) + " |")
            lines.append("|" + "---|" * (len(t2.columns) + 1))
            for kind, row in t2.iterrows():
                lines.append(f"| {kind} | " + " | ".join(str(int(v)) for v in row) + " |")
    out = paths.out_dir(out_dir)
    (out / "review_decomposition.md").write_text("\n".join(lines))
    print(f"Wrote {out}/review_decomposition.md")
    return lines


# ---------------------------------------------------------------------------
# (b) Event-type taxonomy audit
# ---------------------------------------------------------------------------

def export_taxonomy(db_path: str, out_path, n: int = 150, seed: int = 0) -> pd.DataFrame:
    """Blind export of mapped market/doc pairs for reference typing.

    BOTH classifiers' labels are withheld. `classify_market` reads Kalshi
    titles and `classify_doc` reads FactBase speech_type; nobody has ever
    checked that the two taxonomies mean the same thing, and 18% of markets
    land in `other` while 60% land in `meeting`. A reviewer who could see
    either label would anchor on it.
    """
    conn = db.connect(db_path)
    excl = ",".join("?" * len(config.LABELING_EXCLUDED_SERIES)) or "''"
    q = f"""
        SELECT m.ticker, m.series_ticker, m.title AS market_title,
               e.title AS event_title,
               t.doc_id, t.title AS doc_title, t.date AS doc_date,
               t.speech_type AS doc_speech_type
        FROM markets m
        LEFT JOIN events e ON e.event_ticker = m.event_ticker
        JOIN event_doc_map edm ON edm.event_ticker = m.event_ticker
        JOIN transcripts t ON t.doc_id = edm.doc_id
        WHERE m.result IN ('yes','no') AND m.series_ticker NOT IN ({excl})
        GROUP BY m.ticker
    """
    try:
        df = pd.read_sql_query(q, conn, params=list(config.LABELING_EXCLUDED_SERIES))
    finally:
        conn.close()
    if df.empty:
        raise SystemExit("no mapped market/doc pairs — run the event mapper first")

    # computed but NOT exported: kept hidden so review stays blind
    def _s(v):
        """LEFT JOINs give NaN, not None; the classifiers expect text."""
        return "" if v is None or (isinstance(v, float) and pd.isna(v)) else str(v)

    df["_market_type"] = [classify_market(_s(a) or _s(b)) for a, b in
                          zip(df["event_title"], df["market_title"])]
    df["_doc_type"] = [classify_doc(_s(st), _s(ti)) for st, ti in
                       zip(df["doc_speech_type"], df["doc_title"])]
    df["era"] = df["doc_date"].astype(str).str[:10].map(_era)

    sample = stratified_sample(df, n, ["era", "series_ticker", "_market_type"], seed)
    hidden = sample[["ticker", "_market_type", "_doc_type"]].copy()
    blind = sample.drop(columns=["_market_type", "_doc_type"])
    for c in TAXONOMY_COLUMNS:
        blind[c] = ""

    path = paths.data_path(out_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    blind.to_csv(path, index=False)
    key = path.with_name(path.stem + "_key.csv")
    hidden.to_csv(key, index=False)
    print(f"Wrote {path} — {len(blind)} BLIND rows (classifier labels withheld).")
    print(f"Answer key held separately at {key}; do not open it while reviewing.")
    print(f"Fill `review_event_type` with one of: {', '.join(TYPES)}")
    return blind


def import_taxonomy(review_path, out_dir) -> list[str]:
    """Confusion matrices for each classifier against human reference labels."""
    rev = pd.read_csv(paths.data_path(review_path))
    if "review_event_type" not in rev.columns:
        raise SystemExit("review file has no `review_event_type` column")
    key_path = paths.data_path(review_path)
    key_path = key_path.with_name(key_path.stem.replace("_reviewed", "") + "_key.csv")
    if not key_path.exists():
        raise SystemExit(f"answer key not found at {key_path}")
    key = pd.read_csv(key_path)
    df = rev.merge(key, on="ticker", how="inner")
    df["review_event_type"] = df["review_event_type"].fillna("").astype(str).str.strip()
    df = df[df["review_event_type"] != ""]

    lines = ["# Event-type taxonomy audit (T7b)", ""]
    if df.empty:
        lines.append("No rows carry a `review_event_type` yet.")
        out = paths.out_dir(out_dir)
        (out / "taxonomy_audit.md").write_text("\n".join(lines))
        return lines

    lines.append(f"{len(df)} reviewed pairs. Reference labels were assigned blind.")
    lines.append("")
    for col, label in (("_market_type", "classify_market (Kalshi titles)"),
                       ("_doc_type", "classify_doc (FactBase speech_type)")):
        acc = float((df[col] == df["review_event_type"]).mean())
        lines.append(f"## {label} — accuracy {acc:.1%}")
        lines.append("")
        tab = pd.crosstab(df[col], df["review_event_type"])
        lines.append("| predicted \\\\ reference | " + " | ".join(tab.columns) + " |")
        lines.append("|" + "---|" * (len(tab.columns) + 1))
        for pred, row in tab.iterrows():
            lines.append(f"| {pred} | " + " | ".join(str(int(v)) for v in row) + " |")
        lines.append("")
    agree = float((df["_market_type"] == df["_doc_type"]).mean())
    lines.append(f"## The two classifiers agree with EACH OTHER {agree:.1%} of the time")
    lines.append("")
    lines.append("This is the number that matters for Component A's typed variants.")
    lines.append("Off-type history is down-weighted by TYPE_OFFMATCH_WEIGHT=0.15 on")
    lines.append("the assumption that the two taxonomies are commensurable. Where")
    lines.append("they disagree, that weighting is applied to the wrong documents.")
    if "era" in df.columns:
        lines.append("")
        lines.append("| era | market acc | doc acc | mutual agreement | n |")
        lines.append("|---|---|---|---|---|")
        for era, g in df.groupby("era"):
            lines.append(f"| {era} | "
                         f"{(g['_market_type']==g['review_event_type']).mean():.1%} | "
                         f"{(g['_doc_type']==g['review_event_type']).mean():.1%} | "
                         f"{(g['_market_type']==g['_doc_type']).mean():.1%} | {len(g)} |")
    out = paths.out_dir(out_dir)
    (out / "taxonomy_audit.md").write_text("\n".join(lines))
    print(f"Wrote {out}/taxonomy_audit.md")
    return lines


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--out", default="audit")
    ap.add_argument("--export-taxonomy", type=int, metavar="N")
    ap.add_argument("--import-taxonomy", metavar="FILE")
    ap.add_argument("--file", default="audit/taxonomy_sample.csv")
    args = ap.parse_args()
    if args.export_taxonomy:
        export_taxonomy(args.db, args.file, args.export_taxonomy)
    elif args.import_taxonomy:
        import_taxonomy(args.import_taxonomy, args.out)
    else:
        ap.error("choose --export-taxonomy N or --import-taxonomy FILE")


if __name__ == "__main__":
    main()
