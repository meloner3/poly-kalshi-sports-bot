//! Shared paired-order execution core.
//!
//! Both the auto-trade executor and the manual "Execute" endpoint place a
//! Kalshi leg and a Polymarket leg for the same tracked competitor and must
//! obey the same invariants (durable `submitting` record before either leg
//! fires, bounded neutralization on unequal fills, halt-on-persistence-
//! failure). Keeping this in one module means both callers share the exact
//! same safety-critical code path instead of two copies drifting apart.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use tracing::{error, info, warn};

use crate::clients::{
    KalshiOrderResult, PolymarketBookLevel, PolymarketMarketBook, PolymarketOrderResult,
};
use crate::models::PolymarketPositionSide;
use crate::services::{ArbitrageService, AutoTradeExecutionRecord, TelegramClient};

/// For Kalshi: a connection-liveness backstop, checked against the last
/// WebSocket message received on *any* subscribed ticker (see
/// `KalshiClient::get_fresh_orderbook`) - not "this ticker changed
/// recently," which Kalshi only signals on a real book change and would
/// wrongly reject an accurate but quiet market. This just needs to be loose
/// enough to never fire during normal operation and tight enough to catch a
/// genuinely hung connection promptly. The actual per-book correctness
/// check is the connection epoch, not this duration.
/// For Polymarket: the Python endpoint is requested synchronously, so its
/// response is inherently a fresh book: this just bounds total round trip.
pub const EXECUTABLE_BOOK_MAX_AGE: Duration = Duration::from_secs(30);
/// A conservative floor on Polymarket leg notional (contracts * price), kept
/// as an extra safety margin. This is NOT the actual cause of
/// ORD_REJECT_REASON_EXCHANGE_OPTION rejections - real rejected orders have
/// had notional from $3.80-$9.50, well above this floor. That reject reason
/// is a generic exchange catch-all (see docs.polymarket.us) that, per
/// official docs, is most likely explained by each market's own
/// `minimumTradeQty` (see `PolymarketMarketBook::minimum_trade_qty`), not a
/// fixed dollar amount. Kalshi has no equivalent floor, so this only bounds
/// the Polymarket leg's size search.
pub const POLYMARKET_MIN_NOTIONAL_USD: f64 = 1.0;

#[derive(Debug)]
pub struct RecoveryResult {
    pub filled_contracts: i32,
    pub order_id: Option<String>,
    pub error: Option<String>,
}

pub fn failed_poly_order(error: String) -> PolymarketOrderResult {
    PolymarketOrderResult {
        accepted: false,
        filled_contracts: 0,
        fill_quantity_valid: true,
        order_id: None,
        status: None,
        error: Some(error),
        latency_ms: None,
        average_fill_price: None,
    }
}

pub fn failed_kalshi_order(error: String) -> KalshiOrderResult {
    KalshiOrderResult {
        accepted: false,
        filled_contracts: 0,
        fill_quantity_valid: true,
        order_id: None,
        status: None,
        error: Some(error),
        average_fill_price: None,
    }
}

pub fn can_submit_kalshi_after_polymarket(polymarket_filled_contracts: i32) -> bool {
    polymarket_filled_contracts > 0
}

pub fn kalshi_buy_levels(book: &crate::clients::kalshi::OrderBook, side: &str) -> Vec<(i32, i32)> {
    let mut levels = match side {
        "yes" => book
            .no
            .iter()
            .rev()
            .map(|(price, qty)| (100 - price, *qty))
            .collect(),
        "no" => book
            .yes
            .iter()
            .rev()
            .map(|(price, qty)| (100 - price, *qty))
            .collect(),
        _ => Vec::new(),
    };
    levels.sort_by_key(|(price, _)| *price);
    levels
}

pub fn kalshi_sell_levels(book: &crate::clients::kalshi::OrderBook, side: &str) -> Vec<(i32, i32)> {
    let mut levels = match side {
        "yes" => book
            .yes
            .iter()
            .rev()
            .map(|(price, qty)| (*price, *qty))
            .collect(),
        "no" => book
            .no
            .iter()
            .rev()
            .map(|(price, qty)| (*price, *qty))
            .collect(),
        _ => Vec::new(),
    };
    levels.sort_by(|left, right| right.0.cmp(&left.0));
    levels
}

