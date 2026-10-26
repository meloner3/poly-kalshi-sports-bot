//! SQLite storage for arbitrage tracking and auto-trade state
//!
//! Provides async queue-based writing for non-blocking persistence.
//!
//! This module is organized into separate repositories:
//! - `tracking_repo`: Arbitrage tracking records
//! - `auto_trade_repo`: Auto-trade state and history
//! - `settings_repo`: Application settings
//! - `excluded_markets_repo`: Excluded markets for auto-trade

mod auto_trade_repo;
mod directional_repo;
mod excluded_markets_repo;
mod settings_repo;
mod tracking_repo;

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::models::ArbitrageTrackingRecord;

// Re-export types from sub-modules
pub use auto_trade_repo::{AutoTradeExecutionRecord, AutoTradeRecord, AutoTradeState};
pub use directional_repo::{DirectionalPosition, NewDirectionalDecision, NewDirectionalPosition};

/// Storage command for async queue
pub enum StorageCommand {
    TrackStart(ArbitrageTrackingRecord),
    TrackUpdate {
        id: String,
        profit_margin: f64,
    },
    TrackDepth {
        id: String,
        poly_ask_depth: f64,
        poly_ask_size: f64,
    },
    TrackEnd(String),
}

/// Arbitrage storage service
pub struct ArbitrageStorage {
    conn: Arc<Mutex<Connection>>,
    command_tx: mpsc::Sender<StorageCommand>,
}

impl ArbitrageStorage {
    /// Create a new storage instance
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self> {
        let conn = Connection::open(&db_path)
            .with_context(|| format!("Failed to open database: {:?}", db_path.as_ref()))?;

        // Initialize all tables
        Self::init_tracking_table(&conn)?;
        Self::init_auto_trade_tables(&conn)?;
        Self::init_settings_table(&conn)?;
        Self::init_excluded_markets_table(&conn)?;
        Self::init_directional_tables(&conn)?;

        info!("📦 Database initialized with auto_trade_state, auto_trade_history, app_settings, and excluded_markets tables");

        let conn = Arc::new(Mutex::new(conn));

        // Create command channel
        let (command_tx, mut command_rx) = mpsc::channel::<StorageCommand>(1000);

        // Spawn background writer
        let conn_clone = conn.clone();
        tokio::spawn(async move {
            while let Some(cmd) = command_rx.recv().await {
                let conn = conn_clone.lock();
                if let Err(e) = Self::execute_command(&conn, cmd) {
                    error!("Storage error: {:#}", e);
                }
            }
        });

        Ok(Self { conn, command_tx })
    }

    /// Initialize arbitrage_tracking table
    fn init_tracking_table(conn: &Connection) -> Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS arbitrage_tracking (
                id TEXT PRIMARY KEY,
                event_name TEXT NOT NULL,
                team_name TEXT NOT NULL,
                kalshi_market_id TEXT NOT NULL,
                polymarket_market_id TEXT NOT NULL,
                start_time TEXT NOT NULL,
                end_time TEXT,
                initial_profit_margin REAL NOT NULL,
                max_profit_margin REAL NOT NULL,
                kalshi_side TEXT NOT NULL,
                polymarket_side TEXT NOT NULL,
                update_count INTEGER DEFAULT 0,
                poly_ask_depth REAL DEFAULT 0,
                poly_ask_size REAL DEFAULT 0,
                kalshi_ask_depth INTEGER DEFAULT 0,
                duration_ms INTEGER DEFAULT 0,
                kalshi_ask_price REAL DEFAULT 0,
                polymarket_ask_price REAL DEFAULT 0
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_start_time ON arbitrage_tracking(start_time)",
            [],
        )?;

        // Migrate existing tables: add new columns if they don't exist
        let _ = conn.execute(
            "ALTER TABLE arbitrage_tracking ADD COLUMN poly_ask_depth REAL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE arbitrage_tracking ADD COLUMN poly_ask_size REAL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE arbitrage_tracking ADD COLUMN kalshi_ask_depth INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE arbitrage_tracking ADD COLUMN duration_ms INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE arbitrage_tracking ADD COLUMN kalshi_ask_price REAL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE arbitrage_tracking ADD COLUMN polymarket_ask_price REAL DEFAULT 0",
            [],
        );

