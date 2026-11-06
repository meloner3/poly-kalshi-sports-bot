# CLAUDE.md

## What this project is

Polytaoli: a real-time cross-exchange arbitrage system between **Kalshi** and
**Polymarket US** prediction markets. It watches matched markets on both
venues, computes profit margins after fees, and can automatically execute
paired (hedged) orders across both exchanges. This is live financial software
handling real money and real exchange APIs — precision and correctness in the
order-execution path matter more than anywhere else in the codebase.

## Current goal

The user is debugging the arbitrage/execution pipeline so they can start
publishing (executing) real trades on detected arbitrage opportunities with
confidence. Priorities when helping:

- Find and fix correctness bugs in price/quantity calculation, order sizing,
  and paired-execution logic before worrying about features or polish.
- Treat any bug in this path as potentially money-losing. Confirm fixes
  against the invariants below rather than assuming a plausible-looking
  change is correct.
- As of 2026-09-29, the `ORD_REJECT_REASON_EXCHANGE_OPTION` blocker that
  prevented every order from ever filling is resolved and confirmed against
  a real live fill (see that session log below) — read it before assuming
  order submission is still broken. Check `git status` before reasoning
  about "current" behavior generally; this file's session logs describe
  work as it happened, not necessarily what's still uncommitted now.

## Session log (2026-09-24) — read this before continuing

**Done this session, builds/tests green (`cargo build`, `cargo test --lib
paired_execution`, `pytest test_market_book.py`):**

- Polymarket US rate-limit backoff: `poly-order-service/main.py` now has
  `call_with_backoff()`, wrapping every SDK call (balances, positions,
  market book, market detail, order create/retrieve/cancel/list). On a 429
  (`RateLimitError`) it waits then retries at `2**attempt` (1s/2s/4s, 3
  attempts), exactly matching docs.polymarket.us/api-reference/rate-limits'
  own guidance. Only 429s are retried — a real order rejection is never
  retried, so this can't cause a duplicate submission.
- Success/fill logging so fills can be cross-checked against the exchanges
  by grepping logs, not just reading the DB/UI history: Python
  `order_result()` logs `Polymarket US order FILLED: ...` (order_id,
  filled_contracts, avg_price) whenever a market/limit order actually
  fills; Rust `submit_paired_order` (`services/paired_execution.rs`) logs
  one INFO line per attempt where either leg filled (both order IDs, fill
  counts vs. requested size, prices, neutralization/residual outcome); the
  manual single-leg endpoints (`place_kalshi_order`, `place_polymarket_order`
  in `api/routes/orders.rs`) got the same treatment.

**Open investigation — root cause of `ORD_REJECT_REASON_EXCHANGE_OPTION`
still unknown, and the current uncommitted theory is likely wrong:**

The diff already on disk (before this session) added `minimum_trade_qty`/
`price_tick_size` plumbing (`PolymarketMarketBook`, `find_profitable_
contract_size` in `services/paired_execution.rs`, `cached_market_
constraints()` in `poly-order-service/main.py`) on the theory that
`ORD_REJECT_REASON_EXCHANGE_OPTION` rejections are explained by each
market's own `minimumTradeQty` (per docs.polymarket.us/api-reference/
orders/overview). **This was checked against the live API this session and
does not hold**: queried `markets.retrieve_by_slug` directly for three real
markets seen in today's logs (`aec-itfwo-gabsoa-julsil-2026-09-24`,
`aec-itfme-giobra-joasch-2026-09-24`, `aec-itfwo-hancha-alishc-2026-09-24`)
and `minimumTradeQty` came back `0.01` on all three — `orderPriceMinTickSize`
was also `0.01` on all three. `0.01` contracts is below any whole-contract
order this bot would ever place (`min_contracts.max(0.01.ceil())` = a
no-op floor of 1), so it cannot be what's rejecting real orders. Treat the
comments in `services/paired_execution.rs` around `POLYMARKET_MIN_NOTIONAL_
USD` and `find_profitable_contract_size` asserting minimumTradeQty as the
likely cause as **unconfirmed, probably wrong** until re-verified.

Also worth noting: `orderPriceMinTickSize` is fetched and stored on
`PolymarketMarketBook.price_tick_size` but is **never read anywhere** —
no code rounds or validates a submission price against it. Not obviously
the cause (book levels are already exchange-quoted prices), but it's dead
data worth double-checking if price alignment turns out to matter.

The actual `ORD_REJECT_REASON_EXCHANGE_OPTION` hits visible in today's
`rust-backend/logs/polytaoli.log.2026-09-24` (search `EXCHANGE_OPTION`) are
all logged from `api::routes::orders: 149`, i.e. the **standalone
`place_polymarket_order` manual endpoint** — which takes `contracts`/`price`
straight from the request body and never calls `find_profitable_contract_
size` or checks `minimum_trade_qty` at all. Worth confirming whether these
were manual test orders (in which case they say nothing about the real
paired-execution path) or something the frontend calls on the hot path.
`execute_arbitrage` (the real paired path) *does* gate on `poly_book.
minimum_trade_qty`, but since that value is always ~0.01 the gate is
currently a no-op there too.

