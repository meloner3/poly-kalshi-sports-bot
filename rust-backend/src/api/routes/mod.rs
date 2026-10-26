//! HTTP API Routes
//!
//! Organized by functional domain:
//! - health: Health check endpoints
//! - stats: Statistics and monitoring
//! - markets: Market data queries
//! - orders: Order operations
//! - accounts: Account balances and positions
//! - auto_trade: Auto-trade API
//! - settings: Application settings
//! - history: History queries and search

mod accounts;
mod auto_trade;
mod directional;
mod health;
mod history;
mod markets;
mod orders;
mod settings;
mod stats;

// Re-export all handlers
pub use accounts::*;
pub use auto_trade::*;
pub use directional::*;
pub use health::*;
pub use history::*;
pub use markets::*;
pub use orders::*;
pub use settings::*;
pub use stats::*;