pub fn polymarket_buy_levels(
    book: &PolymarketMarketBook,
    position_side: PolymarketPositionSide,
) -> Vec<(f64, i32)> {
    let levels = match position_side {
        // Long buys consume native LONG offers in ascending price order.
        PolymarketPositionSide::Long => &book.offers,
        // Short buys consume native LONG bids descending, at complement price.
        PolymarketPositionSide::Short => &book.bids,
    };
    let iter: Box<dyn Iterator<Item = &PolymarketBookLevel>> = match position_side {
        PolymarketPositionSide::Long => Box::new(levels.iter()),
        PolymarketPositionSide::Short => Box::new(levels.iter().rev()),
    };
    let mut result: Vec<_> = iter
        .filter_map(|level| {
            let quantity = level.quantity.floor() as i32;
            (quantity > 0).then_some((
                if position_side == PolymarketPositionSide::Long {
                    level.price
                } else {
                    1.0 - level.price
                },
                quantity,
            ))
        })
        .collect();
    result.sort_by(|left, right| left.0.total_cmp(&right.0));
    result
}

pub fn polymarket_sell_levels(
    book: &PolymarketMarketBook,
    position_side: PolymarketPositionSide,
) -> Vec<(f64, i32)> {
    let levels = match position_side {
        // Selling LONG consumes native LONG bids descending.
        PolymarketPositionSide::Long => &book.bids,
        // Selling SHORT consumes native LONG offers ascending, complemented.
        PolymarketPositionSide::Short => &book.offers,
    };
    let iter: Box<dyn Iterator<Item = &PolymarketBookLevel>> = match position_side {
        PolymarketPositionSide::Long => Box::new(levels.iter()),
        PolymarketPositionSide::Short => Box::new(levels.iter()),
    };
    let mut result: Vec<_> = iter
        .filter_map(|level| {
            let quantity = level.quantity.floor() as i32;
            (quantity > 0).then_some((
                if position_side == PolymarketPositionSide::Long {
                    level.price
                } else {
                    1.0 - level.price
                },
                quantity,
            ))
        })
        .collect();
    result.sort_by(|left, right| right.0.total_cmp(&left.0));
    result
}

pub fn executable_depth<T>(levels: &[(T, i32)]) -> i32 {
    levels.iter().fold(0_i32, |total, (_, quantity)| {
        total.saturating_add(*quantity)
    })
}

pub fn worst_price<T: Copy>(levels: &[(T, i32)], contracts: i32) -> Option<T> {
    let mut remaining = contracts;
    for (price, quantity) in levels {
        if *quantity <= 0 {
            continue;
        }
        remaining -= (*quantity).min(remaining);
        if remaining <= 0 {
            return Some(*price);
        }
    }
    None
}

pub fn kalshi_fee(contracts: i32, price: f64) -> f64 {
    (0.07 * contracts as f64 * price * (1.0 - price) * 100.0).ceil() / 100.0
}

/// Calculate the number of contracts to trade based on depth and settings
///
/// Returns None if depth is insufficient (below min_contracts)
pub fn calculate_contracts_to_trade(
    kalshi_depth: i32,
    poly_depth_contracts: i32,
    flexible_mode: bool,
    min_contracts: i32,
    max_contracts: i32,
) -> Option<i32> {
    let min_depth = kalshi_depth.min(poly_depth_contracts);

    // Check minimum depth requirement
    if min_depth < min_contracts {
        return None;
    }

    // Fixed mode: always use min_contracts
    if !flexible_mode {
        return Some(min_contracts);
    }

    // Flexible mode logic:
    // - Depth 10-20: use min_contracts (10)
    // - Depth >= 20: use half of the smaller depth
    let contracts = if min_depth < 20 {
        min_contracts
    } else {
        min_depth / 2
    };

    // Apply max_contracts limit
    Some(contracts.min(max_contracts))
}