**Next step, not yet started:** inspect `market_order_params()` /
`limit_order_params()` in `poly-order-service/main.py` (units sent for
price/quantity, e.g. `cashOrderQty` vs `quantity`, `slippageTolerance`
ticks) for a mismatch that could trigger a generic exchange-option
rejection independent of size. Was about to start this when the session
pivoted to the rate-limit/logging work above.

## Session log (2026-09-26)

**Leading theory for `ORD_REJECT_REASON_EXCHANGE_OPTION`, not yet confirmed
against a live order:** `limit_order_params()` (the only path actually used —
`place_polymarket_order`/`submit_limit_order` in Rust always calls
`/order/limit`, never `/order/market`) built `"quantity": request.size`,
where `request.size` is a Pydantic `float`. The SDK's own type stubs
(`polymarket_us/types/orders.py`) declare `quantity: int` on
`CreateOrderParams`/`Order` — every other numeric field in that payload is
carefully type-matched to the spec (`price`/`cashOrderQty` wrapped as
`Amount` dicts, `maxBlockTime` sent as the string `"5"`) except this one,
which was sending e.g. `5.0` instead of `5`. Fixed: both `limit_order_params()`
and the dead-code `market_order_params()` sell path now cast to `int` and
raise if the size isn't a whole number rather than silently truncating.
**Still needs a real order against the live API to confirm this was the
actual cause** — do not mark this closed until that's verified.

