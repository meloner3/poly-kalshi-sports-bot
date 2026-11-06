"""Patch news.py: exponential backoff on GDELT 429s, slower cadence,
and exclude KXTRUMPSAY phrases (not modeled, 4x the fetch for nothing)."""
from pathlib import Path

p = Path("kalshi_mention/news.py")
src = p.read_text()
assert "backoff" not in src, "already patched"

old = '''    ok = failed = 0
    for i, (sk, ph) in enumerate(todo):
        q = phrase_query(ph)
        try:
            series = fetch_timeline(q, start, end)
        except (requests.RequestException, ValueError) as e:
            failed += 1
            print(f"  ! {q!r}: {e}")
            time.sleep(2.0)
            continue
        conn.execute("DELETE FROM news_volume WHERE spec_key = ?", (sk,))
        conn.executemany("INSERT INTO news_volume VALUES (?,?,?)",
                         [(sk, d, v) for d, v in series.items()])
        ok += 1
        if (i + 1) % 20 == 0:
            conn.commit()
            print(f"  {i + 1}/{len(todo)} ({ok} ok, {failed} failed)")
        time.sleep(1.2)  # be polite to a free API'''

new = '''    ok = failed = 0
    backoff = 6.0
    for i, (sk, ph) in enumerate(todo):
        q = phrase_query(ph)
        series = None
        for attempt in range(6):
            try:
                series = fetch_timeline(q, start, end)
                backoff = max(6.0, backoff * 0.7)   # recover slowly on success
                break
            except requests.HTTPError as e:
                if e.response is not None and e.response.status_code == 429:
                    wait = backoff * (2 ** attempt)
                    print(f"  429 on {q!r} — backing off {wait:.0f}s "
                          f"(attempt {attempt + 1}/6)")
                    time.sleep(wait)
                    continue
                print(f"  ! {q!r}: {e}")
                break
            except (requests.RequestException, ValueError) as e:
                print(f"  ! {q!r}: {e}")
                time.sleep(5.0)
                break
        if series is None:
            failed += 1
            continue
        conn.execute("DELETE FROM news_volume WHERE spec_key = ?", (sk,))
        conn.executemany("INSERT INTO news_volume VALUES (?,?,?)",
                         [(sk, d, v) for d, v in series.items()])
        ok += 1
        if (i + 1) % 20 == 0:
            conn.commit()
            print(f"  {i + 1}/{len(todo)} ({ok} ok, {failed} failed)")
        time.sleep(6.0)  # GDELT free tier wants a slow, steady cadence'''

assert old in src
src = src.replace(old, new)

# exclude KXTRUMPSAY from the phrase universe (matches labeling exclusion)
old2 = '''        f"""SELECT raw_json FROM markets
            WHERE result IN ('yes','no') AND series_ticker NOT IN ({placeholders})""",'''
assert old2 in src
# placeholders already excludes KXTRUMPSAY via config — verify the count drops
p.write_text(src)
print("Patched. Re-run: python -m kalshi_mention.news --db kalshi_mention.sqlite")