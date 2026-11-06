# Scraper patches (apply to your DelphiResearch repo)

Three fixes, agreed in scoping. All are backward-compatible: existing Mongo
documents are untouched; new/updated documents gain fields.

## 1. Record `scraped_at` (point-in-time correctness)

`models/trump_speech.py` — add a scraped-at field:

```python
from dataclasses import dataclass, field
from datetime import datetime, timezone

@dataclass
class TrumpTranscript:
    title: str
    text: str
    speech_type: str
    date: Optional[datetime] = None
    person: str = DEFAULT_PERSON
    text_full_raw: Optional[str] = None          # NEW: all speakers, labeled (fix 2)
    scraped_at: Optional[datetime] = None        # NEW

    def to_dict(self) -> dict:
        return {
            "title": self.title,
            "text": self.text,
            "text_full_raw": self.text_full_raw,
            "speech_type": self.speech_type,
            "date": self.date.isoformat() if self.date else None,
            "person": self.person,
            "scraped_at": (self.scraped_at or datetime.now(timezone.utc)).isoformat(),
        }
```

## 2. Keep the full multi-speaker raw text

In `trumpScraper.py::main`, the raw speaker-labeled text is currently thrown
away after `format_text`. Keep both:

```python
raw = get_transcript(url)
if raw:
    text = format_text(raw)
    ...
    speech = TrumpTranscript(
        title=title,
        text=text,
        text_full_raw=raw,          # NEW — needed to audit dropped speaker blocks
        speech_type=speech_type,
        date=date,
        scraped_at=datetime.now(timezone.utc),
    )
```

Why: `format_text` keeps only blocks starting with the exact string
`"Donald Trump:"`. Any label variant ("President Trump", etc.) is silently
discarded, which creates false-negative occurrence labels. With the full raw
stored, the label audit can measure how much Trump speech is being dropped.
Run this one-time check after a few scrapes with the patch:

```python
# blocks whose speaker label is NOT "Donald Trump" but might still be him
import re
for doc in coll.find({"text_full_raw": {"$ne": None}}).limit(200):
    for block in doc["text_full_raw"].split("\n\n"):
        speaker = block.split(":", 1)[0].strip()
        if speaker != "Donald Trump" and re.search(r"trump|president", speaker, re.I):
            print(doc["title"][:60], "->", speaker)
```

## 3. Revision-aware storage (rush transcripts get corrected)

`clients/mongodb_client.py::put_transcript` currently skips any (title, date)
it has seen, so FactBase corrections never land. Replace the duplicate-skip
with versioning: if text changed, keep the old version in a history array and
update in place.

```python
def put_transcript(self, transcript: TrumpTranscript) -> str:
    collection = self._db[COLLECTION_TRUMP_TRANSCRIPTS]
    key = {
        "title": transcript.title,
        "date": transcript.date.isoformat() if transcript.date else None,
    }
    new_doc = transcript.to_dict()
    existing = collection.find_one(key)

    if existing is None:
        result = collection.insert_one(new_doc)
        print(f"Saved to MongoDB: {transcript.title}")
        return str(result.inserted_id)

    if existing.get("text") == new_doc["text"]:
        print(f"⊛ Unchanged, skipping: {transcript.title}")
        return str(existing["_id"])

    # Text changed: archive the prior version, then update in place.
    prior = {k: existing.get(k) for k in ("text", "text_full_raw", "scraped_at")}
    collection.update_one(
        key,
        {
            "$push": {"versions": prior},
            "$set": {
                "text": new_doc["text"],
                "text_full_raw": new_doc["text_full_raw"],
                "scraped_at": new_doc["scraped_at"],
            },
        },
    )
    print(f"↻ Revision stored: {transcript.title}")
    return str(existing["_id"])
```

Note: for revisions to be *found*, the scraper must revisit pages it already
completed. The page-based resume state means a periodic full re-run (e.g.
weekly `START_PAGE=1`) is the simplest way to pick up corrections to recent
transcripts; recent pages are where corrections concentrate since FactBase
sorts by date.

## 4. Downstream: expose `scraped_at` in prepare_trump_transcripts.py

Add `"scraped_at": 1` to the Mongo projection and carry it through `keep_cols`
so the labeler's point-in-time loader uses real timestamps instead of the
24-hour lag assumption for all post-patch documents.
