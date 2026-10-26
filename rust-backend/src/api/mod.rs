//! API Layer
//!
//! HTTP routes and WebSocket server for the frontend.

pub mod routes;
pub mod static_files;
pub mod websocket;

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use axum::{
    routing::{delete, get, post, put},
    Router,
};
use chrono::Utc;
use tokio::sync::{mpsc, RwLock};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{error, info, warn, Level};

use crate::config::{Config, MentionConfig, MentionMode};
use crate::models::PriceUpdate;
use crate::services::paired_execution::{
    calculate_contracts_to_trade, executable_depth, find_profitable_contract_size,
    kalshi_buy_levels, polymarket_buy_levels, submit_paired_order, EXECUTABLE_BOOK_MAX_AGE,
};
use crate::services::{
    ArbitrageService, DirectionalExecutionService, MentionSignalClient, PairedOrderParams,
    PerformanceMetrics, TelegramClient,
};

/// Market scan interval in seconds (5 minutes)
const MARKET_SCAN_INTERVAL_SECS: u64 = 300;

/// Application state shared across handlers
pub struct AppState {
    pub service: RwLock<ArbitrageService>,
    pub config: Config,
    pub telegram_client: Arc<TelegramClient>,
    pub directional_execution: Arc<DirectionalExecutionService>,
}

