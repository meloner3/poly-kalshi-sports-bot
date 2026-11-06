//! Unified position view: per-leg economics and hedge pairing.
//!
//! Pure functions only (no I/O) so the pairing rules can be unit-tested.
//! Pairing matches the full tuple `(kalshi_market_id, kalshi_side,
//! polymarket_market_id, polymarket_side)` against this bot's own trade
//! history, using only the most recent eligible history row per held
//! Kalshi leg. Market id alone is not enough: the same market pair can
//! appear on opposite sides across different trades over time.

use serde::Serialize;
use serde_json::Value;

use crate::services::storage::AutoTradeRecord;

/// History statuses that represent a trade which actually opened a hedge.
const PAIRED_STATUSES: [&str; 3] = ["executed", "neutralized", "partial_paired"];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PositionLeg {
    pub platform: &'static str,
    pub market_id: String,
    pub title: String,
    /// Kalshi: "yes"/"no". Polymarket: "long"/"short".
    pub side: String,
    pub contracts: f64,
    pub entry_price: Option<f64>,
    pub current_price: Option<f64>,
    pub cost: f64,
    pub value: Option<f64>,
    pub unrealized_pnl: Option<f64>,
    /// `None` means unknown (Polymarket exposes no fee field), never $0.
    pub fees: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PositionCard {
    /// "paired" or "standalone"
    pub kind: &'static str,
    pub event_name: Option<String>,
    pub legs: Vec<PositionLeg>,
    pub total_cost: f64,
    pub total_value: Option<f64>,
    pub total_pnl: Option<f64>,
    /// Paired legs whose contract counts differ are only partly hedged.
    pub contracts_mismatch: bool,
}

fn num(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    value.as_f64().or_else(|| value.as_str()?.parse().ok())
}

fn leg_economics(contracts: f64, cost: f64, mark: Option<f64>) -> (Option<f64>, Option<f64>, Option<f64>) {
    let entry = (contracts > 0.0).then(|| cost / contracts);
    let value = mark.map(|m| m * contracts);
    let pnl = value.map(|v| v - cost);
    (entry, value, pnl)
}

/// Build a Kalshi leg from a raw `market_positions` entry. `mark` is the
/// per-contract liquidation price of the held side. Returns `None` for a
/// flat or unparseable position.
pub fn kalshi_leg(position: &Value, mark: Option<f64>) -> Option<PositionLeg> {
    let signed = num(position.get("position_fp").or_else(|| position.get("position")))?;
    if signed == 0.0 {
        return None;
    }
    let ticker = position.get("ticker")?.as_str()?.to_string();
    let contracts = signed.abs();
    // Sign of market_exposure_dollars is undocumented for NO positions.
    let cost = num(position.get("market_exposure_dollars").or_else(|| position.get("market_exposure")))?.abs();
    let (entry_price, value, unrealized_pnl) = leg_economics(contracts, cost, mark);
    Some(PositionLeg {
        platform: "kalshi",
        title: position
            .get("event_ticker")
            .and_then(Value::as_str)
            .unwrap_or(&ticker)
            .to_string(),
        market_id: ticker,
        side: if signed > 0.0 { "yes" } else { "no" }.to_string(),
        contracts,
        entry_price,
        current_price: mark,
        cost,
        value,
        unrealized_pnl,
        fees: num(position.get("fees_paid_dollars").or_else(|| position.get("fees_paid"))),
    })
}

/// Build a Polymarket leg from a `normalize_positions` entry.
pub fn polymarket_leg(position: &Value) -> Option<PositionLeg> {
    let signed = num(position.get("size"))?;
    if signed == 0.0 {
        return None;
    }
    let market_id = position.get("id")?.as_str()?.to_string();
    let contracts = signed.abs();
    let cost = num(position.get("cost"))?.abs();
    let value = num(position.get("value")).map(f64::abs);
    let (entry_price, _, _) = leg_economics(contracts, cost, None);
    Some(PositionLeg {
        platform: "polymarket",
        title: position
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(&market_id)
            .to_string(),
        market_id,
        side: if signed > 0.0 { "long" } else { "short" }.to_string(),
        contracts,
        entry_price,
        current_price: value.map(|v| v / contracts),
        cost,
        value,
        unrealized_pnl: value.map(|v| v - cost),
        fees: None,
    })
}

fn card(kind: &'static str, event_name: Option<String>, legs: Vec<PositionLeg>) -> PositionCard {
    let total_cost = legs.iter().map(|l| l.cost).sum();
    let total_value = legs.iter().map(|l| l.value).sum::<Option<f64>>();
    let contracts_mismatch = legs.len() == 2 && (legs[0].contracts - legs[1].contracts).abs() > 1e-9;
    PositionCard {
        kind,
        event_name,
        total_cost,
        total_pnl: total_value.map(|v| v - total_cost),
        total_value,
        contracts_mismatch,
        legs,
    }
}

/// Group held legs into hedged pairs and standalone leftovers. `history`
/// must be ordered newest first (as `get_auto_trade_history` returns it).
/// Each leg lands in exactly one card.
pub fn pair_positions(
    kalshi: Vec<PositionLeg>,
    polymarket: Vec<PositionLeg>,
    history: &[AutoTradeRecord],
) -> Vec<PositionCard> {
    let mut poly: Vec<Option<PositionLeg>> = polymarket.into_iter().map(Some).collect();
    let mut cards = Vec::new();

    for k in kalshi {
        // Only the most recent eligible row for this exact Kalshi leg counts.
        let row = history.iter().find(|r| {
            PAIRED_STATUSES.contains(&r.status.as_str())
                && r.kalshi_market_id == k.market_id
                && r.kalshi_side.eq_ignore_ascii_case(&k.side)
        });
        let partner = row.and_then(|r| {
            poly.iter().position(|p| {
                p.as_ref().is_some_and(|p| {
                    p.market_id == r.polymarket_market_id
                        && p.side.eq_ignore_ascii_case(&r.polymarket_side)
                })
            })
            .map(|i| (i, r))
        });
        match partner {
            Some((i, r)) => {
                let p = poly[i].take().expect("index found above");
                cards.push(card("paired", Some(r.event_name.clone()), vec![k, p]));
            }
            None => cards.push(card("standalone", None, vec![k])),
        }
    }
    cards.extend(poly.into_iter().flatten().map(|p| card("standalone", None, vec![p])));
    cards
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn leg(platform: &'static str, id: &str, side: &str, contracts: f64, cost: f64) -> PositionLeg {
        PositionLeg {
            platform,
            market_id: id.into(),
            title: id.into(),
            side: side.into(),
            contracts,
            entry_price: Some(cost / contracts),
            current_price: None,
            cost,
            value: None,
            unrealized_pnl: None,
            fees: None,
        }
    }

    fn row(status: &str, k: &str, ks: &str, p: &str, ps: &str) -> AutoTradeRecord {
        AutoTradeRecord {
            id: 0,
            event_name: "Game".into(),
            team_name: "T".into(),
            kalshi_market_id: k.into(),
            polymarket_market_id: p.into(),
            kalshi_side: ks.into(),
            polymarket_side: ps.into(),
            kalshi_contracts: 0,
            kalshi_price: 0.0,
            kalshi_fee: 0.0,
            polymarket_amount: 0.0,
            polymarket_price: 0.0,
            total_amount: 0.0,
            profit_margin: 0.0,
            duration_ms: 0,
            total_duration_ms: 0,
            kalshi_success: true,
            polymarket_success: true,
            kalshi_order_id: None,
            polymarket_order_id: None,
            kalshi_error: None,
            polymarket_error: None,
            kalshi_latency_ms: None,
            poly_latency_ms: None,
            status: status.into(),
            skip_reason: None,
            kalshi_filled_contracts: 0,
            polymarket_filled_contracts: 0,
            kalshi_order_status: None,
            polymarket_order_status: None,
            neutralization_leg: None,
            neutralization_success: None,
            neutralization_order_id: None,
            neutralization_filled_contracts: 0,
            neutralization_error: None,
            residual_leg: None,
            residual_contracts: 0,
            created_at: String::new(),
        }
    }

    #[test]
    fn matching_history_pairs_legs() {
        let cards = pair_positions(
            vec![leg("kalshi", "K1", "yes", 5.0, 2.0)],
            vec![leg("polymarket", "P1", "short", 5.0, 2.5)],
            &[row("executed", "K1", "yes", "P1", "short")],
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].kind, "paired");
        assert_eq!(cards[0].legs.len(), 2);
        assert!((cards[0].total_cost - 4.5).abs() < 1e-9);
        assert!(!cards[0].contracts_mismatch);
    }

    #[test]
    fn unmatched_legs_are_standalone() {
        let cards = pair_positions(
            vec![leg("kalshi", "K1", "yes", 5.0, 2.0)],
            vec![leg("polymarket", "P9", "short", 5.0, 2.5)],
            &[],
        );
        assert_eq!(cards.len(), 2);
        assert!(cards.iter().all(|c| c.kind == "standalone"));
    }

    #[test]
    fn rejected_and_skipped_rows_do_not_pair() {
        for status in ["skipped", "failed", "submitting", "rejected"] {
            let cards = pair_positions(
                vec![leg("kalshi", "K1", "yes", 5.0, 2.0)],
                vec![leg("polymarket", "P1", "short", 5.0, 2.5)],
                &[row(status, "K1", "yes", "P1", "short")],
            );
            assert_eq!(cards.len(), 2, "{status}");
        }
    }

    #[test]
    fn opposite_side_history_does_not_pair() {
        let cards = pair_positions(
            vec![leg("kalshi", "K1", "yes", 5.0, 2.0)],
            vec![leg("polymarket", "P1", "short", 5.0, 2.5)],
            &[row("executed", "K1", "no", "P1", "long")],
        );
        assert_eq!(cards.len(), 2);
    }

    #[test]
    fn only_most_recent_matching_row_counts() {
        // Newest row for K1/yes points at a different Polymarket market;
        // the older row that would match P1 must be ignored.
        let cards = pair_positions(
            vec![leg("kalshi", "K1", "yes", 5.0, 2.0)],
            vec![leg("polymarket", "P1", "short", 5.0, 2.5)],
            &[
                row("executed", "K1", "yes", "P2", "short"),
                row("executed", "K1", "yes", "P1", "short"),
            ],
        );
        assert_eq!(cards.len(), 2);
    }

    #[test]
    fn stale_opposite_row_does_not_shadow_newer_match() {
        let cards = pair_positions(
            vec![leg("kalshi", "K1", "yes", 5.0, 2.0)],
            vec![leg("polymarket", "P1", "short", 5.0, 2.5)],
            &[
                row("executed", "K1", "yes", "P1", "short"),
                row("executed", "K1", "no", "P1", "long"),
            ],
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].kind, "paired");
    }

    #[test]
    fn poly_leg_is_never_in_two_cards() {
        let cards = pair_positions(
            vec![
                leg("kalshi", "K1", "yes", 5.0, 2.0),
                leg("kalshi", "K1", "yes", 3.0, 1.0),
            ],
            vec![leg("polymarket", "P1", "short", 5.0, 2.5)],
            &[row("executed", "K1", "yes", "P1", "short")],
        );
        let poly_count = cards
            .iter()
            .flat_map(|c| &c.legs)
            .filter(|l| l.platform == "polymarket")
            .count();
        assert_eq!(poly_count, 1);
        assert_eq!(cards.iter().map(|c| c.legs.len()).sum::<usize>(), 3);
    }

    #[test]
    fn neutralized_status_pairs_and_flags_mismatch() {
        let cards = pair_positions(
            vec![leg("kalshi", "K1", "yes", 5.0, 2.0)],
            vec![leg("polymarket", "P1", "short", 3.0, 1.5)],
            &[row("neutralized", "K1", "yes", "P1", "short")],
        );
        assert_eq!(cards[0].kind, "paired");
        assert!(cards[0].contracts_mismatch);
    }

    #[test]
    fn kalshi_no_leg_economics() {
        let raw = json!({
            "ticker": "K1", "event_ticker": "EV",
            "position_fp": "-10.00", "market_exposure_dollars": "-4.20",
            "fees_paid_dollars": "0.15"
        });
        let l = kalshi_leg(&raw, Some(0.50)).unwrap();
        assert_eq!(l.side, "no");
        assert_eq!(l.contracts, 10.0);
        assert!((l.entry_price.unwrap() - 0.42).abs() < 1e-9);
        assert!((l.unrealized_pnl.unwrap() - 0.80).abs() < 1e-9);
        assert_eq!(l.fees, Some(0.15));
    }

    #[test]
    fn polymarket_leg_economics_and_unknown_fee() {
        let raw = json!({"id": "P1", "title": "T", "size": "-4", "cost": 1.6, "value": 2.0});
        let l = polymarket_leg(&raw).unwrap();
        assert_eq!(l.side, "short");
        assert!((l.entry_price.unwrap() - 0.4).abs() < 1e-9);
        assert!((l.current_price.unwrap() - 0.5).abs() < 1e-9);
        assert!((l.unrealized_pnl.unwrap() - 0.4).abs() < 1e-9);
        assert_eq!(l.fees, None);
    }

    #[test]
    fn flat_positions_are_dropped() {
        assert!(kalshi_leg(&json!({"ticker":"K","position_fp":"0","market_exposure_dollars":"0"}), None).is_none());
        assert!(polymarket_leg(&json!({"id":"P","size":"0","cost":0})).is_none());
    }
}