Also fixed, unrelated: Kalshi startup fetch had no retry.
`ArbitrageService::initialize()` calls
`kalshi_client.get_supported_events_and_markets().await?` with no retry, so
a single transient network timeout to `external-api.kalshi.com` killed the
whole Rust backend before it ever bound its port (the "health check failed
after 5 attempts" wrapper-script error is just polling a server that never
started — it's not 5 real Kalshi outages). Fixed in `clients/kalshi.rs`:
the low-level `get()` (used by balance/events/quotes/positions/orders-list —
every GET) now retries a transport-level failure (timeout, reset, DNS) with
exponential backoff (1s/2s, 3 attempts total), regenerating the
timestamp+signature on each attempt. Deliberately **not** applied to
`post()`, which is order placement (`/portfolio/events/orders`) — retrying
that blindly on a timeout risks submitting a duplicate order if the first
one actually went through and only the response was lost, mirroring why the
Polymarket backoff only retries a clean 429 and never a generic exception.
A non-success HTTP status from Kalshi (a real API error) is still never
retried, only the transport-level send failure.

**Queued for later, explicitly deferred until the above is settled:**
evaluate dropping the `polymarket_us` SDK / `poly-order-service` Python
process entirely and calling the Polymarket US REST API directly from Rust,
the same way `clients/kalshi.rs` does. Its auth is Ed25519-signing
`f"{timestamp}{method}{path}"` (`polymarket_us/auth.py`) — comparable
complexity to the RSA-PSS signing Rust already does for Kalshi. Would
remove a whole failure class (the extra process/port, the IPC hop, and
JSON type mismatches like the one above) but is a real rewrite of the
order-submission and market-book paths for live-money code — treat it as
its own careful, incremental migration, not a quick swap.

## Session log (2026-09-27) — Polymarket US direct-API migration

Done this session (full cutover, requested by the user after hitting the
still-unresolved `K: Kalshi leg was not submitted because Polymarket did not
fill a positive contract quantity; P: ORD_REJECT_REASON_EXCHANGE_OPTION`
error live): the previously-deferred migration above. `poly-order-service`
(the Python/FastAPI process wrapping the official `polymarket-us` SDK) is no
longer started or called. `clients/polymarket.rs` now calls
`gateway.polymarket.us` (market book, market constraints, events/sports —
unauthenticated, unchanged) and `api.polymarket.us` (balances, positions,
orders list/create/cancel — Ed25519-signed) directly, mirroring how
`clients/kalshi.rs` already signs and calls Kalshi directly. New
`clients/polymarket_auth.rs` replicates `polymarket_us/auth.py` exactly:
sign `{timestamp}{METHOD}{path}` only (never body or query), base64-decode
the secret key, use only its first 32 bytes as the Ed25519 seed if 64 bytes
were provided. `PolymarketConfig` gained `api_base_url` and
`key_id`/`secret_key` fields (env vars `POLYMARKET_KEY_ID`/
`POLYMARKET_SECRET_KEY` still take precedence, matching the old service's
precedence); `order_service_url` is gone. `start_rust_stack.sh`,
`deploy/start.sh`, and `build_linux.sh`'s packaging step no longer
start/bundle the Python process. `poly-order-service/` itself is left in
the repo for reference only — not part of the runtime.

429 retry (1s/2s/4s, 3 attempts, matching docs.polymarket.us/api-reference/
rate-limits) and the balance/positions (30s) and market-constraints (1h)
caches were preserved from the Python service. On any order response with
zero fill (a rejection, not just a transport error), the full outgoing
request JSON and raw response JSON are now logged at `warn!` in
`clients/polymarket.rs::parse_polymarket_order_result` — previously nothing
logged this, so a rejection like `ORD_REJECT_REASON_EXCHANGE_OPTION` left no
record of what was actually sent.

**Important: this migration is not expected to fix
`ORD_REJECT_REASON_EXCHANGE_OPTION` by itself.** That string arrives inside
a normal HTTP 200 (`executions[].orderRejectReason`) — Polymarket accepts
the request and rejects the *order* on some exchange-side check — so
sending the same JSON body from Rust instead of Python changes nothing
about whether the exchange accepts it. The migration was done because it
was already overdue (removes the IPC hop, the extra process/port that can
fail to start, and the class of Python/JSON type-marshaling bugs already
found once) and because it adds the request/response logging above. If
`ORD_REJECT_REASON_EXCHANGE_OPTION` recurs, the next step is to read that
new `warn!` line for the exact payload Polymarket rejected and compare it
field-by-field against a market where an order succeeds.

Read-only endpoints (balance, market book, positions, open orders) were
validated against the live API before this was considered done; order
submission was not live/test-fired as part of this work (per this file's
own rule below) and still needs to be confirmed against a real order.
`cargo build`/`cargo test` pass; `paired_execution.rs` and `polymarket.rs`
unit tests (pure functions) were unchanged and still pass.

**Next steps (not yet started, user wants to pick this up later):** after
running with the direct-API path live, the user is still seeing
`Polymarket US limit order did not fill: ORD_REJECT_REASON_EXCHANGE_OPTION:`
— confirms the prediction above that the migration alone would not fix this;
the new `warn!` request/response logging in `parse_polymarket_order_result`
(`clients/polymarket.rs`) should now have the exact rejected payload the
next time this fires, so start there instead of re-guessing at a cause.
Separately, the user also reported something "seems to disconnect every few
seconds" — not yet triaged, unclear if this is the Kalshi WebSocket
(`clients/kalshi.rs`, reconnect/epoch logic), the frontend's WebSocket
client (`web/src/hooks/useWebSocket.ts`), or something else entirely (e.g.
the backend itself restarting). Needs reproduction and log correlation
before assuming which layer it's in.

User's further observation (2026-09-27, still deferred): the frontend shows
two different messages for what may be the same underlying opportunity —
the opportunity list (left panel) shows what the user describes as "the
typical ORD_QUANTITY exception," while that same opportunity's detail page
(the manual arbitrage-execution view) shows "No profitable size remains
after fees, worst-case prices, Polymarket's minimum trade quantity,
available depth, and the configured max trade amount" (the `reject_with_skip`
message from `find_profitable_contract_size` returning `None` in
`api/routes/orders.rs::execute_arbitrage` — a sizing failure that never
reaches order submission at all). It's not yet established whether "the
typical ORD_QUANTITY exception" is literally a distinct Polymarket reject
reason (i.e. not `ORD_REJECT_REASON_EXCHANGE_OPTION`) or just the user's
shorthand for the same rejection described differently — check the actual
list-panel string against backend logs before assuming either way. If it is
genuinely a different, more specific reject reason than
`ORD_REJECT_REASON_EXCHANGE_OPTION`, that's a stronger lead than anything
found so far. Also worth checking directly: why the list and detail views
would disagree at all for the same opportunity — one implies "never
attempted" (sizing failed) and the other implies "attempted and rejected by
the exchange," which shouldn't both be true for one opportunity at once
unless the two views are reading different data (e.g. a stale/cached
opportunity record vs. a fresh live sizing attempt).

## Session log (2026-09-28) — EXCHANGE_OPTION: new theory + instrumentation added

**Read the full logs first, not just the code comments.** Grepped every
retained log file (`rust-backend/logs/polytaoli.log.2026-07-29` through
`.2026-09-27`, ~50MB, 14 files): there are **zero** `"order FILLED"` /
`ORDER_STATE_FILLED` lines anywhere. Every real order attempt on record (11
total, all in `.2026-09-27`) was rejected with
`ORD_REJECT_REASON_EXCHANGE_OPTION`. So there is no successful-order example
in these logs to diff a rejected payload against — that approach the
2026-09-27 log entry suggested is a dead end until a fill actually happens.

