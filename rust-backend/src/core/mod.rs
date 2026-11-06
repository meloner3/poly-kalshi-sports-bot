//! Core business logic
//!
//! Contains the arbitrage calculator, market matcher, and team normalization.

pub mod calculator;
pub mod competitors;
pub mod matcher;
pub mod nba_teams;

pub use calculator::ArbitrageCalculator;
pub use competitors::{competitor_event_name, normalize_competitor_name};
pub use matcher::{EventMatcher, SubscriptionInfo};
pub use nba_teams::normalize_team_name;