/// Create the Axum application
pub async fn create_app(config: Config) -> Result<Router> {
    // Initialize the arbitrage service
    let mut service = ArbitrageService::new(&config).await?;
    service.initialize().await?;
    if service
        .ws_manager
        .get_storage()
        .has_submitting_auto_trade_execution()?
    {
        service.ws_manager.disable_auto_trade()?;
        error!(
            "Auto-trading was halted at startup because an interrupted paired execution requires reconciliation"
        );
    }

    // Get metrics reference before moving service
    let metrics = service.metrics.clone();

    // Create price update channel
    let (price_tx, mut price_rx) = mpsc::channel::<PriceUpdate>(10000);

    // Start WebSocket connections
    service.start_websocket_connections(price_tx).await?;

    // Start periodic scanning
    service
        .run_periodic_scan(config.settings.refresh_interval)
        .await;
    service
        .run_polymarket_quote_refresh(config.settings.refresh_interval)
        .await;
    service
        .run_kalshi_quote_refresh(config.settings.refresh_interval)
        .await;
    match crate::clients::polymarket::resolve_credentials(&config.polymarket) {
        Ok((key_id, secret_key)) => service.run_polymarket_ws(key_id, secret_key),
        Err(e) => error!("Polymarket WebSocket disabled, REST polling only: {:#}", e),
    }

    // Initialize Telegram client
    let telegram_client = Arc::new(TelegramClient::new(config.telegram.clone()));
    if telegram_client.is_enabled() {
        info!("✅ Telegram notifications enabled");
    } else {
        info!("ℹ️ Telegram notifications disabled");
    }

    // The directional strategy is isolated from the paired path: an invalid [mention]
    // section must never stop arbitrage from starting. It fails closed instead, running
    // with defaults, mode off, and the directional kill switch engaged.
    let mention_config = match config.mention.validate() {
        Ok(()) => config.mention.clone(),
        Err(reason) => {
            error!(
                "Invalid [mention] config; directional trading is disabled: {}",
                reason
            );
            let _ = service.storage.set_directional_mode(MentionMode::Off);
            let _ = service.storage.set_directional_kill_switch(true);
            MentionConfig::default()
        }
    };
    service
        .storage
        .initialize_directional_mode(mention_config.mode)?;
    let mention_signals = MentionSignalClient::new(&mention_config)?;
    let directional_execution = Arc::new(DirectionalExecutionService::new(
        mention_config.clone(),
        service.storage.clone(),
        service.kalshi_client.clone(),
        mention_signals.clone(),
        telegram_client.clone(),
    ));

    // Create shared state
    let state = Arc::new(AppState {
        service: RwLock::new(service),
        config: config.clone(),
        telegram_client,
        directional_execution,
    });

    let directional_executor = state.directional_execution.clone();
    let directional_interval = mention_config.poll_interval_secs.max(1);
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(directional_interval));
        loop {
            interval.tick().await;
            directional_executor.run_once().await;
        }
    });

    // Spawn price update handler
    let state_clone = state.clone();
    tokio::spawn(async move {
        while let Some(update) = price_rx.recv().await {
            let service = state_clone.service.read().await;
            service.ws_manager.on_price_update(update);
        }
    });

    // Spawn metrics reporter and API ping tester (every 10 seconds)
    let state_for_metrics = state.clone();
    let metrics_clone = metrics.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(10));

        loop {
            interval.tick().await;

            // Perform API ping tests
            ping_apis(&state_for_metrics, &metrics_clone).await;

            // Reset metrics for next period (metrics are sent via WebSocket in websocket.rs)
            metrics_clone.reset();
        }
    });

    // Spawn periodic market scanner (every 5 minutes)
    let state_for_scanner = state.clone();
    tokio::spawn(async move {
        info!(
            "🔍 Market scan task started, interval {} seconds ({} minutes)",
            MARKET_SCAN_INTERVAL_SECS,
            MARKET_SCAN_INTERVAL_SECS / 60
        );

        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(MARKET_SCAN_INTERVAL_SECS));
        let mut scan_count = 0u64;

        // Wait for initial WebSocket connections to establish
        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

        loop {
            interval.tick().await;
            scan_count += 1;

            info!("🔄 Starting periodic market scan #{}...", scan_count);

            // Scan for new markets
            let scan_result = {
                let mut service = state_for_scanner.service.write().await;
                service.scan_for_new_markets().await
            };

            match scan_result {
                Ok((new_markets, sub_info)) => {
                    if new_markets.is_empty() {
                        info!(
                            "✅ Periodic scan #{} complete; no new markets found",
                            scan_count
                        );
                    } else {
                        info!(
                            "🆕 Found {} new matching markets; starting hot subscription...",
                            new_markets.len()
                        );

                        // Update WebSocketManager with new markets
                        {
                            let service = state_for_scanner.service.read().await;
                            let added = service.ws_manager.add_matched_markets(
                                new_markets.clone(),
                                sub_info.market_lookup.clone(),
                            );
                            info!("📊 Added {} markets to the WebSocket manager", added);
                        }

                        // Hot subscribe to new markets
                        let kalshi_success = {
                            let service = state_for_scanner.service.read().await;
                            if !sub_info.kalshi_tickers.is_empty() {
                                match service
                                    .kalshi_client
                                    .subscribe_markets(sub_info.kalshi_tickers.clone())
                                    .await
                                {
                                    Ok(success) => success,
                                    Err(e) => {
                                        error!("❌ Kalshi hot-subscription failed: {:#}", e);
                                        false
                                    }
                                }
                            } else {
                                true
                            }
                        };

                        if kalshi_success {
                            let service = state_for_scanner.service.read().await;
                            let total_markets =
                                service.ws_manager.get_matched_markets_for_frontend().len();
                            info!("✅ Hot subscription succeeded; {} matched markets currently active", total_markets);

                            // Broadcast scan stats to frontend via WebSocket
                            let scan_stats = crate::models::ScanStats {
                                scan_count,
                                new_markets_found: new_markets.len(),
                                total_matched_markets: total_markets,
                                timestamp: Utc::now(),
                            };
                            service.ws_manager.broadcast_scan_stats(scan_stats);
                            info!("📡 Broadcast scan stats to the frontend");
                        } else {
                            warn!("⚠️ Part of the hot subscription failed");
                        }
                    }
                }
                Err(e) => {
                    error!("❌ Market scan failed: {:#}", e);
                }
            }
        }
    });

    // Spawn auto-trade queue checker (every 200ms for faster response)
    let state_for_queue = state.clone();
    let metrics_for_queue = metrics.clone();
    tokio::spawn(async move {
        info!("📋 Auto-trade queue check task started, interval 200ms");

        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(200));

        loop {
            interval.tick().await;

            // Check and add opportunities to queue
            check_and_queue_auto_trade(&state_for_queue, &metrics_for_queue).await;
        }
    });

    // Spawn auto-trade executor (processes queue with 1s interval)
    let state_for_executor = state.clone();
    let metrics_for_executor = metrics.clone();
    tokio::spawn(async move {
        info!("🚀 Auto-trade executor started, opportunity interval 1 second");

        loop {
            let service = state_for_executor.service.read().await;

            // Check if already executing
            if service
                .ws_manager
                .is_auto_trading
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                drop(service);
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                continue;
            }

            // Get next opportunity from queue
            let key = {
                let mut queue = service.ws_manager.auto_trade_queue.write();
                queue.pop_front()
            };

            if let Some(key) = key {
                // Mark as executing
                service
                    .ws_manager
                    .is_auto_trading
                    .store(true, std::sync::atomic::Ordering::Relaxed);

                info!("🎯 [Auto-trade executor] Starting processing: {}", key);

                // Execute the opportunity
                execute_single_auto_trade(
                    &service,
                    &state_for_executor,
                    &metrics_for_executor,
                    &key,
                )
                .await;

                // Mark as done
                service
                    .ws_manager
                    .is_auto_trading
                    .store(false, std::sync::atomic::Ordering::Relaxed);

                drop(service);

                info!("⏱️  [Auto-trade executor] Waiting 1 second before processing the next opportunity");
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            } else {
                // Queue empty, wait a bit
                drop(service);
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }
    });

    // Spawn ended market cleanup task (every 60 seconds)
    let state_for_cleanup = state.clone();
    tokio::spawn(async move {
        info!("🧹 Ended-game cleanup task started, interval 60 seconds");

        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));

        // Wait for initial data to be populated
        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;

        loop {
            interval.tick().await;

            // Check and clean up ended markets
            cleanup_ended_markets(&state_for_cleanup).await;
        }
    });

    // Build router
    let app = Router::new()
        // Health check
        .route("/api/health", get(routes::health_check))
        // Authentication
        .route("/api/auth/login", post(routes::login))
        // Stats and data
        .route("/api/stats", get(routes::get_stats))
        .route("/api/data-coverage", get(routes::get_data_coverage))
        .route("/api/opportunities", get(routes::get_opportunities))
        .route("/api/matched-markets", get(routes::get_matched_markets))
        .route("/api/arbitrage-history", get(routes::get_arbitrage_history))
        // Account info
        .route("/api/balance/kalshi", get(routes::get_kalshi_balance))
        .route(
            "/api/balance/polymarket",
            get(routes::get_polymarket_balance),
        )
        .route("/api/account-balance", get(routes::get_account_balance))
        // Orders
        .route("/api/order/kalshi", post(routes::place_kalshi_order))
        .route(
            "/api/order/polymarket",
            post(routes::place_polymarket_order),
        )
        .route("/api/arbitrage/execute", post(routes::execute_arbitrage))
        // Order management
        .route("/api/orders/kalshi", get(routes::get_kalshi_orders))
        .route("/api/orders/polymarket", get(routes::get_polymarket_orders))
        .route(
            "/api/orders/kalshi/:order_id",
            delete(routes::cancel_kalshi_order),
        )
        .route(
            "/api/orders/polymarket/:order_id",
            delete(routes::cancel_polymarket_order),
        )
        // Position management
        .route("/api/positions/kalshi", get(routes::get_kalshi_positions))
        .route(
            "/api/positions/polymarket",
            get(routes::get_polymarket_positions),
        )
        .route("/api/positions/unified", get(routes::get_unified_positions))
        // Tracking
        .route("/api/tracking", get(routes::get_tracking))
        // History search
        .route("/api/history/search", get(routes::search_history))
        .route(
            "/api/history/statistics",
            get(routes::get_history_statistics),
        )
        // Orderbook depth
        .route("/api/orderbook/depth", get(routes::get_orderbook_depth))
        // Auto-trade
        .route("/api/auto-trade/status", get(routes::get_auto_trade_status))
        .route("/api/auto-trade/enable", post(routes::enable_auto_trade))
        .route("/api/auto-trade/disable", post(routes::disable_auto_trade))
        .route("/api/auto-trade/reset", post(routes::reset_auto_trade))
        .route(
            "/api/auto-trade/settings",
            put(routes::update_auto_trade_settings),
        )
        .route(
            "/api/auto-trade/history",
            get(routes::get_auto_trade_history),
        )
        .route(
            "/api/auto-trade/excluded",
            get(routes::get_excluded_markets),
        )
        .route("/api/auto-trade/exclude", post(routes::exclude_market))
        .route("/api/auto-trade/unexclude", post(routes::unexclude_market))
        .route("/api/auto-trade/queue", get(routes::get_auto_trade_queue))
        // Independent single-leg directional mention strategy
        .route("/api/mention/status", get(routes::get_mention_status))
        .route("/api/mention/signals", get(routes::get_mention_signals))
        .route("/api/mention/history", get(routes::get_mention_history))
        .route("/api/mention/decisions", get(routes::get_mention_decisions))
        .route(
            "/api/mention/mode",
            axum::routing::put(routes::set_mention_mode),
        )
        .route("/api/mention/kill", post(routes::mention_kill_switch))
        .route(
            "/api/mention/reset-loss-breaker",
            post(routes::reset_mention_loss_breaker),
        )
        // App settings (hot-updatable)
        .route("/api/settings", get(routes::get_app_settings))
        .route("/api/settings", put(routes::update_app_settings))
        // WebSocket
        .route("/ws", get(websocket::ws_handler))
        // Add state
        .with_state(state)
        // Add middleware
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        // At minimum, every request against the UI's API gets a one-line
        // terminal/log entry on completion (method, path, status, latency) -
        // previously this only fired for failures (on_failure defaults to
        // ERROR), because a successful response's default trace level is
        // DEBUG and main.rs filters tower_http down to `warn`.
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        // Static files - must be last!
        .fallback(static_files::static_handler);

    info!("✅ API routes configured (including frontend static files)");

    Ok(app)
}