**What the rejections actually look like:** every one goes through exactly
two executions, `EXECUTION_TYPE_NEW` (`cumQuantity: 0`) immediately followed
by `EXECUTION_TYPE_EXPIRED` (`cumQuantity: 0`, `leavesQuantity: 0`). That is
an IOC order the exchange *accepted syntactically* and then expired with no
match — not an immediate hard validation rejection. Request payloads were
clean and consistent (whole-number `quantity`, valid `price.value` strings),
so the float/int theory from 2026-09-26 is not the explanation here.

**What `EXCHANGE_OPTION` means, per
`docs.polymarket.us/institutional/fix-api/fix-reject-reasons`:** it's FIX
`OrdRejReason` tag 103, code 0 — a **generic catch-all**, not a specific
error. Covers price/quantity/notional bounds, insufficient buying power,
self-match prevention, and (most relevant given the NEW→EXPIRED pattern)
liquidity-derivation/insufficient-liquidity-for-fill scenarios. Stop treating
it as one identifiable bug to find — it may legitimately mean "no match,"
in which case the fix is pricing/timing, not a payload field.

**New leading theory:** `find_profitable_contract_size`
(`services/paired_execution.rs`) prices the Polymarket leg at
`worst_price(poly_levels, contracts)` — the exact boundary price needed to
fill the full requested size, with **zero slippage buffer**. Between that
book snapshot (`get_market_book`) and the order reaching
`api.polymarket.us` there's a DB persist (`save_auto_trade_execution`) plus
a full network round trip. On a moving sports market that's enough time for
the book to tick past a price with no headroom, which would explain a
zero-fill IOC on every single attempt with no exceptions. The create-order
API documents a `slippageTolerance` field (`currentPrice` + bips/ticks
tolerance) that this code has never set — worth using once the theory is
confirmed.

**Not yet ruled in or out:** every rejection's response echoes
`order.manualOrderIndicator: MANUAL_ORDER_INDICATOR_AUTOMATIC` even though
the request sends `MANUAL_ORDER_INDICATOR_MANUAL` — consistent across all
11 samples. "Account type or customer order capacity validation failures"
is on the EXCHANGE_OPTION list, so this is a real secondary lead, but could
equally be normal exchange-side reclassification of any synchronous/IOC API
order. Don't chase this before the staleness theory is tested — it's the
weaker signal.

**Instrumentation added this session (no execution-logic change, builds and
`cargo test --lib paired_execution` pass):** `PairedOrderParams` gained
`poly_book_fetched_at_ms` (both callers — `api/routes/orders.rs`'s
`execute_arbitrage` and `api/mod.rs`'s auto-trade handler — now pass
`poly_book.fetched_at_ms`). `submit_paired_order`
(`services/paired_execution.rs`) now logs `book_age_ms` at `info!` right
before every Polymarket submission, and on any zero-fill result, does a
**read-only, diagnostic-only** re-fetch of the market book afterward and
logs `current_best_buy_price` vs. the price we submitted plus
`market_moved_past_our_price: bool`. This adds no risk to the execution
path (fires only after the order already resolved) and makes no order
decisions from the re-fetch. Purpose: the *next* real
`ORD_REJECT_REASON_EXCHANGE_OPTION` in the logs will directly confirm or
refute the staleness theory instead of guessing again.

**Next step:** run the bot until the next live rejection (or next intended
test order, per this file's own live-order rule — don't fire one
speculatively), then grep the new `warn!` line
(`"post-rejection book check for"`) and read `market_moved_past_our_price`.
If true, implement a slippage buffer (`slippageTolerance` field, or a small
cushion added to `worst_price` before submission) as the actual fix. If
false on multiple occurrences, the staleness theory is wrong and the
`manualOrderIndicator` mismatch (or a real Polymarket-side account issue)
becomes the next thing to check, likely requiring Polymarket support
contact since it isn't something log-reading alone can resolve.

## Session log (2026-09-29) — ROOT CAUSE FOUND: `manualOrderIndicator` must be `AUTOMATIC`

**Resolved.** The 2026-09-28 staleness instrumentation immediately paid off:
the first real rejection it caught showed `book_age_ms_at_submit=19` and
`market_moved_past_our_price=false` — the order was fresh and marketable and
still got rejected, ruling out staleness entirely. A follow-up manual test
order at an aggressive, obviously-marketable price (buying at 90¢ against a
~72¢ market, 1 contract, tiny notional, clean `BUY_LONG` with no
`action`/`side` contradiction) was *still* rejected with
`ORD_REJECT_REASON_EXCHANGE_OPTION` — ruling out price, size, and balance.
The user confirmed manual orders on Polymarket's own website work fine on
this same account, ruling out account status/KYC.

