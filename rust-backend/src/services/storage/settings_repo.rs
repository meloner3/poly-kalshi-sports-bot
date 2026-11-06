//! Application settings repository
//!
//! Handles operations for the app_settings table.

use anyhow::Result;
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::ArbitrageStorage;

/// Application settings stored in database (hot-updatable from frontend)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    /// 数据刷新间隔（秒）
    pub refresh_interval: u64,
    /// 显示套利机会的最小利润率（%）
    pub min_profit_margin: f64,
    /// 套利计算使用的默认金额（美元）
    pub default_bet_amount: f64,
    /// 开始追踪记录的利润率阈值（%）
    pub tracking_threshold: f64,
    pub updated_at: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            refresh_interval: 3,
            min_profit_margin: 1.0,
            default_bet_amount: 10.0,
            tracking_threshold: 2.0,
            updated_at: None,
        }
    }
}

impl ArbitrageStorage {
    /// Get current application settings
    pub fn get_app_settings(&self) -> Result<AppSettings> {
        let conn = self.conn().lock();

        let result = conn.query_row(
            "SELECT refresh_interval, min_profit_margin, default_bet_amount, tracking_threshold, updated_at
             FROM app_settings WHERE id = 1",
            [],
            |row| {
                Ok(AppSettings {
                    refresh_interval: row.get(0)?,
                    min_profit_margin: row.get(1)?,
                    default_bet_amount: row.get(2)?,
                    tracking_threshold: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        );

        match result {
            Ok(settings) => Ok(settings),
            Err(_) => Ok(AppSettings::default()),
        }
    }

    /// Update application settings
    pub fn update_app_settings(
        &self,
        refresh_interval: Option<u64>,
        min_profit_margin: Option<f64>,
        default_bet_amount: Option<f64>,
        tracking_threshold: Option<f64>,
    ) -> Result<()> {
        let conn = self.conn().lock();

        if let Some(interval) = refresh_interval {
            conn.execute(
                "UPDATE app_settings SET refresh_interval = ?, updated_at = ? WHERE id = 1",
                params![interval as i64, Utc::now().to_rfc3339()],
            )?;
            info!("🔄 refresh_interval 已更新: {}秒", interval);
        }

        if let Some(margin) = min_profit_margin {
            conn.execute(
                "UPDATE app_settings SET min_profit_margin = ?, updated_at = ? WHERE id = 1",
                params![margin, Utc::now().to_rfc3339()],
            )?;
            info!("🔄 min_profit_margin 已更新: {}%", margin);
        }

        if let Some(amount) = default_bet_amount {
            conn.execute(
                "UPDATE app_settings SET default_bet_amount = ?, updated_at = ? WHERE id = 1",
                params![amount, Utc::now().to_rfc3339()],
            )?;
            info!("🔄 default_bet_amount 已更新: ${}", amount);
        }

        if let Some(threshold) = tracking_threshold {
            conn.execute(
                "UPDATE app_settings SET tracking_threshold = ?, updated_at = ? WHERE id = 1",
                params![threshold, Utc::now().to_rfc3339()],
            )?;
            info!("🔄 tracking_threshold 已更新: {}%", threshold);
        }

        Ok(())
    }
}