/// `poly_min_trade_qty` is the Polymarket market's own minimum tradeable
/// quantity (docs.polymarket.us/api-reference/orders/overview: "constraints
/// are market-dependent - retrieve minimumTradeQty ... before order
/// submission"). Every order below it is rejected by the exchange
/// (ORD_REJECT_REASON_EXCHANGE_OPTION). `None` means the order service could
/// not look it up and must block sizing entirely, not be treated as zero.
pub fn find_profitable_contract_size(
    size_cap: i32,
    min_contracts: i32,
    max_amount: f64,
    poly_min_trade_qty: Option<f64>,
    kalshi_levels: &[(i32, i32)],
    poly_levels: &[(f64, i32)],
) -> Option<(i32, i32, f64, f64, f64)> {
    if !max_amount.is_finite() || max_amount <= 0.0 {
        return None;
    }
    let Some(poly_min_trade_qty) = poly_min_trade_qty else {
        return None;
    };
    let min_contracts = if poly_min_trade_qty.is_finite() && poly_min_trade_qty > 0.0 {
        min_contracts.max(poly_min_trade_qty.ceil() as i32)
    } else {
        min_contracts
    };
    for contracts in (min_contracts..=size_cap).rev() {
        let kalshi_price_cents = worst_price(kalshi_levels, contracts)?;
        let poly_price = worst_price(poly_levels, contracts)?;
        if contracts as f64 * poly_price < POLYMARKET_MIN_NOTIONAL_USD {
            continue;
        }
        let kalshi_price = kalshi_price_cents as f64 / 100.0;
        let fee = kalshi_fee(contracts, kalshi_price);
        let total_cost = contracts as f64 * (kalshi_price + poly_price) + fee;
        if total_cost.is_finite() && total_cost <= max_amount && total_cost < contracts as f64 {
            let margin = ((contracts as f64 - total_cost) / total_cost) * 100.0;
            return Some((contracts, kalshi_price_cents, poly_price, fee, margin));
        }
    }
    None
}

pub async fn neutralize_polymarket(
    service: &ArbitrageService,
    market_slug: &str,
    position_side: PolymarketPositionSide,
    contracts: i32,
    entry_price: Option<f64>,
    max_loss_cents: i32,
) -> Result<RecoveryResult, String> {
    let entry_price = entry_price.ok_or_else(|| {
        "Polymarket actual entry price was not returned; bounded close cannot be verified"
            .to_string()
    })?;
    let book = service
        .polymarket_client
        .get_market_book(market_slug)
        .await
        .map_err(|error| format!("Unable to fetch fresh Polymarket close book: {error}"))?;
    let close_price = worst_price(&polymarket_sell_levels(&book, position_side), contracts)
        .ok_or_else(|| "Fresh Polymarket close book lacks required executable depth".to_string())?;
    if close_price + f64::EPSILON < entry_price - max_loss_cents as f64 / 100.0 {
        return Err(format!(
            "Polymarket close price {:.4} exceeds configured {}¢ loss bound from actual entry {:.4}",
            close_price, max_loss_cents, entry_price
        ));
    }
    let result = service
        .polymarket_client
        .submit_limit_order(market_slug, position_side, "sell", contracts, close_price)
        .await
        .map_err(|error| format!("Polymarket bounded close submission failed: {error}"))?;
    Ok(RecoveryResult {
        filled_contracts: result.filled_contracts,
        order_id: result.order_id,
        error: result.error,
    })
}

pub async fn neutralize_kalshi(
    service: &ArbitrageService,
    ticker: &str,
    side: &str,
    contracts: i32,
    entry_price: Option<f64>,
    max_loss_cents: i32,
) -> Result<RecoveryResult, String> {
    let entry_price = entry_price.ok_or_else(|| {
        "Kalshi actual entry price was not returned; bounded close cannot be verified".to_string()
    })?;
    let book = service
        .kalshi_client
        .get_fresh_orderbook(ticker, EXECUTABLE_BOOK_MAX_AGE)
        .ok_or_else(|| "Fresh Kalshi websocket close depth is unavailable".to_string())?;
    let close_price_cents = worst_price(&kalshi_sell_levels(&book, side), contracts)
        .ok_or_else(|| "Fresh Kalshi close book lacks required executable depth".to_string())?;
    let close_price = close_price_cents as f64 / 100.0;
    if close_price + f64::EPSILON < entry_price - max_loss_cents as f64 / 100.0 {
        return Err(format!(
            "Kalshi close price {:.4} exceeds configured {}¢ loss bound from actual entry {:.4}",
            close_price, max_loss_cents, entry_price
        ));
    }
    let result = service
        .kalshi_client
        .submit_order(ticker, "sell", side, contracts, close_price_cents)
        .await
        .map_err(|error| format!("Kalshi bounded close submission failed: {error}"))?;
    Ok(RecoveryResult {
        filled_contracts: result.filled_contracts,
        order_id: result.order_id,
        error: result.error,
    })
}