The one field consistent across every single rejection, long and short
alike: we sent `manualOrderIndicator: MANUAL_ORDER_INDICATOR_MANUAL`, and
the response always echoed back `MANUAL_ORDER_INDICATOR_AUTOMATIC` — the
exchange was silently reclassifying every order regardless of what we sent.
Git history showed the original Python `poly-order-service/main.py` sent
`AUTOMATIC` originally; commit `575eb5b` ("fix: correcting positions with
new UI requirements", 2026-09-11) flipped it to `MANUAL` with no explanation
in the commit message, and the Rust rewrite just carried that value forward.

**Fix:** `clients/polymarket.rs::submit_limit_order` now sends
`manualOrderIndicator: MANUAL_ORDER_INDICATOR_AUTOMATIC`. First test order
after the fix (`aec-atp-jaumun-alekov-2026-09-28`, 1 contract, limit 75¢)
**filled** at `avg_price=0.59` — order id `CSE6Z40H6YC3`, the first
successful fill this bot has ever recorded via the API in the full log
history checked (back to 2026-07-29). Confirmed against live data, not
inference.

**Secondary bug found and fixed along the way:** while chasing a *different*
failure (a stale market slug that had closed for trading), the actual cause
was invisible because nearly every `error!(...)` call across the codebase
logged an `anyhow::Error` with `{}` instead of `{:#}`, which silently
truncates to the outermost context and hides the real cause (HTTP status,
transport failure, etc.). Fixed every such call site (grep `error!(` to
verify none remain using bare `{}` on an error/reason variable). This isn't
specific to Polymarket — it was hiding root causes in Kalshi, storage,
auto-trade, and WebSocket error paths too.

**Cleanup done this session:** `poly-order-service/` deleted entirely (no
longer referenced anywhere; confirmed nothing in the Rust runtime or start
scripts calls it, and the Rust-only path is now proven with a real fill).
`.github/copilot-instructions.md` and the startup scripts updated to remove
stale references to the Python service, its port, and its test suite.

**Not yet investigated:** the frontend price-pairing question raised this
session (Kalshi showing ~50%/52%/52% for a live match) checked out fine on
a live query immediately after — most likely just live in-play odds
movement between screenshots, not a bug. Revisit only if it recurs with
static (non-moving) numbers.

## Session log (2026-09-29, continued) — log noise, stale closed markets, broken manual Sell

**Log noise:** `📥 Retrieved N Polymarket {category} events` (`clients/polymarket.rs`,
fired once per sport code per scan — 8x per cycle) demoted from `info!` to
`debug!`. It was drowning out the signal the user actually wants visible in
the terminal: scan start/end, matched-market discovery, and REST quote
refresh summaries, all of which already log at `info!` and were left alone
(`services/arbitrage.rs`'s `🔄 Starting scan for new markets`,
`✅ Kalshi/Polymarket REST quote refresh updated N markets`, etc.).

**Every UI API call now logs a line on completion**, not just failures.
`TraceLayer` in `api/mod.rs` previously only surfaced failures (`on_failure`
defaults to ERROR; a successful response's default trace level is DEBUG,
and `main.rs` filtered `tower_http` down to `warn`). Now
`.make_span_with(...)`/`.on_response(...)` are both pinned to `Level::INFO`
and the filter raised to `tower_http=info`, so method/path/status/latency
prints for every request. This is verbose (the frontend polls routinely) —
revisit if it's too noisy in practice.

**Stale closed markets stayed visible indefinitely (e.g. a Kalshi market at
84¢ that had already closed).** Root cause: `market_lifecycle.rs`'s cleanup
only ever inferred "ended" from *extreme* prices (Kalshi ≥99¢/≤2¢, Poly
≥100¢/0¢) sustained 20+ minutes — it never checked either exchange's actual
closed/settled status, so a market that simply stops trading at a normal
price was invisible to it forever. Fixed at the source, in the existing
periodic REST quote-refresh functions (`services/websocket_manager/mod.rs`):
- **Kalshi**: `KalshiMarketQuote` now carries the raw `status` field
  (`unopened`/`open`/`paused`/`closed`/`settled` — confirmed via Kalshi's
  public API docs). `update_kalshi_rest_quotes` treats `closed`/`settled` as
  immediate, undebounced confirmation (Kalshi's own status field doesn't
  flicker) and skips the price update entirely for that cycle.
- **Polymarket**: `update_polymarket_rest_quotes` is already driven by the
  same `active=true&closed=false` feed discovery uses, fetched atomically
  per cycle. A previously-matched market missing from that list is now
  tracked via `poly_missing_streak` and confirmed ended after
  `POLY_MISSING_CONFIRM_STREAK` (2) consecutive misses — debounced, unlike
  Kalshi's status field, because absence-from-a-filtered-list is an
  inference, not an explicit per-market field.
- Both paths feed the *existing* `confirmed_ended_markets` set, so all the
  existing removal plumbing (unsubscribe, cache cleanup, opportunity/
  tracking removal) in `remove_ended_markets()` is reused unchanged.
- 5 new tests in `services/websocket_manager/mod.rs::tests` cover: closed/
  settled status triggers immediate removal at a non-extreme price (the
  exact reported bug), open status at a non-extreme price does *not* get
  marked ended, the Polymarket miss-streak requires 2 consecutive misses,
  and a single reappearance resets the streak rather than decrementing it.

**Manual Sell button was broken for any position whose market had fallen
out of active WebSocket tracking**, failing with "No executable Kalshi
quote is available for this order." Root cause: `place_kalshi_order`
(`api/routes/orders.rs`) sourced its price *exclusively* from
`kalshi_client.get_orderbook()`, which only has data for tickers the
scanner currently has WebSocket-subscribed — a scope tied to "currently
matched by the arbitrage scanner," not "any market I hold a position in."
A held position whose market rolled out of that tracking (including,
notably, via the closed-market fix directly above — closing now actively
unsubscribes) could no longer be sold through the UI, at all, even though
the position itself might still be genuinely closeable on the exchange.
Fixed: `place_kalshi_order` now falls back to a fresh REST quote
(`kalshi_client.get_market_quotes`, already used elsewhere, works for any
ticker regardless of subscription state) when the WebSocket book has
nothing. New `KalshiMarketQuote::limit_price_for_order` mirrors
`OrderBook::limit_price_for_order`'s sign convention (REST only has asks,
so a sell price is the complement of the *opposite* side's ask — same
yes/no complement identity used throughout this codebase). This is a
price-only fallback for one manual, already-known-quantity order; it must
never be wired into auto-trade sizing or depth, which stays exclusively on
the fresh WebSocket book. 3 new unit tests in `clients/kalshi.rs::tests`
cover buy/sell price derivation and rejection of an unrecognized side or
action.

