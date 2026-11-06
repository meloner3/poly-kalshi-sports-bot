"""Patch component_b: coerce transcript dates to ISO strings defensively
(SQLite dynamic typing can surface them as floats), so date comparisons
never hit mixed-dtype ufunc errors."""
from pathlib import Path

p = Path("kalshi_mention/component_b.py")
src = p.read_text()
assert "def _iso" not in src, "already patched"

old = '''    tmp: dict[str, list] = {}
    for r in q:
        tmp.setdefault(r["spec_key"], []).append(r["date"])
    for k, v in tmp.items():
        hits[k] = np.array(sorted(v))
    doc_dates = np.array(sorted(r["date"] for r in conn.execute("SELECT date FROM transcripts")))'''

new = '''    def _iso(x):
        s = str(x)[:10]
        return s if len(s) == 10 and s[4] == "-" else None

    tmp: dict[str, list] = {}
    for r in q:
        d = _iso(r["date"])
        if d:
            tmp.setdefault(r["spec_key"], []).append(d)
    for k, v in tmp.items():
        hits[k] = np.array(sorted(v))
    doc_dates = np.array(sorted(
        d for d in (_iso(r["date"]) for r in conn.execute("SELECT date FROM transcripts")) if d))'''

assert old in src
src = src.replace(old, new)

old2 = '''            gap = (datetime.fromisoformat(open_date) - datetime.fromisoformat(str(h_before[-1]))).days'''
assert old2 in src  # already string-safe via str(), keep as is
p.write_text(src)
print("Patched. Dates coerced to ISO strings everywhere.")