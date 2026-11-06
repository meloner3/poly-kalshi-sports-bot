//! Order operations endpoints

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use tracing::{error, info, warn};

use crate::api::AppState;
use crate::models::PolymarketPositionSide;
use crate::services::paired_execution::{
    executable_depth, find_profitable_contract_size, kalshi_buy_levels, polymarket_buy_levels,
    submit_paired_order, EXECUTABLE_BOOK_MAX_AGE,
};
use crate::services::PairedOrderParams;

/// Kalshi order request
#[derive(Deserialize)]
pub struct KalshiOrderRequest {
    ticker: String,
    side: String,
    action: String,
    count: i32,
}

/// Place a Kalshi order
pub async fn place_kalshi_order(
    State(state): State<Arc<AppState>>,
    Json(req): Json<KalshiOrderRequest>,
) -> impl IntoResponse {
    let kalshi_client = {
        let service = state.service.read().await;
        service.kalshi_client.clone()
    };

    if !matches!(req.side.as_str(), "yes" | "no")
        || !matches!(req.action.as_str(), "buy" | "sell")
        || req.count <= 0
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "success": false,
                "error": "side must be yes or no, action must be buy or sell, and count must be positive"
            })),
        )
            .into_response();
    }

    // The WebSocket book only covers tickers the scanner is currently
    // tracking/subscribed to. A held position's market can fall out of that
    // set (e.g. the match ended and it was unsubscribed) while still being
    // perfectly sellable on the exchange, so a manual order - already a
    // known, user-specified quantity, not something auto-trade sizes off of
    // depth - falls back to a fresh REST quote rather than failing outright.
    let ws_price = kalshi_client
        .get_orderbook(&req.ticker)
        .and_then(|book| book.limit_price_for_order(&req.side, &req.action));

    let price = match ws_price {
        Some(price) if (1..=99).contains(&price) => Some(price),
        _ => match kalshi_client.get_market_quotes(&[req.ticker.clone()]).await {
            Ok(quotes) => quotes
                .iter()
                .find(|quote| quote.market_id == req.ticker)
                .and_then(|quote| quote.limit_price_for_order(&req.side, &req.action)),
            Err(e) => {
                warn!(
                    "REST quote fallback failed for manual Kalshi order on {}: {:#}",
                    req.ticker, e
                );
                None
            }
        },
    };

    let Some(price) = price.filter(|price| (1..=99).contains(price)) else {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "success": false,
                "error": "No executable Kalshi quote is available for this order"
            })),
        )
            .into_response();
    };

    match kalshi_client
        .place_order(&req.ticker, &req.action, &req.side, req.count, price)
        .await
    {
        Ok(response) => {
            info!(
                "Manual Kalshi order placed: ticker={} side={} action={} count={} price={}c response={:?}",
                req.ticker, req.side, req.action, req.count, price, response
            );
            Json(serde_json::json!({
                "success": true,
                "order": response,
                "data": response
            }))
            .into_response()
        }
        Err(e) => {
            error!("Failed to place Kalshi order: {:#}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "error": e.to_string()
                })),
            )
                .into_response()
        }
    }
}

/// Polymarket order request
#[derive(Deserialize)]
pub struct PolymarketOrderRequest {
    market_slug: String,
    position_side: PolymarketPositionSide,
    side: String,
    contracts: i32,
    price: f64,
}

