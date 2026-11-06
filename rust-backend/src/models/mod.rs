//! Data models
//!
//! Core concepts:
//! - Kalshi: One event has 2 independent markets, each for one competitor's Yes/No
//! - Polymarket: One event has 1 market with 2 competitor outcomes
//!
//! Matching relationship:
//! - Both Kalshi markets point to the same Poly market
//! - Competitor name determines which Poly price perspective to use

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::services::metrics::MetricsReport;

/// Generate a unified market key including date for accurate identification
/// Format: EVENT-NAME_YYYY-MM-DD_TEAM-NAME (e.g., PHI-TOR_2026-01-11_PHI)
pub fn generate_market_key(
    event_name: &str,
    game_date: Option<NaiveDate>,
    team_name: &str,
) -> String {
    match game_date {
        Some(date) => format!(
            "{}_{}_{}",
            event_name.to_uppercase(),
            date.format("%Y-%m-%d"),
            team_name.to_uppercase()
        ),
        None => format!("{}_{}", event_name.to_uppercase(), team_name.to_uppercase()),
    }
}

/// Platform enum
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Kalshi,
    Polymarket,
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Platform::Kalshi => write!(f, "kalshi"),
            Platform::Polymarket => write!(f, "polymarket"),
        }
    }
}

/// Kalshi market model
///
/// Each Kalshi market is independent, predicting a single team's win/lose
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KalshiMarket {
    /// Market ticker (e.g., KXNBAGAME-26JAN04MEMLAL-MEM)
    pub market_id: String,
    /// Event ID this market belongs to
    pub event_id: String,
    /// Standardized event name (e.g., MEM-LAL or PLAYER A VS PLAYER B)
    pub event_name: String,
    /// Competitor this market predicts (e.g., MEM or a canonical tennis player name)
    pub team_name: String,
    /// Opposing competitor (e.g., LAL or a canonical tennis player name)
    pub opponent_name: String,
    /// Yes price (this team wins)
    pub yes_price: f64,
    /// No price (this team doesn't win)
    pub no_price: f64,
    /// Game start time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<DateTime<Utc>>,
    /// Trading volume
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
    /// Liquidity
    #[serde(skip_serializing_if = "Option::is_none")]
    pub liquidity: Option<f64>,
}

/// The direction of a Polymarket US position for a competitor.
///
/// This is supplied by the Polymarket US gateway's `marketSides[].long`
/// field. It is deliberately independent of an outcome's array index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolymarketPositionSide {
    Long,
    Short,
}

impl PolymarketPositionSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Long => "long",
            Self::Short => "short",
        }
    }
}

/// Native Polymarket US execution details for one named competitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketUsExecution {
    pub market_slug: String,
    pub position_side: PolymarketPositionSide,
}

/// Polymarket US market model.
///
/// A market contains two competitors. Execution uses the native market slug
/// and the gateway-provided long/short position for the selected competitor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolymarketMarket {
    /// Gateway market ID, retained only as a stable data-refresh key.
    pub market_id: String,
    /// Native Polymarket US `marketSlug` required to submit an order.
    pub market_slug: String,
    /// Standardized event name (e.g., MEM-LAL)
    pub event_name: String,
    /// Competitor A (alphabetically first)
    pub team_a: String,
    /// Competitor B (alphabetically second)
    pub team_b: String,
    /// Competitor A win price
    pub price_a: f64,
    /// Competitor B win price
    pub price_b: f64,
    /// Native US order direction for competitor A.
    pub team_a_position: PolymarketPositionSide,
    /// Native US order direction for competitor B.
    pub team_b_position: PolymarketPositionSide,
    /// Game start time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<DateTime<Utc>>,
    /// Trading volume
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
}

impl PolymarketMarket {
    /// Get Yes/No prices for a given competitor
    ///
    /// Returns: (yes_price, no_price) for that team
    pub fn get_price_for_team(&self, team: &str) -> Result<(f64, f64), String> {
        let team_upper = team.to_uppercase();
        if team_upper == self.team_a.to_uppercase() {
            // yes = A wins, no = B wins
            Ok((self.price_a, self.price_b))
        } else if team_upper == self.team_b.to_uppercase() {
            // yes = B wins, no = A wins
            Ok((self.price_b, self.price_a))
        } else {
            Err(format!("Team {} not in market {}", team, self.event_name))
        }
    }

    /// Return the native Polymarket US execution details for a competitor.
    ///
    /// The caller must use this mapping rather than infer a position from the
    /// outcome order, displayed yes/no price, condition ID, or CLOB token.
    pub fn us_execution_for_competitor(&self, team: &str) -> Option<PolymarketUsExecution> {
        let team_key = competitor_lookup_key(team);
        if team_key == competitor_lookup_key(&self.team_a) {
            Some(PolymarketUsExecution {
                market_slug: self.market_slug.clone(),
                position_side: self.team_a_position,
            })
        } else if team_key == competitor_lookup_key(&self.team_b) {
            Some(PolymarketUsExecution {
                market_slug: self.market_slug.clone(),
                position_side: self.team_b_position,
            })
        } else {
            None
        }
    }

