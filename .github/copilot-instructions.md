# Copilot Instructions

## Repository Layout

- `rust-backend/` is the Axum/Tokio API, arbitrage engine, exchange clients, SQLite storage, and WebSocket server. It calls both Kalshi and Polymarket US directly (RSA-PSS and Ed25519-signed requests respectively) — there is no separate order-submission process for either exchange.
- `web/` is the Vite, React, and TypeScript user interface.
- Root scripts start the development stack and build deployment packages. Keep the service ports aligned with the configuration: frontend `5173`, Rust API `8000`.

## Change Guidelines

- Treat all price, order, and position data as financial data. Preserve decimal precision, validate external inputs, and do not weaken order-size, profit-margin, duration, or execution-count safeguards.
- Keep secrets out of source control. Use `rust-backend/config.example.toml` for new configuration defaults and document required configuration in `README.md`; never add real credentials.
- Keep Rust API models, route handlers, frontend types, and API client calls consistent when an endpoint or payload changes.
- Follow existing error-handling patterns. Surface exchange, signing, storage, and network failures rather than hiding them with fallback data.
- Preserve WebSocket message compatibility for the React client when changing the Rust WebSocket manager or opportunity models.

## Exchange Integration Status

- Kalshi uses the v2 API. Sign authenticated requests with the path only; exclude query parameters from the signed payload.
- Polymarket US order submission (`POST /v1/orders`) returns an execution envelope with `executions[].order`; do not read an order ID, state, or fills from the top-level response. `manualOrderIndicator` must be `MANUAL_ORDER_INDICATOR_AUTOMATIC` for API-submitted orders — sending `MANUAL` produced a 100% `ORD_REJECT_REASON_EXCHANGE_OPTION` rejection rate (see CLAUDE.md 2026-09-28/29 session log).
- Polymarket US executable depth comes directly from the gateway's
  `/v1/markets/{market_slug}/book` endpoint (`clients/polymarket.rs::get_market_book`).
  The payload is a `marketData` envelope containing timestamped
  `bids` and `offers` with USD price and quantity. Do not substitute gateway
  display quotes, account caches, or the non-US CLOB API.
- Polymarket US portfolio positions are returned as a market-keyed map. Normalize them into the frontend's position array before rendering.
- Manual Polymarket orders are contract-sized FAK/IOC limit orders. A canceled or rejected order with zero `cumQuantity` is not a successful fill and should clearly report the exchange reason.
- The Polymarket US account endpoints are Cloudflare rate-limited. Balance and position caching is for dashboard reads only; it must not provide pricing, depth, or execution inputs.
- React position responses are external data. Confirm payload arrays and finite position sizes before rendering them. WebSocket cleanup must cancel pending reconnects so an intentionally closed socket does not reconnect after unmount.

## Automatic Paired Executor

- Automatic execution must preflight fresh executable books from both venues.
  Kalshi depth may only come from its fresh WebSocket book; Polymarket US depth
  must come from `/v1/markets/{market_slug}/book`. Size against the worst level
  needed, not a displayed quote or aggregate dashboard liquidity.
- Preserve contract, profit-margin, duration, duplicate-trade, maximum amount,
  and execution-count limits. Kalshi depth is fixed-point: floor it when
  ordering whole contracts; never round up.
- Execute complementary outcomes only. For a tracked competitor, Kalshi YES
  pairs with Polymarket NO, and Kalshi NO pairs with Polymarket YES. Resolve
  the exact Polymarket native LONG/SHORT side from that selected competitor
  mapping. Do not always resolve the tracked competitor when the strategy
  requires the opponent's outcome.
- Parse current response fields: Kalshi fill quantity is `fill_count_fp`; a
  Kalshi NO fill price is the complement of its single YES-book price;
  Polymarket's average fill is `executions[].order.avgPx`.
- Persist a `submitting` record before either order, and durably update each
  acknowledgement before submitting the other leg. On startup, halt execution
  if an unresolved `submitting` record exists; reconcile before re-enabling.
- On unequal fills, attempt a bounded IOC/FAK neutralization only within the
  persisted `neutralization_max_loss_cents` setting (default: 5 cents per
  contract). If it cannot fully close the residual, record it durably, disable
  auto-trading, and alert through the configured Telegram client.

## Validation

- For Rust changes, run `cargo test` from `rust-backend/`.
- For frontend changes, run `npm run lint` and `npm run build` from `web/`.
- Do not enable automatic trading or submit test orders as part of routine development validation.
