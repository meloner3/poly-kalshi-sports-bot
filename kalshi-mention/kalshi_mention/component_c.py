"""Component C — LM Monte Carlo simulator.

    pip install torch transformers   # one-time, large download
    python -m kalshi_mention.component_c --db kalshi_mention.sqlite \
        --since 2026-05-27 --samples 30 --max-new-tokens 400

Where Component A knows how OFTEN Trump says things and Component B knows the
CONTEXT around an event, Component C tries to know what GOES TOGETHER: it
prompts a language model with point-in-time context (the tail of the most
recent same-event-type speech available before the market opened), generates N
continuations, and estimates P(phrase occurs) as the smoothed fraction of
simulations satisfying the market's full Kalshi spec (variants + threshold),
counted by the same matcher that grades ground truth.

Backends:
  --model gpt2                default; base GPT-2 — weak but leakage-clean
  --model /path/to/checkpoint a fine-tuned checkpoint (e.g. from the original
                              DelphiResearch gpt2_finetune.py work)

LEAKAGE CAVEAT (printed into the report): a checkpoint fine-tuned on
transcripts through date T has seen the future relative to any market that
opened before T. Predictions are only point-in-time clean for markets opening
AFTER the checkpoint's training cutoff — pass --since accordingly. Base gpt2
has no such issue.

Compute: this is the expensive component. samples x max-new-tokens x markets
dominates; the defaults (30 x 400) run a few hundred live-era markets in
roughly an evening on CPU, much faster on Apple-silicon MPS (auto-detected).
Output: component_c/component_c_predictions.csv with column p_c, mergeable
into Component B's feature set or the ensemble.
"""

from __future__ import annotations

import argparse
import json
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import pandas as pd

from . import config, db
from .counts import count_in_index, spec_variant_token_lists
from .event_types import classify_doc, classify_market
from .phrase import extract_phrase, tokenize
from . import paths


def spec_hits(text: str, variants, k: int) -> bool:
    tokens = tokenize(text)
    positions: dict[str, list[int]] = {}
    for i, t in enumerate(tokens):
        positions.setdefault(t, []).append(i)
    return count_in_index(variants, positions, tokens) >= k


def make_prompt(context_text: str, max_context_tokens: int = 300) -> str:
    words = context_text.split()
    return " ".join(words[-max_context_tokens:])


def hf_generator(model_name: str, max_new_tokens: int, batch_size: int):
    """Returns generate_fn(prompt, n) -> list[str]. Imported lazily so the
    module (and its tests) work without torch installed."""
    try:
        import torch
        from transformers import AutoModelForCausalLM, AutoTokenizer
    except ImportError as e:
        raise SystemExit(
            "Component C needs torch + transformers:\n"
            "  pip install torch transformers\n"
            f"(import failed: {e})")
    device = ("mps" if torch.backends.mps.is_available()
              else "cuda" if torch.cuda.is_available() else "cpu")
    tok = AutoTokenizer.from_pretrained(model_name)
    tok.pad_token = tok.eos_token
    model = AutoModelForCausalLM.from_pretrained(model_name).to(device).eval()
    print(f"backend: {model_name} on {device}")

    @torch.no_grad()
    def generate(prompt: str, n: int) -> list[str]:
        enc = tok(prompt, return_tensors="pt", truncation=True, max_length=512).to(device)
        outs: list[str] = []
        for i in range(0, n, batch_size):
            b = min(batch_size, n - i)
            gen = model.generate(
                **enc, do_sample=True, temperature=0.9, top_p=0.95,
                max_new_tokens=max_new_tokens, num_return_sequences=b,
                pad_token_id=tok.eos_token_id)
            new = gen[:, enc["input_ids"].shape[1]:]
            outs.extend(tok.decode(s, skip_special_tokens=True) for s in new)
        return outs
    return generate