    /// Get opponent competitor name
    pub fn get_opponent(&self, team: &str) -> Option<&str> {
        let team_upper = team.to_uppercase();
        if team_upper == self.team_a.to_uppercase() {
            Some(&self.team_b)
        } else if team_upper == self.team_b.to_uppercase() {
            Some(&self.team_a)
        } else {
            None
        }
    }
}

/// Match a displayed competitor spelling to the canonical market-side name
/// without deriving position direction from outcome ordering.
fn competitor_lookup_key(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Kalshi event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KalshiEvent {
    pub event_id: String,
    /// Standardized event name (e.g., MEM-LAL)
    pub name: String,
    pub team_a: String,
    pub team_b: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<DateTime<Utc>>,
    #[serde(default = "default_category")]
    pub category: String,
    /// Markets in this event
    #[serde(default)]
    pub markets: Vec<KalshiMarket>,
}

fn default_category() -> String {
    "NBA".to_string()
}

/// Polymarket event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolymarketEvent {
    pub event_id: String,
    /// Standardized event name (e.g., MEM-LAL)
    pub name: String,
    pub team_a: String,
    pub team_b: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<DateTime<Utc>>,
    #[serde(default = "default_category")]
    pub category: String,
    /// Single market for this event
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market: Option<PolymarketMarket>,
}

/// Matched market pair - used for arbitrage calculation
///
/// One Kalshi market corresponds to one Poly market (from a certain perspective)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedMarket {
    pub event_name: String,
    /// Competitor that the Kalshi market predicts
    pub team_name: String,
    /// Game date (for distinguishing same-team games on different dates)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_date: Option<NaiveDate>,

    /// Kalshi side
    pub kalshi_market: KalshiMarket,

    /// Polymarket side (not split, references original market)
    pub polymarket_market: PolymarketMarket,

    /// Cached Poly prices (calculated based on team_name)
    /// Yes price for this team
    pub poly_yes_price: f64,
    /// No price for this team (opponent wins)
    pub poly_no_price: f64,

    /// Match confidence
    #[serde(default)]
    pub confidence: f64,
}

impl MatchedMarket {
    /// Generate unique market key including date
    pub fn market_key(&self) -> String {
        generate_market_key(&self.event_name, self.game_date, &self.team_name)
    }
}

/// Matched event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedEvent {
    pub event_name: String,
    /// Game date (for distinguishing same-team games on different dates)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_date: Option<NaiveDate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kalshi_event: Option<KalshiEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polymarket_event: Option<PolymarketEvent>,
    #[serde(default)]
    pub confidence: f64,
}

/// Price update from WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceUpdate {
    pub platform: Platform,
    /// Kalshi: ticker, Polymarket: token_id
    pub market_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yes_bid: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yes_ask: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_bid: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_ask: Option<f64>,
    pub timestamp: DateTime<Utc>,
}

/// Arbitrage opportunity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArbitrageOpportunity {
    pub event_name: String,
    pub team_name: String,
    /// Game date (for distinguishing same-team games on different dates)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_date: Option<NaiveDate>,

    // Kalshi side
    pub kalshi_market_id: String,
    /// Price used in strategy
    pub kalshi_price: f64,
    /// "yes" or "no"
    pub kalshi_side: String,
    pub kalshi_bet: f64,
    /// Display: Yes price
    #[serde(default)]
    pub kalshi_yes_price: f64,
    /// Display: No price
    #[serde(default)]
    pub kalshi_no_price: f64,
    /// Kalshi contract count
    #[serde(default)]
    pub kalshi_contracts: f64,
    /// Kalshi trading fee
    #[serde(default)]
    pub kalshi_fee: f64,

    // Polymarket side
    pub polymarket_market_id: String,
    /// Price used in strategy
    pub polymarket_price: f64,
    /// "yes" or "no"
    pub polymarket_side: String,
    pub polymarket_bet: f64,
    /// Display: Yes price (own ask)
    #[serde(default)]
    pub polymarket_yes_price: f64,
    /// Display: No price (opponent's ask)
    #[serde(default)]
    pub polymarket_no_price: f64,

    // Arbitrage info
    pub total_bet: f64,
    pub profit_margin: f64,
    pub expected_profit: f64,
    /// Gross profit before fees
    #[serde(default)]
    pub gross_profit: f64,
    pub timestamp: DateTime<Utc>,
    /// Game start time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<DateTime<Utc>>,

    // Depth info for liquidity tracking
    /// Polymarket ask-side depth (USD = price * size)
    #[serde(default)]
    pub poly_ask_depth: f64,
    /// Polymarket ask-side quantity (token count)
    #[serde(default)]
    pub poly_ask_size: f64,
    /// Kalshi ask-side depth (contracts)
    #[serde(default)]
    pub kalshi_ask_depth: i32,
}