All three fixes: `cargo build --release` and `cargo test --lib` both clean
(57 passing).

## Positions tab — design decision (2026-09-29)

**Status: implemented 2026-09-29** (`cargo test --lib` 68 passing incl. 11 new
in `services/positions.rs`; `npm run build` clean; `npm run lint` is broken
independent of this change — `--ext` flag unsupported by the flat eslint
config). **Not yet verified against live data:** the Kalshi NO-leg
`market_exposure_dollars` sign risk below, and Polymarket `cost`/`cashValue`
sign/units on a real short position. Kalshi mark = complement of the opposite
side's ask (liquidation value), not a bid. Original design follows.

**Why:** the Positions panel (`web/src/components/OrderPanel.tsx`) shows a
flat, unrelated list of raw exchange positions. Kalshi legs show no entry
price, no live price, and no unrealized P&L at all (only exposure $ and
*realized* P&L, which is ~always $0 for an open position). Polymarket legs
reference `avgPrice`/`curPrice`/`pnlPercent` in the TS type, but
`normalize_positions` (`clients/polymarket.rs`) never actually populates
them — a pre-existing bug, not by design. Most importantly for a hedged
arbitrage bot: there is no indication that a Kalshi leg and a Polymarket leg
were opened together as one trade, nor any way to tell a properly hedged
position from a naked, one-sided residual (e.g. from a failed
neutralization) at a glance.

**The fix requires no new external API calls.** Polymarket's
`/v1/portfolio/positions` response already includes a `cost` field (total
cost basis) that `normalize_positions` silently discards today — combined
with the `cashValue` it already reads, that's avg entry price, current
mark, and unrealized P&L for free. Kalshi needs one additional call to the
REST quote endpoint already used elsewhere
(`clients/kalshi.rs::get_market_quotes`) for a live mark; entry price is
`market_exposure_dollars / abs(position_fp)`.

**Pairing decision:** match on the full tuple `(kalshi_market_id,
kalshi_side, polymarket_market_id, polymarket_side)` against this bot's own
trade history (`ArbitrageStorage::get_auto_trade_history`, already exists —
no new endpoint needed for the history read itself), using only the most
recent history row with status `executed`/`neutralized`/`partial_paired`
for each currently-held leg. Market ID alone is insufficient — the same
Kalshi+Polymarket market pair can appear on opposite sides across different
trades over time, and only the side-aware match prevents cross-pairing.
Both legs must currently be held (nonzero size) to render as a paired card;
otherwise each renders standalone. New `/api/positions/unified` endpoint
does this server-side (one implementation, not duplicated Rust/TS pairing
logic) and returns already-grouped cards; the frontend becomes a pure
renderer.