def run(db_path: str, out_dir: str, model_name: str, since: str,
        samples: int, max_new_tokens: int, batch_size: int, limit: int | None,
        generate_fn=None) -> None:
    out = paths.out_dir(out_dir)
    conn = db.connect(db_path)

    placeholders = ",".join("?" * len(config.LABELING_EXCLUDED_SERIES)) or "''"
    markets = conn.execute(
        f"""SELECT m.ticker, m.result, m.open_time, m.title AS mtitle, m.raw_json,
                   e.title AS etitle
            FROM markets m LEFT JOIN events e ON m.event_ticker = e.event_ticker
            WHERE m.result IN ('yes','no') AND m.series_ticker NOT IN ({placeholders})
              AND m.open_time >= ?
            ORDER BY m.open_time""",
        list(config.LABELING_EXCLUDED_SERIES) + [since],
    ).fetchall()
    if limit:
        markets = markets[:limit]
    print(f"Markets since {since}: {len(markets)}; {samples} samples x "
          f"{max_new_tokens} tokens each")

    docs = conn.execute(
        "SELECT doc_id, date, available_at, speech_type, title, text_raw "
        "FROM transcripts ORDER BY available_at").fetchall()
    doc_avail = [str(d["available_at"] or "") for d in docs]
    doc_type = [classify_doc(d["speech_type"], d["title"]) for d in docs]

    if generate_fn is None:
        generate_fn = hf_generator(model_name, max_new_tokens, batch_size)

    prompt_cache: dict[tuple[str, str], str] = {}
    sim_cache: dict[str, list[str]] = {}
    rows = []
    for mi, mkt in enumerate(markets):
        obj = json.loads(mkt["raw_json"])
        phrase, _ = extract_phrase(obj)
        if not phrase:
            continue
        variants, k = spec_variant_token_lists(phrase)
        mtype = classify_market(mkt["etitle"] or mkt["mtitle"])
        open_iso = str(mkt["open_time"])
        key = (mtype, open_iso[:13])
        if key not in prompt_cache:
            ctx = ""
            for j in range(len(docs) - 1, -1, -1):
                if doc_avail[j] and doc_avail[j] <= open_iso and doc_type[j] == mtype:
                    ctx = docs[j]["text_raw"]
                    break
            if not ctx:
                for j in range(len(docs) - 1, -1, -1):
                    if doc_avail[j] and doc_avail[j] <= open_iso:
                        ctx = docs[j]["text_raw"]
                        break
            prompt_cache[key] = make_prompt(ctx) if ctx else ""
        prompt = prompt_cache[key]
        if not prompt:
            continue
        # simulations depend only on (prompt); phrases share them — huge saving
        ck = f"{key[0]}|{key[1]}"
        if ck not in sim_cache:
            sim_cache[ck] = generate_fn(prompt, samples)
            print(f"  [{mi + 1}/{len(markets)}] simulated context {ck}")
        sims = sim_cache[ck]
        hits = sum(1 for s in sims if spec_hits(s, variants, k))
        p_c = (hits + 0.5) / (len(sims) + 1.0)
        rows.append({"ticker": mkt["ticker"], "phrase": phrase,
                     "y": 1 if mkt["result"] == "yes" else 0,
                     "p_c": p_c, "n_samples": len(sims), "event_type": mtype})

    df = pd.DataFrame(rows)
    df.to_csv(out / "component_c_predictions.csv", index=False)
    lines = ["# Component C — LM Monte Carlo simulator", ""]
    lines.append(f"Model: `{model_name}`; markets: {len(df)}; samples/context: {samples}.")
    if len(df):
        y = df["y"].to_numpy(float)
        p = np.clip(df["p_c"].to_numpy(float), 1e-6, 1 - 1e-6)
        ll = float(-(y * np.log(p) + (1 - y) * np.log(1 - p)).mean())
        base = float(y.mean())
        pb = np.clip(np.full_like(y, base), 1e-6, 1 - 1e-6)
        llb = float(-(y * np.log(pb) + (1 - y) * np.log(1 - pb)).mean())
        lines.append(f"Standalone log loss: **{ll:.4f}** vs constant-rate {llb:.4f}.")
        lines.append("")
        lines.append("Standalone C is not expected to beat A; its value is tested by adding")
        lines.append("p_c as a Component-B feature (component_b --c-file ...) and checking")
        lines.append("whether the ensemble improves.")
    lines.append("")
    lines.append("## Leakage caveat")
    lines.append("")
    lines.append("A fine-tuned checkpoint has seen transcripts through its training date;")
    lines.append(f"only markets opening after that date (here: since {since}) are clean.")
    lines.append("Base gpt2 is leakage-free but weak — treat it as the machinery test.")
    (out / "component_c_report.md").write_text("\n".join(lines))
    print(f"Wrote {out}/component_c_predictions.csv ({len(df)} markets)")
    conn.close()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=config.DEFAULT_DB_PATH)
    ap.add_argument("--out", default="component_c")
    ap.add_argument("--model", default="gpt2")
    ap.add_argument("--since", default="2026-05-27")
    ap.add_argument("--samples", type=int, default=30)
    ap.add_argument("--max-new-tokens", type=int, default=400)
    ap.add_argument("--batch-size", type=int, default=6)
    ap.add_argument("--limit", type=int, default=None, help="cap market count (smoke runs)")
    args = ap.parse_args()
    run(args.db, args.out, args.model, args.since, args.samples,
        args.max_new_tokens, args.batch_size, args.limit)


if __name__ == "__main__":
    main()
