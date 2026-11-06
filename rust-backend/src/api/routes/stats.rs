//! Statistics and monitoring endpoints

use std::sync::Arc;

use axum::{extract::State, response::IntoResponse, Json};

use crate::api::AppState;

/// Get system statistics
pub async fn get_stats(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;
    let stats = service.get_stats();
    Json(stats)
}

/// Get data coverage
pub async fn get_data_coverage(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;
    let coverage = service.ws_manager.get_data_coverage();
    Json(coverage)
}

/// Get tracking information
pub async fn get_tracking(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;
    let tracking = service.ws_manager.get_tracking_stats();
    Json(tracking)
}
