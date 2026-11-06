//! Event and Market Matcher
//!
//! Matching logic optimization:
//! - Event matching: Use canonical competitors + game date to match events on both platforms
//! - Market matching: 2:1 matching (two Kalshi markets correspond to one Poly market)
//!
//! Key points:
//! - Kalshi: One event has 2 markets (one for each team)
//! - Polymarket: One event has 1 market (contains prices for both teams)
//! - When matching, determine Poly price perspective based on team name

use std::collections::HashMap;
use tracing::{debug, info, warn};

use crate::models::{
    KalshiEvent, KalshiMarket, MatchedEvent, MatchedMarket, PolymarketEvent, PolymarketMarket,
};

/// Event matcher - two-stage matching
pub struct EventMatcher {
    /// Time tolerance in hours for matching
    #[allow(dead_code)]
    time_tolerance_hours: i64,
}

/// Subscription info returned by get_subscription_info
pub struct SubscriptionInfo {
    /// Kalshi tickers to subscribe
    pub kalshi_tickers: Vec<String>,
    /// Lookup map: subscription_id -> Vec<MatchedMarket index>
    pub market_lookup: HashMap<String, Vec<usize>>,
}

impl SubscriptionInfo {
    /// Create an empty subscription info
    pub fn empty() -> Self {
        Self {
            kalshi_tickers: Vec::new(),
            market_lookup: HashMap::new(),
        }
    }
}

impl EventMatcher {
    /// Create a new event matcher
    pub fn new(time_tolerance_hours: i64) -> Self {
        Self {
            time_tolerance_hours,
        }
    }

    /// Execute two-stage matching
    pub fn match_events_and_markets(
        &self,
        kalshi_events: &[KalshiEvent],
        _kalshi_markets: &[KalshiMarket],
        polymarket_events: &[PolymarketEvent],
        _polymarket_markets: &[PolymarketMarket],
    ) -> (Vec<MatchedEvent>, Vec<MatchedMarket>) {
        info!("============================================================");
        info!("🔍 Starting two-stage matching (non-split version)");
        info!("   Kalshi: {} events", kalshi_events.len());
        info!("   Polymarket: {} events", polymarket_events.len());
        info!("============================================================");

        // Stage 1: Event matching
        let matched_events = self.match_events(kalshi_events, polymarket_events);
        info!(
            "📊 Stage 1 complete: matched {} events",
            matched_events.len()
        );

        // Stage 2: Market matching (2:1)
        let matched_markets = self.match_markets(&matched_events);
        info!(
            "📊 Stage 2 complete: matched {} market pairs",
            matched_markets.len()
        );

        (matched_events, matched_markets)
    }

    /// Stage 1: Event matching
    fn match_events(
        &self,
        kalshi_events: &[KalshiEvent],
        polymarket_events: &[PolymarketEvent],
    ) -> Vec<MatchedEvent> {
        info!("----------------------------------------");
        info!("🎯 Stage 1: Event matching");
        info!("----------------------------------------");

        let mut matched_events = Vec::new();
        let mut used_poly_ids = std::collections::HashSet::new();

        // Build Polymarket event index: category + event_name -> [events].
        // Keeping sports separate prevents a coincidental competitor string from
        // ever matching an NBA event to a tennis event.
        let mut poly_index: HashMap<String, Vec<&PolymarketEvent>> = HashMap::new();
        for event in polymarket_events {
            let key = format!("{}:{}", event.category, event.name.to_uppercase());
            poly_index.entry(key).or_default().push(event);
        }

        // Find match for each Kalshi event
        for k_event in kalshi_events {
            let k_name = k_event.name.to_uppercase();
            let k_date = k_event.start_time.map(|t| t.date_naive());

            // Also check reversed name (e.g., MEM-LAL vs LAL-MEM)
            let k_name_reversed = {
                let parts: Vec<&str> = k_name.split('-').collect();
                if parts.len() == 2 {
                    Some(format!("{}-{}", parts[1], parts[0]))
                } else {
                    None
                }
            };

            let mut best_match: Option<&PolymarketEvent> = None;
            let mut best_confidence = 0.0_f64;

            // Look for exact match or reversed match
            for (name_to_check, is_reversed) in
                [(Some(&k_name), false), (k_name_reversed.as_ref(), true)]
            {
                let Some(name) = name_to_check else {
                    continue;
                };

                let index_key = format!("{}:{}", k_event.category, name);
                let candidates = poly_index
                    .get(&index_key)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);

                for p_event in candidates {
                    if used_poly_ids.contains(&p_event.event_id) {
                        continue;
                    }

                    let p_date = p_event.start_time.map(|t| t.date_naive());

                    // Validate date
                    let confidence = match (k_date, p_date) {
                        (Some(kd), Some(pd)) => {
                            if kd != pd {
                                debug!(
                                    "   ❌ Date mismatch: {} ({}) vs {} ({})",
                                    k_event.name, kd, p_event.name, pd
                                );
                                continue;
                            }
                            if is_reversed {
                                0.95
                            } else {
                                1.0
                            }
                        }
                        _ => {
                            warn!(
                                "   ⚠️ Missing date: {} ({:?}) vs {} ({:?})",
                                k_event.name, k_date, p_event.name, p_date
                            );
                            if is_reversed {
                                0.65
                            } else {
                                0.7
                            }
                        }
                    };

                    if confidence > best_confidence {
                        best_confidence = confidence;
                        best_match = Some(p_event);
                    }
                }
            }

            if let Some(p_event) = best_match {
                if best_confidence >= 0.7 {
                    let matched = MatchedEvent {
                        event_name: k_event.name.clone(),
                        game_date: k_date, // Use Kalshi event's date
                        kalshi_event: Some(k_event.clone()),
                        polymarket_event: Some(p_event.clone()),
                        confidence: best_confidence,
                    };
                    matched_events.push(matched);
                    used_poly_ids.insert(p_event.event_id.clone());

                    info!(
                        "   ✅ Match: {} <-> {} (confidence: {:.2})",
                        k_event.name, p_event.name, best_confidence
                    );
                }
            } else {
                warn!("   ❌ No match found: {}", k_event.name);
            }
        }