        Ok(())
    }

    /// Initialize auto_trade_state and auto_trade_history tables
    fn init_auto_trade_tables(conn: &Connection) -> Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS auto_trade_state (
                id INTEGER PRIMARY KEY DEFAULT 1,
                enabled INTEGER DEFAULT 0,
                trade_count INTEGER DEFAULT 0,
                max_trade_count INTEGER DEFAULT 2,
                max_amount REAL DEFAULT 10.0,
                min_duration_ms INTEGER DEFAULT 500,
                last_trade_time TEXT,
                updated_at TEXT DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        // Migrate auto_trade_state: add new columns if they don't exist
        let _ = conn.execute(
            "ALTER TABLE auto_trade_state ADD COLUMN min_duration_ms INTEGER DEFAULT 500",
            [],
        );
        // Added: flexible auto-trade mode fields
        let _ = conn.execute(
            "ALTER TABLE auto_trade_state ADD COLUMN flexible_mode INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_state ADD COLUMN max_contracts INTEGER DEFAULT 100",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_state ADD COLUMN min_contracts INTEGER DEFAULT 10",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_state ADD COLUMN neutralization_max_loss_cents INTEGER DEFAULT 5",
            [],
        );

        // Initialize auto_trade_state with default row if not exists
        conn.execute(
            "INSERT OR IGNORE INTO auto_trade_state (id, enabled, trade_count, max_trade_count, max_amount, min_duration_ms, flexible_mode, max_contracts, min_contracts) 
             VALUES (1, 0, 0, 2, 10.0, 500, 0, 100, 10)",
            [],
        )?;

        // Create auto_trade_history table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS auto_trade_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_name TEXT NOT NULL,
                team_name TEXT NOT NULL,
                kalshi_market_id TEXT NOT NULL,
                polymarket_market_id TEXT NOT NULL,
                kalshi_side TEXT NOT NULL,
                polymarket_side TEXT NOT NULL,
                kalshi_contracts INTEGER NOT NULL,
                kalshi_price REAL NOT NULL,
                kalshi_fee REAL DEFAULT 0,
                polymarket_amount REAL NOT NULL,
                polymarket_price REAL NOT NULL,
                total_amount REAL NOT NULL,
                profit_margin REAL NOT NULL,
                duration_ms INTEGER NOT NULL,
                total_duration_ms INTEGER DEFAULT 0,
                kalshi_success INTEGER NOT NULL,
                polymarket_success INTEGER NOT NULL,
                kalshi_order_id TEXT,
                polymarket_order_id TEXT,
                kalshi_error TEXT,
                polymarket_error TEXT,
                status TEXT DEFAULT 'executed',
                skip_reason TEXT,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        // Migrate existing auto_trade_history table
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN kalshi_fee REAL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN status TEXT DEFAULT 'executed'",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN skip_reason TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN total_duration_ms INTEGER DEFAULT 0",
            [],
        );
        // Added: record API latency separately for each platform
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN kalshi_latency_ms INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN poly_latency_ms INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN kalshi_filled_contracts INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN polymarket_filled_contracts INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN kalshi_order_status TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN polymarket_order_status TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN neutralization_leg TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN neutralization_success INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN neutralization_order_id TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN neutralization_filled_contracts INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN neutralization_error TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN residual_leg TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE auto_trade_history ADD COLUMN residual_contracts INTEGER DEFAULT 0",
            [],
        );

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_auto_trade_created_at ON auto_trade_history(created_at)",
            [],
        )?;

        Ok(())
    }

    /// Initialize app_settings table
    fn init_settings_table(conn: &Connection) -> Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS app_settings (
                id INTEGER PRIMARY KEY DEFAULT 1,
                refresh_interval INTEGER DEFAULT 5,
                min_profit_margin REAL DEFAULT 1.0,
                default_bet_amount REAL DEFAULT 10.0,
                tracking_threshold REAL DEFAULT 2.0,
                updated_at TEXT DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        // Initialize app_settings with default row if not exists
        conn.execute(
            "INSERT OR IGNORE INTO app_settings (id, refresh_interval, min_profit_margin, default_bet_amount, tracking_threshold)
             VALUES (1, 5, 1.0, 10.0, 2.0)",
            [],
        )?;

        Ok(())
    }

    /// Initialize excluded_markets table
    fn init_excluded_markets_table(conn: &Connection) -> Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS excluded_markets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_name TEXT NOT NULL,
                team_name TEXT NOT NULL,
                game_date TEXT,
                market_key TEXT NOT NULL UNIQUE,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        // Migrate existing table: add game_date column if it doesn't exist
        let _ = conn.execute("ALTER TABLE excluded_markets ADD COLUMN game_date TEXT", []);

        Ok(())
    }

    /// Execute a storage command
    fn execute_command(conn: &Connection, cmd: StorageCommand) -> Result<()> {
        match cmd {
            StorageCommand::TrackStart(record) => {
                conn.execute(
                    "INSERT OR REPLACE INTO arbitrage_tracking 
                    (id, event_name, team_name, kalshi_market_id, polymarket_market_id, 
                     start_time, initial_profit_margin, max_profit_margin, 
                     kalshi_side, polymarket_side, update_count,
                     poly_ask_depth, poly_ask_size, kalshi_ask_depth, duration_ms,
                     kalshi_ask_price, polymarket_ask_price)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                    params![
                        record.id,
                        record.event_name,
                        record.team_name,
                        record.kalshi_market_id,
                        record.polymarket_market_id,
                        record.start_time.to_rfc3339(),
                        record.initial_profit_margin,
                        record.max_profit_margin,
                        record.kalshi_side,
                        record.polymarket_side,
                        record.update_count,
                        record.poly_ask_depth,
                        record.poly_ask_size,
                        record.kalshi_ask_depth,
                        record.duration_ms,
                        record.kalshi_ask_price,
                        record.polymarket_ask_price,
                    ],
                )?;
            }
            StorageCommand::TrackUpdate { id, profit_margin } => {
                conn.execute(
                    "UPDATE arbitrage_tracking 
                    SET max_profit_margin = MAX(max_profit_margin, ?1),
                        update_count = update_count + 1
                    WHERE id = ?2",
                    params![profit_margin, id],
                )?;
            }
            StorageCommand::TrackDepth {
                id,
                poly_ask_depth,
                poly_ask_size,
            } => {
                conn.execute(
                    "UPDATE arbitrage_tracking
                    SET poly_ask_depth = ?1,
                        poly_ask_size = ?2
                    WHERE id = ?3",
                    params![poly_ask_depth, poly_ask_size, id],
                )?;
            }
            StorageCommand::TrackEnd(id) => {
                let now = Utc::now();
                // Calculate duration_ms from start_time
                let duration_ms: i64 = conn
                    .query_row(
                        "SELECT start_time FROM arbitrage_tracking WHERE id = ?1",
                        params![&id],
                        |row| {
                            let start_str: String = row.get(0)?;
                            let start_dt = DateTime::parse_from_rfc3339(&start_str)
                                .map(|dt| dt.with_timezone(&Utc))
                                .unwrap_or(now);
                            Ok(now.signed_duration_since(start_dt).num_milliseconds())
                        },
                    )
                    .unwrap_or(0);

                conn.execute(
                    "UPDATE arbitrage_tracking SET end_time = ?1, duration_ms = ?2 WHERE id = ?3",
                    params![now.to_rfc3339(), duration_ms, id],
                )?;
            }
        }
        Ok(())
    }

    /// Get a reference to the connection (for repository methods)
    pub(crate) fn conn(&self) -> &Arc<Mutex<Connection>> {
        &self.conn
    }

    /// Get the command sender (for async tracking operations)
    pub(crate) fn command_tx(&self) -> &mpsc::Sender<StorageCommand> {
        &self.command_tx
    }
}
