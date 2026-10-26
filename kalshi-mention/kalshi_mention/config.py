"""Configuration constants.

Every assumption that affects backtest correctness lives here, explicitly,
so it can be audited and overridden rather than buried in code.
"""

from __future__ import annotations

import math
import os

from . import paths

# ---------------------------------------------------------------------------
# Kalshi API
# ---------------------------------------------------------------------------
# Public market-data endpoints require NO authentication. Trading/portfolio
# endpoints (not used in this pass) require RSA-PSS signed requests.
KALSHI_API_BASE = "https://api.elections.kalshi.com/trade-api/v2"

# Conservative throttle: public read limit is documented around 10-30 req/s;
# we stay far below it because this is a batch collection job, not latency-
# sensitive.
MIN_REQUEST_INTERVAL_SECONDS = 0.25
MAX_RETRIES = 5
RETRY_BACKOFF_BASE_SECONDS = 2.0
REQUEST_TIMEOUT_SECONDS = 30

# Trump word/phrase mention series identified so far. `collect.py` accepts
# --series to extend this list without a code change if new series appear.
MENTION_SERIES = [
    "KXTRUMPMENTION",   # per-event: "What will Trump say during X?"
    "KXTRUMPMENTIONB",  # per-event, secondary batch of the same market type
    "KXPRESMENTION",    # per-event, "President mention" variant
    "KXTRUMPSAY",       # weekly aggregate: "What will Trump say this week?"
]

# Weekly-aggregate series are collected for the taxonomy but EXCLUDED from
# per-event labeling/audit in this pass (confirmed out of scope: requires an
# event-arrival model that does not exist yet).
LABELING_EXCLUDED_SERIES = {"KXTRUMPSAY"}

# ---------------------------------------------------------------------------
# Point-in-time assumptions
# ---------------------------------------------------------------------------
# Historical transcripts have no ingestion timestamp (the scraper predates the
# `scraped_at` patch in patches/). For point-in-time queries we therefore
# assume a transcript became available this many hours after the event date
# (midnight ET of the event day + this lag). This is a DOCUMENTED ASSUMPTION,
# not a fact; the patched scraper records real timestamps going forward so the
# assumption only applies to pre-patch history.
ASSUMED_TRANSCRIPT_AVAILABILITY_LAG_HOURS = 24

# Timezone in which "the event's calendar date" is defined. Kalshi close times
# are UTC; FactBase dates are US-eastern calendar dates.
EVENT_TIMEZONE = "America/New_York"

# ---------------------------------------------------------------------------
# Storage
# ---------------------------------------------------------------------------
# The database does not live next to the code: this package is git-tracked
# inside DelphiResearch, the ~700MB sqlite file is not. See paths.py for the
# resolution rules and $KALSHI_MENTION_DATA_DIR to override.
DEFAULT_DB_PATH = str(paths.db_path())

# Default corpus filename inside the data dir. Use paths.corpus_path() to
# actually locate a corpus — it also falls back to DelphiResearch's
# trump_transcripts_clean.parquet.
DEFAULT_CORPUS_NAME = "trump_transcripts_merged.parquet"

# The predictor must abstain rather than silently decay toward its prior
# when transcript data is too old to support a trustworthy estimate.
PREDICT_MAX_CORPUS_AGE_DAYS = float(os.environ.get(
    "KALSHI_MENTION_MAX_CORPUS_AGE_DAYS", "14"
))
if not math.isfinite(PREDICT_MAX_CORPUS_AGE_DAYS) or PREDICT_MAX_CORPUS_AGE_DAYS <= 0:
    raise ValueError("KALSHI_MENTION_MAX_CORPUS_AGE_DAYS must be a finite positive number")

SIGNAL_MIN_EFFECTIVE_N = float(os.environ.get("KALSHI_MENTION_SIGNAL_MIN_EFFECTIVE_N", "20"))
SIGNAL_MIN_CALIBRATION_MARKETS = int(os.environ.get(
    "KALSHI_MENTION_SIGNAL_MIN_CALIBRATION_MARKETS", "300"
))
SIGNAL_PRIOR_STRENGTH = float(os.environ.get("KALSHI_MENTION_SIGNAL_PRIOR_STRENGTH", "4"))
SIGNAL_CALIBRATION_BOOTSTRAPS = int(os.environ.get(
    "KALSHI_MENTION_SIGNAL_CALIBRATION_BOOTSTRAPS", "500"
))
SIGNAL_MIN_VALID_BOOTSTRAPS = int(os.environ.get(
    "KALSHI_MENTION_SIGNAL_MIN_VALID_BOOTSTRAPS", "400"
))
SIGNAL_INTERVAL_LOW = float(os.environ.get("KALSHI_MENTION_SIGNAL_INTERVAL_LOW", "0.10"))
SIGNAL_INTERVAL_HIGH = float(os.environ.get("KALSHI_MENTION_SIGNAL_INTERVAL_HIGH", "0.90"))
SIGNAL_REFRESH_SECONDS = int(os.environ.get("KALSHI_MENTION_SIGNAL_REFRESH_SECONDS", "300"))
SIGNAL_PORT = int(os.environ.get("KALSHI_MENTION_SIGNAL_PORT", "8765"))

if not math.isfinite(SIGNAL_MIN_EFFECTIVE_N) or SIGNAL_MIN_EFFECTIVE_N <= 0:
    raise ValueError("KALSHI_MENTION_SIGNAL_MIN_EFFECTIVE_N must be finite and positive")
if (SIGNAL_MIN_CALIBRATION_MARKETS < 1 or not math.isfinite(SIGNAL_PRIOR_STRENGTH)
    or SIGNAL_PRIOR_STRENGTH <= 0):
    raise ValueError("Signal calibration market minimum and prior strength must be positive")
if (SIGNAL_CALIBRATION_BOOTSTRAPS < 1 or SIGNAL_MIN_VALID_BOOTSTRAPS < 1
        or SIGNAL_MIN_VALID_BOOTSTRAPS > SIGNAL_CALIBRATION_BOOTSTRAPS):
    raise ValueError("Signal bootstrap counts are inconsistent")
if not (0 < SIGNAL_INTERVAL_LOW < SIGNAL_INTERVAL_HIGH < 1):
    raise ValueError("Signal interval quantiles must satisfy 0 < low < high < 1")
if SIGNAL_REFRESH_SECONDS < 1 or not (1 <= SIGNAL_PORT <= 65535):
    raise ValueError("Signal refresh seconds and port are out of range")
