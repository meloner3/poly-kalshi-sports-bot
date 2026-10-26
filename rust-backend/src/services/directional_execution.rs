use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use parking_lot::Mutex;
use rsa::sha2::{Digest, Sha256};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{error, warn};
use uuid::Uuid;

use crate::clients::{KalshiClient, OrderBook};
use crate::config::{MentionConfig, MentionMode};
use crate::services::mention_signal::{MentionSignal, MentionSignalClient};
use crate::services::paired_execution::kalshi_fee;
use crate::services::storage::{
    ArbitrageStorage, DirectionalPosition, NewDirectionalDecision, NewDirectionalPosition,
};
use crate::services::TelegramClient;

pub const DIRECTIONAL_BOOK_MAX_AGE: Duration = Duration::from_secs(30);
const REJECTION_LOG_INTERVAL: Duration = Duration::from_secs(15 * 60);
// The OpenAPI schema confirms the returned client_order_id and supports
// min_ts+cursor scans; authenticated account-level recovery has not yet been
// exercised in this environment, so live submission stays closed.
pub const LIVE_ORDER_LOOKUP_CONFIRMED: bool = false;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FillEstimate {
    pub contracts: i32,
    pub vwap: f64,
    pub limit_price_cents: i32,
    pub fees: f64,
    pub slippage: f64,
}

pub fn buy_levels(book: &OrderBook, side: &str) -> Vec<(i32, i32)> {
    let mut levels: Vec<(i32, i32)> = match side {
        "yes" => book
            .no
            .iter()
            .rev()
            .map(|(no_bid, quantity)| (100 - no_bid, *quantity))
            .collect(),
        "no" => book
            .yes
            .iter()
            .rev()
            .map(|(yes_bid, quantity)| (100 - yes_bid, *quantity))
            .collect(),
        _ => Vec::new(),
    };
    levels.sort_by_key(|(price, _)| *price);
    levels
}

pub fn sell_levels(book: &OrderBook, side: &str) -> Vec<(i32, i32)> {
    let mut levels: Vec<(i32, i32)> = match side {
        "yes" => book
            .yes
            .iter()
            .rev()
            .map(|(price, quantity)| (*price, *quantity))
            .collect(),
        "no" => book
            .no
            .iter()
            .rev()
            .map(|(price, quantity)| (*price, *quantity))
            .collect(),
        _ => Vec::new(),
    };
    levels.sort_by(|left, right| right.0.cmp(&left.0));
    levels
}

pub fn walk_buy(levels: &[(i32, i32)], contracts: i32) -> Option<FillEstimate> {
    if contracts <= 0 {
        return None;
    }
    let mut remaining = contracts;
    let mut cost_cents = 0_i64;
    let mut fees = 0.0;
    let mut first_price = None;
    let mut worst_price = None;
    let mut filled = 0;
    for (price_cents, available) in levels {
        if *available <= 0 || !(1..=99).contains(price_cents) {
            continue;
        }
        let take = remaining.min(*available);
        let price = *price_cents as f64 / 100.0;
        first_price.get_or_insert(*price_cents);
        worst_price = Some(*price_cents);
        cost_cents += i64::from(*price_cents) * i64::from(take);
        fees += kalshi_fee(take, price);
        filled += take;
        remaining -= take;
        if remaining == 0 {
            break;
        }
    }
    if remaining != 0 || filled == 0 {
        return None;
    }
    let slippage = f64::from(worst_price? - first_price?) / 100.0;
    Some(FillEstimate {
        contracts: filled,
        vwap: cost_cents as f64 / filled as f64 / 100.0,
        limit_price_cents: worst_price?,
        fees,
        slippage,
    })
}

pub fn walk_sell(levels: &[(i32, i32)], contracts: i32) -> Option<FillEstimate> {
    if contracts <= 0 {
        return None;
    }
    let mut remaining = contracts;
    let mut proceeds_cents = 0_i64;
    let mut fees = 0.0;
    let mut first_price = None;
    let mut worst_price = None;
    let mut filled = 0;
    for (price_cents, available) in levels {
        if *available <= 0 || !(1..=99).contains(price_cents) {
            continue;
        }
        let take = remaining.min(*available);
        let price = *price_cents as f64 / 100.0;
        first_price.get_or_insert(*price_cents);
        worst_price = Some(*price_cents);
        proceeds_cents += i64::from(*price_cents) * i64::from(take);
        fees += kalshi_fee(take, price);
        filled += take;
        remaining -= take;
        if remaining == 0 {
            break;
        }
    }
    if filled == 0 {
        return None;
    }
    Some(FillEstimate {
        contracts: filled,
        vwap: proceeds_cents as f64 / filled as f64 / 100.0,
        limit_price_cents: worst_price?,
        fees,
        slippage: f64::from(first_price? - worst_price?) / 100.0,
    })
}

pub fn fractional_kelly_contracts(
    probability: f64,
    unit_cost: f64,
    bankroll: f64,
    fraction: f64,
) -> i32 {
    if !probability.is_finite()
        || !unit_cost.is_finite()
        || !bankroll.is_finite()
        || unit_cost <= 0.0
        || unit_cost >= 1.0
        || bankroll <= 0.0
        || fraction <= 0.0
    {
        return 0;
    }
    let full_kelly = (probability - unit_cost) / (1.0 - unit_cost);
    if full_kelly <= 0.0 {
        return 0;
    }
    (fraction * full_kelly * bankroll / unit_cost)
        .floor()
        .clamp(0.0, i32::MAX as f64) as i32
}

pub fn plan_entry(
    book: &OrderBook,
    side: &str,
    probability: f64,
    budget: f64,
    config: &MentionConfig,
) -> Option<(FillEstimate, f64)> {
    let levels = buy_levels(book, side);
    let (best_bid, best_ask) = match side {
        "yes" => (
            book.yes.last().map(|level| level.0)?,
            levels.first().map(|level| level.0)?,
        ),
        "no" => (
            book.no.last().map(|level| level.0)?,
            levels.first().map(|level| level.0)?,
        ),
        _ => return None,
    };
    if best_ask <= best_bid || f64::from(best_ask - best_bid) / 100.0 > config.max_spread {
        return None;
    }
    let depth = levels
        .iter()
        .map(|(_, quantity)| (*quantity).max(0))
        .sum::<i32>();
    if depth < config.min_book_depth_contracts {
        return None;
    }
    let ask = f64::from(best_ask) / 100.0;
    let best_cost = ask + kalshi_fee(1, ask);
    let mut contracts = fractional_kelly_contracts(
        probability,
        best_cost,
        config.bankroll_usd,
        config.kelly_fraction,
    )
    .min(depth);
    while contracts > 0 {
        if let Some(fill) = walk_buy(&levels, contracts) {
            let unit_cost = fill.vwap + fill.fees / fill.contracts as f64;
            let edge = probability - unit_cost;
            if unit_cost * fill.contracts as f64 <= budget && edge >= config.min_net_edge {
                return Some((fill, edge));
            }
        }
        contracts -= 1;
    }
    None
}

#[derive(Debug, Clone, Deserialize)]
pub struct GateArtifact {
    pub edge_estimate: Option<f64>,
    pub ci_low: Option<f64>,
    pub ci_high: Option<f64>,
    pub n_days: i64,
    pub as_of: Option<DateTime<Utc>>,
    pub model_version: Option<String>,
    pub live_gate_passed: bool,
    pub status: String,
}

