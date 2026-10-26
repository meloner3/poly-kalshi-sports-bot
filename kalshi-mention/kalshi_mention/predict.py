"""Ask the model a question, get a number.

    python -m kalshi_mention.predict --phrase "China / Chinese" --event-type interview
    python -m kalshi_mention.predict --phrase "Iran (3+ times)" --event-type pressconf
    python -m kalshi_mention.predict --all-open          # every open Kalshi mention market

Produces the calibrated probability that Trump says the phrase at least once
(or k+ times) at his next event of the given type, using everything the
project validated: typed recency-weighted occurrence rates over the full
transcript corpus, threshold-aware counting under Kalshi's exact spec
(variant lists, "(N+ times)"), and a Platt calibration layer fitted on all
resolved market outcomes so the number means what it says.

Event types: speech, pressconf, gaggle, interview, meeting, vlog, any.
Phrases accept Kalshi's format verbatim.
"""

from __future__ import annotations

import argparse
import json
import math
from datetime import datetime, timezone

import numpy as np

from . import config, db
from .component_a import (BETA_A, BETA_B, HALF_LIVES_DAYS, TYPE_OFFMATCH_WEIGHT,
                          _ts, apply_platt, fit_platt, logit, predict_models)
from .counts import count_in_index, spec_variant_token_lists
from .event_types import TYPES, classify_doc, classify_market
from .phrase import extract_phrase, parse_phrase_spec, spec_key, tokenize


def load_docs(conn):
    docs = conn.execute(
        "SELECT doc_id, date, available_at, token_count, speech_type, title, text_raw "
        "FROM transcripts WHERE token_count IS NOT NULL ORDER BY available_at"
    ).fetchall()
    lengths = np.array([float(d["token_count"] or 0) for d in docs])
    event_ts = np.array([_ts(str(d["date"])[:10] + "T12:00:00+00:00") or 0.0 for d in docs])
    types = np.array([classify_doc(d["speech_type"], d["title"]) for d in docs])
    return docs, lengths, event_ts, types


def count_spec_in_docs(docs, raw_phrase: str) -> np.ndarray:
    variants, _k = spec_variant_token_lists(raw_phrase)
    out = np.zeros(len(docs))
    for i, d in enumerate(docs):
        tokens = tokenize(d["text_raw"])
        positions: dict[str, list[int]] = {}
        for j, t in enumerate(tokens):
            positions.setdefault(t, []).append(j)
        out[i] = count_in_index(variants, positions, tokens)
    return out


def fit_calibration(conn) -> tuple[float, float]:
    """Platt (a, b) from every resolved market's raw typed-empirical forecast.
    Uses the stored phrase_doc_counts, so it is fast and reproducible."""
    rows = conn.execute(
        f"""SELECT m.ticker, m.result, m.open_time, m.title AS mtitle, m.raw_json,
                   e.title AS etitle
            FROM markets m LEFT JOIN events e ON m.event_ticker = e.event_ticker
            WHERE m.result IN ('yes','no')
              AND m.series_ticker NOT IN ({','.join('?' * len(config.LABELING_EXCLUDED_SERIES)) or "''"})""",
        list(config.LABELING_EXCLUDED_SERIES),
    ).fetchall()
    docs = conn.execute(
        "SELECT doc_id, date, available_at, token_count, speech_type, title "
        "FROM transcripts WHERE token_count IS NOT NULL ORDER BY available_at"
    ).fetchall()
    avail = np.array([_ts(d["available_at"]) or math.inf for d in docs])
    lengths = np.array([float(d["token_count"] or 0) for d in docs])
    event_ts = np.array([_ts(str(d["date"])[:10] + "T12:00:00+00:00") or 0.0 for d in docs])
    types = np.array([classify_doc(d["speech_type"], d["title"]) for d in docs])
    doc_index = {d["doc_id"]: i for i, d in enumerate(docs)}
    counts_by_spec: dict[str, np.ndarray] = {}
    for r in conn.execute("SELECT spec_key, doc_id, count FROM phrase_doc_counts"):
        arr = counts_by_spec.setdefault(r["spec_key"], np.zeros(len(docs)))
        i = doc_index.get(r["doc_id"])
        if i is not None:
            arr[i] = r["count"]
    zero = np.zeros(len(docs))

    ps, ys = [], []
    for m in rows:
        phrase, _ = extract_phrase(json.loads(m["raw_json"]))
        if not phrase:
            continue
        _alts, k = parse_phrase_spec(phrase)
        as_of = _ts(m["open_time"])
        if as_of is None:
            continue
        n = int(np.searchsorted(avail, as_of, side="right"))
        if n < 30:
            continue
        mtype = classify_market(m["etitle"] or m["mtitle"])
        age = np.maximum((as_of - event_ts[:n]) / 86400.0, 0.0)
        w = np.power(0.5, age / 30.0) * np.where(types[:n] == mtype, 1.0, TYPE_OFFMATCH_WEIGHT)
        pr = predict_models(counts_by_spec.get(spec_key(phrase), zero)[:n], lengths[:n], w, k)
        if pr:
            ps.append(pr["empirical"])
            ys.append(1.0 if m["result"] == "yes" else 0.0)
    ps, ys = np.array(ps), np.array(ys)
    if len(ps) < 200 or float(np.std(logit(ps))) < 0.5:
        print(f"  calibration: insufficient spread ({len(ps)} markets) — using identity")
        return 0.0, 1.0
    a, b = fit_platt(logit(ps), ys)
    if not (0.2 <= b <= 5.0):   # degenerate/inverted fit — never trust it
        print(f"  calibration: degenerate slope b={b:.2f} — using identity")
        return 0.0, 1.0
    print(f"  calibration fitted on {len(ps)} resolved markets (a={a:+.2f}, b={b:.2f})")
    return float(a), float(b)


