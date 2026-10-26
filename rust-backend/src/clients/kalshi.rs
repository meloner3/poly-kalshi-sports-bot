//! Kalshi platform client
//!
//! Handles Kalshi API interactions including:
//! - RSA-PSS authentication
//! - Market data retrieval
//! - WebSocket order book subscription
//! - Order placement

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use parking_lot::RwLock;
use reqwest::Client;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::pss::{BlindedSigningKey, Signature};
use rsa::sha2::Sha256;
use rsa::signature::{RandomizedSigner, SignatureEncoding};
use rsa::RsaPrivateKey;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

use crate::config::KalshiConfig;
use crate::core::{competitor_event_name, normalize_competitor_name};
use crate::models::{KalshiEvent, KalshiMarket, Platform, PriceUpdate};

const KALSHI_WS_URL: &str = "wss://external-api-ws.kalshi.com/trade-api/ws/v2";
const KALSHI_MARKET_QUOTE_BATCH_SIZE: usize = 100;
const KALSHI_GET_MAX_RETRIES: u32 = 3;

// Kalshi's explicitly supported head-to-head match-winner series. Keep this
// allowlist narrow: it intentionally excludes sets, games, totals, props,
// futures, and every table-tennis series.
const SUPPORTED_KALSHI_SERIES: &[(&str, &str)] = &[
    ("KXNBAGAME", "NBA"),
    ("KXATPMATCH", "TENNIS"),
    ("KXWTAMATCH", "TENNIS"),
    ("KXITFMATCH", "TENNIS"),
    ("KXITFWMATCH", "TENNIS"),
    ("KXATPCHALLENGERMATCH", "TENNIS"),
];

/// Subscription command for Kalshi WebSocket
#[derive(Debug, Clone)]
pub enum KalshiWsCommand {
    Subscribe(Vec<String>),
    Unsubscribe(Vec<String>),
}

/// Kalshi API client
#[derive(Clone)]
pub struct KalshiClient {
    pub config: KalshiConfig,
    http: Client,
    signing_key: Arc<BlindedSigningKey<Sha256>>,
    /// Order book cache: market_ticker -> { "yes": [[price, qty], ...], "no": [[price, qty], ...] }
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    /// Channel sender for dynamic subscriptions/unsubscriptions
    command_tx: Arc<RwLock<Option<mpsc::Sender<KalshiWsCommand>>>>,
    /// Mention tickers re-subscribed on reconnect; empty unless directional is running.
    directional_subscriptions: Arc<RwLock<HashSet<String>>>,
    /// Incremented every time a new WebSocket connection is established.
    /// Kalshi only pushes `orderbook_delta` on real book changes, not on a
    /// heartbeat, so "time since last delta" is the wrong staleness signal
    /// for a quiet market - it would reject an accurate book just because
    /// nothing traded recently. A book is trustworthy iff it was built
    /// entirely under the currently-active, unbroken connection; tagging
    /// each book with the epoch it was last written under (see
    /// `OrderBook::epoch`) makes that checkable without any elapsed-time
    /// guess, and a reconnect still correctly invalidates every book until
    /// fresh snapshots arrive under the new epoch.
    connection_epoch: Arc<AtomicU64>,
    /// Set on every WebSocket message received, regardless of ticker - a
    /// connection-level liveness signal, separate from any one book's
    /// staleness. Backstops the epoch check against a connection that's
    /// gone silent without erroring (e.g. a hung read).
    last_message_at: Arc<RwLock<Option<Instant>>>,
}

/// Order book structure
#[derive(Debug, Clone, Default)]
pub struct OrderBook {
    pub yes: Vec<(i32, i32)>, // (price_cents, quantity)
    pub no: Vec<(i32, i32)>,
    updated_at: Option<Instant>,
    /// The WebSocket connection epoch this book was last written under. 0
    /// (the Default) never matches a real connection's epoch (which starts
    /// at 1), so a book that was never epoch-tagged is correctly untrusted.
    epoch: u64,
}

/// Authoritative price-only quote from Kalshi's REST market endpoint.
///
/// This intentionally excludes quoted sizes: REST prices must not be used to
/// fabricate order-book depth for trading safeguards.
#[derive(Debug, Clone)]
pub struct KalshiMarketQuote {
    pub market_id: String,
    pub yes_ask: f64,
    pub no_ask: f64,
    /// Kalshi's own market lifecycle status: "unopened", "open", "paused",
    /// "closed", or "settled". Used to detect a closed/settled market
    /// immediately rather than waiting on the extreme-price heuristic in
    /// `market_lifecycle.rs`, which can miss a market that closes without
    /// its price ever drifting to an extreme value.
    pub status: String,
}

impl KalshiMarketQuote {
    /// A marketable limit price (cents) for a manual, user-initiated,
    /// known-quantity order on a ticker we hold no WebSocket book for (e.g.
    /// a position whose market has fallen out of the scanner's actively
    /// tracked/subscribed set). REST only carries asks, so a sell price is
    /// approximated as the complement of the opposite side's ask - the same
    /// yes/no complement convention already used throughout this codebase
    /// (e.g. `OrderBook::limit_price_for_order`,
    /// `paired_execution::kalshi_buy_levels`). This is a price-only
    /// fallback for a single manual order with an already-known contract
    /// count; it must never be used to size or feed depth for auto-trade,
    /// which stays exclusively on the fresh WebSocket book.
    pub fn limit_price_for_order(&self, side: &str, action: &str) -> Option<i32> {
        let yes_ask_cents = (self.yes_ask * 100.0).round() as i32;
        let no_ask_cents = (self.no_ask * 100.0).round() as i32;
        match (action, side) {
            ("buy", "yes") => Some(yes_ask_cents),
            ("buy", "no") => Some(no_ask_cents),
            ("sell", "yes") => 100_i32.checked_sub(no_ask_cents),
            ("sell", "no") => 100_i32.checked_sub(yes_ask_cents),
            _ => None,
        }
    }
}

/// A parsed immediate-or-cancel order acknowledgement. A submitted order is
/// only considered filled when Kalshi returned an explicit positive fill count.
#[derive(Debug, Clone)]
pub struct KalshiOrderResult {
    pub accepted: bool,
    pub filled_contracts: i32,
    pub fill_quantity_valid: bool,
    pub order_id: Option<String>,
    pub status: Option<String>,
    pub error: Option<String>,
    pub average_fill_price: Option<f64>,
}

impl OrderBook {
    /// 计算 yes 侧的 ask 深度（买入 yes 时使用）
    /// Kalshi: yes_ask = 1 - no_bid，所以买 yes 的深度看 no 侧的 bid
    /// no 按价格升序排列，last 是最高买价（best no_bid）
    /// 只看最优档位的深度，避免累加多档位导致过于乐观的深度估计
    pub fn yes_ask_depth(&self, _max_contracts: i32) -> i32 {
        // 只返回最优档位（No 侧最高 bid）的深度
        self.no.last().map(|(_, qty)| *qty).unwrap_or(0)
    }

    /// 计算 no 侧的 ask 深度（买入 no 时使用）
    /// no_ask = 1 - yes_bid，所以买 no 的深度看 yes 侧的 bid
    /// 只看最优档位的深度，避免累加多档位导致过于乐观的深度估计
    pub fn no_ask_depth(&self, _max_contracts: i32) -> i32 {
        // 只返回最优档位（Yes 侧最高 bid）的深度
        self.yes.last().map(|(_, qty)| *qty).unwrap_or(0)
    }

    /// 根据 side 获取对应的 ask 深度
    pub fn ask_depth_for_side(&self, side: &str, max_contracts: i32) -> i32 {
        match side.to_lowercase().as_str() {
            "yes" => self.yes_ask_depth(max_contracts),
            "no" => self.no_ask_depth(max_contracts),
            _ => 0,
        }
    }