/// Ping both APIs to measure latency and cache balances
async fn ping_apis(state: &Arc<AppState>, metrics: &Arc<PerformanceMetrics>) {
    // Test Kalshi API latency and cache balance
    let kalshi_start = Instant::now();
    let service = state.service.read().await;

    match service.kalshi_client.get_balance().await {
        Ok(balance) => {
            let latency_ms = kalshi_start.elapsed().as_millis() as u64;
            metrics.set_kalshi_latency(latency_ms);
            metrics.set_kalshi_balance(balance);
        }
        Err(e) => {
            warn!("Kalshi API ping failed: {}", e);
        }
    }

    // Test Polymarket API latency and cache balance
    let poly_start = Instant::now();
    match service.polymarket_client.get_balance().await {
        Ok(balance) => {
            let latency_ms = poly_start.elapsed().as_millis() as u64;
            metrics.set_polymarket_latency(latency_ms);
            metrics.set_polymarket_balance(balance);
        }
        Err(e) => {
            warn!("Polymarket API ping failed: {}", e);
        }
    }
}

fn save_auto_trade_skip(
    service: &ArbitrageService,
    key: &str,
    record: &crate::models::ArbitrageTrackingRecord,
    opportunity: &crate::models::ArbitrageOpportunity,
    contracts: i32,
    duration_ms: i64,
    reason: &str,
) {
    info!(
        "⚠️ [Auto-trade] {}: {} - {}",
        reason, record.event_name, record.team_name
    );
    if service.ws_manager.should_record_skip(key, reason) {
        if let Err(error) = service
            .ws_manager
            .get_storage()
            .save_skipped_auto_trade_record(
                &record.event_name,
                &record.team_name,
                &record.kalshi_market_id,
                &record.polymarket_market_id,
                &opportunity.kalshi_side,
                &opportunity.polymarket_side,
                contracts,
                opportunity.kalshi_price,
                opportunity.polymarket_price,
                opportunity.profit_margin,
                duration_ms,
                reason,
            )
        {
            error!("Failed to save skipped auto-trade record: {:#}", error);
        }
    }
}

