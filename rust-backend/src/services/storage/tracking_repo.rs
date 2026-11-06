//! Arbitrage tracking repository
//!
//! Handles all operations for the arbitrage_tracking table.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::params;

use super::{ArbitrageStorage, StorageCommand};
use crate::models::ArbitrageTrackingRecord;

impl ArbitrageStorage {
    /// Start tracking an arbitrage opportunity
    pub fn track_start(&self, record: ArbitrageTrackingRecord) {
        let _ = self
            .command_tx()
            .try_send(StorageCommand::TrackStart(record));
    }

    /// Update tracking with new profit margin
    pub fn track_update(&self, id: &str, profit_margin: f64) {
        let _ = self.command_tx().try_send(StorageCommand::TrackUpdate {
            id: id.to_string(),
            profit_margin,
        });
    }

    /// Record real Polymarket CLOB depth observed for a tracked opportunity.
    /// Fetched once from the live order book when tracking starts; this is
    /// reporting only and never substitutes for the fresh execution-time check.
    pub fn track_depth(&self, id: &str, poly_ask_depth: f64, poly_ask_size: f64) {
        let _ = self.command_tx().try_send(StorageCommand::TrackDepth {
            id: id.to_string(),
            poly_ask_depth,
            poly_ask_size,
        });
    }

    /// End tracking for an opportunity
    pub fn track_end(&self, id: &str) {
        let _ = self
            .command_tx()
            .try_send(StorageCommand::TrackEnd(id.to_string()));
    }

