//! Services layer
//!
//! Contains:
//! - ArbitrageService: Orchestrates market scanning and arbitrage detection
//! - WebSocketManager: Manages real-time connections to both platforms
//! - Storage: SQLite persistence for arbitrage tracking
//! - Metrics: Performance monitoring and API latency tracking
//! - Telegram: Telegram notification service for auto-trade alerts

pub mod arbitrage;
pub mod directional_execution;
pub mod mention_signal;
pub mod metrics;
pub mod paired_execution;
pub mod positions;
pub mod storage;
pub mod telegram;
pub mod websocket_manager;

pub use arbitrage::ArbitrageService;
pub use directional_execution::DirectionalExecutionService;
pub use mention_signal::MentionSignalClient;
pub use metrics::{Operation, PerformanceMetrics};
pub use paired_execution::PairedOrderParams;
pub use storage::{ArbitrageStorage, AutoTradeExecutionRecord};
pub use telegram::TelegramClient;
pub use websocket_manager::WebSocketManager;
