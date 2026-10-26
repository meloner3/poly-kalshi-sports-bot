"""Phrase occurrence matching against raw transcript text.

MATCHING SPEC (v1) — every rule here is a decision, not an accident:

  1. Both text and phrase are normalized: NFKC unicode normalization; curly
     quotes/apostrophes -> straight; casefolded; then tokenized.
  2. Tokenization: maximal runs of [letters/digits], with internal apostrophes
     kept inside a token ("don't" -> one token). Everything else (punctuation,
     newlines from the scraper's 80-char hard wrap, "--" interruption marks,
     hyphens, "$") is a token boundary. Consequences:
       * A phrase CAN match across line wraps and across "--" self-interruption
         marks. Rationale: FactBase inserts these editorially; the words were
         spoken contiguously. This is auditable via the mismatch report and
         can be tightened if the audit shows Kalshi rules differently.
       * "$250" contains the token "250" (the $ is a boundary) -> "250" matches.
       * "250th" is a single token -> "250" does NOT match. Same for
         "great" vs "greatest".
  3. Hyphenated phrases generate variants: "anti-fa" matches text tokenized as
     ["anti","fa"] (hyphen->space) and as ["antifa"] (hyphen->joined).
  4. Optional flags (off by default, per-market override via CSV):
       allow_plural    — last phrase token also matches +"s", +"es"
       allow_possessive— last phrase token also matches +"'s"
     Defaults are strict because Kalshi rules text usually names the exact
     string; the audit tells us empirically whether Kalshi counts inflections.
  5. A match = the phrase's token sequence appearing as consecutive tokens in
     the text. count = number of starting positions (overlaps counted; only
     matched>=1 matters for binary markets).

Anything this spec cannot express (nicknames, "mentions [person]" semantics)
must go through the per-market override CSV with an explicit alias list — it
must never be silently approximated.
"""

from __future__ import annotations

import json
import re
import unicodedata
from dataclasses import dataclass, field

_QUOTE_MAP = {
    "\u2018": "'", "\u2019": "'", "\u201B": "'",
    "\u201C": '"', "\u201D": '"',
    "\u2010": "-", "\u2011": "-", "\u2012": "-", "\u2013": "-", "\u2014": "-",
    "\u00A0": " ",
}

# letters-or-digits runs, apostrophes allowed strictly between such runs
_TOKEN_RE = re.compile(r"[^\W_]+(?:'[^\W_]+)*", re.UNICODE)


def _pre_normalize(text: str) -> str:
    for old, new in _QUOTE_MAP.items():
        text = text.replace(old, new)
    return unicodedata.normalize("NFKC", text).casefold()


def tokenize(text: str) -> list[str]:
    return _TOKEN_RE.findall(_pre_normalize(text))


@dataclass
class MatchFlags:
    allow_plural: bool = False
    allow_possessive: bool = False
    aliases: list[str] = field(default_factory=list)  # alternative phrasings that also count

    def to_json(self) -> str:
        return json.dumps(
            {
                "allow_plural": self.allow_plural,
                "allow_possessive": self.allow_possessive,
                "aliases": self.aliases,
            },
            sort_keys=True,
        )


def _phrase_token_variants(phrase: str) -> list[list[str]]:
    """Token sequences that count as the phrase (hyphen split vs joined)."""
    variants: list[list[str]] = []
    base = tokenize(phrase)  # hyphen acts as boundary -> the 'split' variant
    if base:
        variants.append(base)
    joined_src = _pre_normalize(phrase)
    if "-" in joined_src:
        joined = tokenize(joined_src.replace("-", ""))
        if joined and joined not in variants:
            variants.append(joined)
    return variants


def _last_token_alternatives(token: str, flags: MatchFlags) -> set[str]:
    alts = {token}
    if flags.allow_plural:
        alts.add(token + "s")
        alts.add(token + "es")
    if flags.allow_possessive:
        alts.add(token + "'s")
    return alts


