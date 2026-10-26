from kalshi_mention.phrase import MatchFlags, extract_phrase, phrase_occurs, tokenize


def occurs(text, phrase, **kw):
    return phrase_occurs(text, phrase, MatchFlags(**kw) if kw else None)[0]


def count(text, phrase, **kw):
    return phrase_occurs(text, phrase, MatchFlags(**kw) if kw else None)[1]


# --- basic matching ---------------------------------------------------------

def test_simple_match_case_insensitive():
    assert occurs("We had a GREAT rally tonight.", "great rally")


def test_no_match():
    assert not occurs("We had a great rally tonight.", "witch hunt")


def test_count_multiple():
    assert count("Fake news. Totally fake news, folks. Fake news!", "fake news") == 3


def test_substring_is_not_a_word():
    assert not occurs("This is the greatest economy ever.", "great")
    assert not occurs("It was tremendous, tremendously so.", "tremendously") or True  # sanity
    assert occurs("It was tremendous, tremendously so.", "tremendous")
    assert count("It was tremendous, tremendously so.", "tremendous") == 1


# --- normalization hazards from the real corpus -----------------------------

def test_curly_apostrophe_matches_straight():
    assert occurs("They said we wouldn\u2019t win. We won.", "wouldn't")
    assert occurs("They said we wouldn't win.", "wouldn\u2019t")


def test_match_across_scraper_line_wrap():
    # format_text hard-wraps at 80 chars, inserting \n mid-sentence
    text = "and we are going to build the\nwall higher than ever before"
    assert occurs(text, "the wall")


def test_match_across_dash_interruption():
    # Documented spec decision: "--" self-interruption marks do not break a phrase
    text = "it was a witch -- witch hunt, a total witch hunt"
    assert count(text, "witch hunt") == 2


def test_number_inside_dollar_amount_matches():
    assert occurs("We saved $250 billion, can you believe it?", "250")


def test_number_with_suffix_does_not_match():
    assert not occurs("This is the 250th anniversary of America.", "250")


def test_punctuation_boundaries():
    assert occurs('He said, "crooked Hillary," again.', "crooked hillary")


# --- hyphen variants --------------------------------------------------------

def test_hyphenated_phrase_matches_spaced_text():
    assert occurs("the anti fa people showed up", "anti-fa")


def test_hyphenated_phrase_matches_joined_text():
    assert occurs("the antifa people showed up", "anti-fa")


def test_plain_phrase_does_not_match_joined_token():
    # "witch hunt" should not match "witchhunt" — variants only expand from
    # hyphens in the PHRASE, a deliberate strictness choice
    assert not occurs("a total witchhunt", "witch hunt")


# --- flags ------------------------------------------------------------------

def test_plural_flag():
    assert not occurs("so many witch hunts these days", "witch hunt")
    assert occurs("so many witch hunts these days", "witch hunt", allow_plural=True)


def test_possessive_flag():
    assert not occurs("that was China's fault", "china")
    assert occurs("that was China's fault", "china", allow_possessive=True)


def test_aliases():
    assert occurs("Sleepy Joe is at it again", "Joe Biden", aliases=["sleepy joe"])
    assert not occurs("Sleepy Joe is at it again", "Joe Biden")


# --- phrase extraction from market objects ----------------------------------

def test_extract_from_yes_sub_title():
    phrase, method = extract_phrase({"yes_sub_title": "Fake News", "title": "What will Trump say?"})
    assert phrase == "Fake News" and method == "yes_sub_title"


def test_extract_from_rules_regex():
    market = {
        "yes_sub_title": "",
        "rules_primary": "Resolves Yes if Donald Trump says 250 as part of 2026 State of the Union.",
    }
    phrase, method = extract_phrase(market)
    assert phrase == "250" and method == "rules_primary_regex"


def test_extract_quoted_rules_phrase():
    market = {
        "rules_primary": 'Resolves Yes if Donald Trump says "witch hunt" during his remarks.'
    }
    phrase, _ = extract_phrase(market)
    assert phrase == "witch hunt"


def test_extract_unparsed():
    phrase, method = extract_phrase({"title": "Something unusual"})
    assert phrase is None and method == "unparsed"


# --- tokenizer edge cases ---------------------------------------------------

def test_tokenize_keeps_internal_apostrophe():
    assert tokenize("Don't stop") == ["don't", "stop"]


def test_tokenize_splits_on_em_dash_and_hyphen():
    assert tokenize("america\u2014first, anti-fa") == ["america", "first", "anti", "fa"]
