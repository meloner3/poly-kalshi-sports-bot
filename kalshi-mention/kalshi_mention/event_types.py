"""Ex-ante event-type classification.

Both sides of the prediction problem carry type information that is knowable
before the event happens: Kalshi publishes the event title ("...during his
press gaggle...") when the market opens, and FactBase tags every document with
a speech_type. Conditioning exposure and hit rates on this type is the fix for
the pass-2 finding that models predicted rally-length exposure for gaggle-length
events (the dominant source of confident-and-wrong predictions).

Buckets are deliberately coarse — enough history per bucket matters more than
taxonomic purity.
"""

from __future__ import annotations

import re

TYPES = ["speech", "pressconf", "gaggle", "interview", "meeting", "vlog", "other"]

_MARKET_RULES: list[tuple[str, str]] = [
    (r"\b(rally|speech|address|commencement|keynote|convention|amphitheatre|amphitheater)\b", "speech"),
    (r"\bpress conference\b", "pressconf"),
    (r"\b(gaggle|speaks to reporters|to the press pool)\b", "gaggle"),
    (r"\b(interview|radio show|podcast|calls? in|town hall|special report|meet the press)\b", "interview"),
    (r"\b(bilat|bilateral|meeting|meets|welcomes|lunch|luncheon|dinner|cabinet|roundtable|signing|signs|ceremony|reception|visit)\b", "meeting"),
    (r"\bvlog\b", "vlog"),
    (r"\b(remarks|announcement|event)\b", "meeting"),
]

_DOC_SPEECH_TYPE = {
    "speech": "speech",
    "press conference": "pressconf",
    "press gaggle": "gaggle",
    "press briefing": "gaggle",
    "interview": "interview",
    "interview - broadcast edit": "interview",
    "remarks": "meeting",
    "official document": "other",
    "donald trump vlog": "vlog",
    "vlog": "vlog",
}


def classify_market(text: str | None) -> str:
    """Type from a Kalshi event/market title or descriptor (ex-ante info)."""
    if not text:
        return "other"
    t = text.casefold()
    for pattern, bucket in _MARKET_RULES:
        if re.search(pattern, t):
            return bucket
    return "other"


def classify_doc(speech_type: str | None, title: str | None = None) -> str:
    """Type of a FactBase document from its speech_type tag (title fallback)."""
    if speech_type:
        bucket = _DOC_SPEECH_TYPE.get(speech_type.strip().casefold())
        if bucket:
            return bucket
    if title:
        head = title.split(":", 1)[0]
        bucket = _DOC_SPEECH_TYPE.get(head.strip().casefold())
        if bucket:
            return bucket
        return classify_market(title)
    return "other"