**Fees:** Kalshi has `fees_paid_dollars`, ships as planned. Polymarket has
**no fee field anywhere in its API** (positions or trade-history
endpoints) — confirmed via docs.polymarket.us, not an oversight to fix
later trivially. Render "n/a" for a Polymarket leg's fee, never a fake
$0.00. Capturing Polymarket fees going forward (from
`commissionNotionalCollected` on the order-submission response, already
parsed and currently discarded in `parse_polymarket_order_result`) is
explicitly out of scope for this change and cannot backfill fees for
positions already open today.

**Open risk, verify before trusting displayed P&L on Kalshi NO legs:**
`market_exposure_dollars`'s sign is not documented by Kalshi for a NO
position specifically (only `position_fp`'s sign is documented — positive
= YES, negative = NO). The plan uses `abs(market_exposure_dollars)` as cost
basis, which is safe regardless of sign, but check a real held NO
position's raw JSON against this assumption before shipping.

**UI:** paired cards get a 🔗 badge, combined cost/value/P&L, and both legs
shown underneath; standalone/unhedged cards get a ⚠ amber badge so a naked
residual position is impossible to miss. New files: `services/positions.rs`
(pure economics + pairing functions, unit-tested — ~10 test cases covering
unmatched legs, stale opposite-side history rows, rejected trades not
counting as pairs, no double-counting a leg into two cards). Modified:
`clients/polymarket.rs` (`normalize_positions`), `api/routes/accounts.rs`
(new handler), `api/mod.rs` (route registration), `web/src/types/index.ts`,
`web/src/utils/api.ts`, `web/src/components/OrderPanel.tsx`.

## Architecture

Two services, started together via `./start_rust_stack.sh`:

- **`rust-backend/`** (Axum + Tokio, port `8000`) — the core engine.
  - `core/`: `matcher.rs` (event/market matching between exchanges, incl.
    NBA-specific logic in `nba_teams.rs`/`competitors.rs`), `calculator.rs`
    (profit-margin math).
  - `clients/`: `kalshi.rs` (REST + WebSocket, RSA-PSS-signed requests
    against `external-api.kalshi.com`), `polymarket.rs` (REST against
    `gateway.polymarket.us` for market data and `api.polymarket.us` for
    orders/account/portfolio, Ed25519-signed via `polymarket_auth.rs`) —
    both exchanges are called directly, with no separate order-service
    process for either.
  - `services/`: `arbitrage.rs` (opportunity detection/control),
    `websocket_manager/` (live delivery to frontend, `auto_trade.rs` for
    automated paired execution, `market_lifecycle.rs`), `storage/`
    (SQLite persistence, `auto_trade_repo.rs` tracks execution state
    machine), `telegram.rs` (alerts).
  - `api/`: HTTP routes and the `mod.rs` request/response wiring.
- **`web/`** (Vite + React + TS, port `5173`) — dashboard: live
  opportunities, order forms, position/history views, WebSocket client
  (`hooks/useWebSocket.ts`).

`poly-order-service/` (the old FastAPI wrapper around the official
`polymarket-us` SDK) has been deleted from the repo — see the 2026-09-27
session log above for the migration off it, and the 2026-09-29 log for its
removal once the Rust path was proven working end-to-end (first confirmed
live fill).

## Critical invariants (do not weaken without explicit user sign-off)

These come from `.github/copilot-instructions.md` — read that file too, it's
the authoritative day-to-day rulebook and may be more current than this
summary.

- **Kalshi signing**: sign the path only, exclude query params.
- **Polymarket order responses**: read fills from `executions[].order`
  (e.g. `.avgPx`), never from the top-level response envelope.
- **Polymarket depth**: only from `/v1/markets/{market_slug}/book`
  (`clients/polymarket.rs::get_market_book`) — never gateway display quotes,
  account caches, or the non-US CLOB API.
- **Kalshi depth**: only from a fresh Kalshi WebSocket book at execution
  time. Kalshi size is fixed-point — floor when converting to whole
  contracts, never round up.
- **Pairing logic**: Kalshi YES ↔ Polymarket NO, Kalshi NO ↔ Polymarket YES,
  for the *tracked competitor*. The native Polymarket LONG/SHORT side must
  be resolved from that competitor mapping, never inferred from a raw
  outcome index.
- **Execution state machine**: persist a `submitting` record before either
  leg fires; durably update each acknowledgement before submitting the
  second leg. On startup, halt if an unresolved `submitting` record exists
  until it's reconciled.
- **Neutralization**: on unequal fills, attempt a bounded IOC/FAK offset
  only within `neutralization_max_loss_cents` (default 5¢/contract). If it
  can't fully close, record the residual durably, disable auto-trading, and
  alert via Telegram — don't silently retry or hide it.
- **Safeguards**: never weaken order-size, profit-margin, duration,
  duplicate-trade, or execution-count limits without being asked.
