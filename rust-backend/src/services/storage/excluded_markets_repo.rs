//! Excluded markets repository
//!
//! Handles operations for the excluded_markets table.

use anyhow::Result;
use chrono::{NaiveDate, Utc};
use rusqlite::params;
use std::collections::HashSet;
use tracing::info;

use super::ArbitrageStorage;
use crate::models::generate_market_key;

impl ArbitrageStorage {
    /// Add a market to exclusion list
    /// Uses generate_market_key to create consistent key format including date
    pub fn exclude_market(
        &self,
        event_name: &str,
        team_name: &str,
        game_date: Option<NaiveDate>,
    ) -> Result<bool> {
        let conn = self.conn().lock();

        // Use the unified key generator
        let market_key = generate_market_key(event_name, game_date, team_name);
        let game_date_str = game_date.map(|d| d.format("%Y-%m-%d").to_string());

        let result = conn.execute(
            "INSERT OR IGNORE INTO excluded_markets (event_name, team_name, game_date, market_key, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event_name.to_uppercase(),
                team_name.to_uppercase(),
                game_date_str,
                market_key,
                Utc::now().to_rfc3339()
            ],
        )?;

        if result > 0 {
            info!("🚫 市场已排除并保存到数据库: {}", market_key);
        }

        Ok(result > 0)
    }

    /// Remove a market from exclusion list
    /// Uses generate_market_key to create consistent key format including date
    pub fn unexclude_market(
        &self,
        event_name: &str,
        team_name: &str,
        game_date: Option<NaiveDate>,
    ) -> Result<bool> {
        let conn = self.conn().lock();

        // Use the unified key generator
        let market_key = generate_market_key(event_name, game_date, team_name);

        let result = conn.execute(
            "DELETE FROM excluded_markets WHERE market_key = ?1",
            params![market_key],
        )?;

        if result > 0 {
            info!("✅ 市场已取消排除: {}", market_key);
        }

        Ok(result > 0)
    }

    /// Get all excluded market keys
    pub fn get_excluded_markets(&self) -> Result<HashSet<String>> {
        let conn = self.conn().lock();

        let mut stmt = conn.prepare("SELECT market_key FROM excluded_markets")?;

        let keys = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(keys)
    }
}