def count_occurrences(text_tokens: list[str], phrase: str, flags: MatchFlags | None = None) -> int:
    """Number of positions in text_tokens where phrase (or an alias) starts."""
    flags = flags or MatchFlags()
    total = 0
    phrases = [phrase] + list(flags.aliases)
    counted_variants: set[tuple[str, ...]] = set()
    for p in phrases:
        for variant in _phrase_token_variants(p):
            key = tuple(variant)
            if key in counted_variants:  # avoid double counting identical variants
                continue
            counted_variants.add(key)
            n = len(variant)
            if n == 0:
                continue
            last_alts = _last_token_alternatives(variant[-1], flags)
            head = variant[:-1]
            for i in range(len(text_tokens) - n + 1):
                if text_tokens[i : i + n - 1] == head and text_tokens[i + n - 1] in last_alts:
                    total += 1
    return total


def phrase_occurs(text: str, phrase: str, flags: MatchFlags | None = None) -> tuple[bool, int]:
    tokens = tokenize(text)
    c = count_occurrences(tokens, phrase, flags)
    return c > 0, c


# ---------------------------------------------------------------------------
# Extracting the target phrase from a Kalshi market object
# ---------------------------------------------------------------------------

# "Resolves Yes if Donald Trump says 250 as part of 2026 State of the Union."
_RULES_SAYS_RE = re.compile(
    r"says?\s+[\"\u201C]?(?P<phrase>.+?)[\"\u201D]?\s+"
    r"(?:as part of|during|at|in the course of|while)\b",
    re.IGNORECASE,
)


def extract_phrase(market: dict) -> tuple[str | None, str]:
    """Best-effort phrase extraction. Returns (phrase, method).

    Order of trust:
      1. yes_sub_title  — for mention markets this is the phrase itself.
      2. subtitle       — older field name for the same thing.
      3. rules_primary  — regex over the resolution sentence.
    Anything that fails all three must be resolved via the override CSV;
    the taxonomy report lists these explicitly so none are silently dropped.
    """
    for key in ("yes_sub_title", "subtitle"):
        val = (market.get(key) or "").strip()
        if val:
            return val, key
    rules = market.get("rules_primary") or ""
    m = _RULES_SAYS_RE.search(rules)
    if m:
        return m.group("phrase").strip().strip('"').strip(), "rules_primary_regex"
    return None, "unparsed"


# --- Kalshi phrase-spec parsing (variant lists + count thresholds) -----------
_THRESHOLD_RE = re.compile(r"\(\s*(\d+)\s*\+\s*times?\s*\)", re.IGNORECASE)

def parse_phrase_spec(raw: str) -> tuple[list[str], int]:
    """Kalshi yes_sub_title format: variants separated by ' / ', optional
    '(N+ times)' threshold. Returns (alternatives, required_count)."""
    threshold = 1
    m = _THRESHOLD_RE.search(raw)
    if m:
        threshold = int(m.group(1))
        raw = _THRESHOLD_RE.sub(" ", raw)
    alts = [a.strip() for a in raw.split("/") if a.strip()]
    return (alts or [raw.strip()]), threshold

def phrase_spec_occurs(text: str, raw_phrase: str, flags: "MatchFlags | None" = None) -> tuple[bool, int]:
    """Occurrence under the full Kalshi spec: any variant counts, total
    occurrences across variants must reach the threshold."""
    alts, k = parse_phrase_spec(raw_phrase)
    base = flags or MatchFlags()
    merged = MatchFlags(
        allow_plural=base.allow_plural,
        allow_possessive=base.allow_possessive,
        aliases=list(base.aliases) + alts[1:],
    )
    tokens = tokenize(text)
    c = count_occurrences(tokens, alts[0], merged)
    return c >= k, c

def spec_key(raw_phrase: str) -> str:
    """Canonical identifier for a phrase spec (variants + threshold)."""
    alts, k = parse_phrase_spec(raw_phrase)
    return json.dumps({"alts": [a.casefold().strip() for a in alts], "k": k}, sort_keys=True)
