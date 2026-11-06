//! Auto-trade state management
//!
//! Handles auto-trade eligibility checks, market exclusion, and depth validation.

use chrono::NaiveDate;
use tracing::info;

use super::WebSocketManager;
use crate::models::generate_market_key;
use crate::services::storage::AutoTradeState;

impl WebSocketManager {
    /// Get current auto-trade state
    pub fn get_auto_trade_state(&self) -> AutoTradeState {
        self.storage.get_auto_trade_state().unwrap_or_default()
    }

    /// Enable auto-trade
    pub fn enable_auto_trade(&self) -> anyhow::Result<()> {
        self.storage.set_auto_trade_enabled(true)?;
        info!("🤖 Auto-trading enabled");
        Ok(())
    }

    /// Disable auto-trade
    pub fn disable_auto_trade(&self) -> anyhow::Result<()> {
        self.storage.set_auto_trade_enabled(false)?;
        info!("🛑 Auto-trading disabled");
        Ok(())
    }

    /// Reset trade count
    pub fn reset_trade_count(&self) -> anyhow::Result<()> {
        self.storage.reset_trade_count()?;
        self.auto_traded_opportunities.write().clear();
        info!("🔄 Trade count reset");
        Ok(())
    }

    /// Update auto-trade settings
    #[allow(clippy::too_many_arguments)]
    pub fn update_auto_trade_settings(
        &self,
        max_amount: Option<f64>,
        min_duration_ms: Option<i64>,
        max_trade_count: Option<i32>,
        flexible_mode: Option<bool>,
        max_contracts: Option<i32>,
        min_contracts: Option<i32>,
        neutralization_max_loss_cents: Option<i32>,
    ) -> anyhow::Result<()> {
        self.storage.update_auto_trade_settings(
            max_amount,
            min_duration_ms,
            max_trade_count,
            flexible_mode,
            max_contracts,
            min_contracts,
            neutralization_max_loss_cents,
        )?;
        Ok(())
    }

    /// Update application settings (hot-updatable)
    pub fn update_app_settings(
        &self,
        refresh_interval: Option<u64>,
        min_profit_margin: Option<f64>,
        default_bet_amount: Option<f64>,
        tracking_threshold: Option<f64>,
    ) -> anyhow::Result<()> {
        self.storage.update_app_settings(
            refresh_interval,
            min_profit_margin,
            default_bet_amount,
            tracking_threshold,
        )?;

        info!("📝 Application settings updated in the database");
        Ok(())
    }

    /// Check if an opportunity is eligible for auto-trade
    pub fn check_auto_trade_eligibility(&self, key: &str, duration_ms: i64) -> (bool, String) {
        let state = self.get_auto_trade_state();

        if !state.enabled {
            return (false, "Auto-trading is disabled".to_string());
        }

        let normalized_key = key.to_uppercase();
        if self.excluded_markets.read().contains(&normalized_key) {
            return (false, "This market has been excluded".to_string());
        }

        if state.trade_count >= state.max_trade_count {
            return (
                false,
                format!(
                    "Maximum trade count reached ({}/{})",
                    state.trade_count, state.max_trade_count
                ),
            );
        }

        if duration_ms < state.min_duration_ms {
            return (
                false,
                format!(
                    "Duration too short ({}ms < {}ms)",
                    duration_ms, state.min_duration_ms
                ),
            );
        }

        let market_idx = self
            .matched_markets
            .read()
            .iter()
            .position(|market| market.market_key() == key);
        let Some(market_idx) = market_idx else {
            return (false, "Matched market is no longer available".to_string());
        };
        if !self.is_market_ready(market_idx) {
            return (
                false,
                "Market data is incomplete or Polymarket REST quote is stale".to_string(),
            );
        }

        if self.auto_traded_opportunities.read().contains(key) {
            return (
                false,
                "This opportunity has already been auto-traded".to_string(),
            );
        }

        (
            true,
            format!(
                "Eligible for trade ({}/{})",
                state.trade_count + 1,
                state.max_trade_count
            ),
        )
    }

    /// Whether a market key has been excluded from automated/paired trading.
    pub fn is_market_excluded(&self, key: &str) -> bool {
        self.excluded_markets.read().contains(&key.to_uppercase())
    }

    /// Load excluded markets from database on startup
    pub fn load_excluded_markets(&self) {
        match self.storage.get_excluded_markets() {
            Ok(markets) => {
                let count = markets.len();
                *self.excluded_markets.write() = markets;
                if count > 0 {
                    info!("📋 Loaded {} excluded markets from the database", count);
                }
            }
            Err(e) => {
                tracing::error!("Failed to load excluded markets list: {:#}", e);
            }
        }
    }

    /// Exclude a market from auto-trade
    pub fn exclude_market(
        &self,
        event_name: &str,
        team_name: &str,
        game_date: Option<NaiveDate>,
    ) -> bool {
        let key = generate_market_key(event_name, game_date, team_name);

        match self
            .storage
            .exclude_market(event_name, team_name, game_date)
        {
            Ok(inserted) => {
                if inserted {
                    self.excluded_markets.write().insert(key);
                }
                inserted
            }
            Err(e) => {
                tracing::error!("Failed to save excluded market to the database: {:#}", e);
                false
            }
        }
    }

    /// Remove a market from exclusion list
    pub fn unexclude_market(
        &self,
        event_name: &str,
        team_name: &str,
        game_date: Option<NaiveDate>,
    ) -> bool {
        let key = generate_market_key(event_name, game_date, team_name);

        match self
            .storage
            .unexclude_market(event_name, team_name, game_date)
        {
            Ok(removed) => {
                if removed {
                    self.excluded_markets.write().remove(&key);
                }
                removed
            }
            Err(e) => {
                tracing::error!("Failed to remove excluded market from the database: {:#}", e);
                false
            }
        }
    }

    /// Get list of excluded markets
    pub fn get_excluded_markets(&self) -> Vec<String> {
        self.excluded_markets.read().iter().cloned().collect()
    }

    /// Mark an opportunity as auto-traded
    pub fn mark_as_auto_traded(&self, key: &str) {
        self.auto_traded_opportunities
            .write()
            .insert(key.to_string());
        self.clear_skip_records_for_market(key);
    }

    /// Check if a skip reason should be recorded (deduplication)
    pub fn should_record_skip(&self, market_key: &str, skip_reason: &str) -> bool {
        let simplified_reason = if skip_reason.contains("Polymarket US executable liquidity") {
            "poly_us_executable_liquidity_unavailable"
        } else if skip_reason.contains("Kalshi depth insufficient") {
            "kalshi_depth"
        } else if skip_reason.contains("exceeds limit") {
            "over_limit"
        } else if skip_reason.contains("unable to retrieve") {
            "token_not_found"
        } else if skip_reason.contains("combined price exceeds") {
            "price_sum_invalid"
        } else {
            "other"
        };

        let record_key = format!("{}:{}", market_key, simplified_reason);
        let mut recorded = self.recorded_skip_reasons.write();

        if recorded.contains(&record_key) {
            false
        } else {
            recorded.insert(record_key);
            true
        }
    }

    /// Clear skip records for a specific market
    pub fn clear_skip_records_for_market(&self, market_key: &str) {
        let mut recorded = self.recorded_skip_reasons.write();
        let prefix = format!("{}:", market_key);
        recorded.retain(|k| !k.starts_with(&prefix));
    }

    /// Increment trade count after successful auto-trade
    pub fn increment_trade_count(&self) -> anyhow::Result<i32> {
        self.storage.increment_trade_count()
    }
}