/// Check and add eligible opportunities to auto-trade queue
async fn check_and_queue_auto_trade(state: &Arc<AppState>, _metrics: &Arc<PerformanceMetrics>) {
    let service = state.service.read().await;

    // Get auto-trade state
    let auto_state = service.ws_manager.get_auto_trade_state();

    // Early return if not enabled or already at limit
    if !auto_state.enabled {
        return;
    }

    if auto_state.trade_count >= auto_state.max_trade_count {
        return;
    }

    // Get active tracking records
    let tracking_records = service.ws_manager.get_active_tracking_for_auto_trade();

    // Add eligible opportunities to queue
    for (key, _record, duration_ms) in tracking_records {
        // Check eligibility
        let (eligible, _reason) = service
            .ws_manager
            .check_auto_trade_eligibility(&key, duration_ms);

        if !eligible {
            continue;
        }

        // Add to queue if not already queued
        let mut queue = service.ws_manager.auto_trade_queue.write();
        if !queue.contains(&key) {
            queue.push_back(key.clone());
            info!("📋 [Auto-trade queue] Added opportunity: {}", key);
        }
    }
}

/// Execute auto-trade for a single opportunity (extracted from original for loop)
async fn execute_single_auto_trade(
    service: &ArbitrageService,
    state: &Arc<AppState>,
    _metrics: &Arc<PerformanceMetrics>,
    key: &str,
) {
    let (record, duration_ms) = match service.ws_manager.get_tracking_record(key) {
        Some((record, duration_ms)) => (record, duration_ms),
        None => {
            info!("⚠️ [Auto-trade] Opportunity no longer exists: {}", key);
            return;
        }
    };

    let auto_state = service.ws_manager.get_auto_trade_state();
    let (eligible, reason) = service
        .ws_manager
        .check_auto_trade_eligibility(key, duration_ms);
    if !eligible {
        info!(
            "⚠️ [Auto-trade] Opportunity no longer meets conditions: {} - {}",
            key, reason
        );
        return;
    }

    let opportunity = match service.ws_manager.get_opportunity_by_key(key) {
        Some(opportunity) => opportunity,
        None => {
            info!("⚠️ [Auto-trade] Opportunity data does not exist: {}", key);
            return;
        }
    };

    let Some(matched_market) = service
        .get_matched_markets()
        .iter()
        .find(|market| market.market_key() == key)
    else {
        save_auto_trade_skip(
            service,
            key,
            &record,
            &opportunity,
            auto_state.min_contracts,
            duration_ms,
            "Matched market disappeared before execution",
        );
        return;
    };
    let polymarket_competitor = match opportunity.polymarket_side.as_str() {
        "yes" => record.team_name.as_str(),
        "no" => match matched_market
            .polymarket_market
            .get_opponent(&record.team_name)
        {
            Some(opponent) => opponent,
            None => {
                save_auto_trade_skip(
                    service,
                    key,
                    &record,
                    &opportunity,
                    auto_state.min_contracts,
                    duration_ms,
                    "Polymarket US opponent is unavailable for the NO hedge leg",
                );
                return;
            }
        },
        _ => {
            save_auto_trade_skip(
                service,
                key,
                &record,
                &opportunity,
                auto_state.min_contracts,
                duration_ms,
                "Polymarket auto-trade side must be yes or no",
            );
            return;
        }
    };
    let Some(poly_execution) = matched_market
        .polymarket_market
        .us_execution_for_competitor(polymarket_competitor)
    else {
        save_auto_trade_skip(
            service,
            key,
            &record,
            &opportunity,
            auto_state.min_contracts,
            duration_ms,
            "Polymarket US position side is unavailable for the matched competitor",
        );
        return;
    };

    let Some(kalshi_book) = service
        .kalshi_client
        .get_fresh_orderbook(&record.kalshi_market_id, EXECUTABLE_BOOK_MAX_AGE)
    else {
        save_auto_trade_skip(
            service,
            key,
            &record,
            &opportunity,
            auto_state.min_contracts,
            duration_ms,
            "Fresh Kalshi websocket executable depth is unavailable",
        );
        return;
    };
    let poly_book = match service
        .polymarket_client
        .get_market_book(&poly_execution.market_slug)
        .await
    {
        Ok(book) => book,
        Err(reason) => {
            save_auto_trade_skip(
                service,
                key,
                &record,
                &opportunity,
                auto_state.min_contracts,
                duration_ms,
                &format!("Fresh Polymarket US executable book is unavailable: {reason}"),
            );
            return;
        }
    };

    let kalshi_levels = kalshi_buy_levels(&kalshi_book, &opportunity.kalshi_side);
    let poly_levels = polymarket_buy_levels(&poly_book, poly_execution.position_side);
    let max_depth = executable_depth(&kalshi_levels).min(executable_depth(&poly_levels));
    let Some(size_cap) = calculate_contracts_to_trade(
        max_depth,
        max_depth,
        auto_state.flexible_mode,
        auto_state.min_contracts,
        auto_state.max_contracts,
    ) else {
        save_auto_trade_skip(
            service,
            key,
            &record,
            &opportunity,
            auto_state.min_contracts,
            duration_ms,
            "Fresh executable depth is below the configured contract minimum",
        );
        return;
    };

    let Some((contracts, kalshi_price_cents, poly_price, kalshi_fee, profit_margin)) =
        find_profitable_contract_size(
            size_cap,
            auto_state.min_contracts,
            auto_state.max_amount,
            poly_book.minimum_trade_qty,
            &kalshi_levels,
            &poly_levels,
        )
    else {
        let reason = if poly_book.minimum_trade_qty.is_none() {
            "Polymarket US market minimum trade quantity is unavailable"
        } else {
            "No fresh executable size remains profitable after fees, worst-case prices, Polymarket's minimum trade quantity, and amount limit"
        };
        save_auto_trade_skip(
            service,
            key,
            &record,
            &opportunity,
            size_cap,
            duration_ms,
            reason,
        );
        return;
    };

    let outcome = submit_paired_order(
        service,
        &state.telegram_client,
        PairedOrderParams {
            event_name: record.event_name.clone(),
            team_name: record.team_name.clone(),
            kalshi_market_id: record.kalshi_market_id.clone(),
            kalshi_side: opportunity.kalshi_side.clone(),
            polymarket_market_slug: poly_execution.market_slug.clone(),
            polymarket_position_side: poly_execution.position_side,
            contracts,
            kalshi_price_cents,
            poly_price,
            kalshi_fee,
            profit_margin,
            duration_ms,
            poly_book_fetched_at_ms: poly_book.fetched_at_ms,
            neutralization_max_loss_cents: auto_state.neutralization_max_loss_cents,
            dry_run: false,
        },
    )
    .await;

    // submit_paired_order already halts auto-trading (and, for a fill
    // mismatch it can't fully neutralize, sends the Telegram alert) on any
    // of these outcomes, so there is nothing further to react to here.
    match outcome.status.as_str() {
        "persistence_failed_before_submission"
        | "persistence_failed_after_poly_ack"
        | "persistence_failed_after_kalshi_ack" => {}
        _ if outcome.unsafe_fill_quantity || outcome.residual_contracts > 0 => {}
        "executed" => {
            service.ws_manager.mark_as_auto_traded(key);
            if let Err(reason) = service.ws_manager.increment_trade_count() {
                error!("Failed to increment auto-trade count: {:#}", reason);
            }
        }
        _ => {
            service.ws_manager.mark_as_auto_traded(key);
        }
    }
}

