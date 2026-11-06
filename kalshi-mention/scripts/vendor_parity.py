"""Vendoring parity check.

predict_phrase() measures document age from now(), so two runs seconds apart
differ in the last decimal places. That is documented upstream behaviour, not
a vendoring defect -- but it means a naive diff can never be clean. This pins
the clock to a fixed instant and runs the SAME phrase through both trees, so
any remaining difference is attributable to the vendoring.

Run with either venv; it drives both trees via sys.path. Point
$KALSHI_MENTION_UPSTREAM at the upstream checkout if it is not in the default
location. Re-run this after every re-vendoring.

    kalshi-mention/.venv/bin/python kalshi-mention/scripts/vendor_parity.py
"""
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path

FROZEN = datetime(2026, 9, 10, 12, 0, 0, tzinfo=timezone.utc)

# Override when the upstream checkout lives elsewhere.
UPSTREAM = Path(os.environ.get("KALSHI_MENTION_UPSTREAM",
                               Path.home() / "Desktop/Git/DelphiResearch"))
VENDORED = Path(__file__).resolve().parent.parent

TREES = {"upstream": str(UPSTREAM), "vendored": str(VENDORED)}
DBS = {
    "upstream": str(UPSTREAM / "data/kalshi_mention/kalshi_mention.sqlite"),
    "vendored": str(VENDORED.parent / "data/kalshi_mention/kalshi_mention.sqlite"),
}

CASES = [
    ("China / Chinese", "interview"),
    ("Iran (3+ times)", "pressconf"),
    ("Fake News", "speech"),
    ("tariff / tariffs", "any"),
]


class FrozenDatetime(datetime):
    @classmethod
    def now(cls, tz=None):
        return FROZEN if tz else FROZEN.replace(tzinfo=None)


def run_tree(name: str) -> dict:
    # Drop any previously imported copy so the other tree's modules load clean.
    for mod in list(sys.modules):
        if mod == "kalshi_mention" or mod.startswith("kalshi_mention."):
            del sys.modules[mod]
    sys.path.insert(0, TREES[name])
    try:
        from kalshi_mention import db, predict

        loaded = predict.__file__
        assert loaded.startswith(TREES[name]), f"loaded wrong tree: {loaded}"

        predict.datetime = FrozenDatetime  # pin the clock
        conn = db.connect(DBS[name])
        cal = predict.fit_calibration(conn)
        out = {"_calibration": list(cal), "_module": loaded}
        for phrase, etype in CASES:
            out[f"{phrase} @ {etype}"] = predict.predict_phrase(conn, phrase, etype, cal)
        conn.close()
        return out
    finally:
        sys.path.remove(TREES[name])


results = {name: run_tree(name) for name in TREES}

print("\n" + "=" * 72)
mismatches = 0
for key in results["upstream"]:
    if key == "_module":
        continue
    u, v = results["upstream"][key], results["vendored"][key]
    same = u == v
    mismatches += not same
    print(f"{'OK ' if same else 'DIFF'}  {key}")
    if not same:
        print(f"      upstream: {json.dumps(u, sort_keys=True)}")
        print(f"      vendored: {json.dumps(v, sort_keys=True)}")

print("=" * 72)
print(f"upstream module: {results['upstream']['_module']}")
print(f"vendored module: {results['vendored']['_module']}")
print(f"\n{'PARITY CONFIRMED: identical at a pinned clock' if not mismatches else f'{mismatches} MISMATCH(ES)'}")
sys.exit(1 if mismatches else 0)
