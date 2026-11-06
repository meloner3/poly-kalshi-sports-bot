//! Arbitrage Service
//!
//! Orchestrates market data fetching, matching, and arbitrage scanning.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::clients::{KalshiClient, PolymarketClient};
use crate::config::Config;
use crate::core::{EventMatcher, SubscriptionInfo};
use crate::models::{
    ArbitrageOpportunity, MatchedEvent, MatchedMarket, PolymarketPositionSide, PriceUpdate,
    SystemStats,
};
use crate::services::{ArbitrageStorage, Operation, PerformanceMetrics, WebSocketManager};

const MIN_REST_QUOTE_REFRESH_SECS: u64 = 1;

/// Arbitrage service
pub struct ArbitrageService {
    pub kalshi_client: KalshiClient,
    pub polymarket_client: PolymarketClient,
    pub matcher: EventMatcher,
    pub ws_manager: Arc<WebSocketManager>,
    pub storage: Arc<ArbitrageStorage>,
    pub matched_events: Vec<MatchedEvent>,
    pub matched_markets: Vec<MatchedMarket>,
    /// Performance metrics
    pub metrics: Arc<PerformanceMetrics>,
}

impl ArbitrageService {
    /// Create a new arbitrage service
    pub async fn new(config: &Config) -> Result<Self> {
        // Initialize storage
        let storage = Arc::new(ArbitrageStorage::new("arbitrage_history.db")?);

        // Initialize performance metrics
        let metrics = Arc::new(PerformanceMetrics::new());

        // Initialize clients. Polymarket US credentials are validated here
        // (PolymarketClient::new fails fast if they're missing) - there's no
        // separate order service to health-check anymore.
        let kalshi_client = KalshiClient::new(config.kalshi.clone())?;
        let polymarket_client = PolymarketClient::new(config.polymarket.clone())?;

        // Create matcher
        let matcher = EventMatcher::new(24);

        // Create WebSocket manager with metrics
        let mut ws_manager = WebSocketManager::new(
            config.settings.min_profit_margin,
            config.settings.default_bet_amount,
            config.settings.tracking_threshold,
            storage.clone(),
            metrics.clone(),
            Duration::from_secs(
                config
                    .settings
                    .refresh_interval
                    .max(MIN_REST_QUOTE_REFRESH_SECS)
                    .saturating_mul(2),
            ),
        );

        // Kalshi's live order book remains the only executable-depth source.
        ws_manager.set_kalshi_client(kalshi_client.clone());
        // Used only to fetch a one-time real depth snapshot when tracking
        // starts, for Advanced Search visibility; never for order submission.
        ws_manager.set_polymarket_client(polymarket_client.clone());

        // Load excluded markets from database
        ws_manager.load_excluded_markets();

        let ws_manager = Arc::new(ws_manager);

        Ok(Self {
            kalshi_client,
            polymarket_client,
            matcher,
            ws_manager,
            storage,
            matched_events: Vec::new(),
            matched_markets: Vec::new(),
            metrics,
        })
    }

    /// Initialize the service by fetching and matching markets
    pub async fn initialize(&mut self) -> Result<()> {
        info!("🔍 Fetching supported NBA and tennis match-winner data from both platforms...");

        // Fetch data from both platforms
        let (kalshi_events, kalshi_markets) = self
            .kalshi_client
            .get_supported_events_and_markets()
            .await?;

        let (polymarket_events, polymarket_markets) = self
            .polymarket_client
            .get_supported_events_and_markets()
            .await?;

        info!(
            "📊 Loaded supported sports: Kalshi {} events/{} markets, Polymarket {} events/{} markets",
            kalshi_events.len(),
            kalshi_markets.len(),
            polymarket_events.len(),
            polymarket_markets.len()
        );

        // Match events and markets (with timing)
        let match_start = Instant::now();
        let (matched_events, matched_markets) = self.matcher.match_events_and_markets(
            &kalshi_events,
            &kalshi_markets,
            &polymarket_events,
            &polymarket_markets,
        );
        self.metrics
            .record(Operation::MarketMatch, match_start.elapsed());

        self.matched_events = matched_events;
        self.matched_markets = matched_markets.clone();

        // Configure WebSocket manager
        self.ws_manager.set_matched_markets(matched_markets);
        let refreshed_polymarket_quotes = self
            .ws_manager
            .update_polymarket_rest_quotes(&polymarket_markets);
        let kalshi_tickers = self.ws_manager.get_kalshi_quote_tickers();
        let refreshed_kalshi_quotes =
            match self.kalshi_client.get_market_quotes(&kalshi_tickers).await {
                Ok(quotes) => self.ws_manager.update_kalshi_rest_quotes(&quotes),
                Err(error) => {
                    tracing::warn!(
                        "Initial authoritative Kalshi REST quote refresh failed: {}",
                        error
                    );
                    0
                }
            };

        info!(
            "✅ Initialization complete: {} matched markets, {} Kalshi and {} Polymarket REST quote updates",
            self.matched_markets.len(),
            refreshed_kalshi_quotes,
            refreshed_polymarket_quotes,
        );

        Ok(())
    }

