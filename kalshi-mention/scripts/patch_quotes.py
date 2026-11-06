"""Patch: zero/degenerate quotes (empty book placeholders) are not prices."""
from pathlib import Path
p = Path("kalshi_mention/market_baseline.py")
src = p.read_text()
old = '''    bid = _dollars(raw, "yes_bid")'''
assert old in src and "def quote_ok" not in src
src = src.replace('''def candle_prob(c: dict) -> float | None:''',
'''def quote_ok(c: dict) -> bool:
    """A quote is real only if both sides exist strictly inside (0,1) and the
    book saw volume. Historical candles encode an empty book as 0.0000/0.0000,
    which otherwise reads as a perfect free-money quote."""
    b, a = c.get("bid"), c.get("ask")
    return (b is not None and a is not None and 0.0 < b < 1.0 and 0.0 < a < 1.0
            and a >= b and c.get("volume", 0) > 0)


def candle_prob(c: dict) -> float | None:''')
src = src.replace('''    b, a = c["bid"], c["ask"]
    if b is not None and a is not None and a >= b and (a - b) <= 0.30:
        return (a + b) / 2.0''',
'''    b, a = c["bid"], c["ask"]
    if quote_ok(c) and (a - b) <= 0.30:
        return (a + b) / 2.0''')
src = src.replace('''            row = {"ticker": r["ticker"], "h": h, "y": int(r["y"]),
                   "p_market": p_mkt, "bid": c["bid"], "ask": c["ask"],''',
'''            tradeable = quote_ok(c)
            row = {"ticker": r["ticker"], "h": h, "y": int(r["y"]),
                   "p_market": p_mkt,
                   "bid": c["bid"] if tradeable else None,
                   "ask": c["ask"] if tradeable else None,''')
p.write_text(src)
import importlib, kalshi_mention.market_baseline as mb
importlib.reload(mb)
assert not mb.quote_ok({"bid": 0.0, "ask": 0.0, "volume": 5})
assert not mb.quote_ok({"bid": 0.10, "ask": 0.14, "volume": 0})
assert mb.quote_ok({"bid": 0.10, "ask": 0.14, "volume": 5})
print("Patched: zero-quotes and zero-volume candles are no longer tradeable.")