/// Place a Polymarket order
pub async fn place_polymarket_order(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PolymarketOrderRequest>,
) -> impl IntoResponse {
    let service = state.service.read().await;

    if req.market_slug.trim().is_empty()
        || !matches!(req.side.as_str(), "buy" | "sell")
        || req.contracts <= 0
        || !req.price.is_finite()
        || !(0.0..1.0).contains(&req.price)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "success": false,
                "error": "market_slug, position_side, buy or sell action, a positive whole contract count, and a current price are required"
            })),
        )
            .into_response();
    }

    match service
        .place_polymarket_order(
            &req.market_slug,
            req.position_side,
            &req.side,
            req.contracts,
            req.price,
        )
        .await
    {
        Ok(response) => {
            info!(
                "Manual Polymarket order placed: market_slug={} side={} contracts={} price={} order_id={:?} status={:?} filled_contracts={:?}",
                req.market_slug,
                req.side,
                req.contracts,
                req.price,
                response.get("order_id"),
                response.get("status"),
                response.get("filled_contracts"),
            );
            Json(serde_json::json!({
                "success": true,
                "order_id": response.get("order_id"),
                "status": response.get("status"),
                "filled_contracts": response.get("filled_contracts"),
                "elapsed_ms": response.get("latency_ms"),
                "data": response.get("data")
            }))
            .into_response()
        }
        Err(e) => {
            error!("Failed to place Polymarket order: {:#}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "error": e.to_string()
                })),
            )
                .into_response()
        }
    }
}

/// Arbitrage execution request
#[derive(Deserialize)]
pub struct ExecuteArbitrageRequest {
    event_name: String,
    team_name: String,
    kalshi_side: String,
    polymarket_competitor: String,
    contracts: i32,
    /// When true (the default), sizing/pricing is computed against real
    /// fresh depth but nothing is submitted to either exchange. Any caller
    /// that omits this field gets the safe default.
    #[serde(default = "default_dry_run")]
    dry_run: bool,
}

fn default_dry_run() -> bool {
    true
}

/// Persist a skip record (visible in the history explorer) and respond with
/// the specific reason a manual execution attempt was not submitted.
fn reject_with_skip(
    service: &crate::services::ArbitrageService,
    market_key: &str,
    req: &ExecuteArbitrageRequest,
    kalshi_market_id: &str,
    polymarket_market_id: &str,
    reason: &str,
) -> axum::response::Response {
    if service.ws_manager.should_record_skip(market_key, reason) {
        // Kalshi YES <-> Polymarket NO, Kalshi NO <-> Polymarket YES for the
        // tracked competitor (see CLAUDE.md pairing invariant) — this column
        // mirrors kalshi_side, it is not the competitor name.
        let polymarket_side = if req.kalshi_side == "yes" {
            "no"
        } else {
            "yes"
        };
        if let Err(error) = service
            .ws_manager
            .get_storage()
            .save_skipped_auto_trade_record(
                &req.event_name,
                &req.team_name,
                kalshi_market_id,
                polymarket_market_id,
                &req.kalshi_side,
                polymarket_side,
                req.contracts,
                0.0,
                0.0,
                0.0,
                0,
                reason,
            )
        {
            error!("Failed to save skipped manual-execute record: {:#}", error);
        }
    }
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "success": false,
            "error": reason,
        })),
    )
        .into_response()
}