    /// Start WebSocket connections for real-time updates
    pub async fn start_websocket_connections(
        &self,
        price_tx: mpsc::Sender<PriceUpdate>,
    ) -> Result<()> {
        let kalshi_tickers = self.ws_manager.get_kalshi_subscription_ids();

        info!(
            "📡 Starting WebSocket connections: {} Kalshi markets; Polymarket US quotes use their own WebSocket + REST fallback",
            kalshi_tickers.len()
        );

        let kalshi_client = self.kalshi_client.clone();

        let price_tx_kalshi = price_tx.clone();

        // Spawn Kalshi WebSocket
        let kalshi_tickers_clone = kalshi_tickers.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = kalshi_client
                    .connect_websocket(kalshi_tickers_clone.clone(), price_tx_kalshi.clone())
                    .await
                {
                    error!(
                        "Kalshi WebSocket error: {:#}. Reconnecting in 5 seconds...",
                        e
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                }
            }
        });

        Ok(())
    }

    /// Run periodic market scanning
    pub async fn run_periodic_scan(&self, interval_secs: u64) {
        let ws_manager = self.ws_manager.clone();
        let interval_secs = interval_secs.max(MIN_REST_QUOTE_REFRESH_SECS);

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));

            loop {
                interval.tick().await;
                let opportunities = ws_manager.calculate_all();

                if !opportunities.is_empty() {
                    info!(
                        "📊 Periodic scan: found {} arbitrage opportunities, best: {:.2}%",
                        opportunities.len(),
                        opportunities
                            .first()
                            .map(|o| o.profit_margin)
                            .unwrap_or(0.0)
                    );
                }
            }
        });
    }

    /// Poll the Polymarket US gateway for current quotes on the same feed used
    /// by discovery. CLOB order books remain separate and are never inferred
    /// from the gateway's market-side IDs.
    pub async fn run_polymarket_quote_refresh(&self, requested_interval_secs: u64) {
        let interval_secs = requested_interval_secs.max(MIN_REST_QUOTE_REFRESH_SECS);
        if interval_secs != requested_interval_secs {
            tracing::warn!(
                "Polymarket quote refresh interval {}s is too low; clamping to {}s",
                requested_interval_secs,
                interval_secs
            );
        }

        let polymarket_client = self.polymarket_client.clone();
        let ws_manager = self.ws_manager.clone();

        tokio::spawn(async move {
            info!(
                "📡 Polymarket US REST quote refresh started, interval {} seconds",
                interval_secs
            );
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));

            loop {
                interval.tick().await;

                let fetched_at = Instant::now();
                match polymarket_client.get_supported_events_and_markets().await {
                    Ok((_, markets)) => {
                        let updated =
                            ws_manager.update_polymarket_rest_quotes_at(&markets, fetched_at);
                        let matched_count = ws_manager.matched_markets.read().len();
                        if matched_count > 0 && updated == 0 {
                            tracing::warn!(
                                "Polymarket US REST quote refresh returned no quotes for {} matched markets",
                                matched_count
                            );
                        } else {
                            info!(
                                "✅ Polymarket US REST quote refresh updated {} matched markets",
                                updated
                            );
                        }
                    }
                    Err(e) => {
                        error!("❌ Polymarket US REST quote refresh failed: {:#}", e);
                    }
                }
            }
        });
    }

    /// Stream Polymarket US quotes over the markets WebSocket so detection
    /// reacts to a price change immediately instead of on the next REST
    /// poll. The REST poll keeps running as the fallback and for
    /// ended-market detection.
    pub fn run_polymarket_ws(&self, key_id: String, secret_key: String) {
        let ws_manager = self.ws_manager.clone();
        let slug_manager = self.ws_manager.clone();
        tokio::spawn(async move {
            info!("📡 Polymarket US WebSocket quote stream starting");
            crate::clients::polymarket_ws::run_markets_stream(
                key_id,
                secret_key,
                move || slug_manager.get_polymarket_slugs(),
                move |event| match event {
                    crate::clients::polymarket_ws::PolyWsEvent::Quote(quote) => {
                        ws_manager.update_polymarket_ws_quote(
                            &quote.slug,
                            quote.long_quote,
                            quote.short_quote,
                        );
                    }
                    crate::clients::polymarket_ws::PolyWsEvent::Book(book) => {
                        ws_manager.update_polymarket_ws_book(book);
                    }
                },
            )
            .await;
        });
    }

    /// Poll authoritative Kalshi REST asks for currently matched tickers.
    ///
    /// The WebSocket order-book cache stays independent and remains the only
    /// source allowed to satisfy execution-depth checks.
    pub async fn run_kalshi_quote_refresh(&self, requested_interval_secs: u64) {
        let interval_secs = requested_interval_secs.max(MIN_REST_QUOTE_REFRESH_SECS);
        if interval_secs != requested_interval_secs {
            tracing::warn!(
                "Kalshi quote refresh interval {}s is too low; clamping to {}s",
                requested_interval_secs,
                interval_secs
            );
        }

        let kalshi_client = self.kalshi_client.clone();
        let ws_manager = self.ws_manager.clone();

        tokio::spawn(async move {
            info!(
                "📡 Kalshi authoritative REST quote refresh started, interval {} seconds",
                interval_secs
            );
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));

            loop {
                interval.tick().await;
                let tickers = ws_manager.get_kalshi_quote_tickers();
                if tickers.is_empty() {
                    continue;
                }

                let fetched_at = Instant::now();
                match kalshi_client.get_market_quotes(&tickers).await {
                    Ok(quotes) => {
                        let updated = ws_manager.update_kalshi_rest_quotes_at(&quotes, fetched_at);
                        if updated != tickers.len() {
                            tracing::warn!(
                                "Kalshi REST quote refresh updated {}/{} matched tickers",
                                updated,
                                tickers.len()
                            );
                        } else {
                            info!(
                                "✅ Kalshi REST quote refresh updated {} matched tickers",
                                updated
                            );
                        }
                    }
                    Err(error) => {
                        error!("❌ Kalshi REST quote refresh failed: {:#}", error);
                    }
                }
            }
        });
    }

    /// Get current opportunities
    pub fn get_opportunities(&self) -> Vec<ArbitrageOpportunity> {
        self.ws_manager.get_opportunities()
    }

    /// Get system statistics
    pub fn get_stats(&self) -> SystemStats {
        let mut stats = self.ws_manager.get_stats();
        stats.matched_events = self.matched_events.len();
        stats.matched_markets = self.matched_markets.len();
        stats
    }

    /// Get matched markets
    pub fn get_matched_markets(&self) -> &[MatchedMarket] {
        &self.matched_markets
    }

    /// Place an order on Kalshi
    pub async fn place_kalshi_order(
        &self,
        ticker: &str,
        side: &str,
        outcome: &str,
        count: i32,
        price: i32,
    ) -> Result<serde_json::Value> {
        self.kalshi_client
            .place_order(ticker, side, outcome, count, price)
            .await
    }

    /// Place a Polymarket US market order.
    pub async fn place_polymarket_order(
        &self,
        market_slug: &str,
        position_side: PolymarketPositionSide,
        side: &str,
        contracts: i32,
        price: f64,
    ) -> Result<serde_json::Value> {
        self.polymarket_client
            .place_limit_order(market_slug, position_side, side, contracts, price)
            .await
    }

    /// Get arbitrage history
    pub fn get_arbitrage_history(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::models::ArbitrageTrackingRecord>> {
        self.storage.get_history(limit)
    }

    /// Scan for new markets and return incremental subscription info
    ///
    /// This method fetches fresh market data from both platforms, runs matching,
    /// and returns only the NEW matched markets that weren't in the previous set.
    ///
    /// Returns: (new_matched_markets, new_subscription_info)
    pub async fn scan_for_new_markets(&mut self) -> Result<(Vec<MatchedMarket>, SubscriptionInfo)> {
        info!("============================================================");
        info!("🔄 Starting scan for new markets...");
        info!("============================================================");

        // 1. Save old matched market IDs
        let old_matched_ids: HashSet<String> = self
            .matched_markets
            .iter()
            .map(|mm| format!("{}_{}", mm.kalshi_market.market_id, mm.team_name))
            .collect();

        let old_count = self.matched_markets.len();
        info!("   Scan state before scan: {} matched markets", old_count);

        // 2. Fetch fresh market data
        let (kalshi_events, kalshi_markets) =
            match self.kalshi_client.get_supported_events_and_markets().await {
                Ok(data) => data,
                Err(e) => {
                    error!("❌ Failed to fetch Kalshi market data: {:#}", e);
                    return Ok((Vec::new(), SubscriptionInfo::empty()));
                }
            };

        let (polymarket_events, polymarket_markets) = match self
            .polymarket_client
            .get_supported_events_and_markets()
            .await
        {
            Ok(data) => data,
            Err(e) => {
                error!("❌ Failed to fetch Polymarket market data: {:#}", e);
                return Ok((Vec::new(), SubscriptionInfo::empty()));
            }
        };

        info!(
            "   Scan state after scan: Kalshi {} events/{} markets, Polymarket {} events/{} markets",
            kalshi_events.len(),
            kalshi_markets.len(),
            polymarket_events.len(),
            polymarket_markets.len()
        );

        // 3. Re-run matching
        let match_start = Instant::now();
        let (matched_events, matched_markets) = self.matcher.match_events_and_markets(
            &kalshi_events,
            &kalshi_markets,
            &polymarket_events,
            &polymarket_markets,
        );
        self.metrics
            .record(Operation::MarketMatch, match_start.elapsed());

        // 4. Find new matched markets
        let mut new_matched_markets = Vec::new();
        for mm in &matched_markets {
            let key = format!("{}_{}", mm.kalshi_market.market_id, mm.team_name);
            if !old_matched_ids.contains(&key) {
                new_matched_markets.push(mm.clone());
            }
        }

        // 5. Update internal state
        self.matched_events = matched_events;
        self.matched_markets = matched_markets;

        if new_matched_markets.is_empty() {
            info!("   ✅ No new matched markets discovered");
            info!("============================================================");
            return Ok((Vec::new(), SubscriptionInfo::empty()));
        }

        info!("   🆕 发现 {} 个新匹配市场:", new_matched_markets.len());
        for (i, mm) in new_matched_markets.iter().enumerate().take(5) {
            info!("      {}. {} ({})", i + 1, mm.event_name, mm.team_name);
        }
        if new_matched_markets.len() > 5 {
            info!(
                "      ... {} more new markets",
                new_matched_markets.len() - 5
            );
        }

        // 6. Generate subscription info for new markets only
        let new_sub_info = self.matcher.get_subscription_info(&new_matched_markets);

        info!(
            "   📡 New subscription requirements: Kalshi {} markets; Polymarket US quotes use their own WebSocket + REST fallback",
            new_sub_info.kalshi_tickers.len()
        );
        info!("============================================================");

        Ok((new_matched_markets, new_sub_info))
    }

    /// Search history records with filters
    pub fn search_history(
        &self,
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
    ) -> Result<serde_json::Value> {
        self.storage.search_records(
            min_profit,
            max_profit,
            min_duration,
            max_duration,
            event_name,
            team_name,
            sort_by,
            sort_order,
            limit,
            offset,
            include_history,
        )
    }

    /// Get history statistics
    pub fn get_history_statistics(&self) -> Result<serde_json::Value> {
        self.storage.get_statistics()
    }
}