# A 30-day half-life means evidence decays fast: a corpus that ended N days
# ago contributes sum(w) ~ 41 * 0.5**(N/30). Past ~90 days the Jeffreys prior
# dominates and EVERY phrase drifts toward 0.5 -- silently, with no error.
# Measured: corpus 130d stale -> a never-observed phrase scores 0.16; 250d
# stale -> 0.44. These are the thresholds at which the output stops meaning
# anything.
CORPUS_STALE_WARN_DAYS = 45
CORPUS_STALE_FAIL_DAYS = 120


def corpus_staleness(conn) -> float | None:
    """Days since the newest transcript currently available to the model."""
    now = datetime.now(timezone.utc)
    r = conn.execute(
        "SELECT MAX(date) d FROM transcripts WHERE julianday(available_at) <= julianday(?)",
        (now.isoformat(timespec="seconds"),),
    ).fetchone()
    if not r or not r["d"]:
        return None
    try:
        last = datetime.fromisoformat(str(r["d"])[:10]).replace(tzinfo=timezone.utc)
    except ValueError:
        return None
    return (now - last).total_seconds() / 86400.0


def warn_if_stale(conn) -> float | None:
    """Print a staleness warning. Predictions degrade toward 0.5 silently."""
    age = corpus_staleness(conn)
    if age is None:
        return None
    if age >= CORPUS_STALE_FAIL_DAYS:
        print(f"  !! CORPUS {age:.0f} DAYS STALE — recency weights have decayed to "
              f"{0.5 ** (age/30.0):.1%} of fresh; every probability is being pulled "
              "toward 0.5 and these numbers should not be used. "
              "Run: python -m kalshi_mention.pipeline --only scrape")
    elif age >= CORPUS_STALE_WARN_DAYS:
        print(f"  ! corpus {age:.0f} days stale (weights at "
              f"{0.5 ** (age/30.0):.0%} of fresh) — refresh before trusting edges.")
    return age


def require_fresh_corpus(conn, max_age_days: float = config.PREDICT_MAX_CORPUS_AGE_DAYS) -> float:
    """Fail closed when the predictor's SQLite corpus is absent or stale."""
    if not math.isfinite(max_age_days) or max_age_days <= 0:
        raise ValueError("max_age_days must be a finite positive number")
    age = corpus_staleness(conn)
    if age is None:
        raise ValueError("transcript corpus has no usable dates; prediction abstained")
    if age > max_age_days:
        raise ValueError(
            f"transcript corpus is {age:.1f} days old (maximum {max_age_days:g}); "
            "prediction abstained. Refresh with: python -m kalshi_mention.refresh_corpus"
        )
    return age


