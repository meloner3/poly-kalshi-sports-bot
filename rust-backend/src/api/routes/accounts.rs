//! Account balance and position endpoints

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::api::AppState;

/// Get Kalshi balance
pub async fn get_kalshi_balance(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;

    match service.kalshi_client.get_balance().await {
        Ok(balance) => Json(serde_json::json!({
            "success": true,
            "balance": balance
        }))
        .into_response(),
        Err(e) => {
            error!("Failed to get Kalshi balance: {:#}", e);
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

/// Get Polymarket balance
pub async fn get_polymarket_balance(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;

    match service.polymarket_client.get_balance().await {
        Ok(balance) => Json(serde_json::json!({
            "success": true,
            "balance": balance
        }))
        .into_response(),
        Err(e) => {
            error!("Failed to get Polymarket balance: {:#}", e);
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

/// Get unified account balance
pub async fn get_account_balance(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;

    let kalshi_data = match service.kalshi_client.get_balance().await {
        Ok(balance) => {
            serde_json::json!({
                "available": true,
                "balance": balance,
                "portfolio_value": 0.0,
                "updated_ts": 0
            })
        }
        Err(e) => serde_json::json!({
            "available": false,
            "error": e.to_string()
        }),
    };

    let poly_data = match service.polymarket_client.get_balance().await {
        Ok(balance) => {
            serde_json::json!({
                "available": true,
                "balance": balance,
                "pnl": "0",
                "trades": 0,
                "positions": 0
            })
        }
        Err(e) => serde_json::json!({
            "available": false,
            "error": e.to_string()
        }),
    };

    Json(serde_json::json!({
        "kalshi": kalshi_data,
        "polymarket": poly_data
    }))
}

/// Get Kalshi positions
pub async fn get_kalshi_positions(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;

    match service.kalshi_client.get_positions().await {
        Ok(positions) => Json(serde_json::json!({
            "positions": positions.get("market_positions").unwrap_or(&serde_json::json!([]))
        }))
        .into_response(),
        Err(e) => {
            error!("Failed to get Kalshi positions: {:#}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "positions": [],
                    "error": e.to_string()
                })),
            )
                .into_response()
        }
    }
}

/// Get Polymarket positions
pub async fn get_polymarket_positions(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = state.service.read().await;

    match service.polymarket_client.get_positions().await {
        Ok(positions) => Json(serde_json::json!({
            "positions": positions
        }))
        .into_response(),
        Err(e) => {
            error!("Failed to get Polymarket positions: {:#}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "positions": [],
                    "error": e.to_string()
                })),
            )
                .into_response()
        }
    }
}

// ==================== Authentication ====================

/// Login request
#[derive(Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

/// Login response
#[derive(Serialize)]
pub struct LoginResponse {
    access_token: String,
    token_type: String,
    username: String,
}

/// JWT Claims
#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: i64,
    iat: i64,
}

/// Login endpoint
pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {
    let auth_config = &state.config.auth;

    if req.username != auth_config.username || req.password != auth_config.password {
        error!(
            "Login failed: invalid username or password (username: {})",
            req.username
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "detail": "Invalid username or password"
            })),
        )
            .into_response();
    }

    let now = Utc::now();
    let exp = now + Duration::hours(auth_config.token_expire_hours as i64);

    let claims = Claims {
        sub: req.username.clone(),
        exp: exp.timestamp(),
        iat: now.timestamp(),
    };

    let token = match encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(auth_config.secret_key.as_bytes()),
    ) {
        Ok(t) => t,
        Err(e) => {
            error!("Failed to generate JWT: {:#}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "detail": "Failed to generate token"
                })),
            )
                .into_response();
        }
    };

    info!("User {} logged in successfully", req.username);

    Json(LoginResponse {
        access_token: token,
        token_type: "Bearer".to_string(),
        username: req.username,
    })
    .into_response()
}

/// Unified positions: Kalshi and Polymarket legs grouped into hedged pairs
/// (matched against this bot's trade history) and standalone leftovers.
/// Each exchange fails independently; its error is reported alongside
/// whatever the other side returned.
pub async fn get_unified_positions(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use crate::services::positions::{kalshi_leg, pair_positions, polymarket_leg};
    use serde_json::Value;

    let service = state.service.read().await;
    let mut errors: Vec<String> = Vec::new();

    let mut kalshi_raw: Vec<Value> = Vec::new();
    match service.kalshi_client.get_positions().await {
        Ok(v) => {
            kalshi_raw = v
                .get("market_positions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
        }
        Err(e) => {
            error!("Failed to get Kalshi positions: {:#}", e);
            errors.push(format!("Kalshi: {}", e));
        }
    }
    kalshi_raw.retain(|p| kalshi_leg(p, None).is_some());

    // Live marks: liquidation value = complement of the opposite side's ask.
    let tickers: Vec<String> = kalshi_raw
        .iter()
        .filter_map(|p| p.get("ticker").and_then(Value::as_str).map(str::to_string))
        .collect();
    let quotes = if tickers.is_empty() {
        Vec::new()
    } else {
        service
            .kalshi_client
            .get_market_quotes(&tickers)
            .await
            .unwrap_or_else(|e| {
                error!("Failed to get Kalshi quotes for positions: {:#}", e);
                errors.push("Kalshi: live prices unavailable".to_string());
                Vec::new()
            })
    };
    let kalshi_legs: Vec<_> = kalshi_raw
        .iter()
        .filter_map(|p| {
            let ticker = p.get("ticker").and_then(Value::as_str)?;
            let is_yes = kalshi_leg(p, None)?.side == "yes";
            let mark = quotes.iter().find(|q| q.market_id == ticker).map(|q| {
                1.0 - if is_yes { q.no_ask } else { q.yes_ask }
            });
            kalshi_leg(p, mark)
        })
        .collect();

    let poly_legs: Vec<_> = match service.polymarket_client.get_positions().await {
        Ok(Value::Array(items)) => items.iter().filter_map(polymarket_leg).collect(),
        Ok(_) => Vec::new(),
        Err(e) => {
            error!("Failed to get Polymarket positions: {:#}", e);
            errors.push(format!("Polymarket: {}", e));
            Vec::new()
        }
    };

    let history = match service.ws_manager.get_storage().get_auto_trade_history(500) {
        Ok(h) => h,
        Err(e) => {
            error!("Failed to load trade history for position pairing: {:#}", e);
            errors.push("Trade history unavailable: nothing can be shown as paired".to_string());
            Vec::new()
        }
    };

    let cards = pair_positions(kalshi_legs, poly_legs, &history);
    Json(serde_json::json!({ "cards": cards, "errors": errors }))
}