- Preserve WebSocket message compatibility between the Rust manager and the
  React client when touching opportunity models.

## Validation

- Rust: `cargo test` from `rust-backend/`.
- Frontend: `npm run lint && npm run build` from `web/`.
- `rust-backend/examples/verify_polymarket_direct.rs` (`cargo run --example
  verify_polymarket_direct` from `rust-backend/`) hits the live Polymarket
  US API's read-only endpoints (balance, positions, open orders, a market
  book) using real credentials from `config.toml` — useful for confirming
  Ed25519 signing/parsing still works after touching `clients/polymarket.rs`
  or `clients/polymarket_auth.rs`. Do not run it as part of routine/automatic
  validation — it's a manual check, and it never places an order.
- **Never enable automatic trading or submit live/test orders** as part of
  routine development or debugging. Config default is `auto_trade.enabled =
  false`; keep it that way unless the user explicitly asks to test live.

## Config & secrets

- `rust-backend/config.toml` is gitignored and holds real Kalshi/Polymarket
  credentials — never read it back into chat output or commit it.
  `config.example.toml` is the template to update when adding new settings.
- Default dev ports: frontend `5173`, Rust API `8000`. Keep these aligned
  across services if changed.

## Session log (2026-09-29, continued) — faster detection: Polymarket WebSocket

**Why:** detection was entirely REST-polled (~3s per exchange, the Poly poll
itself often 4s+ for 360 markets), and the Kalshi WS prices never fed
detection (`is_market_ready` needs fresh REST quotes on both sides). A price
move could sit unseen 3–4s.

**Done:** `clients/polymarket_ws.rs` streams
`wss://api.polymarket.us/v1/ws/markets` (Ed25519 headers, same signing as
REST, `SUBSCRIPTION_TYPE_MARKET_DATA_LITE`, ≤100 slugs/subscription,
reconnect with 1s→30s backoff, 30s silence watchdog, new slugs subscribed
every 5s). `longQuote`/`shortQuote` were verified live to equal the
gateway's per-side `marketSides[].quote` (11/12 identical, the 12th was
live movement). `WebSocketManager::update_polymarket_ws_quote` maps each
competitor to its own native side, updates the existing quote cache, and
calls `calculate_and_notify` immediately. REST polling stays as fallback and
for ended-market detection; `update_polymarket_rest_quotes_at` won't let a
REST snapshot fetched *before* a WS quote overwrite it. `MIN_REST_QUOTE_
REFRESH_SECS` lowered 3→1 (configured `refresh_interval` is still 3 —
lowering it multiplies Kalshi REST and Polymarket gateway request volume).
78 lib tests pass. **Not yet observed running live end-to-end** (WS-driven
detection in the app's own logs), and `min_duration_ms` (500) is untouched.
Kalshi-side detection is still REST-polled; using the Kalshi WS book to
trigger detection is the remaining lever.
`examples/probe_polymarket_ws.rs` is a read-only WS-vs-REST comparison.

**Kalshi WebSocket-driven detection (same day, follow-up):** Kalshi's WS book
already produced `yes_ask`/`no_ask` per update (`clients/kalshi.rs::
parse_ws_message`) but `on_kalshi_price_update` only stored them.
`WebSocketManager::apply_kalshi_ws_quote` now writes them into the quote
cache and calls `calculate_and_notify` only when the best prices changed
(deeper-level deltas just re-stamp freshness). Same REST-can't-overwrite-
newer-WS rule as Polymarket (`update_kalshi_rest_quotes_at`); REST still
applies closed/settled status and self-heals any WS drift within one cycle.
Price-only: execution depth still comes exclusively from the fresh WS book.
80 lib tests pass. Not yet observed live in the app's logs.

**Polymarket depth on the detail page + tracking (same day):** the detail
page's Polymarket ×size/$ was blank because `/api/orderbook/depth` hardcoded
`polymarket: None` and `calculate_and_notify` forced `poly_ask_depth/size` to
0. The WS client now also subscribes `SUBSCRIPTION_TYPE_MARKET_DATA` (full
book; each message is a full snapshot, verified identical to REST
`/book` on 3/3 markets) and stores it in `poly_live_books`. The depth
endpoint serves best-level price/size for the tracked competitor (Yes) and
its opponent (No) from that book, falling back to one REST `get_market_book`
call if no WS book has arrived yet. Opportunities' `poly_ask_depth/size` now
come from the same book. **These are display/tracking values only — order
sizing still fetches its own fresh REST book (critical invariant
unchanged).** Also fixed along the way: tracking's depth snapshot used the
tracked team's own native side regardless of Kalshi side; the Polymarket leg
actually buys the *opponent* when Kalshi is YES (see `OpportunityList.tsx`
`polymarket_competitor`), so it now uses that side. 82 lib tests pass.