impl ArbitrageOpportunity {
    /// Generate unique market key including date
    pub fn market_key(&self) -> String {
        generate_market_key(&self.event_name, self.game_date, &self.team_name)
    }
}

/// System statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemStats {
    pub total_kalshi_events: usize,
    pub total_kalshi_markets: usize,
    pub total_polymarket_events: usize,
    /// After not splitting, equals event count
    pub total_polymarket_markets: usize,
    pub matched_events: usize,
    /// Kalshi market count (each has corresponding Poly perspective)
    pub matched_markets: usize,
    pub arbitrage_opportunities: usize,
    pub kalshi_ws_connected: bool,
    pub polymarket_ws_connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_update: Option<DateTime<Utc>>,
}

/// Arbitrage tracking record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArbitrageTrackingRecord {
    pub id: String,
    pub event_name: String,
    pub team_name: String,
    /// Game date (for distinguishing same-team games on different dates)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_date: Option<NaiveDate>,
    pub kalshi_market_id: String,
    pub polymarket_market_id: String,
    pub start_time: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<DateTime<Utc>>,
    pub initial_profit_margin: f64,
    pub max_profit_margin: f64,
    pub kalshi_side: String,
    pub polymarket_side: String,
    #[serde(default)]
    pub update_count: u64,
    /// Polymarket ask-side depth (USD = price * size)
    #[serde(default)]
    pub poly_ask_depth: f64,
    /// Polymarket ask-side quantity (token count)
    #[serde(default)]
    pub poly_ask_size: f64,
    /// Kalshi ask-side depth (contracts)
    #[serde(default)]
    pub kalshi_ask_depth: i32,
    /// Duration (milliseconds)
    #[serde(default)]
    pub duration_ms: i64,
    /// Kalshi ask price (on the side used for arbitrage)
    #[serde(default)]
    pub kalshi_ask_price: f64,
    /// Polymarket ask price (on the side used for arbitrage)
    #[serde(default)]
    pub polymarket_ask_price: f64,
}

/// Matched market data for frontend display
/// Corresponds to frontend's MatchedMarketData type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedMarketFrontend {
    pub event_name: String,
    pub team_name: String,
    /// Game date (for distinguishing same-team games on different dates)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_date: Option<String>,
    pub kalshi_market_id: String,
    /// Stable gateway market identifier used for quote refreshes.
    pub polymarket_market_id: String,
    /// Native Polymarket US slug required for an order.
    pub polymarket_market_slug: String,
    /// Native position for `team_name`.
    pub polymarket_team_position_side: PolymarketPositionSide,
    /// The other competitor in the native binary market.
    pub polymarket_opponent_name: String,
    /// Native position for the opposing competitor.
    pub polymarket_opponent_position_side: PolymarketPositionSide,
    pub kalshi_yes_price: f64,
    pub kalshi_no_price: f64,
    pub poly_yes_price: f64,
    pub poly_no_price: f64,
    pub kalshi_ready: bool,
    pub poly_ready: bool,
    pub both_ready: bool,
    pub confidence: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<String>,
    // Arbitrage related fields
    pub has_opportunity: bool,
    pub profit_margin: f64,
    pub expected_profit: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gross_profit: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kalshi_contracts: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kalshi_fee: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arbitrage_type: Option<String>,
}

/// Market scan statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanStats {
    /// Number of scans performed
    pub scan_count: u64,
    /// Number of new markets found in this scan
    pub new_markets_found: usize,
    /// Total matched markets after this scan
    pub total_matched_markets: usize,
    /// Timestamp of this scan
    pub timestamp: DateTime<Utc>,
}

/// WebSocket message types for frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WsMessage {
    #[serde(rename = "opportunity")]
    Opportunity { data: ArbitrageOpportunity },
    #[serde(rename = "opportunities")]
    Opportunities { data: Vec<ArbitrageOpportunity> },
    #[serde(rename = "stats")]
    Stats { data: SystemStats },
    #[serde(rename = "matched_markets_list")]
    MatchedMarketsList {
        data: Vec<MatchedMarketFrontend>,
        count: usize,
        opportunities_count: usize,
    },
    #[serde(rename = "metrics")]
    Metrics { data: MetricsReport },
    #[serde(rename = "mention_signals")]
    MentionSignals {
        data: Vec<serde_json::Value>,
        status: serde_json::Value,
    },
    #[serde(rename = "scan_stats")]
    ScanStats { data: ScanStats },
    #[serde(rename = "log")]
    Log { level: String, message: String },
    #[serde(rename = "price_update")]
    PriceUpdate { data: PriceUpdate },
    #[serde(rename = "error")]
    Error { message: String },
}
