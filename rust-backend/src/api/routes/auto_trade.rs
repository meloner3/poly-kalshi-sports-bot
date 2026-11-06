//! Auto-trade API endpoints

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::error;

use crate::api::AppState;

/// Auto-trade status response
#[derive(Serialize)]
pub struct AutoTradeStatusResponse {
    pub enabled: bool,
    pub trade_count: i32,
    pub max_trade_count: i32,
    pub remaining: i32,
    pub max_amount: f64,
    pub min_duration_ms: i64,
    /// 是否启用灵活下单模式
    pub flexible_mode: bool,
    /// 单次最大合同数
    pub max_contracts: i32,
    /// 最低合同数（深度低于此值不下单）
    pub min_contracts: i32,
    /// Maximum permitted adverse close price after a partial/unpaired fill.
    pub neutralization_max_loss_cents: i32,
    pub last_trade_time: Option<String>,
}

/// Get auto-trade status
pub async fn get_auto_trade_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;
    let auto_state = service.ws_manager.get_auto_trade_state();

    Json(AutoTradeStatusResponse {
        enabled: auto_state.enabled,
        trade_count: auto_state.trade_count,
        max_trade_count: auto_state.max_trade_count,
        remaining: auto_state.max_trade_count - auto_state.trade_count,
        max_amount: auto_state.max_amount,
        min_duration_ms: auto_state.min_duration_ms,
        flexible_mode: auto_state.flexible_mode,
        max_contracts: auto_state.max_contracts,
        min_contracts: auto_state.min_contracts,
        neutralization_max_loss_cents: auto_state.neutralization_max_loss_cents,
        last_trade_time: auto_state.last_trade_time,
    })
}

/// Enable auto-trade
pub async fn enable_auto_trade(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;

    match service.ws_manager.enable_auto_trade() {
        Ok(_) => Json(serde_json::json!({
            "success": true,
            "message": "自动下单已开启"
        }))
        .into_response(),
        Err(e) => {
            error!("开启自动下单失败: {:#}", e);
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

/// Disable auto-trade
pub async fn disable_auto_trade(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;

    match service.ws_manager.disable_auto_trade() {
        Ok(_) => Json(serde_json::json!({
            "success": true,
            "message": "自动下单已关闭"
        }))
        .into_response(),
        Err(e) => {
            error!("关闭自动下单失败: {:#}", e);
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

/// Reset auto-trade count
pub async fn reset_auto_trade(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;

    match service.ws_manager.reset_trade_count() {
        Ok(_) => Json(serde_json::json!({
            "success": true,
            "message": "下单次数已重置"
        }))
        .into_response(),
        Err(e) => {
            error!("重置下单次数失败: {:#}", e);
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

/// Auto-trade settings update request
#[derive(Deserialize)]
pub struct AutoTradeSettingsRequest {
    pub max_amount: Option<f64>,
    pub min_duration_ms: Option<i64>,
    pub max_trade_count: Option<i32>,
    /// 是否启用灵活下单模式
    pub flexible_mode: Option<bool>,
    /// 单次最大合同数
    pub max_contracts: Option<i32>,
    /// 最低合同数（深度低于此值不下单）
    pub min_contracts: Option<i32>,
    pub neutralization_max_loss_cents: Option<i32>,
}

/// Update auto-trade settings
pub async fn update_auto_trade_settings(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AutoTradeSettingsRequest>,
) -> impl IntoResponse {
    if req
        .neutralization_max_loss_cents
        .is_some_and(|cents| !(0..=99).contains(&cents))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "success": false,
                "error": "neutralization_max_loss_cents must be between 0 and 99"
            })),
        )
            .into_response();
    }
    let service = state.service.read().await;

    match service.ws_manager.update_auto_trade_settings(
        req.max_amount,
        req.min_duration_ms,
        req.max_trade_count,
        req.flexible_mode,
        req.max_contracts,
        req.min_contracts,
        req.neutralization_max_loss_cents,
    ) {
        Ok(_) => Json(serde_json::json!({
            "success": true,
            "message": "设置已更新"
        }))
        .into_response(),
        Err(e) => {
            error!("更新自动下单设置失败: {:#}", e);
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

/// Query params for auto-trade history
#[derive(Deserialize)]
pub struct AutoTradeHistoryQuery {
    pub limit: Option<usize>,
}

/// Get auto-trade execution history
pub async fn get_auto_trade_history(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AutoTradeHistoryQuery>,
) -> impl IntoResponse {
    let service = state.service.read().await;
    let storage = service.ws_manager.get_storage();
    let limit = query.limit.unwrap_or(50);

    match storage.get_auto_trade_history(limit) {
        Ok(records) => Json(serde_json::json!({
            "records": records,
            "total": records.len()
        }))
        .into_response(),
        Err(e) => {
            error!("获取自动下单历史失败: {:#}", e);
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

// ==================== Market Exclusion APIs ====================

/// Market exclusion request
#[derive(Deserialize)]
pub struct MarketExclusionRequest {
    pub event_name: String,
    pub team_name: String,
    /// Game date in YYYY-MM-DD format (optional for backwards compatibility)
    pub game_date: Option<String>,
}

/// Parse game_date string to NaiveDate
fn parse_game_date(game_date: Option<&str>) -> Option<chrono::NaiveDate> {
    game_date.and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
}

/// Get list of excluded markets
pub async fn get_excluded_markets(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;
    let excluded = service.ws_manager.get_excluded_markets();

    Json(serde_json::json!({
        "excluded_markets": excluded,
        "count": excluded.len()
    }))
}

/// Exclude a market from auto-trade
pub async fn exclude_market(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MarketExclusionRequest>,
) -> impl IntoResponse {
    let service = state.service.read().await;
    let game_date = parse_game_date(req.game_date.as_deref());
    let inserted = service
        .ws_manager
        .exclude_market(&req.event_name, &req.team_name, game_date);

    Json(serde_json::json!({
        "success": true,
        "message": if inserted { "市场已排除" } else { "市场已在排除列表中" },
        "event_name": req.event_name,
        "team_name": req.team_name,
        "game_date": req.game_date
    }))
}

/// Remove a market from exclusion list
pub async fn unexclude_market(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MarketExclusionRequest>,
) -> impl IntoResponse {
    let service = state.service.read().await;
    let game_date = parse_game_date(req.game_date.as_deref());
    let removed = service
        .ws_manager
        .unexclude_market(&req.event_name, &req.team_name, game_date);

    Json(serde_json::json!({
        "success": true,
        "message": if removed { "市场已取消排除" } else { "市场不在排除列表中" },
        "event_name": req.event_name,
        "team_name": req.team_name,
        "game_date": req.game_date
    }))
}