    /// Return the executable limit price for an order using the current best quote.
    pub fn limit_price_for_order(&self, side: &str, action: &str) -> Option<i32> {
        let yes_bid = self.yes.last().map(|(price, _)| *price);
        let no_bid = self.no.last().map(|(price, _)| *price);

        match (action, side) {
            ("buy", "yes") => no_bid.and_then(|price| 100_i32.checked_sub(price)),
            ("buy", "no") => yes_bid.and_then(|price| 100_i32.checked_sub(price)),
            ("sell", "yes") => yes_bid,
            ("sell", "no") => no_bid,
            _ => None,
        }
    }
}

impl KalshiClient {
    /// Create a new Kalshi client
    pub fn new(config: KalshiConfig) -> Result<Self> {
        // Parse RSA private key - 支持 PKCS#8 和 PKCS#1 两种格式
        let private_key = RsaPrivateKey::from_pkcs8_pem(&config.api_secret)
            .or_else(|_| RsaPrivateKey::from_pkcs1_pem(&config.api_secret))
            .with_context(|| {
                "Failed to parse Kalshi RSA private key (tried both PKCS#8 and PKCS#1 formats)"
            })?;
        let signing_key = Arc::new(BlindedSigningKey::<Sha256>::new(private_key));

        let http = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("Failed to build Kalshi HTTP client")?;

        Ok(Self {
            config,
            http,
            signing_key,
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            command_tx: Arc::new(RwLock::new(None)),
            directional_subscriptions: Arc::new(RwLock::new(HashSet::new())),
            connection_epoch: Arc::new(AtomicU64::new(0)),
            last_message_at: Arc::new(RwLock::new(None)),
        })
    }

    /// Sign a request using RSA-PSS
    fn sign_request(&self, timestamp: i64, method: &str, path: &str) -> String {
        let message = format!("{}{}{}", timestamp, method, path);
        let mut rng = rand::thread_rng();
        let signature: Signature = self.signing_key.sign_with_rng(&mut rng, message.as_bytes());
        BASE64.encode(signature.to_bytes())
    }

    /// Kalshi signatures cover the API path but never query parameters.
    fn signing_path(path: &str) -> String {
        format!("/trade-api/v2{}", path.split('?').next().unwrap_or(path))
    }

    /// Get current timestamp in milliseconds
    fn get_timestamp_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    /// Make an authenticated GET request. A transport-level failure (a
    /// connection timeout, reset, or DNS error before any response arrives)
    /// is retried with exponential backoff (1s/2s, 3 attempts total) since a
    /// GET can never have caused a side effect on Kalshi's side - unlike
    /// `post()`, which places orders and must never be blindly retried. A
    /// real Kalshi API error (a non-success HTTP status) is not retried.
    async fn get(&self, path: &str) -> Result<Value> {
        let mut attempt = 0u32;
        let response = loop {
            let timestamp = Self::get_timestamp_ms();
            let sign_path = Self::signing_path(path);
            let signature = self.sign_request(timestamp, "GET", &sign_path);
            let url = format!("{}{}", self.config.base_url, path);

            match self
                .http
                .get(&url)
                .header("KALSHI-ACCESS-KEY", &self.config.api_key)
                .header("KALSHI-ACCESS-SIGNATURE", &signature)
                .header("KALSHI-ACCESS-TIMESTAMP", timestamp.to_string())
                .send()
                .await
            {
                Ok(response) => break response,
                Err(err) if attempt + 1 < KALSHI_GET_MAX_RETRIES => {
                    let delay = Duration::from_secs(2u64.pow(attempt));
                    warn!(
                        "Kalshi GET {} failed ({}), retrying in {:?} (attempt {}/{})",
                        path,
                        err,
                        delay,
                        attempt + 1,
                        KALSHI_GET_MAX_RETRIES
                    );
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(err) => return Err(err.into()),
            }
        };

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            bail!("Kalshi API error {}: {}", status, body);
        }

        serde_json::from_str(&body).with_context(|| format!("Failed to parse response: {}", body))
    }

    /// Make an authenticated POST request
    async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let timestamp = Self::get_timestamp_ms();
        let sign_path = Self::signing_path(path);
        let signature = self.sign_request(timestamp, "POST", &sign_path);

        let url = format!("{}{}", self.config.base_url, path);

        let response = self
            .http
            .post(&url)
            .header("KALSHI-ACCESS-KEY", &self.config.api_key)
            .header("KALSHI-ACCESS-SIGNATURE", &signature)
            .header("KALSHI-ACCESS-TIMESTAMP", timestamp.to_string())
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await?;

        let status = response.status();
        let resp_body = response.text().await?;

        if !status.is_success() {
            bail!("Kalshi API error {}: {}", status, resp_body);
        }

