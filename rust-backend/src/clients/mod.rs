//! Platform clients for Kalshi and Polymarket
//!
//! These clients handle:
//! - REST API interactions
//! - WebSocket connections for real-time price updates
//! - Authentication (RSA-PSS for Kalshi, Ed25519 for Polymarket US)

pub mod kalshi;
pub mod polymarket;
pub mod polymarket_ws;
pub mod polymarket_auth;

pub use kalshi::{KalshiClient, KalshiMarketQuote, KalshiOrderResult, OrderBook};
pub use polymarket::{
    PolymarketBookLevel, PolymarketClient, PolymarketMarketBook, PolymarketOrderResult,
};
