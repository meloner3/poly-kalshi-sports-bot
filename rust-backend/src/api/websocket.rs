//! WebSocket server for real-time updates to frontend

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tracing::{error, info};

use super::AppState;
use crate::models::WsMessage;

/// WebSocket handler
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Handle WebSocket connection
async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();

    info!("New WebSocket client connected");

    // Create channel for outgoing messages
    let (tx, mut rx) = mpsc::channel::<String>(100);

    // Send initial data
    {
        let service = state.service.read().await;

        // Send matched markets list (key message for frontend)
        let markets = service.ws_manager.get_matched_markets_for_frontend();
        let opportunities_count = markets.iter().filter(|m| m.has_opportunity).count();
        let msg = WsMessage::MatchedMarketsList {
            data: markets.clone(),
            count: markets.len(),
            opportunities_count,
        };
        if let Ok(json) = serde_json::to_string(&msg) {
            let _ = tx.send(json).await;
        }

        // Send stats
        let stats = service.get_stats();
        let msg = WsMessage::Stats { data: stats };
        if let Ok(json) = serde_json::to_string(&msg) {
            let _ = tx.send(json).await;
        }

        // Send current opportunities
        let opportunities = service.get_opportunities();
        if !opportunities.is_empty() {
            let msg = WsMessage::Opportunities {
                data: opportunities,
            };
            if let Ok(json) = serde_json::to_string(&msg) {
                let _ = tx.send(json).await;
            }
        }

        let status = state.directional_execution.status().unwrap_or_else(|error| {
            serde_json::json!({"mode":"off","error":error.to_string()})
        });
        let message = WsMessage::MentionSignals {
            data: state.directional_execution.signal_dashboard(),
            status,
        };
        if let Ok(json) = serde_json::to_string(&message) {
            let _ = tx.send(json).await;
        }
    }

    // Subscribe to opportunity updates and scan stats
    let service = state.service.read().await;
    let mut opportunity_rx = service.ws_manager.subscribe();
    let mut scan_stats_rx = service.ws_manager.subscribe_scan_stats();
    drop(service);

    // Clone state and tx for periodic updates task
    let state_clone = state.clone();
    let tx_periodic = tx.clone();

    // Spawn task for periodic matched_markets_list updates (every 2 seconds)
    let periodic_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            interval.tick().await;

            let service = state_clone.service.read().await;
            let markets = service.ws_manager.get_matched_markets_for_frontend();
            let opportunities_count = markets.iter().filter(|m| m.has_opportunity).count();

            let msg = WsMessage::MatchedMarketsList {
                data: markets.clone(),
                count: markets.len(),
                opportunities_count,
            };

            if let Ok(json) = serde_json::to_string(&msg) {
                if tx_periodic.send(json).await.is_err() {
                    break;
                }
            }
        }
    });

    // Clone tx for opportunity updates task
    let tx_opp = tx.clone();

    // Spawn task to forward opportunity updates
    let opportunity_task = tokio::spawn(async move {
        while let Ok(opportunity) = opportunity_rx.recv().await {
            let msg = WsMessage::Opportunity { data: opportunity };
            if let Ok(json) = serde_json::to_string(&msg) {
                if tx_opp.send(json).await.is_err() {
                    break;
                }
            }
        }
    });

    // Clone tx for scan stats updates task
    let tx_scan = tx.clone();

    // Spawn task to forward scan stats updates
    let scan_stats_task = tokio::spawn(async move {
        while let Ok(scan_stats) = scan_stats_rx.recv().await {
            let msg = WsMessage::ScanStats { data: scan_stats };
            if let Ok(json) = serde_json::to_string(&msg) {
                if tx_scan.send(json).await.is_err() {
                    break;
                }
            }
        }
    });

    // Clone state and tx for metrics updates task
    let state_metrics = state.clone();
    let tx_metrics = tx.clone();

    // Spawn task for periodic metrics updates (every 10 seconds)
    let metrics_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;

            let service = state_metrics.service.read().await;
            let report = service.metrics.report();

            let msg = WsMessage::Metrics { data: report };

            if let Ok(json) = serde_json::to_string(&msg) {
                if tx_metrics.send(json).await.is_err() {
                    break;
                }
            }
        }
    });

    let state_mentions = state.clone();
    let tx_mentions = tx.clone();
    let mention_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            interval.tick().await;
            let executor = &state_mentions.directional_execution;
            let status = executor.status().unwrap_or_else(|error| {
                serde_json::json!({"mode":"off","error":error.to_string()})
            });
            let message = WsMessage::MentionSignals {
                data: executor.signal_dashboard(),
                status,
            };
            if let Ok(json) = serde_json::to_string(&message) {
                if tx_mentions.send(json).await.is_err() { break; }
            }
        }
    });

    // Spawn task to send messages to WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(json) = rx.recv().await {
            if sender.send(Message::Text(json)).await.is_err() {
                break;
            }
        }
    });

    // Handle incoming messages (ping/pong, close)
    let recv_task = tokio::spawn(async move {
        while let Some(result) = receiver.next().await {
            match result {
                Ok(Message::Ping(_data)) => {
                    // Pong is handled automatically by axum
                }
                Ok(Message::Close(_)) => {
                    break;
                }
                Err(e) => {
                    error!("WebSocket error: {:#}", e);
                    break;
                }
                _ => {}
            }
        }
    });

    // Wait for recv_task to complete (client disconnected)
    let _ = recv_task.await;

    // Cleanup: abort all other tasks
    periodic_task.abort();
    opportunity_task.abort();
    scan_stats_task.abort();
    metrics_task.abort();
    mention_task.abort();
    send_task.abort();

    info!("WebSocket client disconnected");
}
