//! Market data query endpoints

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::api::AppState;

/// Get current arbitrage opportunities
pub async fn get_opportunities(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;
    let opportunities = service.get_opportunities();
    Json(opportunities)
}

/// Get matched markets
pub async fn get_matched_markets(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;
    let markets = service.ws_manager.get_matched_markets_for_frontend();
    Json(markets)
}

/// Query params for orderbook depth
#[derive(Deserialize)]
pub struct OrderbookDepthQuery {
    pub kalshi_ticker: Option<String>,
}

/// Depth for a single side (Yes or No)
#[derive(Serialize, Default)]
pub struct SideDepth {
    pub price: Option<f64>,
    pub size: Option<f64>,
}

/// Orderbook depth for a single platform with Yes/No
#[derive(Serialize, Default)]
pub struct PlatformDepthDual {
    pub yes: SideDepth,
    pub no: SideDepth,
}

/// Full orderbook depth response
#[derive(Serialize)]
pub struct OrderbookDepthResponse {
    pub kalshi: Option<PlatformDepthDual>,
    pub polymarket: Option<PlatformDepthDual>,
}

/// Best executable buy level (price, size) for a native position side:
/// LONG buys the best offer; SHORT buys against the best bid, priced as its
/// complement.
fn best_buy_level(
    book: &crate::clients::polymarket::PolymarketMarketBook,
    side: crate::models::PolymarketPositionSide,
) -> SideDepth {
    use crate::models::PolymarketPositionSide::{Long, Short};
    let level = match side {
        Long => book.offers.first().map(|l| (l.price, l.quantity)),
        Short => book.bids.first().map(|l| (1.0 - l.price, l.quantity)),
    };
    match level {
        Some((price, size)) => SideDepth { price: Some(price), size: Some(size) },
        None => SideDepth::default(),
    }
}

/// Get orderbook depth for specified markets
pub async fn get_orderbook_depth(
    State(state): State<Arc<AppState>>,
    Query(query): Query<OrderbookDepthQuery>,
) -> impl IntoResponse {
    let service = state.service.read().await;

    // Get Kalshi orderbook depth (Yes and No)
    let kalshi_depth = query.kalshi_ticker.as_ref().and_then(|ticker| {
        service.kalshi_client.get_orderbook(ticker).map(|book| {
            let yes_best_bid = book.yes.last();
            let no_best_bid = book.no.last();

            let yes = if let Some((price_cents, qty)) = no_best_bid {
                SideDepth {
                    price: Some(1.0 - (*price_cents as f64 / 100.0)),
                    size: Some(*qty as f64),
                }
            } else {
                SideDepth::default()
            };

            let no = if let Some((price_cents, qty)) = yes_best_bid {
                SideDepth {
                    price: Some(1.0 - (*price_cents as f64 / 100.0)),
                    size: Some(*qty as f64),
                }
            } else {
                SideDepth::default()
            };

            PlatformDepthDual { yes, no }
        })
    });

    // Polymarket depth for the same matched market, from the live
    // WebSocket book (REST book as a fallback until one arrives). Display
    // only - order sizing fetches its own fresh book.
    let polymarket_depth = match query
        .kalshi_ticker
        .as_deref()
        .and_then(|t| service.ws_manager.matched_market_for_kalshi_ticker(t))
    {
        Some(matched) => {
            let pm = &matched.polymarket_market;
            let team_exec = pm.us_execution_for_competitor(&matched.team_name);
            let opp_exec = pm
                .get_opponent(&matched.team_name)
                .and_then(|opponent| pm.us_execution_for_competitor(opponent));
            match (team_exec, opp_exec) {
                (Some(team_exec), Some(opp_exec)) => {
                    let book = match service.ws_manager.polymarket_live_book(&pm.market_slug) {
                        Some(book) => Some(book),
                        None => service
                            .polymarket_client
                            .get_market_book(&pm.market_slug)
                            .await
                            .map_err(|e| tracing::debug!("Polymarket depth fallback failed: {:#}", e))
                            .ok(),
                    };
                    book.map(|book| PlatformDepthDual {
                        yes: best_buy_level(&book, team_exec.position_side),
                        no: best_buy_level(&book, opp_exec.position_side),
                    })
                }
                _ => None,
            }
        }
        None => None,
    };

    Json(OrderbookDepthResponse {
        kalshi: kalshi_depth,
        polymarket: polymarket_depth,
    })
}