/// Execute an arbitrage trade. Behind `dry_run` (default true): the request
/// is sized and priced against the same fresh, executable depth the
/// auto-trade executor uses, but nothing is submitted to either exchange
/// until the caller explicitly sends `dry_run: false`.
pub async fn execute_arbitrage(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExecuteArbitrageRequest>,
) -> impl IntoResponse {
    if !matches!(req.kalshi_side.as_str(), "yes" | "no") || req.contracts <= 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "success": false,
                "error": "kalshi_side must be yes or no and contract count must be positive"
            })),
        )
            .into_response();
    }

    let service = state.service.read().await;
    let matched_market = service
        .get_matched_markets()
        .iter()
        .find(|market| market.event_name == req.event_name && market.team_name == req.team_name)
        .cloned();
    let Some(matched_market) = matched_market else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"success": false, "error": "Market not found"})),
        )
            .into_response();
    };

    // Resolve the user-selected competitor against the matched US gateway
    // market. This yields the authoritative market slug and LONG/SHORT side;
    // neither is inferred from a yes/no array position.
    let Some(execution) = matched_market
        .polymarket_market
        .us_execution_for_competitor(&req.polymarket_competitor)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "success": false,
                "error": "Polymarket competitor is not part of the matched market"
            })),
        )
            .into_response();
    };

    let market_key = matched_market.market_key();
    let kalshi_market_id = matched_market.kalshi_market.market_id.clone();

    // Same system-wide breakers the auto-trade executor respects, so the
    // manual button can't bypass them.
    if service.ws_manager.is_market_excluded(&market_key) {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "success": false,
                "error": "This market has been excluded from paired trading"
            })),
        )
            .into_response();
    }

    let auto_state = service.ws_manager.get_auto_trade_state();
    if auto_state.trade_count >= auto_state.max_trade_count {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "success": false,
                "error": format!(
                    "Maximum trade count reached ({}/{}); reset it in Auto-Trade settings before executing more",
                    auto_state.trade_count, auto_state.max_trade_count
                )
            })),
        )
            .into_response();
    }

    // Real, fresh executable depth only — never cached display quotes.
    let Some(kalshi_book) = service
        .kalshi_client
        .get_fresh_orderbook(&kalshi_market_id, EXECUTABLE_BOOK_MAX_AGE)
    else {
        return reject_with_skip(
            &service,
            &market_key,
            &req,
            &kalshi_market_id,
            &execution.market_slug,
            "Fresh Kalshi websocket executable depth is unavailable",
        );
    };
    let poly_book = match service
        .polymarket_client
        .get_market_book(&execution.market_slug)
        .await
    {
        Ok(book) => book,
        Err(reason) => {
            return reject_with_skip(
                &service,
                &market_key,
                &req,
                &kalshi_market_id,
                &execution.market_slug,
                &format!("Fresh Polymarket US executable book is unavailable: {reason}"),
            );
        }
    };

    let kalshi_levels = kalshi_buy_levels(&kalshi_book, &req.kalshi_side);
    let poly_levels = polymarket_buy_levels(&poly_book, execution.position_side);
    let size_cap = executable_depth(&kalshi_levels)
        .min(executable_depth(&poly_levels))
        .min(req.contracts);

    // min_contracts is 1 here (not the auto-trade config floor): a manual
    // click should fill whatever profitable size is available up to the
    // requested amount, not bail out under a floor meant to decide whether
    // an opportunity is worth automating.
    let Some((contracts, kalshi_price_cents, poly_price, fee, profit_margin)) =
        find_profitable_contract_size(
            size_cap,
            1,
            auto_state.max_amount,
            poly_book.minimum_trade_qty,
            &kalshi_levels,
            &poly_levels,
        )
    else {
        let reason = if poly_book.minimum_trade_qty.is_none() {
            "Polymarket US market minimum trade quantity is unavailable"
        } else {
            "No profitable size remains after fees, worst-case prices, Polymarket's minimum trade quantity, available depth, and the configured max trade amount"
        };
        return reject_with_skip(
            &service,
            &market_key,
            &req,
            &kalshi_market_id,
            &execution.market_slug,
            reason,
        );
    };

    let outcome = submit_paired_order(
        &service,
        &state.telegram_client,
        PairedOrderParams {
            event_name: req.event_name.clone(),
            team_name: req.team_name.clone(),
            kalshi_market_id: kalshi_market_id.clone(),
            kalshi_side: req.kalshi_side.clone(),
            polymarket_market_slug: execution.market_slug.clone(),
            polymarket_position_side: execution.position_side,
            contracts,
            kalshi_price_cents,
            poly_price,
            kalshi_fee: fee,
            profit_margin,
            duration_ms: 0,
            poly_book_fetched_at_ms: poly_book.fetched_at_ms,
            neutralization_max_loss_cents: auto_state.neutralization_max_loss_cents,
            dry_run: req.dry_run,
        },
    )
    .await;

    if outcome.status == "executed" {
        if let Err(reason) = service.ws_manager.increment_trade_count() {
            error!(
                "Failed to increment trade count after manual execution: {:#}",
                reason
            );
        }
    }

    let record = &outcome.execution_record;
    let status_code = match outcome.status.as_str() {
        "dry_run" | "executed" | "neutralized" | "partial_paired" | "rejected" => StatusCode::OK,
        "exposed" => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let top_level_error = match outcome.status.as_str() {
        "exposed" => Some(
            "Unequal fills could not be fully neutralized within the configured bound; auto-trading has been halted"
                .to_string(),
        ),
        "rejected" => Some("Both legs were rejected by the exchanges".to_string()),
        "partial_paired" => Some("Only a partial paired fill was achieved".to_string()),
        "persistence_failed_before_submission" | "persistence_failed_after_poly_ack"
        | "persistence_failed_after_kalshi_ack" => Some(
            "Execution could not be durably recorded and was not (fully) retried".to_string(),
        ),
        _ => None,
    };
    let success = matches!(outcome.status.as_str(), "dry_run" | "executed");

    (
        status_code,
        Json(serde_json::json!({
            "success": success,
            "status": outcome.status,
            "error": top_level_error,
            "contracts": record.contracts,
            "kalshi_price": record.kalshi_price,
            "polymarket_price": record.polymarket_price,
            "profit_margin": record.profit_margin,
            "kalshi": {
                "success": record.kalshi_success,
                "error": record.kalshi_error,
                "order_id": record.kalshi_order_id,
                "filled_contracts": record.kalshi_filled_contracts,
            },
            "polymarket": {
                "success": record.polymarket_success,
                "error": record.polymarket_error,
                "order_id": record.polymarket_order_id,
                "filled_contracts": record.polymarket_filled_contracts,
            },
        })),
    )
        .into_response()
}

