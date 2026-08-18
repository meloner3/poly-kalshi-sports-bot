//! Core business logic
//!
//! Contains the arbitrage calculator, market matcher, and team normalization.

pub mod calculator;
pub mod matcher;
pub mod nba_teams;
pub mod tennis_players;

pub use calculator::ArbitrageCalculator;
pub use matcher::{EventMatcher, SubscriptionInfo};
pub use nba_teams::normalize_team_name;
pub use tennis_players::{normalize_player_name, ticker_code_matches};