/// Inputs for one paired (Kalshi + Polymarket) order attempt. All prices and
/// depths must already come from fresh, executable books — never cached
/// display quotes.
pub struct PairedOrderParams {
    pub event_name: String,
    pub team_name: String,
    pub kalshi_market_id: String,
    pub kalshi_side: String,
    pub polymarket_market_slug: String,
    pub polymarket_position_side: PolymarketPositionSide,
    pub contracts: i32,
    pub kalshi_price_cents: i32,
    pub poly_price: f64,
    pub kalshi_fee: f64,
    pub profit_margin: f64,
    pub duration_ms: i64,
    /// `PolymarketMarketBook::fetched_at_ms` of the book `poly_price` was
    /// computed from. Diagnostic only - lets us log how stale that book was
    /// by the time the order actually reached the exchange, to confirm or
    /// rule out a market-moved-between-snapshot-and-submission explanation
    /// for `ORD_REJECT_REASON_EXCHANGE_OPTION` (see CLAUDE.md session log).
    pub poly_book_fetched_at_ms: i64,
    pub neutralization_max_loss_cents: i32,
    /// When true, no order is submitted to either exchange and nothing is
    /// persisted as a real attempt; the returned outcome carries the
    /// computed sizing/pricing only.
    pub dry_run: bool,
}

pub struct PairedOrderOutcome {
    pub status: String,
    pub execution_record: AutoTradeExecutionRecord,
    pub unsafe_fill_quantity: bool,
    pub residual_contracts: i32,
}

fn dry_run_record(params: &PairedOrderParams) -> AutoTradeExecutionRecord {
    AutoTradeExecutionRecord {
        event_name: params.event_name.clone(),
        team_name: params.team_name.clone(),
        kalshi_market_id: params.kalshi_market_id.clone(),
        polymarket_market_id: params.polymarket_market_slug.clone(),
        kalshi_side: params.kalshi_side.clone(),
        polymarket_side: params.polymarket_position_side.as_str().to_string(),
        contracts: params.contracts,
        kalshi_price: params.kalshi_price_cents as f64 / 100.0,
        kalshi_fee: params.kalshi_fee,
        polymarket_price: params.poly_price,
        profit_margin: params.profit_margin,
        duration_ms: params.duration_ms,
        total_duration_ms: 0,
        kalshi_success: false,
        polymarket_success: false,
        kalshi_order_id: None,
        polymarket_order_id: None,
        kalshi_error: None,
        polymarket_error: None,
        kalshi_latency_ms: None,
        poly_latency_ms: None,
        kalshi_filled_contracts: 0,
        polymarket_filled_contracts: 0,
        kalshi_order_status: None,
        polymarket_order_status: None,
        neutralization_leg: None,
        neutralization_success: None,
        neutralization_order_id: None,
        neutralization_filled_contracts: 0,
        neutralization_error: None,
        residual_leg: None,
        residual_contracts: 0,
        status: "dry_run".to_string(),
    }
}