        serde_json::from_str(&resp_body)
            .with_context(|| format!("Failed to parse response: {}", resp_body))
    }

    /// Get account balance
    pub async fn get_balance(&self) -> Result<f64> {
        let response = self.get("/portfolio/balance").await?;
        let balance = response["balance"]
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("Invalid balance response"))?;
        Ok(balance / 100.0) // Convert cents to dollars
    }

    /// Get order book from cache
    pub fn get_orderbook(&self, ticker: &str) -> Option<OrderBook> {
        self.orderbook_cache.read().get(ticker).cloned()
    }

    /// Return only a websocket book we can vouch for. REST quotes are
    /// intentionally not used for executable depth or automatic execution.
    ///
    /// "Fresh" means: this book was last written under the *currently
    /// active* connection (a reconnect bumps the epoch and invalidates
    /// every book until it's resnapshotted), not merely "changed recently" -
    /// Kalshi only sends a delta on a real book change, so a quiet market
    /// can go untouched for a while while still being perfectly accurate.
    /// `max_age` backstops that against a connection that's gone silent
    /// without erroring (e.g. a hung read): it's checked against the last
    /// message received on *any* ticker, not this one, since a quiet book
    /// isn't evidence of a dead connection.
    pub fn get_fresh_orderbook(&self, ticker: &str, max_age: Duration) -> Option<OrderBook> {
        let current_epoch = self.connection_epoch.load(Ordering::SeqCst);
        let connection_alive = self
            .last_message_at
            .read()
            .is_some_and(|last| last.elapsed() <= max_age);
        if !connection_alive {
            return None;
        }
        self.get_orderbook(ticker)
            .filter(|book| book.epoch == current_epoch)
    }

    /// Fetch authoritative current ask quotes for the requested market tickers.
    ///
    /// Kalshi supports a comma-separated `tickers` filter. Requests are
    /// bounded so a large matched-market set cannot create an oversized URL.
    /// Sizes are deliberately not returned because only the WebSocket
    /// order-book cache is allowed to satisfy depth checks.
    pub async fn get_market_quotes(&self, tickers: &[String]) -> Result<Vec<KalshiMarketQuote>> {
        let mut seen = HashSet::new();
        let unique_tickers: Vec<String> = tickers
            .iter()
            .filter(|ticker| seen.insert((*ticker).clone()))
            .cloned()
            .collect();
        let mut quotes = Vec::with_capacity(unique_tickers.len());

        for ticker_batch in unique_tickers.chunks(KALSHI_MARKET_QUOTE_BATCH_SIZE) {
            let path = {
                let mut query = url::form_urlencoded::Serializer::new(String::new());
                query.append_pair("tickers", &ticker_batch.join(","));
                query.append_pair("limit", &ticker_batch.len().to_string());
                format!("/markets?{}", query.finish())
            };
            let response = self.get(&path).await?;
            let markets = response["markets"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Invalid Kalshi markets quote response"))?;

            for market in markets {
                quotes.push(parse_market_rest_quote(market)?);
            }
        }

        Ok(quotes)
    }

    /// Get all supported NBA and tennis match-winner events and markets.
    pub async fn get_supported_events_and_markets(
        &self,
    ) -> Result<(Vec<KalshiEvent>, Vec<KalshiMarket>)> {
        let mut events = Vec::new();
        let mut markets = Vec::new();

        for &(series_ticker, category) in SUPPORTED_KALSHI_SERIES {
            let (mut series_events, mut series_markets) = self
                .get_series_events_and_markets(series_ticker, category)
                .await?;
            events.append(&mut series_events);
            markets.append(&mut series_markets);
        }

        info!(
            "Loaded {} supported Kalshi events and {} markets",
            events.len(),
            markets.len()
        );

        Ok((events, markets))
    }

    /// Fetch only the legacy NBA feed for callers that do not use the
    /// supported-sports scan.
    #[allow(dead_code)]
    pub async fn get_nba_events_and_markets(
        &self,
    ) -> Result<(Vec<KalshiEvent>, Vec<KalshiMarket>)> {
        self.get_series_events_and_markets("KXNBAGAME", "NBA").await
    }

    async fn get_series_events_and_markets(
        &self,
        series_ticker: &str,
        category: &str,
    ) -> Result<(Vec<KalshiEvent>, Vec<KalshiMarket>)> {
        let mut events = Vec::new();
        let mut markets = Vec::new();
        let mut cursor = None;

        loop {
            let path = {
                let mut query = url::form_urlencoded::Serializer::new(String::new());
                query.append_pair("series_ticker", series_ticker);
                query.append_pair("status", "open");
                query.append_pair("with_nested_markets", "true");
                query.append_pair("limit", "200");
                if let Some(cursor) = cursor.as_deref() {
                    query.append_pair("cursor", cursor);
                }
                format!("/events?{}", query.finish())
            };
            let response = self.get(&path).await?;
            let event_array = response["events"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Invalid events response"))?;

            for event_data in event_array {
                let event = match category {
                    "NBA" => parse_nba_event(event_data),
                    "TENNIS" => parse_tennis_event(event_data),
                    _ => None,
                };

                if let Some(event) = event {
                    markets.extend(event.markets.iter().cloned());
                    events.push(event);
                }
            }

            let next_cursor = response["cursor"].as_str().unwrap_or_default();
            if next_cursor.is_empty() {
                break;
            }
            cursor = Some(next_cursor.to_string());
        }

        info!(
            "Loaded {} Kalshi {} events and {} markets",
            category,
            events.len(),
            markets.len()
        );

        Ok((events, markets))
    }

    /// Place a market order - fill immediately at best available prices
    pub async fn place_order(
        &self,
        ticker: &str,
        action: &str,
        outcome: &str,
        count: i32,
        price: i32,
    ) -> Result<Value> {
        let (book_side, yes_price_cents) = v2_order_book_side_and_price(action, outcome, price)?;
        let body = v2_order_payload(ticker, book_side, count, yes_price_cents)?;

        self.post("/portfolio/events/orders", &body).await
    }

    pub async fn place_order_with_client_order_id(
        &self,
        ticker: &str,
        action: &str,
        outcome: &str,
        count: i32,
        price: i32,
        client_order_id: &str,
    ) -> Result<Value> {
        let (book_side, yes_price_cents) = v2_order_book_side_and_price(action, outcome, price)?;
        let body =
            v2_order_payload_with_id(ticker, book_side, count, yes_price_cents, client_order_id)?;
        self.post("/portfolio/events/orders", &body).await
    }

    /// Submit an IOC order and parse only explicit fill data from the
    /// acknowledgement. Unknown acknowledgement shapes fail closed.
    pub async fn submit_order(
        &self,
        ticker: &str,
        action: &str,
        outcome: &str,
        count: i32,
        price: i32,
    ) -> Result<KalshiOrderResult> {
        let response = self
            .place_order(ticker, action, outcome, count, price)
            .await?;
        Ok(parse_kalshi_order_result(&response, outcome))
    }

    pub async fn submit_order_with_client_order_id(
        &self,
        ticker: &str,
        action: &str,
        outcome: &str,
        count: i32,
        price: i32,
        client_order_id: &str,
    ) -> Result<KalshiOrderResult> {
        let response = self
            .place_order_with_client_order_id(
                ticker,
                action,
                outcome,
                count,
                price,
                client_order_id,
            )
            .await?;
        Ok(parse_kalshi_order_result(&response, outcome))
    }

    /// Get orders with optional status filter
    pub async fn get_orders(&self, status: Option<&str>) -> Result<Value> {
        let path = if let Some(s) = status {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query.append_pair("status", s);
            format!("/portfolio/orders?{}", query.finish())
        } else {
            "/portfolio/orders".to_string()
        };

        self.get(&path).await
    }

    /// Kalshi returns `client_order_id` on each order but does not expose it
    /// as a server-side filter. Page the documented time-window query and
    /// match the exact id locally. Absence is not proof of non-submission when
    /// the scan fails or the order has moved to historical storage.
    pub async fn get_order_by_client_order_id(
        &self,
        client_order_id: &str,
        min_timestamp: i64,
    ) -> Result<Option<Value>> {
        let mut cursor: Option<String> = None;
        for _ in 0..100 {
            let path = {
                let mut query = url::form_urlencoded::Serializer::new(String::new());
                query.append_pair("min_ts", &min_timestamp.to_string());
                query.append_pair("limit", "1000");
                if let Some(value) = cursor.as_deref() {
                    query.append_pair("cursor", value);
                }
                format!("/portfolio/orders?{}", query.finish())
            };
            let response = self.get(&path).await?;
            let orders = response["orders"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Invalid Kalshi orders response"))?;
            if let Some(order) = orders
                .iter()
                .find(|order| order["client_order_id"].as_str() == Some(client_order_id))
            {
                return Ok(Some(order.clone()));
            }
            let next = response["cursor"].as_str().unwrap_or_default();
            if next.is_empty() || orders.is_empty() {
                return Ok(None);
            }
            cursor = Some(next.to_string());
        }
        anyhow::bail!("Kalshi order lookup exceeded pagination safety limit")
    }

    /// Get positions
    pub async fn get_positions(&self) -> Result<Value> {
        self.get("/portfolio/positions").await
    }

    pub async fn get_market(&self, ticker: &str) -> Result<Value> {
        let escaped = url::form_urlencoded::byte_serialize(ticker.as_bytes()).collect::<String>();
        self.get(&format!("/markets/{escaped}")).await
    }

    /// Cancel an order
    pub async fn cancel_order(&self, order_id: &str) -> Result<Value> {
        let timestamp = Self::get_timestamp_ms();
        let path = format!("/portfolio/events/orders/{}", order_id);
        let sign_path = Self::signing_path(&path);
        let signature = self.sign_request(timestamp, "DELETE", &sign_path);

        let url = format!("{}{}", self.config.base_url, path);

        let response = self
            .http
            .delete(&url)
            .header("KALSHI-ACCESS-KEY", &self.config.api_key)
            .header("KALSHI-ACCESS-SIGNATURE", &signature)
            .header("KALSHI-ACCESS-TIMESTAMP", timestamp.to_string())
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            bail!("Kalshi API error {}: {}", status, body);
        }

        serde_json::from_str(&body).with_context(|| format!("Failed to parse response: {}", body))
    }

    /// Subscribe to additional markets dynamically (hot subscription)
    ///
    /// This can be called after the WebSocket connection is established
    /// to add new market subscriptions.
    pub async fn subscribe_markets(&self, tickers: Vec<String>) -> Result<bool> {
        if tickers.is_empty() {
            return Ok(true);
        }

        let tx = self.command_tx.read().clone();
        if let Some(tx) = tx {
            match tx.send(KalshiWsCommand::Subscribe(tickers.clone())).await {
                Ok(_) => {
                    info!(
                        "🔌 [Kalshi] Sending hot-subscription request for {} markets",
                        tickers.len()
                    );
                    Ok(true)
                }
                Err(e) => {
                    warn!("⚠️ [Kalshi] Hot-subscription request failed: {}", e);
                    Ok(false)
                }
            }
        } else {
            warn!("⚠️ [Kalshi] WebSocket is not connected; cannot hot-subscribe");
            Ok(false)
        }
    }

    /// Unsubscribe from markets dynamically
    ///
    /// This can be called after the WebSocket connection is established
    /// to remove market subscriptions for ended games.
    pub async fn unsubscribe_markets(&self, tickers: Vec<String>) -> Result<bool> {
        if tickers.is_empty() {
            return Ok(true);
        }

        let tx = self.command_tx.read().clone();
        if let Some(tx) = tx {
            match tx.send(KalshiWsCommand::Unsubscribe(tickers.clone())).await {
                Ok(_) => {
                    info!(
                        "🔌 [Kalshi] Sending unsubscribe request for {} markets",
                        tickers.len()
                    );
                    Ok(true)
                }
                Err(e) => {
                    warn!("⚠️ [Kalshi] Unsubscribe request failed: {}", e);
                    Ok(false)
                }
            }
        } else {
            warn!("⚠️ [Kalshi] WebSocket is not connected; cannot unsubscribe");
            Ok(false)
        }
    }

    /// Directional (mention) variant of `subscribe_markets`. Records the tickers so they
    /// are re-subscribed after a reconnect, then delegates to the shared hot-subscribe.
    pub async fn subscribe_directional_markets(&self, tickers: Vec<String>) -> Result<bool> {
        self.directional_subscriptions
            .write()
            .extend(tickers.iter().cloned());
        self.subscribe_markets(tickers).await
    }

    /// Directional (mention) variant of `unsubscribe_markets`.
    pub async fn unsubscribe_directional_markets(&self, tickers: Vec<String>) -> Result<bool> {
        {
            let mut subscriptions = self.directional_subscriptions.write();
            for ticker in &tickers {
                subscriptions.remove(ticker);
            }
        }
        self.unsubscribe_markets(tickers).await
    }

    /// Connect to WebSocket for real-time updates
    pub async fn connect_websocket(
        &self,
        tickers: Vec<String>,
        price_tx: mpsc::Sender<PriceUpdate>,
    ) -> Result<()> {
        let mut tickers = tickers;
        {
            // Re-subscribe directional (mention) tickers after a reconnect. The set is
            // empty unless the directional service is running, leaving the paired
            // path's ticker list exactly as the caller supplied it.
            let directional = self.directional_subscriptions.read();
            for ticker in directional.iter() {
                if !tickers.contains(ticker) {
                    tickers.push(ticker.clone());
                }
            }
        }
        let timestamp = Self::get_timestamp_ms();
        let signature = self.sign_request(timestamp, "GET", "/trade-api/ws/v2");

        // 使用 HTTP headers 传递认证信息（与 Python 版本一致）
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = KALSHI_WS_URL.into_client_request()?;
        request
            .headers_mut()
            .insert("KALSHI-ACCESS-KEY", self.config.api_key.parse().unwrap());
        request
            .headers_mut()
            .insert("KALSHI-ACCESS-SIGNATURE", signature.parse().unwrap());
        request.headers_mut().insert(
            "KALSHI-ACCESS-TIMESTAMP",
            timestamp.to_string().parse().unwrap(),
        );

        info!("Connecting to the Kalshi WebSocket...");

        let (ws_stream, _) = connect_async(request)
            .await
            .with_context(|| "连接 Kalshi WebSocket 失败")?;

        // A fresh connection means no book we've cached so far can be
        // vouched for until it's rewritten under this epoch - see
        // `get_fresh_orderbook`.
        let current_epoch = self.connection_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        *self.last_message_at.write() = Some(Instant::now());

        let (mut write, mut read) = ws_stream.split();
        let mut pending_subscription_ids = HashMap::new();
        let mut subscription_ids = HashMap::new();

        // Create channel for dynamic subscriptions/unsubscriptions
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<KalshiWsCommand>(100);
        *self.command_tx.write() = Some(cmd_tx);

        // Subscribe to initial order books - 逐个订阅（与 Python 版本一致）
        let mut next_msg_id = 1;
        for ticker in tickers.iter() {
            pending_subscription_ids.insert(next_msg_id, ticker.clone());
            let subscribe_msg = json!({
                "id": next_msg_id,
                "cmd": "subscribe",
                "params": {
                    "channels": ["orderbook_delta"],
                    "market_ticker": ticker  // 单个 ticker，不是数组
                }
            });
            next_msg_id += 1;

            write.send(Message::Text(subscribe_msg.to_string())).await?;
        }

        info!("Subscribed to {} Kalshi markets", tickers.len());

        let orderbook_cache = self.orderbook_cache.clone();

        // Process messages with dynamic subscription/unsubscription support
        loop {
            tokio::select! {
                // Handle incoming WebSocket messages
                msg = read.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            *self.last_message_at.write() = Some(Instant::now());
                            Self::record_subscription(
                                &text,
                                &mut pending_subscription_ids,
                                &mut subscription_ids,
                            );
                            if let Some(update) = Self::parse_ws_message(&text, &orderbook_cache, current_epoch) {
                                if price_tx.send(update).await.is_err() {
                                    warn!("Price update channel has closed");
                                    break;
                                }
                            }
                        }
                        Some(Ok(Message::Close(_))) => {
                            info!("Kalshi WebSocket closed");
                            break;
                        }
                        Some(Err(e)) => {
                            error!("Kalshi WebSocket error: {:#}", e);
                            break;
                        }
                        None => {
                            info!("Kalshi WebSocket stream ended");
                            break;
                        }
                        Some(Ok(_)) => {
                            // Ping/Pong/Binary frames still confirm the
                            // connection is alive.
                            *self.last_message_at.write() = Some(Instant::now());
                        }
                    }
                }
                // Handle dynamic subscription/unsubscription requests
                Some(command) = cmd_rx.recv() => {
                    match command {
                        KalshiWsCommand::Subscribe(new_tickers) => {
                            info!("🔌 [Kalshi] Processing hot subscription for {} new markets", new_tickers.len());
                            for ticker in new_tickers.iter() {
                                pending_subscription_ids.insert(next_msg_id, ticker.clone());
                                let subscribe_msg = json!({
                                    "id": next_msg_id,
                                    "cmd": "subscribe",
                                    "params": {
                                        "channels": ["orderbook_delta"],
                                        "market_ticker": ticker
                                    }
                                });
                                next_msg_id += 1;

                                if let Err(e) = write.send(Message::Text(subscribe_msg.to_string())).await {
                                    error!("❌ [Kalshi] Hot subscription send failed: {:#}", e);
                                }
                            }
                            info!("✅ [Kalshi] Hot subscription completed for {} markets", new_tickers.len());
                        }
                        KalshiWsCommand::Unsubscribe(tickers_to_unsub) => {
                            let ticker_count = tickers_to_unsub.len();
                            info!("🔌 [Kalshi] Processing unsubscribe for {} markets", ticker_count);
                            let subscription_ids_to_remove: Vec<u64> = tickers_to_unsub
                                .iter()
                                .filter_map(|ticker| {
                                    let sid = subscription_ids.remove(ticker);
                                    if sid.is_none() {
                                        warn!("Kalshi subscription for {} is not active", ticker);
                                    }
                                    sid
                                })
                                .collect();

                            if !subscription_ids_to_remove.is_empty() {
                                let unsubscribe_msg = json!({
                                    "id": next_msg_id,
                                    "cmd": "unsubscribe",
                                    "params": {
                                        "sids": subscription_ids_to_remove
                                    }
                                });
                                next_msg_id += 1;

                                if let Err(e) = write.send(Message::Text(unsubscribe_msg.to_string())).await {
                                    error!("❌ [Kalshi] Unsubscribe send failed: {:#}", e);
                                }
                            }
                            for ticker in tickers_to_unsub {
                                orderbook_cache.write().remove(&ticker);
                            }
                            info!("✅ [Kalshi] Unsubscribe completed for {} markets", ticker_count);
                        }
                    }
                }
            }
        }

        // Clear command channel on disconnect
        *self.command_tx.write() = None;

        Ok(())
    }

    /// Associate Kalshi's server subscription ID with the market that initiated it.
    fn record_subscription(
        text: &str,
        pending_subscription_ids: &mut HashMap<u64, String>,
        subscription_ids: &mut HashMap<String, u64>,
    ) {
        let Ok(data) = serde_json::from_str::<Value>(text) else {
            return;
        };
        if data["type"].as_str() != Some("subscribed") {
            return;
        }

        let (Some(request_id), Some(subscription_id)) =
            (data["id"].as_u64(), data["msg"]["sid"].as_u64())
        else {
            warn!("Kalshi returned an invalid subscription confirmation");
            return;
        };

        if let Some(ticker) = pending_subscription_ids.remove(&request_id) {
            subscription_ids.insert(ticker, subscription_id);
        } else {
            warn!(
                "Kalshi returned an unexpected subscription confirmation: {}",
                request_id
            );
        }
    }

    /// Parse WebSocket message. `epoch` is the current connection's epoch
    /// (see `KalshiClient::connection_epoch`) and is stamped onto any book
    /// this message writes, so `get_fresh_orderbook` can tell it was built
    /// under the connection that's still active.
    fn parse_ws_message(
        text: &str,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
        epoch: u64,
    ) -> Option<PriceUpdate> {
        let data: Value = serde_json::from_str(text).ok()?;
        let msg_type = data.get("type")?.as_str()?;

        match msg_type {
            "orderbook_snapshot" => {
                let msg = data.get("msg")?;
                let ticker = msg.get("market_ticker")?.as_str()?;

                let mut book = OrderBook::default();
                book.yes = parse_price_levels(msg, "yes_dollars_fp", "yes");
                book.no = parse_price_levels(msg, "no_dollars_fp", "no");

                // Sort by price
                book.yes.sort_by_key(|(p, _)| *p);
                book.no.sort_by_key(|(p, _)| *p);
                book.updated_at = Some(Instant::now());
                book.epoch = epoch;

                orderbook_cache
                    .write()
                    .insert(ticker.to_string(), book.clone());

                // Calculate prices
                let yes_bid = book.yes.last().map(|(p, _)| *p as f64 / 100.0);
                let no_bid = book.no.last().map(|(p, _)| *p as f64 / 100.0);

                if let (Some(yb), Some(nb)) = (yes_bid, no_bid) {
                    Some(PriceUpdate {
                        platform: Platform::Kalshi,
                        market_id: ticker.to_string(),
                        yes_bid: Some(yb),
                        yes_ask: Some(1.0 - nb),
                        no_bid: Some(nb),
                        no_ask: Some(1.0 - yb),
                        timestamp: Utc::now(),
                    })
                } else {
                    None
                }
            }
            "orderbook_delta" => {
                let msg = data.get("msg")?;
                let ticker = msg.get("market_ticker")?.as_str()?;
                let price = msg
                    .get("price_dollars")
                    .and_then(dollars_to_cents)
                    .or_else(|| {
                        msg.get("price")
                            .and_then(Value::as_i64)
                            .map(|price| price as i32)
                    })?;
                let delta = msg
                    .get("delta_fp")
                    .and_then(fixed_point_to_i32)
                    .or_else(|| {
                        msg.get("delta")
                            .and_then(Value::as_i64)
                            .map(|delta| delta as i32)
                    })?;
                let side = msg.get("side")?.as_str()?;

                // Apply delta
                let mut cache = orderbook_cache.write();
                let book = cache.get_mut(ticker)?;
                // Never build on a book left over from a previous connection
                // - wait for the fresh snapshot this epoch's subscribe will
                // bring instead of layering a delta on a stale base.
                if book.epoch != epoch {
                    return None;
                }

                let book_side = if side == "yes" {
                    &mut book.yes
                } else {
                    &mut book.no
                };

                // Find and update or insert
                if let Some(pos) = book_side.iter().position(|(p, _)| *p == price) {
                    let new_qty = book_side[pos].1 + delta;
                    if new_qty <= 0 {
                        book_side.remove(pos);
                    } else {
                        book_side[pos].1 = new_qty;
                    }
                } else if delta > 0 {
                    book_side.push((price, delta));
                    book_side.sort_by_key(|(p, _)| *p);
                }
                book.updated_at = Some(Instant::now());
                book.epoch = epoch;

                // Recalculate prices
                let yes_bid = book.yes.last().map(|(p, _)| *p as f64 / 100.0);
                let no_bid = book.no.last().map(|(p, _)| *p as f64 / 100.0);

                drop(cache);

                if let (Some(yb), Some(nb)) = (yes_bid, no_bid) {
                    Some(PriceUpdate {
                        platform: Platform::Kalshi,
                        market_id: ticker.to_string(),
                        yes_bid: Some(yb),
                        yes_ask: Some(1.0 - nb),
                        no_bid: Some(nb),
                        no_ask: Some(1.0 - yb),
                        timestamp: Utc::now(),
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

fn parse_kalshi_order_result(response: &Value, outcome: &str) -> KalshiOrderResult {
    let order = response.get("order").unwrap_or(response);
    let order_id = order
        .get("order_id")
        .or_else(|| order.get("id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let status = order
        .get("status")
        .or_else(|| order.get("state"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let reported_fill = [
        "fill_count_fp",
        "fill_count",
        "filled_count",
        "filled_contracts",
    ]
    .iter()
    .find_map(|field| order.get(*field).and_then(value_to_f64));
    let fill_quantity_valid = reported_fill
        .map(|value| {
            value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= i32::MAX as f64
        })
        .unwrap_or(true);
    let filled_contracts = reported_fill
        .filter(|_| fill_quantity_valid)
        .map(|value| value as i32)
        .unwrap_or(0);
    let average_fill_price = ["average_fill_price", "average_price", "avg_price"]
        .iter()
        .find_map(|field| order.get(*field).and_then(value_to_price))
        .and_then(|price| match outcome {
            "yes" => Some(price),
            "no" => Some(1.0 - price),
            _ => None,
        });
    let error = response
        .get("error")
        .or_else(|| response.get("message"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            (!fill_quantity_valid)
                .then(|| "Kalshi returned a non-whole or invalid fill quantity".to_string())
        })
        .or_else(|| {
            (filled_contracts == 0).then(|| {
                format!(
                    "Kalshi IOC order has no explicit fill (status: {})",
                    status.as_deref().unwrap_or("unknown")
                )
            })
        });
    KalshiOrderResult {
        accepted: order_id.is_some() || status.is_some(),
        filled_contracts,
        fill_quantity_valid,
        order_id,
        status,
        error,
        average_fill_price,
    }
}

fn value_to_f64(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str()?.parse().ok())
}

fn value_to_price(value: &Value) -> Option<f64> {
    let value = value.as_f64().or_else(|| value.as_str()?.parse().ok())?;
    (value.is_finite() && (0.0..1.0).contains(&value)).then_some(value)
}

fn v2_order_book_side_and_price(
    action: &str,
    outcome: &str,
    price: i32,
) -> Result<(&'static str, i32)> {
    if !(1..=99).contains(&price) {
        bail!("Kalshi order price must be between 1 and 99 cents");
    }

    match (action, outcome) {
        ("buy", "yes") => Ok(("bid", price)),
        ("sell", "yes") => Ok(("ask", price)),
        ("buy", "no") => Ok(("ask", 100 - price)),
        ("sell", "no") => Ok(("bid", 100 - price)),
        _ => bail!("Kalshi order action must be buy or sell and outcome must be yes or no"),
    }
}

fn v2_order_payload(
    ticker: &str,
    book_side: &str,
    count: i32,
    yes_price_cents: i32,
) -> Result<Value> {
    v2_order_payload_with_id(
        ticker,
        book_side,
        count,
        yes_price_cents,
        &uuid::Uuid::new_v4().to_string(),
    )
}

fn v2_order_payload_with_id(
    ticker: &str,
    book_side: &str,
    count: i32,
    yes_price_cents: i32,
    client_order_id: &str,
) -> Result<Value> {
    if ticker.trim().is_empty()
        || client_order_id.trim().is_empty()
        || count <= 0
        || !matches!(book_side, "bid" | "ask")
        || !(1..=99).contains(&yes_price_cents)
    {
        bail!("Invalid Kalshi V2 order parameters");
    }

    Ok(json!({
        "ticker": ticker,
        "client_order_id": client_order_id,
        "side": book_side,
        "count": format!("{count}.00"),
        "price": format!("{:.4}", yes_price_cents as f64 / 100.0),
        "time_in_force": "immediate_or_cancel",
        "self_trade_prevention_type": "taker_at_cross",
        "cancel_order_on_pause": true,
    }))
}

/// Parse current fixed-point levels and the legacy integer-cent representation.
fn parse_price_levels(message: &Value, fixed_point_key: &str, legacy_key: &str) -> Vec<(i32, i32)> {
    let Some(levels) = message
        .get(fixed_point_key)
        .or_else(|| message.get(legacy_key))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    let fixed_point = message.get(fixed_point_key).is_some();
    levels
        .iter()
        .filter_map(|entry| {
            let price = entry.get(0)?;
            let quantity = entry.get(1).and_then(quantity_to_i32)?;
            let price = if fixed_point {
                dollars_to_cents(price)?
            } else {
                price.as_i64()? as i32
            };
            Some((price, quantity))
        })
        .collect()
}

/// Convert Kalshi's fixed-point dollar price to an exact whole-cent price.
fn dollars_to_cents(value: &Value) -> Option<i32> {
    let price = value
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| value.as_number().map(ToString::to_string))?;
    let (dollars, fractional) = price.split_once('.').unwrap_or((&price, ""));
    let dollars = dollars.parse::<i32>().ok()?;
    let mut cents = fractional.chars().take(2).collect::<String>();
    while cents.len() < 2 {
        cents.push('0');
    }
    if fractional.chars().skip(2).any(|digit| digit != '0') {
        return None;
    }

    let cents = dollars
        .checked_mul(100)?
        .checked_add(cents.parse::<i32>().ok()?)?;
    (0..=100).contains(&cents).then_some(cents)
}

/// Parse a Kalshi fixed-point dollar quote without accepting lossy or
/// out-of-range external values.
fn parse_quote_dollars(value: &Value, field: &str) -> Result<f64> {
    let raw = value
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Kalshi {field} must be a fixed-point string"))?;
    let (whole, fractional) = raw.split_once('.').unwrap_or((raw, ""));

    if whole.is_empty()
        || fractional.len() > 6
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
    {
        bail!("Invalid Kalshi {field} fixed-point quote: {raw}");
    }

    let price = raw
        .parse::<f64>()
        .with_context(|| format!("Invalid Kalshi {field} quote: {raw}"))?;
    if !price.is_finite() || !(0.0..=1.0).contains(&price) {
        bail!("Kalshi {field} quote is outside [0, 1]: {raw}");
    }

    Ok(price)
}

fn parse_market_rest_quote(market: &Value) -> Result<KalshiMarketQuote> {
    let market_id = market["ticker"]
        .as_str()
        .filter(|ticker| !ticker.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Kalshi market quote is missing ticker"))?
        .to_string();
    let yes_ask = parse_quote_dollars(&market["yes_ask_dollars"], "yes_ask_dollars")
        .with_context(|| format!("Invalid Kalshi REST quote for {market_id}"))?;
    let no_ask = parse_quote_dollars(&market["no_ask_dollars"], "no_ask_dollars")
        .with_context(|| format!("Invalid Kalshi REST quote for {market_id}"))?;
    let status = market["status"].as_str().unwrap_or("").to_string();

    Ok(KalshiMarketQuote {
        market_id,
        yes_ask,
        no_ask,
        status,
    })
}

fn quantity_to_i32(value: &Value) -> Option<i32> {
    fixed_point_to_i32(value).filter(|quantity| *quantity >= 0)
}

fn fixed_point_to_i32(value: &Value) -> Option<i32> {
    let quantity = value
        .as_f64()
        .or_else(|| {
            value
                .as_str()
                .and_then(|quantity| quantity.parse::<f64>().ok())
        })?
        .floor();
    (quantity.is_finite() && quantity >= i32::MIN as f64 && quantity <= i32::MAX as f64)
        .then_some(quantity as i32)
}

fn parse_nba_event(event_data: &Value) -> Option<KalshiEvent> {
    let event_ticker = event_data["event_ticker"].as_str()?.to_string();
    let (mut team_a, mut team_b) = extract_teams_from_ticker(&event_ticker)?;
    if team_a > team_b {
        std::mem::swap(&mut team_a, &mut team_b);
    }

    let event_name = format!("{team_a}-{team_b}");
    let start_time = extract_event_start_time(event_data, &event_ticker);
    let mut event = KalshiEvent {
        event_id: event_ticker.clone(),
        name: event_name.clone(),
        team_a: team_a.clone(),
        team_b: team_b.clone(),
        start_time,
        category: "NBA".to_string(),
        markets: Vec::new(),
    };

    let Some(markets) = event_data["markets"].as_array() else {
        return Some(event);
    };
    for market_data in markets {
        let Some(ticker) = market_data["ticker"].as_str().map(ToOwned::to_owned) else {
            continue;
        };
        let Some(team_name) = extract_team_from_ticker(&ticker) else {
            continue;
        };
        let opponent_name = if team_name == team_a {
            &team_b
        } else {
            &team_a
        };
        if let Some(market) = build_kalshi_market(
            market_data,
            ticker,
            event_ticker.clone(),
            event_name.clone(),
            team_name,
            opponent_name.clone(),
            start_time,
        ) {
            event.markets.push(market);
        }
    }

    Some(event)
}

fn parse_tennis_event(event_data: &Value) -> Option<KalshiEvent> {
    let event_ticker = event_data["event_ticker"].as_str()?.to_string();
    let market_data = event_data["markets"].as_array()?;

    // A match winner event must expose exactly two independent binary markets,
    // one for each competitor. Anything else is deliberately excluded.
    if market_data.len() != 2 {
        return None;
    }

    let mut parsed_markets = Vec::with_capacity(2);
    for market in market_data {
        let ticker = market["ticker"].as_str()?.to_string();
        let competitor = extract_tennis_competitor(market)?;
        parsed_markets.push((competitor, ticker, market));
    }

    if parsed_markets[0].0 == parsed_markets[1].0 {
        return None;
    }

    let mut competitors = [parsed_markets[0].0.clone(), parsed_markets[1].0.clone()];
    competitors.sort();
    let event_name = competitor_event_name(&competitors[0], &competitors[1])?;
    let start_time = extract_event_start_time(event_data, &event_ticker);
    let mut event = KalshiEvent {
        event_id: event_ticker.clone(),
        name: event_name.clone(),
        team_a: competitors[0].clone(),
        team_b: competitors[1].clone(),
        start_time,
        category: "TENNIS".to_string(),
        markets: Vec::with_capacity(2),
    };

    for (competitor, ticker, market) in parsed_markets {
        let opponent = if competitor == competitors[0] {
            competitors[1].clone()
        } else {
            competitors[0].clone()
        };
        event.markets.push(build_kalshi_market(
            market,
            ticker,
            event_ticker.clone(),
            event_name.clone(),
            competitor,
            opponent,
            start_time,
        )?);
    }

    Some(event)
}

fn build_kalshi_market(
    market_data: &Value,
    ticker: String,
    event_id: String,
    event_name: String,
    team_name: String,
    opponent_name: String,
    start_time: Option<DateTime<Utc>>,
) -> Option<KalshiMarket> {
    let yes_price = initial_yes_price(market_data)?;

    Some(KalshiMarket {
        market_id: ticker,
        event_id,
        event_name,
        team_name,
        opponent_name,
        yes_price,
        no_price: 1.0 - yes_price,
        start_time,
        volume: market_data["volume"].as_f64().or_else(|| {
            market_data["volume_fp"]
                .as_str()
                .and_then(|value| value.parse().ok())
        }),
        liquidity: market_data["open_interest"].as_f64().or_else(|| {
            market_data["open_interest_fp"]
                .as_str()
                .and_then(|value| value.parse().ok())
        }),
    })
}

fn initial_yes_price(market_data: &Value) -> Option<f64> {
    if !market_data["yes_ask_dollars"].is_null() {
        return parse_quote_dollars(&market_data["yes_ask_dollars"], "yes_ask_dollars").ok();
    }
    if !market_data["last_price_dollars"].is_null() {
        return parse_quote_dollars(&market_data["last_price_dollars"], "last_price_dollars").ok();
    }

    market_data["yes_ask"]
        .as_f64()
        .or_else(|| market_data["last_price"].as_f64())
        .filter(|price| price.is_finite() && (0.0..=100.0).contains(price))
        .map(|price| price / 100.0)
}

fn extract_tennis_competitor(market_data: &Value) -> Option<String> {
    market_data["yes_sub_title"]
        .as_str()
        .and_then(normalize_competitor_name)
        .or_else(|| {
            market_data["title"]
                .as_str()
                .and_then(extract_tennis_competitor_from_market_title)
        })
}

fn extract_tennis_competitor_from_market_title(title: &str) -> Option<String> {
    let title = title.trim();
    let title_lower = title.to_ascii_lowercase();
    let prefix = "will ";
    let separator = " win the ";

    if title_lower.starts_with(prefix) {
        let name_end = title_lower[prefix.len()..].find(separator)? + prefix.len();
        return normalize_competitor_name(&title[prefix.len()..name_end]);
    }

    let suffix = " wins";
    let competitor = title_lower.strip_suffix(suffix)?;
    normalize_competitor_name(&title[..competitor.len()])
}

fn extract_event_start_time(event_data: &Value, event_ticker: &str) -> Option<DateTime<Utc>> {
    ["expected_expiration_time", "close_time", "open_time"]
        .iter()
        .find_map(|field| {
            event_data[*field]
                .as_str()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc))
        })
        .or_else(|| extract_game_date_from_ticker(event_ticker))
}

/// Parse team names from event title
/// Extract team names from event_ticker
/// Example: "KXNBAGAME-26JAN07CLELAL" -> ("CLE", "LAL")
fn extract_teams_from_ticker(event_ticker: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = event_ticker.split('-').collect();
    if parts.len() < 2 {
        return None;
    }

    let last_part = parts.last()?;
    if last_part.len() <= 7 {
        return None;
    }

    // Skip the date part (first 7 chars like "26JAN07")
    let teams_str = &last_part[7..];

    // Most common: 6 characters (3 + 3)
    if teams_str.len() == 6 {
        return Some((teams_str[..3].to_uppercase(), teams_str[3..].to_uppercase()));
    }

    // Handle 7+ characters by splitting in the middle
    if teams_str.len() >= 4 {
        let mid = teams_str.len() / 2;
        return Some((
            teams_str[..mid].to_uppercase(),
            teams_str[mid..].to_uppercase(),
        ));
    }

    None
}

/// Extract team from market ticker
/// Example: "KXNBAGAME-26JAN07CLELAL-CLE" -> "CLE"
fn extract_team_from_ticker(ticker: &str) -> Option<String> {
    let parts: Vec<&str> = ticker.split('-').collect();
    if parts.len() < 3 {
        return None;
    }

    // Last part is the team abbreviation
    Some(parts.last()?.to_uppercase())
}

/// Extract game date from event ticker (e.g., "KXNBA-26JAN08-DAL-UTA" -> 2026-01-08)
///
/// Format: The second part contains the date as "YYMMMDD" where:
/// - YY: two-digit year (e.g., "26" for 2026)
/// - MMM: three-letter month abbreviation (e.g., "JAN")
/// - DD: two-digit day (e.g., "08")
fn extract_game_date_from_ticker(event_ticker: &str) -> Option<DateTime<Utc>> {
    let parts: Vec<&str> = event_ticker.split('-').collect();
    if parts.len() < 2 {
        return None;
    }

    let date_part = parts[1];
    if date_part.len() < 7 {
        return None;
    }

    // Parse year (first 2 characters)
    let year_str = &date_part[..2];
    let year: i32 = match year_str.parse::<i32>() {
        Ok(y) => 2000 + y,
        Err(_) => return None,
    };

    // Parse month (characters 2-5, e.g., "JAN")
    let month_str = &date_part[2..5];
    let month: u32 = match month_str.to_uppercase().as_str() {
        "JAN" => 1,
        "FEB" => 2,
        "MAR" => 3,
        "APR" => 4,
        "MAY" => 5,
        "JUN" => 6,
        "JUL" => 7,
        "AUG" => 8,
        "SEP" => 9,
        "OCT" => 10,
        "NOV" => 11,
        "DEC" => 12,
        _ => return None,
    };

    // Parse day (characters 5-7)
    let day_str = &date_part[5..7];
    let day: u32 = match day_str.parse() {
        Ok(d) => d,
        Err(_) => return None,
    };

    use chrono::NaiveDate;
    let naive_date = NaiveDate::from_ymd_opt(year, month, day)?;
    let naive_datetime = naive_date.and_hms_opt(12, 0, 0)?;

    Some(DateTime::from_naive_utc_and_offset(naive_datetime, Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(yes_ask: f64, no_ask: f64) -> KalshiMarketQuote {
        KalshiMarketQuote {
            market_id: "KXLAL".to_string(),
            yes_ask,
            no_ask,
            status: "open".to_string(),
        }
    }

    #[test]
    fn rest_quote_buy_prices_use_the_direct_ask() {
        let q = quote(0.45, 0.58);
        assert_eq!(q.limit_price_for_order("yes", "buy"), Some(45));
        assert_eq!(q.limit_price_for_order("no", "buy"), Some(58));
    }

    #[test]
    fn rest_quote_sell_prices_use_the_opposite_asks_complement() {
        let q = quote(0.45, 0.58);
        // Selling YES needs a YES bid, approximated as 100 - no_ask.
        assert_eq!(q.limit_price_for_order("yes", "sell"), Some(42));
        assert_eq!(q.limit_price_for_order("no", "sell"), Some(55));
    }

    #[test]
    fn rest_quote_rejects_an_unknown_side_or_action() {
        let q = quote(0.45, 0.58);
        assert_eq!(q.limit_price_for_order("yes", "hold"), None);
        assert_eq!(q.limit_price_for_order("maybe", "sell"), None);
    }

    #[test]
    fn does_not_treat_ioc_acknowledgement_as_fill() {
        let result = parse_kalshi_order_result(
            &json!({
                "order": {"order_id": "order-1", "status": "canceled", "fill_count": "0.00"}
            }),
            "yes",
        );
        assert!(result.accepted);
        assert_eq!(result.filled_contracts, 0);
        assert!(result.error.unwrap().contains("no explicit fill"));
    }

    #[test]
    fn parses_current_fixed_point_ioc_fill_count() {
        let result = parse_kalshi_order_result(
            &json!({
                "order": {
                    "order_id": "order-1",
                    "status": "executed",
                    "fill_count_fp": "2.00"
                }
            }),
            "yes",
        );

        assert_eq!(result.filled_contracts, 2);
        assert!(result.fill_quantity_valid);
    }

    #[test]
    fn converts_no_order_fill_price_from_the_yes_book() {
        let result = parse_kalshi_order_result(
            &json!({
                "order": {
                    "order_id": "order-1",
                    "fill_count_fp": "1.00",
                    "avg_price": "0.6700"
                }
            }),
            "no",
        );

        assert!(
            (result.average_fill_price.unwrap() - 0.33).abs() < f64::EPSILON,
            "expected a 33-cent NO fill price"
        );
    }

    #[test]
    fn floors_fractional_orderbook_depth_for_whole_contract_orders() {
        assert_eq!(fixed_point_to_i32(&json!("9.99")), Some(9));
    }

    #[test]
    fn extracts_full_tennis_identity_from_market_title() {
        assert_eq!(
            extract_tennis_competitor_from_market_title(
                "Will Juan Manuel Cerundolo win the Cerundolo vs Auger-Aliassime: Round Of 32 match?"
            ),
            Some("JUAN MANUEL CERUNDOLO".to_string())
        );
        assert_eq!(
            extract_tennis_competitor_from_market_title(
                "Will Cerundolo win the Cerundolo vs Auger-Aliassime match?"
            ),
            None
        );
        assert_eq!(
            extract_tennis_competitor_from_market_title("Jesper De Jong wins"),
            Some("JESPER DE JONG".to_string())
        );
    }

    #[test]
    fn parses_only_two_competitor_tennis_events() {
        let event = json!({
            "event_ticker": "KXATPMATCH-26AUG18TEST",
            "markets": [
                {
                    "ticker": "KXATPMATCH-26AUG18TEST-CERUNDOLO",
                    "title": "Will Juan Manuel Cerundolo win the Cerundolo vs Auger-Aliassime match?",
                    "yes_ask": 45
                },
                {
                    "ticker": "KXATPMATCH-26AUG18TEST-AUGER",
                    "title": "Will Felix Auger-Aliassime win the Cerundolo vs Auger-Aliassime match?",
                    "yes_ask": 55
                }
            ]
        });

        let parsed = parse_tennis_event(&event).unwrap();
        assert_eq!(
            parsed.name,
            "FELIX AUGER ALIASSIME VS JUAN MANUEL CERUNDOLO"
        );
        assert_eq!(parsed.markets.len(), 2);
    }

    #[test]
    fn parses_current_tennis_market_titles_and_dollar_prices() {
        let event = json!({
            "event_ticker": "KXATPMATCH-26AUG30DEJPAS",
            "markets": [
                {
                    "ticker": "KXATPMATCH-26AUG30DEJPAS-DEJ",
                    "title": "Jesper De Jong wins",
                    "yes_sub_title": "Jesper De Jong",
                    "yes_ask_dollars": "0.4400",
                    "volume_fp": "740891.24",
                    "open_interest_fp": "404822.66"
                },
                {
                    "ticker": "KXATPMATCH-26AUG30DEJPAS-PAS",
                    "title": "Francesco Passaro wins",
                    "yes_sub_title": "Francesco Passaro",
                    "yes_ask_dollars": "0.5600"
                }
            ]
        });

        let parsed = parse_tennis_event(&event).unwrap();
        assert_eq!(parsed.name, "FRANCESCO PASSARO VS JESPER DE JONG");
        assert_eq!(parsed.markets[0].yes_price, 0.44);
        assert_eq!(parsed.markets[0].volume, Some(740891.24));
        assert_eq!(parsed.markets[0].liquidity, Some(404822.66));
    }

    #[test]
    fn rejects_current_tennis_events_with_invalid_prices() {
        let event = json!({
            "event_ticker": "KXATPMATCH-26AUG30DEJPAS",
            "markets": [
                {
                    "ticker": "KXATPMATCH-26AUG30DEJPAS-DEJ",
                    "yes_sub_title": "Jesper De Jong",
                    "yes_ask_dollars": "1.1"
                },
                {
                    "ticker": "KXATPMATCH-26AUG30DEJPAS-PAS",
                    "yes_sub_title": "Francesco Passaro",
                    "yes_ask_dollars": "0.5600"
                }
            ]
        });

        assert!(parse_tennis_event(&event).is_none());
    }

    #[test]
    fn parses_rest_quote_asks_as_dollar_prices() {
        let quote = parse_market_rest_quote(&json!({
            "ticker": "KXATPMATCH-26AUG18CERAUG-CER",
            "yes_bid_dollars": "0.2500",
            "yes_ask_dollars": "0.2600",
            "no_bid_dollars": "0.7400",
            "no_ask_dollars": "0.7500"
        }))
        .unwrap();

        assert_eq!(quote.market_id, "KXATPMATCH-26AUG18CERAUG-CER");
        assert_eq!(quote.yes_ask, 0.26);
        assert_eq!(quote.no_ask, 0.75);
    }

    #[test]
    fn rejects_invalid_rest_quote_price() {
        let error = parse_market_rest_quote(&json!({
            "ticker": "KXATPMATCH-26AUG18CERAUG-CER",
            "yes_ask_dollars": "1.0000001",
            "no_ask_dollars": "0.7500"
        }))
        .unwrap_err();

        assert!(error.to_string().contains("Invalid Kalshi REST quote"));
    }

    #[test]
    fn converts_no_side_orders_to_v2_yes_book_orders() {
        assert_eq!(
            v2_order_book_side_and_price("buy", "no", 44).unwrap(),
            ("ask", 56)
        );
        assert_eq!(
            v2_order_book_side_and_price("sell", "no", 44).unwrap(),
            ("bid", 56)
        );

        let payload = v2_order_payload("KXATPMATCH-26AUG30DEJPAS-DEJ", "ask", 3, 56).unwrap();
        assert_eq!(payload["side"], "ask");
        assert_eq!(payload["count"], "3.00");
        assert_eq!(payload["price"], "0.5600");
        assert_eq!(payload["time_in_force"], "immediate_or_cancel");
        assert_eq!(payload["self_trade_prevention_type"], "taker_at_cross");
        assert!(payload.get("exchange_index").is_none());
        assert!(payload["client_order_id"].as_str().is_some());
    }

    #[test]
    fn signing_path_excludes_query_parameters() {
        assert_eq!(
            KalshiClient::signing_path("/portfolio/orders?status=open&limit=10"),
            "/trade-api/v2/portfolio/orders"
        );
    }
}
