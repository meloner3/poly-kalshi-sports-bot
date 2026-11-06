# Pre-registration v2 — kalshi_mention divergence strategy

**Frozen:** 2026-09-10
**Code version:** `35857a1f68a7fac88870f91256009004c00e5e53` (`35857a1`)
**Executable copy of these parameters:** `kalshi_mention/prereg.py`
**Tracker:** `python -m kalshi_mention.verdict --prereg`

This document and `prereg.py` are the same freeze in two forms. If they ever
disagree, the code is authoritative and the disagreement is a bug.

---

## Why v1 is void

Either reason alone would be sufficient.

1. **The entry rule changed.** v1 traded when `|model − mid| > threshold`.
   v2 trades on executable expected value against the touch. These select
   different markets: a wide spread mechanically inflates `|model − mid|`
   whether or not the forecast is any good, so the old rule preferentially
   picked illiquid books and then paid their half-spread. That is adverse
   selection wired into the entry condition, and T3 measured its cost — at
   the 1h snapshot, `crossing` averaged 0.103/contract against a
   `settle_vs_mid` of +0.002.

2. **The evidence is no longer virgin.** v1 said "evaluate ONLY on data after
   2026-08-11". Post-2026-08-11 outcomes may already have been observed during
   ordinary pipeline runs. That promise cannot now be honoured.

**And the prior that motivated v1 has weakened.** v1 rested on four
"replicated" structures. T2 showed one of them — NO-side +0.05 — reverses
sign under decision-time-legal eligibility (all-eligible NO +0.0670;
pre-event NO −0.0722). Separately, those four models share ~24 near-collinear
features and one 228-day sample, so their agreement was close to
tautological and never constituted replication.

---

## The frozen rule

| parameter | value |
|---|---|
| forecast column | `empirical_hl30_cal` (Component A, selected calibrated) |
| snapshot | 6 hours after market open |
| eligibility | `pre_event` — T2 decision-time-legal variant |
| entry | executable EV > **$0.02**/contract |
| EV definition | `EV_yes = p − ask − fee(ask)`; `EV_no = bid − p − fee(1−bid)`; take the better side |
| max spread | 0.10 (ask − bid) |
| quote freshness | candle timestamp within 3600s of the snapshot |
| size | 1 contract per signal (no sizing rule is under test) |
| fee | `ceil_to_cent(0.07 × P × (1−P))` per contract |
| endpoint | mean net P&L per contract, day-clustered |

**Why this forecast column.** Component A's calibrated column is the only
forecaster here whose advantage survives day-clustered inference: −0.0428 vs
`base_pooled`, CI [−0.0559, −0.0293]. `model_u` (+0.0065, CI [−0.0034,
+0.0174]) and `model_b` (+0.0042, CI [−0.0011, +0.0096]) both fail their
gates with intervals spanning zero. Choosing either would add complexity and
researcher degrees of freedom for no demonstrated gain.

**`prior_hits`, frozen definition** (recorded because it appears in v1's
hypothesis and is otherwise ambiguous): the count of documents whose
occurrence count for this market's phrase spec reaches its threshold *k*, of
the same classified event type, available strictly before the market's
`open_time`.

**Fee provenance.** Verified 2026-09-10 against
[kalshi.com/docs/kalshi-fee-schedule.pdf](https://kalshi.com/docs/kalshi-fee-schedule.pdf)
("Fee Schedule for July 2026 — 7.7.26 Update"), formula
`fees = roundup(M × 0.07 × C × P × (1−P))`, and
[docs.kalshi.com/getting_started/fee_rounding](https://docs.kalshi.com/getting_started/fee_rounding).
The published model fee rounds up to $0.000001, *not* to the cent; a standard
member's (position cost + fee) is then floored to the $0.01 grid with the
shortfall charged as a rounding fee. Because candle prices are whole cents and
this rule trades one contract, the position cost is already cent-aligned and
the alignment falls entirely on the fee — making the total exactly the model
fee rounded up to the next cent. Verified numerically across all 99 whole-cent
prices: 0 mismatches.

---

## Evaluation window

Evaluation begins at the **first event-day after the commit date of this
file**. The tracker derives that date from git rather than a constant, so it
cannot be quietly moved. Days before it are not eligible under any
circumstance.

---

## Sequential decision rule

Planned maximum **320 eligible event-days** (T0.3: the best-powered cell needs
~318 days to resolve a 2¢ edge at 80% power). Interim looks at **80 / 160 /
240 / 320** eligible days.

Information fraction `t_k = days_k / 320`. Two-sided O'Brien–Fleming
boundary:

```
Z_k       = 1.959964 / sqrt(t_k)
alpha*(t) = 2 − 2·Phi(Z_k)          (cumulative alpha spent by look k)
```

| look | days | t | Z boundary | cumulative alpha |
|---|---|---|---|---|
| 1 | 80 | 0.25 | 3.92 | 0.0001 |
| 2 | 160 | 0.50 | 2.77 | 0.0056 |
| 3 | 240 | 0.75 | 2.26 | 0.0237 |
| 4 | 320 | 1.00 | 1.96 | 0.0500 |

At each look, exactly one of:

1. **SUCCESS** — the OBF-adjusted lower confidence bound is above zero.
2. **COMMERCIAL FUTILITY** — the OBF-adjusted upper bound is below
   **$0.02/contract**. A real edge may exist but is too small to trade;
   stopping is conservative and spends no alpha.
3. **CONTINUE** — otherwise.

---

## What may not change

Any change to the forecast column, snapshot, eligibility, entry rule,
thresholds, filters, fee rule, endpoint, planned maximum, look schedule or
boundary **voids v2** and requires a v3 with a new freeze date and a new
evaluation window. Fixing an outright bug is permitted and must be recorded
here with its commit hash and the reason.

The tracker reports one number and one decision. It does not scan thresholds,
compare cells, or decompose subgroups — each of those would reintroduce the
multiplicity this freeze exists to remove.

---

## Status at freeze

0 eligible event-days. Decision: **CONTINUE**. This is the expected state and
will persist for months; see `analysis/verification_report.md` (T0.3) before
expecting a verdict.