/// Query params for orders
#[derive(Deserialize)]
pub struct OrdersQuery {
    status: Option<String>,
}

/// Get Kalshi orders
pub async fn get_kalshi_orders(
    State(state): State<Arc<AppState>>,
    Query(query): Query<OrdersQuery>,
) -> impl IntoResponse {
    let service = state.service.read().await;

    match service
        .kalshi_client
        .get_orders(query.status.as_deref())
        .await
    {
        Ok(orders) => Json(serde_json::json!({
            "orders": orders.get("orders").unwrap_or(&serde_json::json!([]))
        }))
        .into_response(),
        Err(e) => {
            error!("Failed to get Kalshi orders: {:#}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "orders": [],
                    "error": e.to_string()
                })),
            )
                .into_response()
        }
    }
}

/// Get Polymarket orders
pub async fn get_polymarket_orders(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;

    match service.polymarket_client.get_open_orders().await {
        Ok(orders) => Json(serde_json::json!({
            "orders": orders
        }))
        .into_response(),
        Err(e) => {
            error!("Failed to get Polymarket orders: {:#}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "orders": [],
                    "error": e.to_string()
                })),
            )
                .into_response()
        }
    }
}

/// Cancel Kalshi order
pub async fn cancel_kalshi_order(
    State(state): State<Arc<AppState>>,
    Path(order_id): Path<String>,
) -> impl IntoResponse {
    let service = state.service.read().await;

    match service.kalshi_client.cancel_order(&order_id).await {
        Ok(_) => Json(serde_json::json!({
            "success": true
        }))
        .into_response(),
        Err(e) => {
            error!("Failed to cancel Kalshi order: {:#}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "error": e.to_string()
                })),
            )
                .into_response()
        }
    }
}

/// Cancel Polymarket order
pub async fn cancel_polymarket_order(
    State(state): State<Arc<AppState>>,
    Path(order_id): Path<String>,
) -> impl IntoResponse {
    let service = state.service.read().await;

    match service.polymarket_client.cancel_order(&order_id).await {
        Ok(_) => Json(serde_json::json!({
            "success": true
        }))
        .into_response(),
        Err(e) => {
            error!("Failed to cancel Polymarket order: {:#}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "error": e.to_string()
                })),
            )
                .into_response()
        }
    }
}

/// Get auto-trade queue status
pub async fn get_auto_trade_queue(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;
    let queue: Vec<String> = service
        .ws_manager
        .auto_trade_queue
        .read()
        .iter()
        .cloned()
        .collect();
    let is_executing = service
        .ws_manager
        .is_auto_trading
        .load(std::sync::atomic::Ordering::Relaxed);

    Json(serde_json::json!({
        "queue": queue,
        "queue_length": queue.len(),
        "is_executing": is_executing
    }))
}
