"""Sample manifests — every report says exactly what data produced it.

Reports in this project have quoted market counts that could not later be
reconciled (T0 found 8,952 / 8,260 / 7,550 / 7,682 in circulation, one of
which no query reproduces). A number without its denominator and vintage is
not a result. Every report writer now appends `render(manifest(...))`, so
any two reports can be checked for having come from the same database state.

The "dataset identity" line is the part to compare: db path, size, mtime and
settled-market count. If two reports disagree there, they are not comparable
and no cross-report claim built on them is valid.
"""

from __future__ import annotations

import os
from datetime import datetime, timezone

import pandas as pd

from . import cluster, config, paths

HIST_CUTOFF = "2026-05-27"
_MONTHS = {m: i + 1 for i, m in enumerate(
    ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"])}


def day_to_date(d) -> str | None:
    """Event-day code (26JUL24) -> ISO date. Ticker dates are authoritative."""
    if not isinstance(d, str) or len(d) < 7:
        return None
    core = d[-7:]
    try:
        return "20%s-%02d-%02d" % (core[:2], _MONTHS[core[2:5]], int(core[5:7]))
    except (KeyError, ValueError):
        return None


def _db_file(conn) -> str | None:
    try:
        for row in conn.execute("PRAGMA database_list"):
            if row[1] == "main" and row[2]:
                return row[2]
    except Exception:
        pass
    return str(paths.db_path())


def manifest(conn, preds_df: pd.DataFrame | None = None,
             snaps_df: pd.DataFrame | None = None) -> dict:
    """Everything needed to identify the sample a report was built from."""
    m: dict = {"generated_at": datetime.now(timezone.utc).isoformat(timespec="seconds")}

    path = _db_file(conn)
    m["db_path"] = path
    if path and os.path.exists(path):
        st = os.stat(path)
        m["db_bytes"] = st.st_size
        m["db_mtime"] = datetime.fromtimestamp(st.st_mtime, tz=timezone.utc).isoformat(
            timespec="seconds")
    else:
        m["db_bytes"] = m["db_mtime"] = None

    series = {}
    total_all = total_settled = 0
    for r in conn.execute("""SELECT series_ticker, COUNT(*) n,
                                    SUM(result IN ('yes','no')) s
                             FROM markets GROUP BY series_ticker ORDER BY series_ticker"""):
        series[r["series_ticker"]] = {"all": int(r["n"]), "settled": int(r["s"] or 0)}
        total_all += int(r["n"])
        total_settled += int(r["s"] or 0)
    m["series"] = series
    m["markets_all"] = total_all
    m["markets_settled"] = total_settled

    excluded = sorted(config.LABELING_EXCLUDED_SERIES)
    m["excluded_series"] = excluded
    m["markets_excluded"] = sum(series.get(s, {}).get("settled", 0) for s in excluded)
    m["markets_modeled"] = total_settled - m["markets_excluded"]

    m["events"] = conn.execute("SELECT COUNT(*) c FROM events").fetchone()["c"]
    m["transcripts"] = conn.execute("SELECT COUNT(*) c FROM transcripts").fetchone()["c"]
    r = conn.execute("SELECT MIN(date) a, MAX(date) b FROM transcripts").fetchone()
    m["corpus_range"] = (str(r["a"])[:10] if r["a"] else None,
                         str(r["b"])[:10] if r["b"] else None)
    m["live_era_boundary"] = HIST_CUTOFF

    if preds_df is not None and len(preds_df):
        days = cluster.event_day(preds_df["ticker"])
        dates = days.map(day_to_date)
        m["preds_rows"] = int(len(preds_df))
        m["event_days"] = int(days.nunique())
        m["event_days_live"] = int(days[dates >= HIST_CUTOFF].nunique())
        m["preds_date_range"] = (dates.min(), dates.max())
        m["markets_per_day_median"] = float(preds_df.groupby(days).size().median())

    if snaps_df is not None and len(snaps_df):
        q = snaps_df[snaps_df["bid"].notna() & snaps_df["ask"].notna()]
        days = cluster.event_day(q["ticker"])
        dates = days.map(day_to_date)
        per = {}
        for h in sorted(snaps_df["h"].unique()):
            sel = q["h"] == h
            live = sel & (dates >= HIST_CUTOFF).to_numpy()
            per[int(h)] = {"quoted_rows": int(sel.sum()),
                           "quoted_days": int(days[sel].nunique()),
                           "quoted_rows_live": int(live.sum()),
                           "quoted_days_live": int(days[live].nunique())}
        m["snapshots"] = per
    return m


def identity_line(m: dict) -> str:
    """The one line two reports must agree on to be comparable."""
    mb = f"{m['db_bytes']/1e6:.1f}MB" if m.get("db_bytes") else "?"
    return (f"`{m.get('db_path')}` | {mb} | mtime {m.get('db_mtime')} | "
            f"{m.get('markets_settled')} settled")


def render(m: dict) -> list[str]:
    """Markdown lines for the '## Sample manifest' section of a report."""
    L = ["", "## Sample manifest", ""]
    L.append(f"**Dataset identity:** {identity_line(m)}")
    L.append("")
    L.append(f"Generated {m['generated_at']}. Two reports are only comparable if")
    L.append("their dataset identity lines match exactly.")
    L.append("")
    L.append("| quantity | value |")
    L.append("|---|---|")
    L.append(f"| markets, all | {m.get('markets_all')} |")
    L.append(f"| markets, settled | {m.get('markets_settled')} |")
    L.append(f"| excluded series | {', '.join(m.get('excluded_series') or []) or 'none'} "
             f"({m.get('markets_excluded')} settled rows) |")
    L.append(f"| markets, modeled | {m.get('markets_modeled')} |")
    if "preds_rows" in m:
        L.append(f"| markets in this report | {m['preds_rows']} |")
        L.append(f"| **unique event-days** | **{m['event_days']}** "
                 f"(live era: {m['event_days_live']}) |")
        L.append(f"| markets per event-day (median) | {m['markets_per_day_median']:.0f} |")
        L.append(f"| market date range | {m['preds_date_range'][0]} .. "
                 f"{m['preds_date_range'][1]} |")
    L.append(f"| events | {m.get('events')} |")
    L.append(f"| transcripts | {m.get('transcripts')} |")
    L.append(f"| corpus range | {m['corpus_range'][0]} .. {m['corpus_range'][1]} |")
    L.append(f"| live-era boundary | {m.get('live_era_boundary')} |")
    L.append("")
    L.append("Event-days, not markets, are the unit of independence: markets on")
    L.append("one day share a transcript and one realisation of the world.")
    if "snapshots" in m:
        L.append("")
        L.append("| snapshot | quoted rows | days | quoted rows (live) | days (live) |")
        L.append("|---|---|---|---|---|")
        for h, v in sorted(m["snapshots"].items()):
            L.append(f"| {h}h | {v['quoted_rows']} | {v['quoted_days']} | "
                     f"{v['quoted_rows_live']} | {v['quoted_days_live']} |")
    return L


def per_series_table(m: dict) -> list[str]:
    L = ["", "| series | all | settled |", "|---|---|---|"]
    for s, v in sorted(m.get("series", {}).items()):
        L.append(f"| {s} | {v['all']} | {v['settled']} |")
    return L


def append_to(lines: list[str], db_path: str | None = None, conn=None,
              preds_df: pd.DataFrame | None = None,
              snaps_df: pd.DataFrame | None = None) -> list[str]:
    """Append a '## Sample manifest' section to a report's line list.

    Opens its own connection when the caller has already closed theirs
    (several report writers close the db long before rendering).
    """
    from . import db as _db
    own = False
    if conn is None:
        conn = _db.connect(db_path or config.DEFAULT_DB_PATH)
        own = True
    try:
        lines.extend(render(manifest(conn, preds_df, snaps_df)))
    finally:
        if own:
            conn.close()
    return lines
