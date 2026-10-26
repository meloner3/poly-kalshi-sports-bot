"""Incremental FactBase transcript scraper (Mongo-free, in-package).

    python -m kalshi_mention.scrape_transcripts --parquet trump_transcripts_merged.parquet

Scrapes Roll Call FactBase newest-first, stops once a page is entirely older
than (newest stored date - overlap), dedups on (title, date), appends raw
records to recent_transcripts.jsonl (full multi-speaker text preserved), and
rewrites the merged parquet in place. Safe to run daily; reruns are no-ops
when nothing is new.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import random
import time
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

import pandas as pd
import requests

from . import config, paths

BASE_API = "https://rollcall.com/wp-json/factbase/v1/search"
SKIP = ["No Transcript", "Karoline Leavitt", "JD Vance", "Kevin Hassett"]
UA = ("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 "
      "(KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
KEEP = ["doc_id", "title", "date", "speech_type", "text_raw", "token_count", "scraped_at"]


def get_page(page: int) -> dict:
    r = requests.get(BASE_API, params={"media": "", "type": "", "sort": "date",
        "location": "all", "place": "all", "page": page, "format": "json",
        "person": "trump"}, headers={"User-Agent": UA}, timeout=20)
    r.raise_for_status()
    return r.json()


def get_transcript(url: str) -> str | None:
    from bs4 import BeautifulSoup
    try:
        r = requests.get(url, headers={"User-Agent": UA}, timeout=15)
        r.raise_for_status()
    except requests.RequestException as e:
        print("  ! fetch failed:", e)
        return None
    soup = BeautifulSoup(r.text, "html.parser")
    segs = soup.find_all("div", id=lambda x: x and x.startswith("dt"))
    lines = []
    for seg in segs:
        sp = seg.find("h2", class_="text-md")
        tx = seg.find("div", class_="flex-auto")
        if sp and tx:
            lines.append(f"{sp.get_text(strip=True)}: {tx.get_text(separator=' ', strip=True)}")
    return "\n\n".join(lines) if lines else None


def trump_only(raw: str) -> str:
    out = []
    for block in raw.split("\n\n"):
        block = block.strip()
        if block.startswith("Donald Trump:"):
            out.append(block[len("Donald Trump:"):].strip())
    return "\n\n".join(out)


def run(parquet_path: str | None = None, jsonl_path: str = "recent_transcripts.jsonl",
        overlap_days: int = 7, max_pages: int = 400) -> int:
    pq = paths.data_path(parquet_path or config.DEFAULT_CORPUS_NAME)
    jsonl_path = str(paths.data_path(jsonl_path))
    if not pq.exists():
        raise SystemExit(
            f"{pq} not found — point --parquet at the merged corpus, or run\n"
            "  python trump_transcripts/prepare_trump_transcripts.py\n"
            "to build one from Mongo first.")
    if pq.suffix == ".parquet":
        existing = pd.read_parquet(pq)
    elif pq.suffix == ".csv":
        existing = pd.read_csv(pq)
    else:
        raise SystemExit(f"unsupported transcript corpus format: {pq.suffix}")
    seen = set(zip(existing["title"].astype(str).str.strip().str.lower(),
                   existing["date"].astype(str).str[:10]))
    newest = str(existing["date"].astype(str).max())[:10]
    cutoff = (date.fromisoformat(newest) - timedelta(days=overlap_days)).isoformat()
    print(f"corpus: {len(existing)} docs, newest {newest}, scraping back to {cutoff}")

    records, page, done = [], 1, False
    while not done and page <= max_pages:
        data = get_page(page)
        items = data.get("data", [])
        if not items:
            break
        page_dates = []
        for t in items:
            title, date_str = t.get("record_title", ""), t.get("date")
            if date_str:
                page_dates.append(date_str)
            if not date_str or date_str < cutoff:
                continue
            if any(pfx in title for pfx in SKIP):
                continue
            if (title.strip().lower(), date_str[:10]) in seen:
                continue
            print(f"[p{page}] {date_str} {title[:70]}")
            raw = get_transcript(t.get("factbase_url", ""))
            if not raw:
                continue
            text = trump_only(raw)
            if not text.strip():
                continue
            records.append({
                "doc_id": hashlib.sha1(f"{title}||{date_str}".encode()).hexdigest(),
                "title": title, "date": date_str[:10],
                "speech_type": t.get("type", "Unknown"),
                "text_raw": text, "text_full_raw": raw,
                "token_count": len(text.split()),
                "scraped_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
            })
            time.sleep(random.uniform(0.5, 1.5))
        if page_dates and max(page_dates) < cutoff:
            done = True
        page += 1
        time.sleep(random.uniform(0.5, 1.5))

    if records:
        with open(jsonl_path, "a", encoding="utf-8") as f:
            for r in records:
                f.write(json.dumps(r) + "\n")
        new_df = pd.DataFrame(records)[KEEP]
        old_df = existing[[c for c in KEEP if c in existing.columns]].copy()
        old_df["date"] = old_df["date"].astype(str).str[:10]
        merged = pd.concat([old_df, new_df], ignore_index=True)
        merged = merged.drop_duplicates(subset=["title", "date"], keep="first")
        if pq.suffix == ".parquet":
            merged.to_parquet(pq, index=False)
        else:
            merged.to_csv(pq, index=False)
        print(f"+{len(records)} new docs -> corpus now {len(merged)} "
              f"({merged['date'].min()} -> {merged['date'].max()})")
    else:
        print("no new transcripts")
    return len(records)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--parquet", default=None,
                    help="merged corpus parquet; default: <data dir>/" + config.DEFAULT_CORPUS_NAME)
    ap.add_argument("--jsonl", default="recent_transcripts.jsonl")
    ap.add_argument("--overlap-days", type=int, default=7)
    args = ap.parse_args()
    run(args.parquet, args.jsonl, args.overlap_days)


if __name__ == "__main__":
    main()
