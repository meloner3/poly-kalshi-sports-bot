//! Application settings endpoints

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use tracing::error;

use crate::api::AppState;

/// App settings response
#[derive(Serialize)]
pub struct AppSettingsResponse {
    /// 数据刷新间隔（秒）
    pub refresh_interval: u64,
    /// 显示套利机会的最小利润率（%）
    pub min_profit_margin: f64,
    /// 套利计算使用的默认金额（美元）
    pub default_bet_amount: f64,
    /// 开始追踪记录的利润率阈值（%）
    pub tracking_threshold: f64,
    pub updated_at: Option<String>,
}

/// Get application settings
pub async fn get_app_settings(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;
    let storage = service.ws_manager.get_storage();

    match storage.get_app_settings() {
        Ok(settings) => Json(AppSettingsResponse {
            refresh_interval: settings.refresh_interval,
            min_profit_margin: settings.min_profit_margin,
            default_bet_amount: settings.default_bet_amount,
            tracking_threshold: settings.tracking_threshold,
            updated_at: settings.updated_at,
        })
        .into_response(),
        Err(e) => {
            error!("获取应用设置失败: {:#}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": e.to_string()
                })),
            )
                .into_response()
        }
    }
}

/// App settings update request
#[derive(Deserialize)]
pub struct AppSettingsRequest {
    /// 数据刷新间隔（秒）
    pub refresh_interval: Option<u64>,
    /// 显示套利机会的最小利润率（%）
    pub min_profit_margin: Option<f64>,
    /// 套利计算使用的默认金额（美元）
    pub default_bet_amount: Option<f64>,
    /// 开始追踪记录的利润率阈值（%）
    pub tracking_threshold: Option<f64>,
}

/// Update application settings
pub async fn update_app_settings(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AppSettingsRequest>,
) -> impl IntoResponse {
    let service = state.service.read().await;

    match service.ws_manager.update_app_settings(
        req.refresh_interval,
        req.min_profit_margin,
        req.default_bet_amount,
        req.tracking_threshold,
    ) {
        Ok(_) => Json(serde_json::json!({
            "success": true,
            "message": "应用设置已更新"
        }))
        .into_response(),
        Err(e) => {
            error!("更新应用设置失败: {:#}", e);
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
