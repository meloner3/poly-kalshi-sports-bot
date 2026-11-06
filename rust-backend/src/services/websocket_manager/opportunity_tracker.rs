//! Opportunity tracking logic
//!
//! Handles tracking of high-profit arbitrage opportunities.

use std::time::{Duration, Instant};

use chrono::Utc;
use tracing::{debug, info};

use super::WebSocketManager;
use crate::models::{ArbitrageOpportunity, ArbitrageTrackingRecord, PolymarketUsExecution};

/// A market flapping in and out of tracking (profit margin oscillating near
/// the threshold) must not re-hit Polymarket's real book endpoint on every
/// restart — that is what triggered a Cloudflare IP rate limit (error 1015)
/// that then starved out the endpoint the real execution path depends on.
/// One live snapshot per market within this window is enough for the
/// Advanced Search "depth at tracking start" display.
const POLY_DEPTH_SNAPSHOT_COOLDOWN: Duration = Duration::from_secs(600);

impl WebSocketManager {
    /// Track a high-profit opportunity
    pub(crate) fn track_opportunity(
        &self,
        opp: &ArbitrageOpportunity,
        poly_execution: Option<PolymarketUsExecution>,
    ) {
        let key = opp.market_key();

        let mut tracking = self.active_tracking.write();

        if let Some(record) = tracking.get_mut(&key) {
            // Update existing tracking
            if opp.profit_margin > record.max_profit_margin {
                record.max_profit_margin = opp.profit_margin;
            }
            record.update_count += 1;
            self.storage.track_update(&key, opp.profit_margin);
        } else {
            // Start new tracking with depth info
            let record = ArbitrageTrackingRecord {
                id: key.clone(),
                event_name: opp.event_name.clone(),
                team_name: opp.team_name.clone(),
                game_date: opp.game_date,
                kalshi_market_id: opp.kalshi_market_id.clone(),
                polymarket_market_id: opp.polymarket_market_id.clone(),
                start_time: Utc::now(),
                end_time: None,
                initial_profit_margin: opp.profit_margin,
                max_profit_margin: opp.profit_margin,
                kalshi_side: opp.kalshi_side.clone(),
                polymarket_side: opp.polymarket_side.clone(),
                update_count: 1,
                poly_ask_depth: opp.poly_ask_depth,
                poly_ask_size: opp.poly_ask_size,
                kalshi_ask_depth: opp.kalshi_ask_depth,
                duration_ms: 0,
                kalshi_ask_price: opp.kalshi_price,
                polymarket_ask_price: opp.polymarket_price,
            };

            info!(
                "📈 Tracking started: {} {} - {:.2}% (poly_depth: ${:.2}, poly_size: {:.0}, kalshi_depth: {})",
                opp.event_name, opp.team_name, opp.profit_margin,
                opp.poly_ask_depth, opp.poly_ask_size, opp.kalshi_ask_depth
            );

            self.storage.track_start(record.clone());
            tracking.insert(key.clone(), record);

            // One real CLOB depth snapshot per tracked opportunity, for
            // Advanced Search visibility only — never used for execution
            // sizing, and never a substitute for the fresh book fetched
            // immediately before an order in the auto-trade path. Skip it
            // if this market was snapshotted recently, so a market flapping
            // in and out of tracking can't repeatedly hit Polymarket's real
            // gateway (see POLY_DEPTH_SNAPSHOT_COOLDOWN).
            let recently_snapshotted = self
                .poly_depth_snapshot_cooldown
                .read()
                .get(&key)
                .is_some_and(|last| last.elapsed() < POLY_DEPTH_SNAPSHOT_COOLDOWN);

            if !recently_snapshotted {
                if let (Some(exec), Some(client)) = (poly_execution, self.polymarket_client.clone())
                {
                    self.poly_depth_snapshot_cooldown
                        .write()
                        .insert(key.clone(), Instant::now());
                    let storage = self.storage.clone();
                    tokio::spawn(async move {
                        match client.get_market_book(&exec.market_slug).await {
                            Ok(book) => {
                                let (usd, size) = book.buy_depth(exec.position_side);
                                storage.track_depth(&key, usd, size);
                            }
                            Err(error) => {
                                debug!("Tracking depth snapshot failed for {}: {}", key, error);
                            }
                        }
                    });
                }
            }
        }
    }

    /// End tracking if opportunity no longer exists
    pub(crate) fn maybe_end_tracking(&self, key: &str) {
        let mut tracking = self.active_tracking.write();

        if let Some(record) = tracking.remove(key) {
            info!(
                "📉 跟踪结束: {} {} - 最高 {:.2}%",
                record.event_name, record.team_name, record.max_profit_margin
            );
            self.storage.track_end(key);
            drop(tracking);
            self.clear_skip_records_for_market(key);
        }
    }

    /// Get tracking statistics
    pub fn get_tracking_stats(&self) -> serde_json::Value {
        let active = self.active_tracking.read();
        let active_records: Vec<serde_json::Value> = active
            .values()
            .map(|r| {
                serde_json::json!({
                    "event_name": r.event_name,
                    "team_name": r.team_name,
                    "kalshi_market_id": r.kalshi_market_id,
                    "polymarket_market_id": r.polymarket_market_id,
                    "start_time": r.start_time,
                    "max_profit_margin": r.max_profit_margin,
                    "duration_seconds": Utc::now().signed_duration_since(r.start_time).num_seconds(),
                })
            })
            .collect();

        let recent_completed = self.storage.get_history(20).unwrap_or_default();

        serde_json::json!({
            "active_count": active.len(),
            "completed_count": recent_completed.len(),
            "active": active_records,
            "recent_completed": recent_completed,
        })
    }

    /// Get active tracking records for auto-trade checking
    pub fn get_active_tracking_for_auto_trade(
        &self,
    ) -> Vec<(String, ArbitrageTrackingRecord, i64)> {
        let now = Utc::now();
        let tracking = self.active_tracking.read();

        tracking
            .iter()
            .map(|(key, record)| {
                let duration_ms = now
                    .signed_duration_since(record.start_time)
                    .num_milliseconds();
                (key.clone(), record.clone(), duration_ms)
            })
            .collect()
    }

    /// Get current opportunity by key
    pub fn get_opportunity_by_key(&self, key: &str) -> Option<ArbitrageOpportunity> {
        let opps = self.opportunities.read();
        opps.iter().find(|o| o.market_key() == key).cloned()
    }

    /// Get tracking record and duration by key
    pub fn get_tracking_record(&self, key: &str) -> Option<(ArbitrageTrackingRecord, i64)> {
        let now = Utc::now();
        let tracking = self.active_tracking.read();

        tracking.get(key).map(|record| {
            let duration_ms = now
                .signed_duration_since(record.start_time)
                .num_milliseconds();
            (record.clone(), duration_ms)
        })
    }
}