    /// Get recent arbitrage history
    pub fn get_history(&self, limit: usize) -> Result<Vec<ArbitrageTrackingRecord>> {
        let conn = self.conn().lock();
        let mut stmt = conn.prepare(
            "SELECT id, event_name, team_name, kalshi_market_id, polymarket_market_id,
                    start_time, end_time, initial_profit_margin, max_profit_margin,
                    kalshi_side, polymarket_side, update_count,
                    poly_ask_depth, poly_ask_size, kalshi_ask_depth, duration_ms,
                    kalshi_ask_price, polymarket_ask_price
             FROM arbitrage_tracking
             ORDER BY start_time DESC
             LIMIT ?1",
        )?;

        let records = stmt
            .query_map([limit], |row| {
                Ok(ArbitrageTrackingRecord {
                    id: row.get(0)?,
                    event_name: row.get(1)?,
                    team_name: row.get(2)?,
                    game_date: None, // Not stored in old records
                    kalshi_market_id: row.get(3)?,
                    polymarket_market_id: row.get(4)?,
                    start_time: DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    end_time: row
                        .get::<_, Option<String>>(6)?
                        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                        .map(|dt| dt.with_timezone(&Utc)),
                    initial_profit_margin: row.get(7)?,
                    max_profit_margin: row.get(8)?,
                    kalshi_side: row.get(9)?,
                    polymarket_side: row.get(10)?,
                    update_count: row.get(11)?,
                    poly_ask_depth: row.get(12).unwrap_or(0.0),
                    poly_ask_size: row.get(13).unwrap_or(0.0),
                    kalshi_ask_depth: row.get(14).unwrap_or(0),
                    duration_ms: row.get(15).unwrap_or(0),
                    kalshi_ask_price: row.get(16).unwrap_or(0.0),
                    polymarket_ask_price: row.get(17).unwrap_or(0.0),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Search records with filters
    #[allow(clippy::too_many_arguments)]
    pub fn search_records(
        &self,
        min_profit: Option<f64>,
        max_profit: Option<f64>,
        _min_duration: Option<f64>,
        _max_duration: Option<f64>,
        event_name: Option<String>,
        team_name: Option<String>,
        sort_by: Option<String>,
        sort_order: Option<String>,
        limit: Option<usize>,
        offset: Option<usize>,
        _include_history: Option<bool>,
    ) -> Result<serde_json::Value> {
        let conn = self.conn().lock();

        // Build WHERE clause
        let mut where_clauses = vec!["end_time IS NOT NULL"];
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(min) = min_profit {
            where_clauses.push("max_profit_margin >= ?");
            params.push(Box::new(min));
        }
        if let Some(max) = max_profit {
            where_clauses.push("max_profit_margin <= ?");
            params.push(Box::new(max));
        }
        if let Some(name) = event_name {
            where_clauses.push("event_name LIKE ?");
            params.push(Box::new(format!("%{}%", name)));
        }
        if let Some(team) = team_name {
            where_clauses.push("team_name LIKE ?");
            params.push(Box::new(format!("%{}%", team)));
        }

        let where_clause = where_clauses.join(" AND ");

        // Build ORDER BY clause
        let sort_field = sort_by.unwrap_or_else(|| "start_time".to_string());
        let sort_dir = sort_order.unwrap_or_else(|| "desc".to_string());
        let order_clause = format!("{} {}", sort_field, sort_dir);

        // Count total matching records
        let count_query = format!(
            "SELECT COUNT(*) FROM arbitrage_tracking WHERE {}",
            where_clause
        );
        let total: i64 = {
            let mut stmt = conn.prepare(&count_query)?;
            let params_refs: Vec<&dyn rusqlite::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();
            stmt.query_row(&params_refs[..], |row| row.get(0))?
        };

        // Fetch records
        let limit_val = limit.unwrap_or(50);
        let offset_val = offset.unwrap_or(0);

        let query = format!(
            "SELECT id, event_name, team_name, kalshi_market_id, polymarket_market_id,
                    start_time, end_time, initial_profit_margin, max_profit_margin,
                    kalshi_side, polymarket_side, update_count,
                    poly_ask_depth, poly_ask_size, kalshi_ask_depth, duration_ms,
                    kalshi_ask_price, polymarket_ask_price
             FROM arbitrage_tracking
             WHERE {}
             ORDER BY {}
             LIMIT ? OFFSET ?",
            where_clause, order_clause
        );

        let mut stmt = conn.prepare(&query)?;
        params.push(Box::new(limit_val as i64));
        params.push(Box::new(offset_val as i64));
        let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let records: Vec<serde_json::Value> = stmt
            .query_map(&params_refs[..], |row| {
                let start_time: String = row.get(5)?;
                let end_time: Option<String> = row.get(6)?;
                let poly_depth: f64 = row.get(12).unwrap_or(0.0);
                let poly_size: f64 = row.get(13).unwrap_or(0.0);
                let kalshi_depth: i32 = row.get(14).unwrap_or(0);
                let duration_ms: i64 = row.get(15).unwrap_or(0);
                let kalshi_ask_price: f64 = row.get(16).unwrap_or(0.0);
                let polymarket_ask_price: f64 = row.get(17).unwrap_or(0.0);

                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "event_name": row.get::<_, String>(1)?,
                    "team_name": row.get::<_, String>(2)?,
                    "kalshi_market_id": row.get::<_, String>(3)?,
                    "polymarket_market_id": row.get::<_, String>(4)?,
                    "kalshi_side": row.get::<_, String>(9)?,
                    "polymarket_side": row.get::<_, String>(10)?,
                    "start_time": start_time,
                    "end_time": end_time,
                    "duration_ms": duration_ms,
                    "duration_seconds": duration_ms / 1000,
                    "max_profit_margin": row.get::<_, f64>(8)?,
                    "poly_ask_depth": poly_depth,
                    "poly_ask_size": poly_size,
                    "kalshi_ask_depth": kalshi_depth,
                    "kalshi_ask_price": kalshi_ask_price,
                    "polymarket_ask_price": polymarket_ask_price,
                }))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(serde_json::json!({
            "records": records,
            "total": total,
            "limit": limit_val,
            "offset": offset_val,
            "has_more": (offset_val + records.len()) < total as usize,
        }))
    }

    /// Get comprehensive statistics
    pub fn get_statistics(&self) -> Result<serde_json::Value> {
        let conn = self.conn().lock();

        // Basic stats
        let total_records: i64 = conn.query_row(
            "SELECT COUNT(*) FROM arbitrage_tracking WHERE end_time IS NOT NULL",
            [],
            |row| row.get(0),
        )?;

        if total_records == 0 {
            return Ok(serde_json::json!({
                "total_records": 0,
                "avg_profit": 0.0,
                "max_profit": 0.0,
                "min_profit": 0.0,
                "avg_duration": 0.0,
                "max_duration": 0.0,
                "min_duration": 0.0,
                "top_events": [],
                "top_teams": [],
                "profit_distribution": [],
            }));
        }

        // Profit stats
        let (avg_profit, max_profit, min_profit): (f64, f64, f64) = conn.query_row(
            "SELECT AVG(max_profit_margin), MAX(max_profit_margin), MIN(max_profit_margin) 
             FROM arbitrage_tracking WHERE end_time IS NOT NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;

        // Duration stats (calculate from timestamps)
        let mut duration_stmt = conn.prepare(
            "SELECT start_time, end_time FROM arbitrage_tracking WHERE end_time IS NOT NULL",
        )?;

        let durations: Vec<i64> = duration_stmt
            .query_map([], |row| {
                let start: String = row.get(0)?;
                let end: String = row.get(1)?;
                let start_dt = DateTime::parse_from_rfc3339(&start)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                let end_dt = DateTime::parse_from_rfc3339(&end)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                Ok(end_dt.signed_duration_since(start_dt).num_seconds())
            })?
            .filter_map(|r| r.ok())
            .collect();

        let avg_duration = if !durations.is_empty() {
            durations.iter().sum::<i64>() as f64 / durations.len() as f64
        } else {
            0.0
        };
        let max_duration = durations.iter().max().copied().unwrap_or(0) as f64;
        let min_duration = durations.iter().min().copied().unwrap_or(0) as f64;

        // Top events
        let mut event_stmt = conn.prepare(
            "SELECT event_name, COUNT(*) as count, AVG(max_profit_margin) as avg_profit
             FROM arbitrage_tracking WHERE end_time IS NOT NULL
             GROUP BY event_name ORDER BY count DESC LIMIT 10",
        )?;
        let top_events: Vec<serde_json::Value> = event_stmt
            .query_map([], |row| {
                Ok(serde_json::json!({
                    "event_name": row.get::<_, String>(0)?,
                    "count": row.get::<_, i64>(1)?,
                    "avg_profit": row.get::<_, f64>(2)?,
                }))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        // Top teams
        let mut team_stmt = conn.prepare(
            "SELECT team_name, COUNT(*) as count, AVG(max_profit_margin) as avg_profit
             FROM arbitrage_tracking WHERE end_time IS NOT NULL
             GROUP BY team_name ORDER BY count DESC LIMIT 10",
        )?;
        let top_teams: Vec<serde_json::Value> = team_stmt
            .query_map([], |row| {
                Ok(serde_json::json!({
                    "team_name": row.get::<_, String>(0)?,
                    "count": row.get::<_, i64>(1)?,
                    "avg_profit": row.get::<_, f64>(2)?,
                }))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        // Profit distribution
        let profit_ranges = vec![
            ("0-2%", 0.0, 2.0),
            ("2-5%", 2.0, 5.0),
            ("5-10%", 5.0, 10.0),
            ("10-20%", 10.0, 20.0),
            ("20%+", 20.0, 1000.0),
        ];

        let mut profit_distribution = Vec::new();
        for (label, min, max) in profit_ranges {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM arbitrage_tracking 
                 WHERE end_time IS NOT NULL AND max_profit_margin >= ? AND max_profit_margin < ?",
                params![min, max],
                |row| row.get(0),
            )?;
            profit_distribution.push(serde_json::json!({
                "range": label,
                "count": count,
            }));
        }

        Ok(serde_json::json!({
            "total_records": total_records,
            "avg_profit": avg_profit,
            "max_profit": max_profit,
            "min_profit": min_profit,
            "avg_duration": avg_duration,
            "max_duration": max_duration,
            "min_duration": min_duration,
            "top_events": top_events,
            "top_teams": top_teams,
            "profit_distribution": profit_distribution,
        }))
    }
}