        matched_events
    }

    /// Stage 2: Market matching (2:1)
    ///
    /// For each matched event:
    /// - Kalshi has 2 markets (one for each team)
    /// - Polymarket has 1 market (contains both teams)
    /// - Create 2 MatchedMarkets, each corresponding to one Kalshi market
    fn match_markets(&self, matched_events: &[MatchedEvent]) -> Vec<MatchedMarket> {
        info!("----------------------------------------");
        info!("🎯 Stage 2: Market matching (2:1)");
        info!("----------------------------------------");

        let mut matched_markets = Vec::new();

        for matched_event in matched_events {
            let Some(k_event) = &matched_event.kalshi_event else {
                continue;
            };
            let Some(p_event) = &matched_event.polymarket_event else {
                continue;
            };
            let Some(poly_market) = &p_event.market else {
                continue;
            };

            // For each Kalshi market, create a MatchedMarket
            for k_market in &k_event.markets {
                let team_name = k_market.team_name.to_uppercase();

                // Get Poly market prices for this team
                let (poly_yes, poly_no) = match poly_market.get_price_for_team(&team_name) {
                    Ok(prices) => prices,
                    Err(e) => {
                        warn!("   ⚠️ {}", e);
                        continue;
                    }
                };

                let matched = MatchedMarket {
                    event_name: matched_event.event_name.clone(),
                    team_name: team_name.clone(),
                    game_date: matched_event.game_date, // Inherit from MatchedEvent
                    kalshi_market: k_market.clone(),
                    polymarket_market: poly_market.clone(),
                    poly_yes_price: poly_yes,
                    poly_no_price: poly_no,
                    confidence: matched_event.confidence,
                };
                matched_markets.push(matched);

                info!(
                    "   ✅ Market match: {} - {}",
                    matched_event.event_name, team_name
                );
                debug!(
                    "      Kalshi: Yes={:.2}, No={:.2}",
                    k_market.yes_price, k_market.no_price
                );
                debug!("      Poly:   Yes={:.2}, No={:.2}", poly_yes, poly_no);
            }
        }

        matched_markets
    }

    /// Get WebSocket subscription info
    ///
    /// Polymarket US gateway markets are REST-polled, not CLOB-subscribed.
    /// The lookup therefore contains Kalshi tickers only.
    pub fn get_subscription_info(&self, matched_markets: &[MatchedMarket]) -> SubscriptionInfo {
        let mut kalshi_tickers = Vec::new();
        let mut market_lookup: HashMap<String, Vec<usize>> = HashMap::new();

        let mut seen_kalshi = std::collections::HashSet::new();

        for (idx, mm) in matched_markets.iter().enumerate() {
            // Kalshi ticker
            let k_id = &mm.kalshi_market.market_id;
            if !k_id.is_empty() && !seen_kalshi.contains(k_id) {
                kalshi_tickers.push(k_id.clone());
                seen_kalshi.insert(k_id.clone());
            }

            // Add to lookup
            market_lookup.entry(k_id.clone()).or_default().push(idx);
        }

        info!(
            "📡 Subscription information: Kalshi {} tickers; Polymarket US uses REST quotes",
            kalshi_tickers.len()
        );

        SubscriptionInfo {
            kalshi_tickers,
            market_lookup,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_poly_market() -> PolymarketMarket {
        PolymarketMarket {
            market_id: "poly-123".to_string(),
            market_slug: "lal-mem-2026-08-17".to_string(),
            event_name: "LAL-MEM".to_string(),
            team_a: "LAL".to_string(),
            team_b: "MEM".to_string(),
            price_a: 0.45,
            price_b: 0.55,
            team_a_position: crate::models::PolymarketPositionSide::Long,
            team_b_position: crate::models::PolymarketPositionSide::Short,
            start_time: None,
            volume: None,
        }
    }

    #[test]
    fn test_get_price_for_team() {
        let market = create_test_poly_market();

        // LAL is team_a
        let (yes, no) = market.get_price_for_team("LAL").unwrap();
        assert!((yes - 0.45).abs() < 0.001);
        assert!((no - 0.55).abs() < 0.001);

        // MEM is team_b
        let (yes, no) = market.get_price_for_team("MEM").unwrap();
        assert!((yes - 0.55).abs() < 0.001);
        assert!((no - 0.45).abs() < 0.001);

        // Case insensitive
        let (yes, _) = market.get_price_for_team("lal").unwrap();
        assert!((yes - 0.45).abs() < 0.001);

        // Invalid team
        assert!(market.get_price_for_team("BOS").is_err());
    }

    #[test]
    fn test_us_execution_for_competitor() {
        let market = create_test_poly_market();

        assert_eq!(
            market
                .us_execution_for_competitor("LAL")
                .unwrap()
                .position_side,
            crate::models::PolymarketPositionSide::Long
        );
        assert_eq!(market.us_execution_for_competitor("BOS"), None);
    }
}