/// Clean up ended markets by unsubscribing from WebSocket feeds
///
/// This function:
/// 1. Checks all markets for extreme prices (Kalshi 99/2, Poly 100/0)
/// 2. If extreme prices persist for 20+ minutes, marks market as ended
/// 3. Unsubscribes from WebSocket feeds for ended markets
/// 4. Removes ended markets from internal caches
async fn cleanup_ended_markets(state: &Arc<AppState>) {
    let service = state.service.read().await;

    // Get detection counts for logging
    let detecting_count = service.ws_manager.get_ending_detection_count();
    let ended_count = service.ws_manager.get_confirmed_ended_count();

    if detecting_count > 0 {
        info!(
            "🔍 [Cleanup] Monitoring {} potentially ended markets, confirmed {}",
            detecting_count, ended_count
        );
    }

    // Remove ended markets and get Kalshi subscriptions to unsubscribe.
    let kalshi_to_unsub = service.ws_manager.remove_ended_markets();

    if kalshi_to_unsub.is_empty() {
        return;
    }

    // Unsubscribe from Kalshi markets
    if !kalshi_to_unsub.is_empty() {
        match service
            .kalshi_client
            .unsubscribe_markets(kalshi_to_unsub.clone())
            .await
        {
            Ok(success) => {
                if success {
                    info!(
                        "✅ [Cleanup] Kalshi unsubscribe succeeded: {} markets",
                        kalshi_to_unsub.len()
                    );
                } else {
                    warn!("⚠️ [Cleanup] Kalshi unsubscribe partially failed");
                }
            }
            Err(e) => {
                error!("❌ [Cleanup] Kalshi unsubscribe failed: {:#}", e);
            }
        }
    }

    // Log summary
    let remaining_markets = service.ws_manager.get_matched_markets_for_frontend().len();
    info!(
        "✅ [Cleanup] Complete; {} active markets remain",
        remaining_markets
    );
}