impl GateArtifact {
    pub fn check(
        &self,
        expected_model: &str,
        config: &MentionConfig,
        now: DateTime<Utc>,
    ) -> Result<()> {
        if !LIVE_ORDER_LOOKUP_CONFIRMED {
            anyhow::bail!("live mode is locked: Kalshi client_order_id lookup has not been smoke-tested against the authenticated API");
        }
        if self.status != "passed" || !self.live_gate_passed {
            anyhow::bail!("directional paper gate has not passed");
        }
        if self.n_days < config.live_gate_min_days {
            anyhow::bail!(
                "paper gate has {} event-days; {} required",
                self.n_days,
                config.live_gate_min_days
            );
        }
        if self.model_version.as_deref() != Some(expected_model) {
            anyhow::bail!(
                "paper gate model version does not match the active predictor and strategy"
            );
        }
        if !self
            .ci_low
            .is_some_and(|bound| bound.is_finite() && bound > 0.0)
        {
            anyhow::bail!("paper gate lower confidence bound is not above zero");
        }
        let Some(as_of) = self.as_of else {
            anyhow::bail!("paper gate artifact has no as_of timestamp");
        };
        let maximum_age = chrono::Duration::days(config.live_gate_max_age_days);
        if as_of > now || now - as_of > maximum_age {
            anyhow::bail!("paper gate artifact is stale");
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct DirectionalExecutionService {
    config: MentionConfig,
    storage: Arc<ArbitrageStorage>,
    kalshi: KalshiClient,
    signals: MentionSignalClient,
    telegram: Arc<TelegramClient>,
    subscribed: Arc<Mutex<HashSet<String>>>,
    last_rejection: Arc<Mutex<HashMap<String, (String, Instant)>>>,
}

impl DirectionalExecutionService {
    pub fn new(
        config: MentionConfig,
        storage: Arc<ArbitrageStorage>,
        kalshi: KalshiClient,
        signals: MentionSignalClient,
        telegram: Arc<TelegramClient>,
    ) -> Self {
        Self {
            config,
            storage,
            kalshi,
            signals,
            telegram,
            subscribed: Arc::new(Mutex::new(HashSet::new())),
            last_rejection: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn strategy_version(&self, predictor_version: &str) -> String {
        compute_strategy_version(&self.config, predictor_version)
    }

    pub fn set_mode(&self, mode: MentionMode) -> Result<()> {
        if mode == MentionMode::Live {
            let version = self
                .signals
                .health()
                .and_then(|health| health.model_version)
                .context("predictor health is unavailable")?;
            self.validate_live_gate(&self.strategy_version(&version), Utc::now())?;
            if self.storage.has_unresolved_directional_submission()? {
                anyhow::bail!("an unresolved directional order intent blocks live mode");
            }
        }
        self.storage.set_directional_mode(mode)
    }

    pub fn kill_switch(&self, enabled: bool) -> Result<()> {
        self.storage.set_directional_kill_switch(enabled)
    }

    pub fn reset_loss_breaker(&self) -> Result<()> {
        self.storage.reset_directional_loss_breaker()
    }

    pub fn status(&self) -> Result<Value> {
        let state = self.storage.directional_state()?;
        let health = self.signals.health();
        let open = self.storage.open_directional_positions()?;
        let day = Utc::now().format("%Y-%m-%d").to_string();
        let paper_today = self.storage.directional_day_pnl(&day, "paper")?;
        let live_today = self.storage.directional_day_pnl(&day, "live")?;
        let paper_total = self.storage.directional_total_pnl("paper")?;
        let live_total = self.storage.directional_total_pnl("live")?;
        // Legacy keys follow the active mode so a paper P&L is never shown as live.
        let (today_pnl, realized_pnl) = if state.mode == MentionMode::Live {
            (live_today, live_total)
        } else {
            (paper_today, paper_total)
        };
        let total_exposure: f64 = open
            .iter()
            .filter(|position| position.status == "open")
            .map(|position| {
                position.entry_vwap * position.remaining_contracts as f64 + position.entry_fees
            })
            .sum();
        let newest_predictor = health.as_ref().and_then(|h| h.model_version.clone());
        let version = newest_predictor
            .as_deref()
            .map(|value| self.strategy_version(value));
        let gate = health.as_ref().map(|h| h.latest_verdict.clone());
        Ok(json!({
            "mode": state.mode,
            "kill_switch": state.kill_switch,
            "loss_breaker": state.loss_breaker,
            "breaker_day": state.breaker_day,
            "signal_service_healthy": self.signals.is_fresh(),
            "predictor_model_version": newest_predictor,
            "strategy_model_version": version,
            "open_positions": open.iter().filter(|p| p.status == "open").count(),
            "open_exposure": total_exposure,
            "today_realized_pnl": today_pnl,
            "realized_pnl": realized_pnl,
            "today_realized_pnl_paper": paper_today,
            "today_realized_pnl_live": live_today,
            "realized_pnl_paper": paper_total,
            "realized_pnl_live": live_total,
            "live_order_lookup_confirmed": LIVE_ORDER_LOOKUP_CONFIRMED,
            "limits": {
                "min_net_edge": self.config.min_net_edge,
                "max_open_positions": self.config.max_open_positions,
                "live_gate_min_days": self.config.live_gate_min_days,
                "max_spread": self.config.max_spread,
                "max_signal_age_secs": self.config.max_signal_age_secs,
                "pre_event_only": self.config.pre_event_only,
                "bankroll_usd": self.config.bankroll_usd,
            },
            "gate": gate,
        }))
    }

    pub fn history(&self, limit: usize) -> Result<Vec<Value>> {
        self.storage.directional_history(limit)
    }

    pub fn decisions(&self, limit: usize) -> Result<Vec<Value>> {
        let conn = self.storage.conn().lock();
        let mut statement = conn.prepare(
            "SELECT id,market_ticker,event_id,event_day,side,model_version,p_hat,p_lo,p_hi,
             yes_bid,yes_ask,net_edge,vwap,fee_per_contract,contracts,decision,reason,created_at
             FROM directional_decisions ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map(rusqlite::params![limit.min(500) as i64], |row| {
            Ok(json!({
                "id":row.get::<_,i64>(0)?,"market_ticker":row.get::<_,String>(1)?,
                "event_id":row.get::<_,Option<String>>(2)?,"event_day":row.get::<_,Option<String>>(3)?,
                "side":row.get::<_,Option<String>>(4)?,"model_version":row.get::<_,Option<String>>(5)?,
                "p_hat":row.get::<_,Option<f64>>(6)?,"p_lo":row.get::<_,Option<f64>>(7)?,
                "p_hi":row.get::<_,Option<f64>>(8)?,"yes_bid":row.get::<_,Option<f64>>(9)?,
                "yes_ask":row.get::<_,Option<f64>>(10)?,"net_edge":row.get::<_,Option<f64>>(11)?,
                "vwap":row.get::<_,Option<f64>>(12)?,"fee_per_contract":row.get::<_,Option<f64>>(13)?,
                "contracts":row.get::<_,i32>(14)?,"decision":row.get::<_,String>(15)?,
                "reason":row.get::<_,String>(16)?,"created_at":row.get::<_,String>(17)?,
            }))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn signal_dashboard(&self) -> Vec<Value> {
        self.signals
            .all_signals()
            .into_iter()
            .map(|signal| {
                let fresh = signal.is_usable_at(Utc::now(), self.signals.max_age());
                let mut yes_bid = None;
                let mut yes_ask = None;
                let mut no_bid = None;
                let mut no_ask = None;
                if let Some(book) = self
                    .kalshi
                    .get_fresh_orderbook(&signal.market_ticker, DIRECTIONAL_BOOK_MAX_AGE)
                {
                    yes_bid = book.yes.last().map(|(price, _)| *price as f64 / 100.0);
                    no_bid = book.no.last().map(|(price, _)| *price as f64 / 100.0);
                    yes_ask = book.no.last().map(|(price, _)| 1.0 - *price as f64 / 100.0);
                    no_ask = book
                        .yes
                        .last()
                        .map(|(price, _)| 1.0 - *price as f64 / 100.0);
                }
                let yes_edge = signal
                    .p_lo
                    .zip(yes_ask)
                    .map(|(p, a)| p - a - kalshi_fee(1, a));
                let no_probability = signal.p_hi.map(|p| 1.0 - p);
                let no_edge = no_probability
                    .zip(no_ask)
                    .map(|(p, a)| p - a - kalshi_fee(1, a));
                json!({
                    "signal":signal,
                    "signal_fresh":fresh,
                    "yes_bid":yes_bid,"yes_ask":yes_ask,"no_bid":no_bid,"no_ask":no_ask,
                    "net_edge_yes":yes_edge,"net_edge_no":no_edge,
                    "mode":"directional"
                })
            })
            .collect()
    }

    pub async fn run_once(&self) {
        if let Err(reason) = self.signals.refresh().await {
            warn!("Directional signal polling failed closed: {:#}", reason);
            let _ = self.log_system_decision(
                "signal-service",
                "unavailable",
                &format!("signal service unreachable: {reason:#}"),
            );
        }
        let all_signals = self.signals.all_signals();
        let signals = self.signals.signals();
        let actionable: HashSet<&str> = signals
            .iter()
            .map(|signal| signal.market_ticker.as_str())
            .collect();
        for signal in &all_signals {
            if signal.status != "ok" || !actionable.contains(signal.market_ticker.as_str()) {
                let reason = signal
                    .reason
                    .as_deref()
                    .unwrap_or("signal was stale, malformed, or version-mismatched");
                self.reject(signal, "none", reason).await;
            }
        }
        if let Some(predictor_version) = self
            .signals
            .health()
            .and_then(|health| health.model_version)
        {
            let _ = self
                .storage
                .set_directional_model_version(&self.strategy_version(&predictor_version));
        }
        if let Err(reason) = self.sync_subscriptions(&signals).await {
            warn!("Mention WebSocket subscription update failed: {:#}", reason);
        }
        self.reconcile_unresolved().await;
        self.reconcile_outcomes().await;

        let positions = match self.storage.open_directional_positions() {
            Ok(positions) => positions,
            Err(reason) => {
                error!("Unable to load directional positions: {:#}", reason);
                return;
            }
        };
        let unresolved_positions: HashSet<i64> = self
            .storage
            .unresolved_directional_order_intents()
            .unwrap_or_default()
            .into_iter()
            .map(|intent| intent.position_id)
            .collect();
        let signal_by_ticker: HashMap<String, MentionSignal> = signals
            .into_iter()
            .map(|signal| (signal.market_ticker.clone(), signal))
            .collect();
        for position in positions.iter().filter(|position| {
            position.status == "open" && !unresolved_positions.contains(&position.id)
        }) {
            let current = signal_by_ticker.get(&position.market_ticker);
            self.manage_exit(position, current).await;
        }
        // Covers live exits/reconciliation, which do not check the breaker themselves,
        // and must run before the policy state is read below.
        self.check_daily_loss_breaker("paper").await;
        self.check_daily_loss_breaker("live").await;

        let state = match self.storage.directional_state() {
            Ok(state) => state,
            Err(reason) => {
                error!("Unable to load directional policy state: {:#}", reason);
                return;
            }
        };
        if state.mode == MentionMode::Off {
            return;
        }
        if state.mode == MentionMode::Live {
            let validation = self
                .signals
                .health()
                .and_then(|health| health.model_version)
                .context("signal service health unavailable")
                .and_then(|version| {
                    self.validate_live_gate(&self.strategy_version(&version), Utc::now())
                });
            if let Err(reason) = validation {
                let _ = self.storage.set_directional_mode(MentionMode::Paper);
                error!("Directional live mode downgraded to paper: {:#}", reason);
                self.telegram
                    .send_alert(&format!(
                        "Directional live mode downgraded to paper: {reason:#}"
                    ))
                    .await;
                let _ = self.log_system_decision(
                    "directional-gate",
                    "downgraded",
                    &format!("live gate failed: {reason:#}"),
                );
                return;
            }
        }
        if state.kill_switch || state.loss_breaker {
            return;
        }
        for signal in signal_by_ticker.values() {
            self.consider_entry(signal, state.mode).await;
        }
    }

    async fn sync_subscriptions(&self, signals: &[MentionSignal]) -> Result<()> {
        let mut desired: HashSet<String> =
            signals.iter().map(|s| s.market_ticker.clone()).collect();
        for position in self.storage.open_directional_positions()? {
            if position.remaining_contracts > 0 {
                desired.insert(position.market_ticker);
            }
        }
        let (add, remove) = {
            let current = self.subscribed.lock();
            (
                desired.difference(&current).cloned().collect::<Vec<_>>(),
                current.difference(&desired).cloned().collect::<Vec<_>>(),
            )
        };
        if !add.is_empty() && !self.kalshi.subscribe_directional_markets(add).await? {
            anyhow::bail!("Kalshi WebSocket did not accept mention ticker subscriptions");
        }
        if !remove.is_empty() && !self.kalshi.unsubscribe_directional_markets(remove).await? {
            anyhow::bail!("Kalshi WebSocket did not accept mention ticker unsubscriptions");
        }
        *self.subscribed.lock() = desired;
        Ok(())
    }

    async fn consider_entry(&self, signal: &MentionSignal, mode: MentionMode) {
        let now = Utc::now();
        let (Some(p_yes), Some(p_no)) = (signal.p_lo, signal.p_hi.map(|upper| 1.0 - upper)) else {
            return self.reject(signal, "none", "signal has no interval").await;
        };
        if signal.status != "ok" || !signal.is_usable_at(now, self.signals.max_age()) {
            return self
                .reject(
                    signal,
                    "none",
                    signal
                        .reason
                        .as_deref()
                        .unwrap_or("signal stale or abstained"),
                )
                .await;
        }
        let Some(close_time) = signal.close_time else {
            return self
                .reject(signal, "none", "market close time unavailable")
                .await;
        };
        if close_time <= now + chrono::Duration::minutes(self.config.max_close_minutes) {
            return self
                .reject(signal, "none", "market too close to resolution")
                .await;
        }
        if self.config.pre_event_only {
            match pre_event_entry_cutoff(&signal.market_ticker) {
                Some(cutoff) if now < cutoff => {}
                Some(cutoff) => {
                    return self
                        .reject(
                            signal,
                            "none",
                            &format!(
                                "pre-event entry window closed at {}",
                                cutoff.format("%Y-%m-%d %H:%M UTC")
                            ),
                        )
                        .await
                }
                None => {
                    return self
                        .reject(
                            signal,
                            "none",
                            "event date could not be parsed from ticker; pre-event restriction fails closed",
                        )
                        .await
                }
            }
        }
        let Some(book) = self
            .kalshi
            .get_fresh_orderbook(&signal.market_ticker, DIRECTIONAL_BOOK_MAX_AGE)
        else {
            return self
                .reject(signal, "none", "fresh Kalshi WebSocket book unavailable")
                .await;
        };
        let predictor_version = signal.model_version.as_deref().unwrap_or_default();
        let strategy_version = self.strategy_version(predictor_version);
        let event_day = event_day_from_ticker(&signal.market_ticker)
            .unwrap_or_else(|| now.format("%Y-%m-%d").to_string());
        let calendar_day = now.format("%Y-%m-%d").to_string();
        let mode_label = match mode {
            MentionMode::Live => "live",
            _ => "paper",
        };
        let (total_open, market_open, event_open, day_used, open_count) =
            match self.storage.directional_risk_totals(
                &signal.market_ticker,
                signal.event_id.as_deref().unwrap_or(""),
                &calendar_day,
                mode_label,
            ) {
                Ok(totals) => totals,
                Err(reason) => {
                    return self
                        .reject(
                            signal,
                            "none",
                            &format!("risk accounting failed: {reason:#}"),
                        )
                        .await
                }
            };
        if open_count >= i64::from(self.config.max_open_positions) {
            return self
                .reject(signal, "none", "maximum open directional positions reached")
                .await;
        }
        let market_budget =
            self.config.bankroll_usd * self.config.max_market_risk_pct - market_open;
        let event_budget = self.config.bankroll_usd * self.config.max_event_risk_pct - event_open;
        let day_budget = self.config.bankroll_usd * self.config.max_day_risk_pct - day_used;
        let total_budget =
            self.config.bankroll_usd * self.config.max_total_open_risk_pct - total_open;
        let budget = market_budget
            .min(event_budget)
            .min(day_budget)
            .min(total_budget);
        if budget <= 0.0 {
            return self
                .reject(signal, "none", "bankroll exposure cap exhausted")
                .await;
        }
        let yes_plan = plan_entry(&book, "yes", p_yes, budget, &self.config);
        let no_plan = plan_entry(&book, "no", p_no, budget, &self.config);
        let (side, fill, edge) = match (yes_plan, no_plan) {
            (Some(yes), Some(no)) if yes.1 >= no.1 => ("yes", yes.0, yes.1),
            (Some(_), Some(no)) => ("no", no.0, no.1),
            (Some(yes), None) => ("yes", yes.0, yes.1),
            (None, Some(no)) => ("no", no.0, no.1),
            (None, None) => return self
                .reject(
                    signal,
                    "none",
                    "neither side clears edge, spread, depth, and bankroll safeguards after VWAP",
                )
                .await,
        };
        let position_id = match mode {
            MentionMode::Paper => self.persist_entry(
                signal,
                side,
                &event_day,
                &strategy_version,
                fill,
                close_time,
                "paper",
                "open",
                None,
            ),
            MentionMode::Live => {
                let gate_result = self.live_gate(signal, &strategy_version, now);
                if let Err(reason) = gate_result {
                    let _ = self.storage.set_directional_mode(MentionMode::Paper);
                    error!("Directional live mode downgraded to paper: {:#}", reason);
                    self.telegram
                        .send_alert(&format!(
                            "Directional live mode downgraded to paper: {reason:#}"
                        ))
                        .await;
                    return self
                        .reject(
                            signal,
                            side,
                            &format!("live gate failed; downgraded to paper: {reason:#}"),
                        )
                        .await;
                }
                let client_order_id = Uuid::new_v4().to_string();
                match self.persist_entry_with_client_id(
                    signal,
                    side,
                    &event_day,
                    &strategy_version,
                    fill,
                    close_time,
                    "live",
                    "submitting",
                    &client_order_id,
                    None,
                ) {
                    Ok(id) => {
                        self.submit_live_entry(id, signal, side, fill, &client_order_id)
                            .await
                    }
                    Err(reason) => Err(reason),
                }
            }
            MentionMode::Off => return self.reject(signal, side, "directional mode is off").await,
        };
        match position_id {
            Ok(_) => {
                let _ = self.log_decision(
                    signal,
                    side,
                    Some(edge),
                    Some(fill),
                    fill.contracts,
                    "entered",
                    "paper fill or live fill acknowledged",
                );
            }
            Err(reason) => {
                self.reject(signal, side, &format!("entry failed: {reason:#}"))
                    .await
            }
        }
    }

    fn live_gate(
        &self,
        signal: &MentionSignal,
        expected_version: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let health = self.signals.health().context("signal health unavailable")?;
        if health.model_version.as_deref() != signal.model_version.as_deref() {
            anyhow::bail!("predictor model changed since signal was fetched");
        }
        self.validate_live_gate(expected_version, now)
    }

    fn validate_live_gate(&self, expected_version: &str, now: DateTime<Utc>) -> Result<()> {
        if self.storage.has_unresolved_directional_submission()? {
            anyhow::bail!("an unresolved directional order intent blocks live entries");
        }
        let health = self.signals.health().context("signal health unavailable")?;
        let verdict = GateArtifact {
            edge_estimate: health.latest_verdict.edge_estimate,
            ci_low: health.latest_verdict.ci_low,
            ci_high: health.latest_verdict.ci_high,
            n_days: health.latest_verdict.n_days,
            as_of: health.latest_verdict.as_of,
            model_version: health.latest_verdict.model_version,
            live_gate_passed: health.latest_verdict.live_gate_passed.unwrap_or(false),
            status: health
                .latest_verdict
                .status
                .unwrap_or_else(|| "unavailable".to_string()),
        };
        verdict.check(expected_version, &self.config, now)
    }

    fn persist_entry(
        &self,
        signal: &MentionSignal,
        side: &str,
        day: &str,
        version: &str,
        fill: FillEstimate,
        close_time: DateTime<Utc>,
        mode: &str,
        status: &str,
        order_id: Option<&str>,
    ) -> Result<i64> {
        self.persist_entry_with_client_id(
            signal,
            side,
            day,
            version,
            fill,
            close_time,
            mode,
            status,
            &format!("PAPER-{}", Uuid::new_v4()),
            order_id,
        )
    }

    fn persist_entry_with_client_id(
        &self,
        signal: &MentionSignal,
        side: &str,
        day: &str,
        version: &str,
        fill: FillEstimate,
        close_time: DateTime<Utc>,
        mode: &str,
        status: &str,
        client_order_id: &str,
        order_id: Option<&str>,
    ) -> Result<i64> {
        let conservative_probability = if side == "yes" {
            signal.p_lo.unwrap_or(0.0)
        } else {
            1.0 - signal.p_hi.unwrap_or(1.0)
        };
        let id = self
            .storage
            .create_directional_position(NewDirectionalPosition {
                market_ticker: &signal.market_ticker,
                event_id: signal.event_id.as_deref().unwrap_or(""),
                event_day: day,
                side,
                mode,
                model_version: version,
                p_hat: signal.p_hat.unwrap_or(0.5),
                p_lo: signal.p_lo.unwrap_or(0.0),
                p_hi: signal.p_hi.unwrap_or(1.0),
                contracts: fill.contracts,
                entry_vwap: fill.vwap,
                entry_fees: fill.fees,
                slippage: fill.slippage,
                predicted_edge: conservative_probability
                    - fill.vwap
                    - fill.fees / fill.contracts as f64,
                close_time: Some(&close_time.to_rfc3339()),
                client_order_id,
                order_id,
                status,
            })?;
        Ok(id)
    }

    async fn submit_live_entry(
        &self,
        id: i64,
        signal: &MentionSignal,
        side: &str,
        fill: FillEstimate,
        client_order_id: &str,
    ) -> Result<i64> {
        let limit_price = levels_for_side(&self.kalshi, &signal.market_ticker, side, true)
            .and_then(|levels| walk_buy(&levels, fill.contracts))
            .map(|estimate| estimate.limit_price_cents)
            .context("Fresh book disappeared before live submission")?;
        let intent_id = self.storage.create_directional_order_intent(
            id,
            client_order_id,
            &signal.market_ticker,
            "buy",
            side,
            fill.contracts,
            limit_price,
        )?;
        match self
            .kalshi
            .submit_order_with_client_order_id(
                &signal.market_ticker,
                "buy",
                side,
                fill.contracts,
                limit_price,
                client_order_id,
            )
            .await
        {
            Ok(result) if result.filled_contracts > fill.contracts => {
                self.storage.update_directional_entry_ack(
                    id,
                    None,
                    0,
                    fill.vwap,
                    0.0,
                    "unresolved",
                )?;
                self.storage.finish_directional_order_intent(
                    intent_id,
                    None,
                    0,
                    "unresolved",
                    Some("reported fill exceeds durable requested quantity"),
                )?;
                anyhow::bail!("Kalshi reported a fill larger than the persisted request; manual reconciliation required")
            }
            Ok(result) if result.fill_quantity_valid && result.filled_contracts > 0 => {
                let actual_fill = result.average_fill_price.unwrap_or(fill.vwap);
                self.storage.update_directional_entry_ack(
                    id,
                    result.order_id.as_deref(),
                    result.filled_contracts,
                    actual_fill,
                    fill.fees,
                    "open",
                )?;
                self.storage.finish_directional_order_intent(
                    intent_id,
                    result.order_id.as_deref(),
                    result.filled_contracts,
                    "filled",
                    None,
                )?;
                Ok(id)
            }
            Ok(result) if result.fill_quantity_valid => {
                self.storage.update_directional_entry_ack(
                    id,
                    result.order_id.as_deref(),
                    0,
                    fill.vwap,
                    0.0,
                    "rejected",
                )?;
                self.storage.finish_directional_order_intent(
                    intent_id,
                    result.order_id.as_deref(),
                    0,
                    "rejected",
                    result.error.as_deref(),
                )?;
                anyhow::bail!(
                    "Kalshi IOC order had no positive fill: {}",
                    result.error.unwrap_or_else(|| "no fill".into())
                )
            }
            Ok(_) => {
                self.storage.update_directional_entry_ack(
                    id,
                    None,
                    0,
                    fill.vwap,
                    0.0,
                    "unresolved",
                )?;
                self.storage.finish_directional_order_intent(
                    intent_id,
                    None,
                    0,
                    "unresolved",
                    Some("invalid fill quantity"),
                )?;
                anyhow::bail!("Kalshi returned invalid fill quantity; intent remains unresolved")
            }
            Err(submit_error) => {
                let created = Utc::now().timestamp() - 60;
                match self
                    .kalshi
                    .get_order_by_client_order_id(client_order_id, created)
                    .await
                {
                    Ok(Some(order)) => {
                        let count = parse_order_count(&order["fill_count_fp"]).unwrap_or(0);
                        if count > 0 {
                            let order_id = order["order_id"].as_str();
                            self.storage.update_directional_entry_ack(
                                id, order_id, count, fill.vwap, fill.fees, "open",
                            )?;
                            self.storage.finish_directional_order_intent(
                                intent_id, order_id, count, "filled", None,
                            )?;
                            Ok(id)
                        } else {
                            self.storage.update_directional_entry_ack(
                                id,
                                order["order_id"].as_str(),
                                0,
                                fill.vwap,
                                0.0,
                                "rejected",
                            )?;
                            self.storage.finish_directional_order_intent(
                                intent_id,
                                order["order_id"].as_str(),
                                0,
                                "rejected",
                                Some("reconciled zero fill"),
                            )?;
                            anyhow::bail!("Order lookup found the IOC with zero fill after submission error: {submit_error:#}")
                        }
                    }
                    Ok(None) => {
                        self.storage.update_directional_entry_ack(
                            id,
                            None,
                            0,
                            fill.vwap,
                            0.0,
                            "unresolved",
                        )?;
                        self.storage.finish_directional_order_intent(
                            intent_id,
                            None,
                            0,
                            "unresolved",
                            Some(&format!("submit ambiguous: {submit_error:#}")),
                        )?;
                        anyhow::bail!("Submit result ambiguous and exact client_order_id not found; refusing retry: {submit_error:#}")
                    }
                    Err(lookup_error) => {
                        self.storage.update_directional_entry_ack(
                            id,
                            None,
                            0,
                            fill.vwap,
                            0.0,
                            "unresolved",
                        )?;
                        self.storage.finish_directional_order_intent(
                            intent_id,
                            None,
                            0,
                            "unresolved",
                            Some(&format!("lookup failed: {lookup_error:#}")),
                        )?;
                        anyhow::bail!("Submit result ambiguous ({submit_error:#}); lookup failed ({lookup_error:#}); refusing retry")
                    }
                }
            }
        }
    }

    async fn manage_exit(&self, position: &DirectionalPosition, signal: Option<&MentionSignal>) {
        let now = Utc::now();
        let time_stop = !self.config.allow_hold_to_settlement
            && position
                .close_time
                .as_deref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|close| {
                    close.with_timezone(&Utc)
                        <= now + chrono::Duration::minutes(self.config.max_close_minutes)
                })
                .unwrap_or(true);
        let should_exit_by_signal = signal
            .filter(|value| value.is_usable_at(now, self.signals.max_age()))
            .and_then(|value| {
                let probability = match position.side.as_str() {
                    "yes" => value.p_lo?,
                    "no" => 1.0 - value.p_hi?,
                    _ => return None,
                };
                let book = self
                    .kalshi
                    .get_fresh_orderbook(&position.market_ticker, DIRECTIONAL_BOOK_MAX_AGE)?;
                let bids = sell_levels(&book, &position.side);
                let (bid_cents, _) = bids.first().copied()?;
                let fee = kalshi_fee(1, bid_cents as f64 / 100.0);
                Some(probability < bid_cents as f64 / 100.0 - fee)
            })
            .unwrap_or(false);
        if !time_stop && !should_exit_by_signal {
            return;
        }
        let Some(book) = self
            .kalshi
            .get_fresh_orderbook(&position.market_ticker, DIRECTIONAL_BOOK_MAX_AGE)
        else {
            let _ = self.storage.record_directional_action(
                position.id,
                "exit",
                "rejected",
                "time stop or model trigger fired, but fresh sell book unavailable",
                &json!({}),
            );
            return;
        };
        let levels = sell_levels(&book, &position.side);
        let Some(fill) = walk_sell(&levels, position.remaining_contracts) else {
            let _ = self.storage.record_directional_action(
                position.id,
                "exit",
                "rejected",
                "no executable sell depth",
                &json!({}),
            );
            return;
        };
        let mode = position.mode.as_str();
        let vwap = fill.vwap;
        if mode == "live" {
            let client_order_id = Uuid::new_v4().to_string();
            let created = Utc::now().timestamp() - 60;
            let intent_id = match self.storage.create_directional_order_intent(
                position.id,
                &client_order_id,
                &position.market_ticker,
                "sell",
                &position.side,
                fill.contracts,
                fill.limit_price_cents,
            ) {
                Ok(id) => id,
                Err(error) => {
                    let _ = self.storage.record_directional_action(
                        position.id,
                        "exit",
                        "persistence_failed",
                        &format!("refusing live exit because intent persistence failed: {error:#}"),
                        &json!({}),
                    );
                    return;
                }
            };
            match self
                .kalshi
                .submit_order_with_client_order_id(
                    &position.market_ticker,
                    "sell",
                    &position.side,
                    fill.contracts,
                    fill.limit_price_cents,
                    &client_order_id,
                )
                .await
            {
                Ok(result) if result.filled_contracts > fill.contracts => {
                    let _ = self.storage.finish_directional_order_intent(
                        intent_id,
                        None,
                        0,
                        "unresolved",
                        Some("reported exit fill exceeds request"),
                    );
                    let _=self.storage.record_directional_action(position.id,"exit","unresolved",
                        "Kalshi reported an exit fill larger than the persisted request; manual reconciliation required",&json!({"client_order_id":client_order_id}));
                }
                Ok(result) if result.fill_quantity_valid && result.filled_contracts > 0 => {
                    let count = result.filled_contracts.min(position.remaining_contracts);
                    let proceeds = result.average_fill_price.unwrap_or(vwap);
                    self.storage
                        .finish_directional_order_intent(
                            intent_id,
                            result.order_id.as_deref(),
                            count,
                            "filled",
                            None,
                        )
                        .ok();
                    let allocated_entry_fee =
                        position.entry_fees * count as f64 / position.contracts.max(1) as f64;
                    let pnl = (proceeds - position.entry_vwap) * count as f64
                        - allocated_entry_fee
                        - fill.fees;
                    let remaining = position.remaining_contracts - count;
                    let status = if remaining == 0 { "closed" } else { "open" };
                    let _ = self.storage.close_directional_position(
                        position.id,
                        count,
                        proceeds,
                        fill.fees,
                        pnl,
                        fill.slippage,
                        status,
                    );
                }
                Ok(_) => {
                    self.storage
                        .finish_directional_order_intent(
                            intent_id,
                            None,
                            0,
                            "rejected",
                            Some("invalid or zero fill acknowledgement"),
                        )
                        .ok();
                    let _ = self.storage.record_directional_action(
                        position.id,
                        "exit",
                        "unresolved",
                        "live exit acknowledgement did not include a positive valid fill",
                        &json!({"client_order_id":client_order_id}),
                    );
                }
                Err(error) => match self
                    .kalshi
                    .get_order_by_client_order_id(&client_order_id, created)
                    .await
                {
                    Ok(Some(order)) => {
                        let count = parse_order_count(&order["fill_count_fp"])
                            .unwrap_or(0)
                            .min(position.remaining_contracts);
                        if count > 0 {
                            let proceeds = parse_price(&order["yes_price_dollars"]).unwrap_or(vwap);
                            let allocated_entry_fee = position.entry_fees * count as f64
                                / position.contracts.max(1) as f64;
                            let pnl = (proceeds - position.entry_vwap) * count as f64
                                - allocated_entry_fee
                                - fill.fees;
                            let remaining = position.remaining_contracts - count;
                            let _ = self.storage.finish_directional_order_intent(
                                intent_id,
                                order["order_id"].as_str(),
                                count,
                                "filled",
                                None,
                            );
                            let _ = self.storage.close_directional_position(
                                position.id,
                                count,
                                proceeds,
                                fill.fees,
                                pnl,
                                fill.slippage,
                                if remaining == 0 { "closed" } else { "open" },
                            );
                        } else {
                            let _ = self.storage.finish_directional_order_intent(
                                intent_id,
                                order["order_id"].as_str(),
                                0,
                                "rejected",
                                Some("reconciled zero fill"),
                            );
                        }
                    }
                    Ok(None) => {
                        let _ = self.storage.finish_directional_order_intent(
                            intent_id,
                            None,
                            0,
                            "unresolved",
                            Some(&format!("submit ambiguous: {error:#}")),
                        );
                        let _ = self.storage.record_directional_action(
                            position.id,
                            "exit",
                            "unresolved",
                            &format!("exit submit ambiguous; no retry: {error:#}"),
                            &json!({"client_order_id":client_order_id}),
                        );
                    }
                    Err(lookup_error) => {
                        let _ = self.storage.finish_directional_order_intent(
                            intent_id,
                            None,
                            0,
                            "unresolved",
                            Some(&format!("lookup failed: {lookup_error:#}")),
                        );
                        let _ = self.storage.record_directional_action(position.id,"exit","unresolved",
                            &format!("exit submit ambiguous; lookup failed; no retry: {error:#}; {lookup_error:#}"),&json!({"client_order_id":client_order_id}));
                    }
                },
            }
        } else {
            let allocated_entry_fee =
                position.entry_fees * fill.contracts as f64 / position.contracts.max(1) as f64;
            let pnl = (vwap - position.entry_vwap) * fill.contracts as f64
                - allocated_entry_fee
                - fill.fees;
            let remaining = position.remaining_contracts - fill.contracts;
            let status = if remaining == 0 { "closed" } else { "open" };
            let _ = self.storage.close_directional_position(
                position.id,
                fill.contracts,
                vwap,
                fill.fees,
                pnl,
                fill.slippage,
                status,
            );
            let _ = self.storage.record_directional_action(position.id,"paper_exit","filled",
                if time_stop {"configured time stop"} else {"model edge fell below executable liquidation value"},
                &json!({"contracts":fill.contracts,"exit_vwap":vwap,"net_pnl":pnl,"remaining":remaining}));
            self.check_daily_loss_breaker("paper").await;
        }
    }

    async fn reconcile_unresolved(&self) {
        let intents = match self.storage.unresolved_directional_order_intents() {
            Ok(v) => v,
            Err(_) => return,
        };
        for intent in intents {
            let min_ts = DateTime::parse_from_rfc3339(&intent.created_at)
                .map(|value| value.timestamp())
                .unwrap_or_else(|_| Utc::now().timestamp() - 120);
            match self
                .kalshi
                .get_order_by_client_order_id(&intent.client_order_id, min_ts)
                .await
            {
                Ok(Some(order)) => {
                    let filled = parse_order_count(&order["fill_count_fp"]).unwrap_or(0);
                    if filled > intent.contracts {
                        warn!("Kalshi order {} reports {} fills over requested {}; keeping intent unresolved",intent.client_order_id,filled,intent.contracts);
                        continue;
                    }
                    let order_id = order["order_id"].as_str();
                    if filled <= 0 {
                        let _ = self.storage.finish_directional_order_intent(
                            intent.id,
                            order_id,
                            0,
                            "rejected",
                            Some("reconciled zero fill"),
                        );
                        if intent.action == "buy" {
                            let _ = self.storage.update_directional_entry_ack(
                                intent.position_id,
                                order_id,
                                0,
                                0.0,
                                0.0,
                                "rejected",
                            );
                        }
                        continue;
                    }
                    let position = match self.storage.directional_position(intent.position_id) {
                        Ok(Some(value)) => value,
                        _ => continue,
                    };
                    if intent.action == "buy" {
                        let average = order["average_fill_price"]
                            .as_f64()
                            .or_else(|| order["average_fill_price"].as_str()?.parse().ok())
                            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
                            .unwrap_or(position.entry_vwap);
                        let _ = self.storage.update_directional_entry_ack(
                            intent.position_id,
                            order_id,
                            filled,
                            average,
                            position.entry_fees,
                            "open",
                        );
                        let _ = self.storage.finish_directional_order_intent(
                            intent.id, order_id, filled, "filled", None,
                        );
                    } else {
                        let key = if intent.side == "yes" {
                            "yes_price_dollars"
                        } else {
                            "no_price_dollars"
                        };
                        let proceeds = parse_price(&order[key]).unwrap_or(position.entry_vwap);
                        let allocated_entry_fee =
                            position.entry_fees * filled as f64 / position.contracts.max(1) as f64;
                        let fees = order["taker_fees_dollars"]
                            .as_f64()
                            .or_else(|| order["taker_fees_dollars"].as_str()?.parse().ok())
                            .unwrap_or(0.0);
                        let pnl = (proceeds - position.entry_vwap) * filled as f64
                            - allocated_entry_fee
                            - fees;
                        let remaining = position.remaining_contracts - filled;
                        let _ = self.storage.close_directional_position(
                            intent.position_id,
                            filled,
                            proceeds,
                            fees,
                            pnl,
                            0.0,
                            if remaining == 0 { "closed" } else { "open" },
                        );
                        let _ = self.storage.finish_directional_order_intent(
                            intent.id, order_id, filled, "filled", None,
                        );
                    }
                }
                Ok(None) => warn!(
                    "Unresolved directional order {} remains unmatched; no retry",
                    intent.client_order_id
                ),
                Err(error) => warn!(
                    "Directional client-order reconciliation failed for {}: {:#}",
                    intent.client_order_id, error
                ),
            }
        }
        let positions = match self.storage.open_directional_positions() {
            Ok(v) => v,
            Err(_) => return,
        };
        for position in positions.iter().filter(|p| {
            p.mode == "live" && matches!(p.status.as_str(), "submitting" | "unresolved")
        }) {
            let min_ts = DateTime::parse_from_rfc3339(&position.created_at)
                .map(|v| v.timestamp())
                .unwrap_or_else(|_| Utc::now().timestamp() - 60);
            match self.kalshi.get_order_by_client_order_id(&position.client_order_id,min_ts).await {
                Ok(Some(order)) => {
                    let count=parse_order_count(&order["fill_count_fp"]).unwrap_or(0);
                    if count>position.contracts {
                        warn!("Kalshi order {} reports {} fills over requested {}; keeping position unresolved",position.client_order_id,count,position.contracts);
                        continue;
                    }
                    if count>0 {
                        let _=self.storage.update_directional_entry_ack(position.id,order["order_id"].as_str(),count,position.entry_vwap,position.entry_fees,"open");
                    } else {
                        let _=self.storage.update_directional_entry_ack(position.id,order["order_id"].as_str(),0,position.entry_vwap,0.0,"rejected");
                    }
                }
                Ok(None) => warn!("Unresolved directional client_order_id {} still not located; live entries remain blocked",position.client_order_id),
                Err(error) => warn!("Directional order reconciliation failed for {}: {:#}",position.client_order_id,error),
            }
        }
    }

    async fn reconcile_outcomes(&self) {
        let positions = match self.storage.directional_positions_missing_outcome() {
            Ok(v) => v,
            Err(_) => return,
        };
        for position in positions {
            let detail = match self.kalshi.get_market(&position.market_ticker).await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let market = detail.get("market").unwrap_or(&detail);
            let result = market["result"].as_str();
            let Some(outcome_yes) = (match result {
                Some("yes") => Some(true),
                Some("no") => Some(false),
                _ => None,
            }) else {
                continue;
            };
            let settlement_pnl =
                if position.remaining_contracts > 0 && self.config.allow_hold_to_settlement {
                    let won = (position.side == "yes") == outcome_yes;
                    let remaining = position.remaining_contracts as f64;
                    let allocated_fee =
                        position.entry_fees * remaining / position.contracts.max(1) as f64;
                    Some(
                        ((if won { 1.0 } else { 0.0 }) - position.entry_vwap) * remaining
                            - allocated_fee,
                    )
                } else {
                    None
                };
            if position.remaining_contracts > 0 && !self.config.allow_hold_to_settlement {
                let won = (position.side == "yes") == outcome_yes;
                let remaining = position.remaining_contracts as f64;
                let allocated_fee =
                    position.entry_fees * remaining / position.contracts.max(1) as f64;
                let forced_pnl = ((if won { 1.0 } else { 0.0 }) - position.entry_vwap) * remaining
                    - allocated_fee;
                self.telegram.send_alert(&format!("Directional position {} reached settlement before its required exit; residual settled by Kalshi.",position.market_ticker)).await;
                let _ = self.storage.resolve_directional_outcome(
                    position.id,
                    outcome_yes,
                    Some(forced_pnl),
                );
                self.check_daily_loss_breaker(&position.mode).await;
                continue;
            }
            let _ =
                self.storage
                    .resolve_directional_outcome(position.id, outcome_yes, settlement_pnl);
            if settlement_pnl.is_some() {
                self.check_daily_loss_breaker(&position.mode).await;
            }
        }
    }

    /// Trips the shared breaker when `mode`'s realized P&L for today breaches the
    /// configured loss limit. Idempotent within a day so polling does not spam alerts.
    async fn check_daily_loss_breaker(&self, mode: &str) {
        let day = Utc::now().format("%Y-%m-%d").to_string();
        if self.storage.directional_state().is_ok_and(|state| {
            state.loss_breaker && state.breaker_day.as_deref() == Some(day.as_str())
        }) {
            return;
        }
        if self
            .storage
            .directional_day_pnl(&day, mode)
            .map(|pnl| pnl <= -self.config.bankroll_usd * self.config.daily_loss_breaker_pct)
            .unwrap_or(false)
        {
            let _ = self.storage.trip_directional_loss_breaker(&day);
            error!("Directional daily loss breaker tripped for {day} ({mode})");
            self.telegram
                .send_alert(&format!(
                    "Directional daily loss breaker tripped for {day} ({mode} P&L); manual reset required."
                ))
                .await;
        }
    }

    async fn reject(&self, signal: &MentionSignal, side: &str, reason: &str) {
        if !self.should_log_rejection(&signal.market_ticker, reason) {
            return;
        }
        let _ = self.log_decision(signal, side, None, None, 0, "rejected", reason);
    }

    /// Rejections repeat identically on every poll; log a market again only when its
    /// reason changes or `REJECTION_LOG_INTERVAL` has elapsed. Entries are never deduped.
    fn should_log_rejection(&self, ticker: &str, reason: &str) -> bool {
        let now = Instant::now();
        let mut seen = self.last_rejection.lock();
        match seen.get(ticker) {
            Some((previous, at))
                if previous == reason && now.duration_since(*at) < REJECTION_LOG_INTERVAL =>
            {
                false
            }
            _ => {
                seen.insert(ticker.to_string(), (reason.to_string(), now));
                true
            }
        }
    }

    fn log_decision(
        &self,
        signal: &MentionSignal,
        side: &str,
        edge: Option<f64>,
        fill: Option<FillEstimate>,
        contracts: i32,
        decision: &str,
        reason: &str,
    ) -> Result<()> {
        let (yes_bid, yes_ask) = self
            .kalshi
            .get_fresh_orderbook(&signal.market_ticker, DIRECTIONAL_BOOK_MAX_AGE)
            .map(|book| {
                (
                    book.yes.last().map(|(price, _)| f64::from(*price) / 100.0),
                    book.no
                        .last()
                        .map(|(price, _)| 1.0 - f64::from(*price) / 100.0),
                )
            })
            .unwrap_or((None, None));
        self.storage
            .save_directional_decision(NewDirectionalDecision {
                market_ticker: &signal.market_ticker,
                event_id: signal.event_id.as_deref(),
                event_day: event_day_from_ticker(&signal.market_ticker).as_deref(),
                side: Some(side),
                model_version: signal.model_version.as_deref(),
                p_hat: signal.p_hat,
                p_lo: signal.p_lo,
                p_hi: signal.p_hi,
                yes_bid,
                yes_ask,
                net_edge: edge,
                vwap: fill.map(|value| value.vwap),
                fee_per_contract: fill.map(|value| value.fees / value.contracts as f64),
                contracts,
                decision,
                reason,
            })?;
        Ok(())
    }

    fn log_system_decision(&self, ticker: &str, decision: &str, reason: &str) -> Result<()> {
        self.storage
            .save_directional_decision(NewDirectionalDecision {
                market_ticker: ticker,
                event_id: None,
                event_day: None,
                side: None,
                model_version: None,
                p_hat: None,
                p_lo: None,
                p_hi: None,
                yes_bid: None,
                yes_ask: None,
                net_edge: None,
                vwap: None,
                fee_per_contract: None,
                contracts: 0,
                decision,
                reason,
            })?;
        Ok(())
    }
}

/// Stable identifier for the predictor version plus every config field that changes
/// trading behavior. SHA-256 (not `DefaultHasher`) so the value is identical across
/// builds and Rust releases and can be persisted and compared against the paper gate.
pub fn compute_strategy_version(config: &MentionConfig, predictor_version: &str) -> String {
    let canonical = format!(
        "v2|{:016x}|{:016x}|{:016x}|{:016x}|{}|{}|{}|{}|{:016x}|{:016x}|{:016x}|{:016x}|{}|{:016x}|{}|{}|{}",
        config.bankroll_usd.to_bits(),
        config.kelly_fraction.to_bits(),
        config.min_net_edge.to_bits(),
        config.max_spread.to_bits(),
        config.min_book_depth_contracts,
        config.max_close_minutes,
        config.allow_hold_to_settlement,
        config.pre_event_only,
        config.max_market_risk_pct.to_bits(),
        config.max_event_risk_pct.to_bits(),
        config.max_day_risk_pct.to_bits(),
        config.max_total_open_risk_pct.to_bits(),
        config.max_open_positions,
        config.daily_loss_breaker_pct.to_bits(),
        config.max_signal_age_secs,
        config.live_gate_min_days,
        config.live_gate_max_age_days,
    );
    let digest = Sha256::digest(canonical.as_bytes());
    format!("{}:{}", predictor_version, &hex::encode(digest)[..16])
}

fn levels_for_side(
    kalshi: &KalshiClient,
    ticker: &str,
    side: &str,
    buy: bool,
) -> Option<Vec<(i32, i32)>> {
    let book = kalshi.get_fresh_orderbook(ticker, DIRECTIONAL_BOOK_MAX_AGE)?;
    Some(if buy {
        buy_levels(&book, side)
    } else {
        sell_levels(&book, side)
    })
}

fn parse_order_count(value: &Value) -> Option<i32> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0 && *v <= i32::MAX as f64)
        .map(|v| v.floor() as i32)
}

fn parse_price(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())
        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
}

/// Matches the signal service's `-([A-Z]*\d{2}[A-Z]{3}\d{2})(?:-|$)` event-day token and
/// returns it together with the calendar date it encodes (yyMonDD).
fn parse_event_day(ticker: &str) -> Option<(String, NaiveDate)> {
    const MONTHS: [&str; 12] = [
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ];
    for part in ticker.split('-').skip(1) {
        if !part.is_ascii() {
            continue;
        }
        let prefix = part
            .bytes()
            .take_while(|byte| byte.is_ascii_uppercase())
            .count();
        let rest = &part[prefix..];
        if rest.len() != 7
            || !rest[0..2].bytes().all(|byte| byte.is_ascii_digit())
            || !rest[5..7].bytes().all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        let Some(month) = MONTHS.iter().position(|name| *name == &rest[2..5]) else {
            continue;
        };
        let (Ok(year), Ok(day)) = (rest[0..2].parse::<i32>(), rest[5..7].parse::<u32>()) else {
            continue;
        };
        if let Some(date) = NaiveDate::from_ymd_opt(2000 + year, month as u32 + 1, day) {
            return Some((part.to_string(), date));
        }
    }
    None
}

fn event_day_from_ticker(ticker: &str) -> Option<String> {
    parse_event_day(ticker).map(|(token, _)| token)
}

/// Latest instant a pre-event entry is allowed: 04:00 UTC on the ticker's event date,
/// which is 00:00 EDT (and 23:00 the previous evening EST), i.e. never later than
/// midnight Eastern at the start of the event day.
fn pre_event_entry_cutoff(ticker: &str) -> Option<DateTime<Utc>> {
    let (_, date) = parse_event_day(ticker)?;
    Some(date.and_hms_opt(4, 0, 0)?.and_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_buy_uses_vwap_and_each_level_fee_rounds_up() {
        let fill = walk_buy(&[(40, 1), (50, 2)], 3).unwrap();
        assert_eq!(fill.contracts, 3);
        assert!((fill.vwap - 0.4666666666666667).abs() < 1e-9);
        assert_eq!(fill.limit_price_cents, 50);
        assert_eq!(fill.fees, 0.06);
        assert!((fill.slippage - 0.10).abs() < 1e-9);
    }

    #[test]
    fn no_side_uses_complementary_yes_bid_as_ask() {
        let mut book = OrderBook::default();
        book.yes = vec![(40, 2), (45, 3)];
        book.no = vec![(50, 1), (55, 2)];
        assert_eq!(buy_levels(&book, "no"), vec![(55, 3), (60, 2)]);
        assert_eq!(sell_levels(&book, "no"), vec![(55, 2), (50, 1)]);
    }

    #[test]
    fn fractional_kelly_is_zero_without_positive_edge() {
        assert_eq!(fractional_kelly_contracts(0.4, 0.5, 1000.0, 0.25), 0);
        assert!(fractional_kelly_contracts(0.7, 0.5, 1000.0, 0.25) > 0);
    }

    #[test]
    fn event_day_parses_with_and_without_letter_prefix() {
        assert_eq!(
            event_day_from_ticker("KXTRUMPSAY-26JUN15-XYZ").as_deref(),
            Some("26JUN15")
        );
        assert_eq!(
            event_day_from_ticker("KXTRUMPMENTION-T26JUN15").as_deref(),
            Some("T26JUN15")
        );
        // The series segment is never an event day, and bad dates are rejected.
        assert_eq!(event_day_from_ticker("26JUN15"), None);
        assert_eq!(event_day_from_ticker("KXTRUMPSAY-26FOO15-A"), None);
        assert_eq!(event_day_from_ticker("KXTRUMPSAY-26FEB31-A"), None);
        assert_eq!(event_day_from_ticker("KXTRUMPSAY-ABC"), None);
    }

    #[test]
    fn pre_event_cutoff_is_midnight_eastern_or_earlier() {
        let cutoff = pre_event_entry_cutoff("KXTRUMPSAY-26JUN15-A").unwrap();
        assert_eq!(cutoff.to_rfc3339(), "2026-06-15T04:00:00+00:00");
        // 04:00 UTC is 00:00 EDT; in EST it is 23:00 the previous evening (earlier = stricter).
        assert!(pre_event_entry_cutoff("KXTRUMPSAY-not-a-date").is_none());
    }

    #[test]
    fn strategy_version_is_deterministic_and_tracks_config() {
        let base = MentionConfig::default();
        let first = compute_strategy_version(&base, "pred-1");
        assert_eq!(first, compute_strategy_version(&base, "pred-1"));
        assert!(first.starts_with("pred-1:"));
        assert_eq!(first.len(), "pred-1:".len() + 16);
        assert_ne!(first, compute_strategy_version(&base, "pred-2"));

        let mut changed = base.clone();
        changed.min_net_edge += 0.01;
        assert_ne!(first, compute_strategy_version(&changed, "pred-1"));
        let mut changed = base.clone();
        changed.pre_event_only = !changed.pre_event_only;
        assert_ne!(first, compute_strategy_version(&changed, "pred-1"));
    }
}
