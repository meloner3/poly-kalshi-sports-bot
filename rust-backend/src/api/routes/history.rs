//! History query and search endpoints

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use tracing::error;

use crate::api::AppState;

/// Query params for history
#[derive(Deserialize)]
pub struct HistoryQuery {
    limit: Option<usize>,
}

/// Get arbitrage history
pub async fn get_arbitrage_history(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HistoryQuery>,
) -> impl IntoResponse {
    let service = state.service.read().await;
    let limit = query.limit.unwrap_or(100);

    match service.get_arbitrage_history(limit) {
        Ok(history) => Json(serde_json::json!({
            "success": true,
            "data": history
        }))
        .into_response(),
        Err(e) => {
            error!("Failed to get history records: {:#}", e);
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

/// Search history query params
#[derive(Deserialize)]
pub struct SearchHistoryQuery {
    min_profit: Option<f64>,
    max_profit: Option<f64>,
    min_duration: Option<f64>,
    max_duration: Option<f64>,
    event_name: Option<String>,
    team_name: Option<String>,
    sort_by: Option<String>,
    sort_order: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    include_history: Option<bool>,
}

/// Search history records
pub async fn search_history(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchHistoryQuery>,
) -> impl IntoResponse {
    let service = state.service.read().await;

    match service.search_history(
        query.min_profit,
        query.max_profit,
        query.min_duration,
        query.max_duration,
        query.event_name,
        query.team_name,
        query.sort_by,
        query.sort_order,
        query.limit,
        query.offset,
        query.include_history,
    ) {
        Ok(result) => Json(result).into_response(),
        Err(e) => {
            error!("Failed to search history records: {:#}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "records": [],
                    "total": 0,
                    "error": e.to_string()
                })),
            )
                .into_response()
        }
    }
}

/// Get history statistics
pub async fn get_history_statistics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;

    match service.get_history_statistics() {
        Ok(stats) => Json(stats).into_response(),
        Err(e) => {
            error!("Failed to get history statistics: {:#}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "total_records": 0,
                    "error": e.to_string()
                })),
            )
                .into_response()
        }
    }
}