def predict_phrase(conn, raw_phrase: str, event_type: str,
                   cal: tuple[float, float]) -> dict:
    docs, lengths, event_ts, types = load_docs(conn)
    counts = count_spec_in_docs(docs, raw_phrase)
    _alts, k = parse_phrase_spec(raw_phrase)
    now = datetime.now(timezone.utc).timestamp()
    age = np.maximum((now - event_ts) / 86400.0, 0.0)
    tmask = np.ones(len(docs)) if event_type == "any" else \
        np.where(types == event_type, 1.0, TYPE_OFFMATCH_WEIGHT)
    w = np.power(0.5, age / 30.0) * tmask
    pr = predict_models(counts, lengths, w, k)
    p_cal = float(apply_platt(np.array([pr["empirical"]]), *cal)[0])
    same = types == event_type if event_type != "any" else np.ones(len(docs), bool)
    recent = age <= 90
    return {
        "p": p_cal,
        "p_raw": pr["empirical"],
        "k": k,
        "n_same_type": int(same.sum()),
        "n_same_type_hit": int(((counts >= k) & same).sum()),
        "recent_rate": float(((counts >= k) & same & recent).sum() / max((same & recent).sum(), 1)),
        "last_hit_days": (float(age[counts >= k].min()) if (counts >= k).any() else None),
    }


CALIBRATION_CAVEAT = (
    "  note: calibration was learned on Kalshi-LISTED strikes, which Kalshi "
    "selects to sit near 50/50.\n"
    "        For a phrase you chose yourself (not a listed market) that "
    "mapping may not apply\n"
    "        and the raw rate is often the better estimate. Both are shown.")


def render(phrase: str, event_type: str, r: dict) -> str:
    last = "never" if r["last_hit_days"] is None else f"{r['last_hit_days']:.0f}d ago"
    need = "" if r["k"] == 1 else f" ({r['k']}+ times)"
    return (f"P(says {phrase!r}{need} at next {event_type}) = {r['p']:.2f}\n"
            f"  evidence: {r['n_same_type_hit']}/{r['n_same_type']} prior "
            f"{event_type} events contained it; last-90d rate {r['recent_rate']:.2f}; "
            f"last occurrence {last}; raw {r['p_raw']:.2f} -> calibrated {r['p']:.2f}\n"
            + CALIBRATION_CAVEAT)


def all_open(conn, cal) -> None:
    from .kalshi_client import KalshiPublicClient
    client = KalshiPublicClient()
    rows = []
    for st in config.MENTION_SERIES:
        if st in config.LABELING_EXCLUDED_SERIES:
            continue
        try:
            for m in client.iter_markets(st, status="open"):
                phrase, _ = extract_phrase(m)
                if not phrase:
                    continue
                mtype = classify_market(m.get("title"))
                rows.append((m.get("ticker"), phrase, mtype))
        except RuntimeError as e:
            print(f"  ! {st}: {e}")
    if not rows:
        print("No open mention markets found.")
        return
    print(f"{len(rows)} open markets:\n")
    docs_cache = {}
    for ticker, phrase, mtype in rows:
        r = predict_phrase(conn, phrase, mtype, cal)
        print(f"{r['p']:.2f}  {ticker:<38} {phrase[:40]:<42} [{mtype}] "
              f"({r['n_same_type_hit']}/{r['n_same_type']} prior)")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--phrase", default=None, help='Kalshi format, e.g. "China / Chinese" or "Iran (3+ times)"')
    ap.add_argument("--event-type", default="any", choices=TYPES + ["any"])
    ap.add_argument("--all-open", action="store_true", help="score every open Kalshi mention market")
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    args = ap.parse_args()
    if not args.phrase and not args.all_open:
        ap.error("give --phrase (with optional --event-type) or --all-open")

    conn = db.connect(args.db)
    try:
        require_fresh_corpus(conn)
    except ValueError as exc:
        conn.close()
        raise SystemExit(str(exc)) from exc
    warn_if_stale(conn)
    n = conn.execute("SELECT COUNT(*) c FROM transcripts").fetchone()["c"]
    if n == 0:
        raise SystemExit("transcripts table empty — run the pipeline first")
    print("fitting calibration on resolved markets...", flush=True)
    cal = fit_calibration(conn)

    if args.all_open:
        all_open(conn, cal)
    else:
        r = predict_phrase(conn, args.phrase, args.event_type, cal)
        if args.json:
            print(json.dumps({"phrase": args.phrase, "event_type": args.event_type, **r}))
        else:
            print()
            print(render(args.phrase, args.event_type, r))
    conn.close()


if __name__ == "__main__":
    main()
