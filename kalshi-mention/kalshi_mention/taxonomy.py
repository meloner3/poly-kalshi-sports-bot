"""Generate the Phase 0 market-taxonomy report from a collected database.

    python -m kalshi_mention.taxonomy --db kalshi_mention.sqlite --out taxonomy

Outputs:
    taxonomy/market_taxonomy.md   — human-readable catalog
    taxonomy/markets.csv          — one row per market with parsed phrase
    taxonomy/unparsed_markets.csv — markets needing a manual phrase override
"""

from __future__ import annotations

import argparse
import json
import re
from collections import Counter
from pathlib import Path

import pandas as pd

from . import config, db
from .phrase import extract_phrase
from . import paths


def _rules_template(rules: str, phrase: str | None) -> str:
    """Collapse a rules sentence into a template so distinct resolution
    languages can be counted (phrase and dates slotted out)."""
    t = rules or ""
    if phrase:
        t = t.replace(phrase, "<PHRASE>")
    t = re.sub(r"\b\d{4}\b", "<YEAR>", t)
    t = re.sub(r"\b\d+\b", "<N>", t)
    t = re.sub(r"\s+", " ", t).strip()
    return t[:300]


def build(db_path: str, out_dir: str) -> None:
    conn = db.connect(db_path)
    out = paths.out_dir(out_dir)

    rows = conn.execute("SELECT * FROM markets").fetchall()
    if not rows:
        raise SystemExit("No markets in DB — run kalshi_mention.collect first.")

    records = []
    for r in rows:
        market = json.loads(r["raw_json"])
        phrase, method = extract_phrase(market)
        records.append(
            {
                "ticker": r["ticker"],
                "series_ticker": r["series_ticker"],
                "event_ticker": r["event_ticker"],
                "market_type": r["market_type"],
                "title": r["title"],
                "phrase": phrase,
                "phrase_parse_method": method,
                "status": r["status"],
                "result": r["result"],
                "open_time": r["open_time"],
                "close_time": r["close_time"],
                "volume": r["volume"],
                "liquidity": r["liquidity"],
                "rules_primary": r["rules_primary"],
            }
        )
    mdf = pd.DataFrame(records)
    mdf.to_csv(out / "markets.csv", index=False)

    unparsed = mdf[mdf["phrase_parse_method"] == "unparsed"]
    unparsed.to_csv(out / "unparsed_markets.csv", index=False)

    settled = mdf[mdf["result"].isin(["yes", "no"])]
    lines = ["# Kalshi Trump-Mention Market Taxonomy", ""]
    lines.append(f"Generated from `{db_path}`. Markets: {len(mdf)}, settled yes/no: {len(settled)}.")
    lines.append("")

    lines.append("## Per-series summary")
    lines.append("")
    lines.append("| series | markets | settled | yes-rate (settled) | median volume | phrase parse ok |")
    lines.append("|---|---|---|---|---|---|")
    for st, grp in mdf.groupby("series_ticker"):
        g_settled = grp[grp["result"].isin(["yes", "no"])]
        yes_rate = (g_settled["result"] == "yes").mean() if len(g_settled) else float("nan")
        parse_ok = (grp["phrase_parse_method"] != "unparsed").mean()
        med_vol = grp["volume"].median()
        lines.append(
            f"| {st} | {len(grp)} | {len(g_settled)} | {yes_rate:.3f} | {med_vol} | {parse_ok:.1%} |"
        )
    lines.append("")

    lines.append("## Resolution-language templates (top 15)")
    lines.append("")
    lines.append("Distinct resolution wordings, with phrase/number slots removed. Every distinct")
    lines.append("template is a distinct resolution regime that the labeler spec must cover.")
    lines.append("")
    templates = Counter(
        _rules_template(rec["rules_primary"], rec["phrase"]) for rec in records if rec["rules_primary"]
    )
    for tmpl, n in templates.most_common(15):
        lines.append(f"- ({n}x) {tmpl}")
    lines.append("")

    lines.append("## Threshold / count markets")
    lines.append("")
    thr = mdf[mdf["rules_primary"].fillna("").str.contains(r"\btimes\b|\bat least\b \d", regex=True)]
    lines.append(f"{len(thr)} markets whose rules mention counts/thresholds — these need the")
    lines.append("P(count >= k) treatment, not the binary labeler. Listed in markets.csv.")
    lines.append("")

    lines.append("## Unparsed phrases")
    lines.append("")
    lines.append(f"{len(unparsed)} markets where no phrase could be auto-extracted; they are in")
    lines.append("`unparsed_markets.csv` and MUST get rows in the override CSV before labeling —")
    lines.append("the labeler refuses to guess.")

    (out / "market_taxonomy.md").write_text("\n".join(lines))
    conn.close()
    print(f"Wrote {out/'market_taxonomy.md'}, markets.csv ({len(mdf)}), unparsed_markets.csv ({len(unparsed)})")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--out", default="taxonomy")
    args = ap.parse_args()
    build(args.db, args.out)


if __name__ == "__main__":
    main()
