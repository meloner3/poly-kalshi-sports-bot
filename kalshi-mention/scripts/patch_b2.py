"""Patch 2 for component_b: the empty-hits fallback array must be string-typed
(np.empty(0) defaults to float64, which cannot compare against date strings —
hit only by phrases with zero prior transcript occurrences)."""
from pathlib import Path

p = Path("kalshi_mention/component_b.py")
src = p.read_text()

old = '        h = hits.get(sk, np.empty(0))'
assert old in src, "expected fallback line not found"
src = src.replace(old, '        h = hits.get(sk, np.empty(0, dtype="<U10"))')
p.write_text(src)

import importlib
import numpy as np
import kalshi_mention.component_b as cb
importlib.reload(cb)
h = np.empty(0, dtype="<U10")
assert len(h[h < "2026-01-01"]) == 0  # the exact operation that crashed
print("Patched. Empty-hit phrases now compare cleanly.")