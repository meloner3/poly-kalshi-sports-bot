use std::sync::Arc;

use axum::{extract::{Query, State}, http::StatusCode, response::IntoResponse, Json};
use serde::Deserialize;

use crate::api::AppState;
use crate::config::MentionMode;

#[derive(Deserialize)]
pub struct MentionModeRequest {
    pub mode: MentionMode,
}

#[derive(Deserialize)]
pub struct MentionLimitQuery {
    pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct MentionKillRequest {
    pub enabled: bool,
}

pub async fn get_mention_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.directional_execution.status() {
        Ok(status) => Json(status).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR,Json(serde_json::json!({"error":error.to_string()}))).into_response(),
    }
}

pub async fn get_mention_signals(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.directional_execution.signal_dashboard())
}

pub async fn get_mention_history(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MentionLimitQuery>,
) -> impl IntoResponse {
    match state.directional_execution.history(query.limit.unwrap_or(100)) {
        Ok(rows) => Json(serde_json::json!({"records":rows,"total":rows.len()})).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR,Json(serde_json::json!({"error":error.to_string()}))).into_response(),
    }
}

pub async fn get_mention_decisions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MentionLimitQuery>,
) -> impl IntoResponse {
    match state.directional_execution.decisions(query.limit.unwrap_or(100)) {
        Ok(rows) => Json(serde_json::json!({"records":rows,"total":rows.len()})).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR,Json(serde_json::json!({"error":error.to_string()}))).into_response(),
    }
}

pub async fn set_mention_mode(
    State(state): State<Arc<AppState>>,
    Json(request): Json<MentionModeRequest>,
) -> impl IntoResponse {
    match state.directional_execution.set_mode(request.mode) {
        Ok(()) => Json(serde_json::json!({"success":true,"mode":request.mode})).into_response(),
        Err(error) => (StatusCode::CONFLICT,Json(serde_json::json!({"success":false,"error":format!("{error:#}"),"mode":"paper"}))).into_response(),
    }
}

pub async fn mention_kill_switch(
    State(state): State<Arc<AppState>>,
    Json(request): Json<MentionKillRequest>,
) -> impl IntoResponse {
    match state.directional_execution.kill_switch(request.enabled) {
        Ok(()) => Json(serde_json::json!({"success":true,"kill_switch":request.enabled})).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR,Json(serde_json::json!({"success":false,"error":error.to_string()}))).into_response(),
    }
}

pub async fn reset_mention_loss_breaker(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.directional_execution.reset_loss_breaker() {
        Ok(()) => Json(serde_json::json!({"success":true,"loss_breaker":false})).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR,Json(serde_json::json!({"success":false,"error":error.to_string()}))).into_response(),
    }
}