/// Attempt one paired order: persist a durable `submitting` record, place the
/// Polymarket leg, persist its acknowledgement, then (only if Polymarket
/// filled a positive quantity) place the Kalshi leg and persist its
/// acknowledgement, then neutralize any unequal fill within the configured
/// bound. Halts auto-trading and alerts via Telegram if either exchange
/// acknowledgement can't be durably recorded, or if a residual, un-neutralized
/// exposure remains.
pub async fn submit_paired_order(
    service: &ArbitrageService,
    telegram_client: &Arc<TelegramClient>,
    params: PairedOrderParams,
) -> PairedOrderOutcome {
    if params.dry_run {
        return PairedOrderOutcome {
            status: "dry_run".to_string(),
            execution_record: dry_run_record(&params),
            unsafe_fill_quantity: false,
            residual_contracts: 0,
        };
    }

    let started = Instant::now();
    let lifecycle_id = match service.ws_manager.get_storage().save_auto_trade_execution(
        &AutoTradeExecutionRecord {
            event_name: params.event_name.clone(),
            team_name: params.team_name.clone(),
            kalshi_market_id: params.kalshi_market_id.clone(),
            polymarket_market_id: params.polymarket_market_slug.clone(),
            kalshi_side: params.kalshi_side.clone(),
            polymarket_side: params.polymarket_position_side.as_str().to_string(),
            contracts: params.contracts,
            kalshi_price: params.kalshi_price_cents as f64 / 100.0,
            kalshi_fee: params.kalshi_fee,
            polymarket_price: params.poly_price,
            profit_margin: params.profit_margin,
            duration_ms: params.duration_ms,
            total_duration_ms: 0,
            kalshi_success: false,
            polymarket_success: false,
            kalshi_order_id: None,
            polymarket_order_id: None,
            kalshi_error: None,
            polymarket_error: None,
            kalshi_latency_ms: None,
            poly_latency_ms: None,
            kalshi_filled_contracts: 0,
            polymarket_filled_contracts: 0,
            kalshi_order_status: None,
            polymarket_order_status: None,
            neutralization_leg: None,
            neutralization_success: None,
            neutralization_order_id: None,
            neutralization_filled_contracts: 0,
            neutralization_error: None,
            residual_leg: None,
            residual_contracts: 0,
            status: "submitting".to_string(),
        },
    ) {
        Ok(id) => id,
        Err(reason) => {
            error!(
                "Refusing paired order submission because durable intent persistence failed: {:#}",
                reason
            );
            let mut record = dry_run_record(&params);
            record.status = "persistence_failed_before_submission".to_string();
            return PairedOrderOutcome {
                status: "persistence_failed_before_submission".to_string(),
                execution_record: record,
                unsafe_fill_quantity: false,
                residual_contracts: 0,
            };
        }
    };

    // Every input below came from the direct executable books resolved by
    // the caller. Both native orders are IOC/FAK; an acknowledgement with
    // zero fill is failure.
    let poly_started = Instant::now();
    let book_age_at_submit_ms = Utc::now().timestamp_millis() - params.poly_book_fetched_at_ms;
    info!(
        "Polymarket leg submitting: market_slug={} price={:.4} contracts={} book_age_ms={}",
        params.polymarket_market_slug, params.poly_price, params.contracts, book_age_at_submit_ms
    );
    let poly_result = match service
        .polymarket_client
        .submit_limit_order(
            &params.polymarket_market_slug,
            params.polymarket_position_side,
            "buy",
            params.contracts,
            params.poly_price,
        )
        .await
    {
        Ok(result) => result,
        Err(reason) => failed_poly_order(reason.to_string()),
    };
    let poly_latency = poly_result
        .latency_ms
        .or(Some(poly_started.elapsed().as_millis() as i64));

    // Diagnostic only, fires after the order already resolved: re-fetch the
    // book to see whether the market had already moved past our submitted
    // price by the time we placed it. `poly_price` was computed as the exact
    // worst-case boundary price for `contracts` (find_profitable_contract_size),
    // with no slippage buffer, so any movement between that snapshot and
    // submission is a plausible explanation for a zero-fill IOC rejection
    // independent of payload format (see CLAUDE.md session log).
    if poly_result.filled_contracts <= 0 {
        match service
            .polymarket_client
            .get_market_book(&params.polymarket_market_slug)
            .await
        {
            Ok(post_book) => {
                let post_levels = polymarket_buy_levels(&post_book, params.polymarket_position_side);
                let current_best_price = post_levels.first().map(|(price, _)| *price);
                let market_moved_past_our_price =
                    current_best_price.is_some_and(|price| price > params.poly_price);
                warn!(
                    "Polymarket leg rejected: post-rejection book check for {} — submitted_price={:.4} book_age_ms_at_submit={} current_best_buy_price={:?} market_moved_past_our_price={}",
                    params.polymarket_market_slug,
                    params.poly_price,
                    book_age_at_submit_ms,
                    current_best_price,
                    market_moved_past_our_price
                );
            }
            Err(error) => {
                warn!(
                    "Polymarket leg rejected: post-rejection book check failed for {}: {}",
                    params.polymarket_market_slug, error
                );
            }
        }
    }
    if let Err(reason) = service.ws_manager.get_storage().update_auto_trade_leg(
        lifecycle_id,
        "polymarket",
        poly_result.filled_contracts > 0,
        poly_result.filled_contracts,
        poly_result.order_id.as_deref(),
        poly_result.status.as_deref(),
        poly_result.error.as_deref(),
        poly_latency,
    ) {
        error!(
            "Halting auto-trading because Polymarket acknowledgement could not be persisted: {:#}",
            reason
        );
        if let Err(disable_reason) = service.ws_manager.disable_auto_trade() {
            error!(
                "Failed to halt auto-trading after persistence failure: {:#}",
                disable_reason
            );
        }
        let mut record = dry_run_record(&params);
        record.status = "persistence_failed_after_poly_ack".to_string();
        return PairedOrderOutcome {
            status: "persistence_failed_after_poly_ack".to_string(),
            execution_record: record,
            unsafe_fill_quantity: false,
            residual_contracts: 0,
        };
    }

    // Never create Kalshi exposure after a zero-fill Polymarket response.
    // The Polymarket acknowledgement is durable above, so this rejection is
    // also auditable without submitting a second leg.
    let (kalshi_result, kalshi_latency) = if !can_submit_kalshi_after_polymarket(
        poly_result.filled_contracts,
    ) {
        (
            failed_kalshi_order(
                "Kalshi leg was not submitted because Polymarket did not fill a positive contract quantity"
                    .to_string(),
            ),
            None,
        )
    } else {
        let kalshi_started = Instant::now();
        let result = match service
            .kalshi_client
            .submit_order(
                &params.kalshi_market_id,
                "buy",
                &params.kalshi_side,
                params.contracts,
                params.kalshi_price_cents,
            )
            .await
        {
            Ok(result) => result,
            Err(reason) => failed_kalshi_order(reason.to_string()),
        };
        (result, Some(kalshi_started.elapsed().as_millis() as i64))
    };
    if let Err(reason) = service.ws_manager.get_storage().update_auto_trade_leg(
        lifecycle_id,
        "kalshi",
        kalshi_result.filled_contracts > 0,
        kalshi_result.filled_contracts,
        kalshi_result.order_id.as_deref(),
        kalshi_result.status.as_deref(),
        kalshi_result.error.as_deref(),
        kalshi_latency,
    ) {
        error!(
            "Halting auto-trading because Kalshi acknowledgement could not be persisted: {:#}",
            reason
        );
        if let Err(disable_reason) = service.ws_manager.disable_auto_trade() {
            error!(
                "Failed to halt auto-trading after persistence failure: {:#}",
                disable_reason
            );
        }
        let mut record = dry_run_record(&params);
        record.status = "persistence_failed_after_kalshi_ack".to_string();
        return PairedOrderOutcome {
            status: "persistence_failed_after_kalshi_ack".to_string(),
            execution_record: record,
            unsafe_fill_quantity: false,
            residual_contracts: 0,
        };
    }

    let mut neutralization_leg = None;
    let mut neutralization_success = None;
    let mut neutralization_order_id = None;
    let mut neutralization_filled_contracts = 0;
    let mut neutralization_error = None;
    let mut residual_leg = None;
    let mut residual_contracts = 0;
    let unsafe_fill_quantity =
        !poly_result.fill_quantity_valid || !kalshi_result.fill_quantity_valid;

    if unsafe_fill_quantity {
        residual_leg = Some("unknown".to_string());
        neutralization_error = Some(
            "An exchange reported a non-whole fill quantity; exact bounded neutralization is unsafe"
                .to_string(),
        );
    } else if poly_result.filled_contracts != kalshi_result.filled_contracts {
        let (leg, excess) = if poly_result.filled_contracts > kalshi_result.filled_contracts {
            (
                "polymarket",
                poly_result.filled_contracts - kalshi_result.filled_contracts,
            )
        } else {
            (
                "kalshi",
                kalshi_result.filled_contracts - poly_result.filled_contracts,
            )
        };
        neutralization_leg = Some(leg.to_string());
        let recovery = if leg == "polymarket" {
            neutralize_polymarket(
                service,
                &params.polymarket_market_slug,
                params.polymarket_position_side,
                excess,
                poly_result.average_fill_price,
                params.neutralization_max_loss_cents,
            )
            .await
        } else {
            neutralize_kalshi(
                service,
                &params.kalshi_market_id,
                &params.kalshi_side,
                excess,
                kalshi_result.average_fill_price,
                params.neutralization_max_loss_cents,
            )
            .await
        };
        match recovery {
            Ok(recovery) if recovery.filled_contracts >= excess => {
                neutralization_success = Some(true);
                neutralization_order_id = recovery.order_id;
                neutralization_filled_contracts = recovery.filled_contracts;
            }
            Ok(recovery) => {
                neutralization_success = Some(false);
                neutralization_order_id = recovery.order_id;
                neutralization_filled_contracts = recovery.filled_contracts;
                neutralization_error = recovery.error;
                residual_leg = Some(leg.to_string());
                residual_contracts = excess - neutralization_filled_contracts;
            }
            Err(reason) => {
                neutralization_success = Some(false);
                neutralization_error = Some(reason);
                residual_leg = Some(leg.to_string());
                residual_contracts = excess;
            }
        }
    }

    let status = if unsafe_fill_quantity || residual_contracts > 0 {
        "exposed"
    } else if neutralization_leg.is_some() {
        "neutralized"
    } else if poly_result.filled_contracts == params.contracts
        && kalshi_result.filled_contracts == params.contracts
    {
        "executed"
    } else if poly_result.filled_contracts == 0 && kalshi_result.filled_contracts == 0 {
        "rejected"
    } else {
        // Equal partial fills can still be directionally paired, but are
        // recorded explicitly rather than presented as a full execution.
        "partial_paired"
    };
    let execution_record = AutoTradeExecutionRecord {
        event_name: params.event_name.clone(),
        team_name: params.team_name.clone(),
        kalshi_market_id: params.kalshi_market_id.clone(),
        polymarket_market_id: params.polymarket_market_slug.clone(),
        kalshi_side: params.kalshi_side.clone(),
        polymarket_side: params.polymarket_position_side.as_str().to_string(),
        contracts: params.contracts,
        kalshi_price: params.kalshi_price_cents as f64 / 100.0,
        kalshi_fee: params.kalshi_fee,
        polymarket_price: params.poly_price,
        profit_margin: params.profit_margin,
        duration_ms: params.duration_ms,
        total_duration_ms: started.elapsed().as_millis() as i64,
        kalshi_success: kalshi_result.filled_contracts > 0,
        polymarket_success: poly_result.filled_contracts > 0,
        kalshi_order_id: kalshi_result.order_id.clone(),
        polymarket_order_id: poly_result.order_id.clone(),
        kalshi_error: kalshi_result.error.clone(),
        polymarket_error: poly_result.error.clone(),
        kalshi_latency_ms: kalshi_latency,
        poly_latency_ms: poly_latency,
        kalshi_filled_contracts: kalshi_result.filled_contracts,
        polymarket_filled_contracts: poly_result.filled_contracts,
        kalshi_order_status: kalshi_result.status.clone(),
        polymarket_order_status: poly_result.status.clone(),
        neutralization_leg,
        neutralization_success,
        neutralization_order_id,
        neutralization_filled_contracts,
        neutralization_error,
        residual_leg,
        residual_contracts,
        status: status.to_string(),
    };
    if let Err(reason) = service
        .ws_manager
        .get_storage()
        .finish_auto_trade_execution(lifecycle_id, &execution_record)
    {
        error!("Failed to finalize paired execution: {:#}", reason);
    }

    // Every attempt that put real money on either exchange gets an explicit
    // success/outcome log line here, independent of the DB record, so fills
    // can be grepped and cross-checked against the exchanges directly.
    if kalshi_result.filled_contracts > 0 || poly_result.filled_contracts > 0 {
        info!(
            "Paired order {}: {} {} | kalshi order_id={:?} filled={}/{} @ {}c | polymarket order_id={:?} filled={}/{} @ {:.4} | neutralization={:?} residual={}",
            status,
            params.event_name,
            params.team_name,
            kalshi_result.order_id,
            kalshi_result.filled_contracts,
            params.contracts,
            params.kalshi_price_cents,
            poly_result.order_id,
            poly_result.filled_contracts,
            params.contracts,
            params.poly_price,
            neutralization_success,
            residual_contracts,
        );
    }

    if unsafe_fill_quantity || residual_contracts > 0 {
        // The recovery bound was unavailable or could not be met. Stop future
        // automated submissions and notify through the configured alert path,
        // regardless of whether this attempt was auto-triggered or manual.
        if let Err(reason) = service.ws_manager.disable_auto_trade() {
            error!(
                "Failed to halt auto-trading after residual exposure: {:#}",
                reason
            );
        }
        telegram_client
            .send_auto_trade_notification(
                &params.event_name,
                &params.team_name,
                params.profit_margin,
                execution_record.kalshi_success,
                execution_record.polymarket_success,
                execution_record.kalshi_error.as_deref(),
                execution_record.polymarket_error.as_deref(),
                params.contracts as f64
                    * (params.kalshi_price_cents as f64 / 100.0 + params.poly_price)
                    + params.kalshi_fee,
                0.0,
            )
            .await;
    }

    PairedOrderOutcome {
        status: status.to_string(),
        execution_record,
        unsafe_fill_quantity,
        residual_contracts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native_book() -> PolymarketMarketBook {
        PolymarketMarketBook {
            success: true,
            market_slug: "match-winner".to_string(),
            state: "open".to_string(),
            transact_time: Some("2026-09-11T19:37:00Z".to_string()),
            fetched_at_ms: 1,
            bids: vec![
                PolymarketBookLevel {
                    price: 0.62,
                    quantity: 2.0,
                },
                PolymarketBookLevel {
                    price: 0.60,
                    quantity: 3.0,
                },
            ],
            offers: vec![
                PolymarketBookLevel {
                    price: 0.64,
                    quantity: 2.0,
                },
                PolymarketBookLevel {
                    price: 0.66,
                    quantity: 3.0,
                },
            ],
            minimum_trade_qty: None,
            price_tick_size: None,
        }
    }

    #[test]
    fn derives_short_buy_depth_from_descending_native_bids() {
        let levels = polymarket_buy_levels(&native_book(), PolymarketPositionSide::Short);
        assert_eq!(levels, vec![(0.38, 2), (0.40, 3)]);
        assert_eq!(worst_price(&levels, 3), Some(0.40));
    }

    #[test]
    fn rejects_size_when_worst_case_fee_removes_profit() {
        let kalshi = vec![(50, 10)];
        let poly = vec![(0.50, 10)];
        assert!(find_profitable_contract_size(10, 1, 100.0, Some(1.0), &kalshi, &poly).is_none());
    }

    #[test]
    fn uses_worst_level_and_respects_total_amount_limit() {
        let kalshi = vec![(40, 2), (42, 3)];
        let poly = vec![(0.45, 2), (0.47, 3)];
        let result = find_profitable_contract_size(5, 1, 5.0, Some(1.0), &kalshi, &poly).unwrap();
        assert_eq!(result.0, 5);
        assert_eq!(result.1, 42);
        assert_eq!(result.2, 0.47);
    }

    #[test]
    fn rejects_size_below_polymarket_minimum_notional() {
        // 1 contract at $0.22 is $0.22 notional, below our conservative
        // $1 notional floor (POLYMARKET_MIN_NOTIONAL_USD).
        let kalshi = vec![(63, 10)];
        let poly = vec![(0.22, 10)];
        assert!(find_profitable_contract_size(1, 1, 100.0, Some(1.0), &kalshi, &poly).is_none());
    }

    #[test]
    fn accepts_size_at_or_above_polymarket_minimum_notional() {
        let kalshi = vec![(40, 10)];
        let poly = vec![(0.22, 10)];
        // 5 contracts * $0.22 = $1.10, clears the $1 floor.
        let result = find_profitable_contract_size(10, 5, 100.0, Some(1.0), &kalshi, &poly).unwrap();
        assert_eq!(result.0, 10);
    }

    #[test]
    fn blocks_sizing_when_polymarket_minimum_trade_qty_is_unknown() {
        // The exchange rejects any order below its own per-market minimum
        // (ORD_REJECT_REASON_EXCHANGE_OPTION). If the order service couldn't
        // look that minimum up, we must not guess it's zero.
        let kalshi = vec![(50, 10)];
        let poly = vec![(0.50, 10)];
        assert!(find_profitable_contract_size(10, 1, 100.0, None, &kalshi, &poly).is_none());
    }

    #[test]
    fn rejects_size_below_polymarket_minimum_trade_qty() {
        let kalshi = vec![(50, 5)];
        let poly = vec![(0.50, 5)];
        // Only 5 contracts of depth are available, but the market requires
        // at least 8 per order - no size can ever clear both constraints.
        assert!(find_profitable_contract_size(5, 1, 100.0, Some(8.0), &kalshi, &poly).is_none());
    }

    #[test]
    fn accepts_size_at_polymarket_minimum_trade_qty() {
        let kalshi = vec![(40, 10)];
        let poly = vec![(0.45, 10)];
        let result =
            find_profitable_contract_size(10, 1, 100.0, Some(8.0), &kalshi, &poly).unwrap();
        assert_eq!(result.0, 10);
    }

    #[test]
    fn does_not_submit_kalshi_without_a_positive_polymarket_fill() {
        assert!(!can_submit_kalshi_after_polymarket(0));
        assert!(!can_submit_kalshi_after_polymarket(-1));
        assert!(can_submit_kalshi_after_polymarket(1));
    }
}
