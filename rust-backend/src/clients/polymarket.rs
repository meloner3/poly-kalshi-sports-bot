//! Polymarket platform client
//!
//! Handles Polymarket API interactions including:
//! - Market data retrieval from the Polymarket US gateway
//! - Ed25519-signed order placement directly against the Polymarket US API

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::{Client, Method, StatusCode};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;
use tracing::{debug, info, warn};

use crate::clients::polymarket_auth;
use crate::config::PolymarketConfig;
use crate::core::{competitor_event_name, normalize_competitor_name, normalize_team_name};
use crate::models::{PolymarketEvent, PolymarketMarket, PolymarketPositionSide};

// Polymarket US sport codes for the supported tennis match-winner feeds.
// Series IDs are resolved from /v1/sports at runtime so they are not baked in.
const TENNIS_SPORT_CODES: &[&str] = &["atp", "wta", "itfm", "itfw", "itfme", "itfwo", "atpcq"];

const RATE_LIMIT_MAX_RETRIES: u32 = 3;
const BALANCE_CACHE_TTL: Duration = Duration::from_secs(30);
const POSITIONS_CACHE_TTL: Duration = Duration::from_secs(30);
// minimumTradeQty / orderPriceMinTickSize are static per-market validation
// constraints, not live pricing data - cache them well past a single trading
// session so we don't add a second live API call (and more rate-limit risk)
// to every depth check and order attempt.
const MARKET_CONSTRAINTS_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Fresh, normalized native Polymarket US LONG-price order book.
#[derive(Debug, Clone, Deserialize)]
pub struct PolymarketMarketBook {
    pub success: bool,
    pub market_slug: String,
    pub state: String,
    pub transact_time: Option<String>,
    pub fetched_at_ms: i64,
    pub bids: Vec<PolymarketBookLevel>,
    pub offers: Vec<PolymarketBookLevel>,
    /// Per-market minimum tradeable quantity (contracts). None means the
    /// order service could not look it up - callers must treat that as
    /// "unknown minimum", never as "no minimum".
    #[serde(default)]
    pub minimum_trade_qty: Option<f64>,
    /// Per-market required price increment. None means unknown, same
    /// caveat as `minimum_trade_qty`.
    #[serde(default)]
    pub price_tick_size: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PolymarketBookLevel {
    pub price: f64,
    pub quantity: f64,
}

impl PolymarketMarketBook {
    /// Total executable buy-side depth for a competitor's native position,
    /// from this same fresh CLOB book. Read-only reporting for tracking/UI
    /// visibility — not used on the order-submission path.
    ///
    /// Mirrors the buy-side selection and complement pricing the execution
    /// path applies: Long buys consume native LONG offers ascending; Short
    /// buys consume native LONG bids descending at complement price.
    /// Returns (usd_notional, token_size).
    pub fn buy_depth(&self, position_side: PolymarketPositionSide) -> (f64, f64) {
        let levels: &[PolymarketBookLevel] = match position_side {
            PolymarketPositionSide::Long => &self.offers,
            PolymarketPositionSide::Short => &self.bids,
        };
        levels
            .iter()
            .fold((0.0_f64, 0.0_f64), |(usd, size), level| {
                let quantity = level.quantity.floor();
                if quantity <= 0.0 {
                    return (usd, size);
                }
                let price = match position_side {
                    PolymarketPositionSide::Long => level.price,
                    PolymarketPositionSide::Short => 1.0 - level.price,
                };
                (usd + quantity * price, size + quantity)
            })
    }
}

/// Result returned by the official SDK execution envelope, normalized by the
/// local Python service. A submission without a positive fill is not success.
#[derive(Debug, Clone)]
pub struct PolymarketOrderResult {
    pub accepted: bool,
    pub filled_contracts: i32,
    pub fill_quantity_valid: bool,
    pub order_id: Option<String>,
    pub status: Option<String>,
    pub error: Option<String>,
    pub latency_ms: Option<i64>,
    pub average_fill_price: Option<f64>,
}

/// Position data for aggregation (internal use)
#[allow(dead_code)]
struct PositionData {
    asset_id: String,
    market: String,
    outcome: String,
    size: f64,
    total_cost: f64,
    trade_count: u32,
}

/// Polymarket API client
#[derive(Clone)]
pub struct PolymarketClient {
    pub config: PolymarketConfig,
    http: Client,
    key_id: String,
    secret_key: String,
    balance_cache: std::sync::Arc<AsyncMutex<Option<(Instant, f64)>>>,
    positions_cache: std::sync::Arc<AsyncMutex<Option<(Instant, Value)>>>,
    market_constraints_cache:
        std::sync::Arc<AsyncMutex<HashMap<String, (Instant, Option<f64>, Option<f64>)>>>,
}

/// Read Polymarket US API credentials, env vars first, falling back to
/// config file values.
pub fn resolve_credentials(config: &PolymarketConfig) -> Result<(String, String)> {
    let key_id = std::env::var("POLYMARKET_KEY_ID")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| (!config.key_id.trim().is_empty()).then(|| config.key_id.clone()));
    let secret_key = std::env::var("POLYMARKET_SECRET_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| (!config.secret_key.trim().is_empty()).then(|| config.secret_key.clone()));
    match (key_id, secret_key) {
        (Some(key_id), Some(secret_key)) => Ok((key_id, secret_key)),
        _ => anyhow::bail!(
            "Polymarket US credentials are required. Set POLYMARKET_KEY_ID and \
             POLYMARKET_SECRET_KEY, or configure polymarket.key_id and polymarket.secret_key."
        ),
    }
}

impl PolymarketClient {
    /// Create a new Polymarket client
    pub fn new(config: PolymarketConfig) -> Result<Self> {
        let (key_id, secret_key) = resolve_credentials(&config)?;
        let http = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("Failed to build Polymarket HTTP client")?;

        info!(
            "✅ Initialized Polymarket US client with API key ID {}...",
            &key_id[..key_id.len().min(8)]
        );

        Ok(Self {
            config,
            http,
            key_id,
            secret_key,
            balance_cache: std::sync::Arc::new(AsyncMutex::new(None)),
            positions_cache: std::sync::Arc::new(AsyncMutex::new(None)),
            market_constraints_cache: std::sync::Arc::new(AsyncMutex::new(HashMap::new())),
        })
    }

    /// Send a request to the Polymarket US API, signing it (Ed25519) when
    /// `signed_path` is set. Retries only on HTTP 429 - per
    /// docs.polymarket.us/api-reference/rate-limits, stop immediately, wait
    /// at least 1 second, then retry with exponential backoff (1s/2s/4s, 3
    /// attempts total). A 429 means the request was rejected before
    /// reaching the matching engine, so retrying an order submission on a
    /// 429 cannot cause a duplicate order - unlike a transport-level
    /// failure (timeout/reset), which is never retried here for the same
    /// reason `clients/kalshi.rs::post()` never retries those blindly. The
    /// timestamp and signature are regenerated on every attempt.
    async fn request_json(
        &self,
        method: Method,
        url: &str,
        signed_path: Option<&str>,
        body: Option<&Value>,
    ) -> Result<Value> {
        let mut attempt = 0u32;
        loop {
            let mut builder = self.http.request(method.clone(), url);
            if let Some(path) = signed_path {
                let headers =
                    polymarket_auth::sign_request(&self.key_id, &self.secret_key, method.as_str(), path)?;
                builder = builder
                    .header("X-PM-Access-Key", headers.access_key)
                    .header("X-PM-Timestamp", headers.timestamp)
                    .header("X-PM-Signature", headers.signature);
            }
            builder = builder.header("Content-Type", "application/json");
            if let Some(body) = body {
                builder = builder.json(body);
            }

            let response = builder
                .send()
                .await
                .context("Failed to reach Polymarket US")?;
            let status = response.status();

            if status.as_u16() == 429 {
                if attempt + 1 >= RATE_LIMIT_MAX_RETRIES {
                    anyhow::bail!(
                        "Polymarket US is rate limiting this IP. Retry after the temporary restriction expires."
                    );
                }
                let delay = Duration::from_secs(2u64.pow(attempt));
                warn!(
                    "Polymarket US rate limit hit for {} {}, retrying in {:?} (attempt {}/{})",
                    method,
                    url,
                    delay,
                    attempt + 1,
                    RATE_LIMIT_MAX_RETRIES
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
                continue;
            }

            let text = response.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!(concise_api_error(status, &text));
            }
            if text.trim().is_empty() {
                return Ok(json!({}));
            }
            return serde_json::from_str(&text)
                .with_context(|| format!("Failed to parse Polymarket US response: {}", text));
        }
    }

    /// Get account balance from the Polymarket US API, cached briefly to
    /// keep repeated balance checks from adding rate-limit pressure.
    pub async fn get_balance(&self) -> Result<f64> {
        let mut cache = self.balance_cache.lock().await;
        if let Some((cached_at, balance)) = cache.as_ref() {
            if cached_at.elapsed() < BALANCE_CACHE_TTL {
                return Ok(*balance);
            }
        }

        let url = format!("{}/v1/account/balances", self.config.api_base_url);
        let response = self
            .request_json(Method::GET, &url, Some("/v1/account/balances"), None)
            .await
            .context("Failed to fetch Polymarket US balances")?;
        let balances = response
            .get("balances")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                anyhow::anyhow!("Polymarket US balances response did not contain a balance list")
            })?;
        let total: f64 = balances
            .iter()
            .filter_map(|balance| amount_as_f64(&balance["buyingPower"]))
            .sum();

        *cache = Some((Instant::now(), total));
        Ok(total)
    }

    /// Get all supported NBA and tennis match-winner events and markets.
    pub async fn get_supported_events_and_markets(
        &self,
    ) -> Result<(Vec<PolymarketEvent>, Vec<PolymarketMarket>)> {
        let sports = self.get_sports().await?;
        let mut events = Vec::new();
        let mut markets = Vec::new();
        let mut fetched_series = HashSet::new();

        let nba_sport = sports
            .iter()
            .find(|sport| {
                let name = sport_code(sport).unwrap_or_default().to_ascii_uppercase();
                name.contains("NBA") && !name.contains("WNBA")
            })
            .ok_or_else(|| anyhow::anyhow!("NBA league not found"))?;
        let nba_series =
            sport_series_id(nba_sport).ok_or_else(|| anyhow::anyhow!("NBA series_id not found"))?;
        self.append_series_events_and_markets(
            &nba_series,
            "NBA",
            &mut fetched_series,
            &mut events,
            &mut markets,
        )
        .await?;

        for sport_code_name in TENNIS_SPORT_CODES {
            let Some(sport) = sports
                .iter()
                .find(|sport| sport_has_code(sport, sport_code_name))
            else {
                warn!(
                    "Polymarket US tennis sport code {} is unavailable; skipping it",
                    sport_code_name
                );
                continue;
            };

            let Some(series_id) = sport_series_id(sport) else {
                warn!(
                    "Polymarket US tennis sport code {} has no series ID; skipping it",
                    sport_code_name
                );
                continue;
            };

            self.append_series_events_and_markets(
                &series_id,
                "TENNIS",
                &mut fetched_series,
                &mut events,
                &mut markets,
            )
            .await?;
        }

        info!(
            "✅ Polymarket supported sports: {} events, {} markets",
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
    ) -> Result<(Vec<PolymarketEvent>, Vec<PolymarketMarket>)> {
        let sports = self.get_sports().await?;
        let nba_sport = sports
            .iter()
            .find(|sport| {
                let name = sport_code(sport).unwrap_or_default().to_ascii_uppercase();
                name.contains("NBA") && !name.contains("WNBA")
            })
            .ok_or_else(|| anyhow::anyhow!("NBA league not found"))?;
        let nba_series =
            sport_series_id(nba_sport).ok_or_else(|| anyhow::anyhow!("NBA series_id not found"))?;
        self.get_series_events_and_markets(&nba_series, "NBA").await
    }

    async fn get_sports(&self) -> Result<Vec<Value>> {
        let sports_url = format!("{}/v1/sports", self.config.base_url);
        let response = self.http.get(&sports_url).send().await?;
        if !response.status().is_success() {
            anyhow::bail!("Failed to get sports leagues: {}", response.status());
        }

        response
            .json::<Value>()
            .await?
            .get("sports")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Polymarket US sports response is missing sports"))
    }

    async fn append_series_events_and_markets(
        &self,
        series_id: &str,
        category: &str,
        fetched_series: &mut HashSet<String>,
        events: &mut Vec<PolymarketEvent>,
        markets: &mut Vec<PolymarketMarket>,
    ) -> Result<()> {
        if !fetched_series.insert(series_id.to_string()) {
            return Ok(());
        }

        let (mut series_events, mut series_markets) = self
            .get_series_events_and_markets(series_id, category)
            .await?;
        events.append(&mut series_events);
        markets.append(&mut series_markets);
        Ok(())
    }

    async fn get_series_events_and_markets(
        &self,
        series_id: &str,
        category: &str,
    ) -> Result<(Vec<PolymarketEvent>, Vec<PolymarketMarket>)> {
        let events_url = format!(
            "{}/v1/events?seriesId={}&active=true&closed=false&limit=100",
            self.config.base_url, series_id
        );
        let response = self.http.get(&events_url).send().await?;
        if !response.status().is_success() {
            anyhow::bail!(
                "Failed to get Polymarket {} events: {}",
                category,
                response.status()
            );
        }

        let api_events: Vec<Value> = response
            .json::<Value>()
            .await?
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Polymarket US events response is missing events"))?;
        debug!(
            "📥 Retrieved {} Polymarket {} events",
            api_events.len(),
            category
        );

        let mut events = Vec::new();
        let mut markets = Vec::new();
        for api_event in &api_events {
            let event_title = api_event["title"].as_str().unwrap_or("");
            let event_date = extract_date_from_slug(api_event["slug"].as_str().unwrap_or(""));

            for market_data in api_event["markets"].as_array().into_iter().flatten() {
                let parsed = match category {
                    "NBA" => parse_nba_market(event_title, event_date, market_data),
                    "TENNIS" => parse_tennis_match_winner_market(event_date, market_data),
                    _ => None,
                };
                if let Some((event, market)) = parsed {
                    events.push(event);
                    markets.push(market);
                }
            }
        }

        Ok((events, markets))
    }

    // ========================================================================
    // ORDER PLACEMENT API (direct, Ed25519-signed against api.polymarket.us)
    // ========================================================================

    /// Place an immediate-or-cancel Polymarket US limit order for whole contracts.
    pub async fn place_limit_order(
        &self,
        market_slug: &str,
        position_side: PolymarketPositionSide,
        side: &str,
        contracts: i32,
        price: f64,
    ) -> Result<Value> {
        let response = self
            .submit_limit_order(market_slug, position_side, side, contracts, price)
            .await?;
        if response.filled_contracts <= 0 {
            anyhow::bail!(
                "Polymarket US limit order did not fill: {}",
                response
                    .error
                    .unwrap_or_else(|| "Unknown error".to_string())
            );
        }
        Ok(json!({
            "success": true,
            "order_id": response.order_id,
            "status": response.status,
            "filled_contracts": response.filled_contracts,
            "latency_ms": response.latency_ms,
        }))
    }

    /// Submit a FAK order and retain explicit acceptance/fill information for
    /// paired-execution recovery. Transport failures remain errors; exchange
    /// rejections and zero fills are returned for durable recording.
    pub async fn submit_limit_order(
        &self,
        market_slug: &str,
        position_side: PolymarketPositionSide,
        side: &str,
        contracts: i32,
        price: f64,
    ) -> Result<PolymarketOrderResult> {
        let side = side.to_ascii_lowercase();
        if market_slug.trim().is_empty()
            || contracts <= 0
            || !price.is_finite()
            || !(0.0..1.0).contains(&price)
            || !matches!(side.as_str(), "buy" | "sell")
        {
            anyhow::bail!("Polymarket US limit order requires a slug, positive whole contract count, limit price, and buy or sell action");
        }

        // `quantity` is a JSON integer here because `contracts` is already
        // `i32` - the SDK's own type stubs declare `quantity: int`, and a
        // prior bug sent a JSON float (`5.0`) through the old Python hop,
        // which this path can no longer reproduce.
        let intent = format!(
            "ORDER_INTENT_{}_{}",
            side.to_ascii_uppercase(),
            position_side.as_str().to_ascii_uppercase()
        );
        let body = json!({
            "marketSlug": market_slug,
            "intent": intent,
            "type": "ORDER_TYPE_LIMIT",
            "price": {"value": price.to_string(), "currency": "USD"},
            "quantity": contracts,
            "tif": "TIME_IN_FORCE_IMMEDIATE_OR_CANCEL",
            "manualOrderIndicator": "MANUAL_ORDER_INDICATOR_AUTOMATIC",
            "synchronousExecution": true,
            "maxBlockTime": "5",
        });

        let started = Instant::now();
        let url = format!("{}/v1/orders", self.config.api_base_url);
        let response = self
            .request_json(Method::POST, &url, Some("/v1/orders"), Some(&body))
            .await
            .context("Failed to submit Polymarket US limit order")?;
        let latency_ms = started.elapsed().as_millis() as i64;

        Ok(parse_polymarket_order_result(&response, latency_ms, &body))
    }

    /// Retrieve a fresh book directly from the Polymarket US gateway. This
    /// must be called immediately before an automated paired order; display
    /// quotes and cached account data are deliberately not substitutes.
    pub async fn get_market_book(&self, market_slug: &str) -> Result<PolymarketMarketBook> {
        if market_slug.trim().is_empty() {
            anyhow::bail!("Polymarket US market slug cannot be empty");
        }
        let base = self.config.base_url.trim_end_matches('/');
        let url = format!("{}/v1/markets/{}/book", base, market_slug);
        let response = self
            .request_json(Method::GET, &url, None, None)
            .await
            .context("Failed to retrieve Polymarket US market book")?;
        let mut book = build_market_book(&response, market_slug)?;

        // Cached after the first lookup, so this normally costs nothing
        // extra. A failure here must not silently look like "no minimum" -
        // leave both fields None and let the caller refuse to size an order
        // against them.
        match self.market_constraints(market_slug).await {
            Ok((min_qty, tick_size)) => {
                book.minimum_trade_qty = min_qty;
                book.price_tick_size = tick_size;
            }
            Err(error) => {
                warn!(
                    "Polymarket US market constraint lookup failed for {}: {}",
                    market_slug, error
                );
            }
        }

        Ok(book)
    }

    /// `minimumTradeQty` / `orderPriceMinTickSize` for a market, cached for
    /// an hour (docs.polymarket.us/api-reference/orders/overview:
    /// "constraints are market-dependent - retrieve ... before order
    /// submission").
    async fn market_constraints(&self, market_slug: &str) -> Result<(Option<f64>, Option<f64>)> {
        let mut cache = self.market_constraints_cache.lock().await;
        if let Some((cached_at, min_qty, tick_size)) = cache.get(market_slug) {
            if cached_at.elapsed() < MARKET_CONSTRAINTS_CACHE_TTL {
                return Ok((*min_qty, *tick_size));
            }
        }

        let base = self.config.base_url.trim_end_matches('/');
        let url = format!("{}/v1/market/slug/{}", base, market_slug);
        let response = self.request_json(Method::GET, &url, None, None).await?;
        let market = response.get("market");
        let min_qty = market.and_then(|m| amount_as_f64(&m["minimumTradeQty"]));
        let tick_size = market.and_then(|m| amount_as_f64(&m["orderPriceMinTickSize"]));

        cache.insert(market_slug.to_string(), (Instant::now(), min_qty, tick_size));
        Ok((min_qty, tick_size))
    }

    /// Get open orders directly from the Polymarket US API.
    pub async fn get_open_orders(&self) -> Result<Value> {
        let url = format!("{}/v1/orders/open", self.config.api_base_url);
        let response = self
            .request_json(Method::GET, &url, Some("/v1/orders/open"), None)
            .await
            .context("Failed to fetch Polymarket US open orders")?;
        Ok(response.get("orders").cloned().unwrap_or_else(|| json!([])))
    }

    /// Get positions directly from the Polymarket US API, cached briefly to
    /// keep repeated lookups from adding rate-limit pressure.
    pub async fn get_positions(&self) -> Result<Value> {
        let mut cache = self.positions_cache.lock().await;
        if let Some((cached_at, positions)) = cache.as_ref() {
            if cached_at.elapsed() < POSITIONS_CACHE_TTL {
                return Ok(positions.clone());
            }
        }

        let url = format!("{}/v1/portfolio/positions", self.config.api_base_url);
        let response = self
            .request_json(Method::GET, &url, Some("/v1/portfolio/positions"), None)
            .await
            .context("Failed to fetch Polymarket US positions")?;
        let positions = normalize_positions(&response)?;

        *cache = Some((Instant::now(), positions.clone()));
        Ok(positions)
    }

    /// Cancel an order directly against the Polymarket US API. The
    /// exchange requires the order's `marketSlug` on cancel, so this looks
    /// the order up first.
    pub async fn cancel_order(&self, order_id: &str) -> Result<Value> {
        let retrieve_path = format!("/v1/order/{}", order_id);
        let retrieve_url = format!("{}{}", self.config.api_base_url, retrieve_path);
        let order = self
            .request_json(Method::GET, &retrieve_url, Some(&retrieve_path), None)
            .await
            .context("Failed to look up Polymarket US order before cancelling")?;
        let market_slug = order
            .get("order")
            .and_then(|order| order.get("marketSlug"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                anyhow::anyhow!("Polymarket US did not return a marketSlug for this order")
            })?;

        let cancel_path = format!("/v1/order/{}/cancel", order_id);
        let cancel_url = format!("{}{}", self.config.api_base_url, cancel_path);
        self.request_json(
            Method::POST,
            &cancel_url,
            Some(&cancel_path),
            Some(&json!({ "marketSlug": market_slug })),
        )
        .await
        .context("Failed to cancel Polymarket US order")?;

        Ok(json!({"success": true, "order_id": order_id}))
    }
}

/// Build and validate a `PolymarketMarketBook` straight from the gateway's
/// raw `{marketData: {...}}` envelope.
pub fn build_market_book(response: &Value, requested_slug: &str) -> Result<PolymarketMarketBook> {
    let market_data = response
        .get("marketData")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            anyhow::anyhow!("Polymarket US market book did not contain a marketData envelope")
        })?;
    let market_slug = market_data
        .get("marketSlug")
        .and_then(Value::as_str)
        .filter(|slug| *slug == requested_slug)
        .ok_or_else(|| {
            anyhow::anyhow!("Polymarket US market book slug did not match the requested market")
        })?;
    let state = market_data
        .get("state")
        .and_then(Value::as_str)
        .filter(|state| !state.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("Polymarket US market book did not contain a state"))?;

    let mut bids = normalize_book_levels(market_data.get("bids"), "bids")?;
    let mut offers = normalize_book_levels(market_data.get("offers"), "offers")?;
    bids.sort_by(|a, b| b.price.total_cmp(&a.price));
    offers.sort_by(|a, b| a.price.total_cmp(&b.price));

    let transact_time = match market_data.get("transactTime") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Number(value)) => Some(value.to_string()),
        Some(_) => anyhow::bail!("Polymarket US market book contained an invalid transactTime"),
    };

    Ok(PolymarketMarketBook {
        success: true,
        market_slug: market_slug.to_string(),
        state: state.to_string(),
        transact_time,
        fetched_at_ms: Utc::now().timestamp_millis(),
        bids,
        offers,
        minimum_trade_qty: None,
        price_tick_size: None,
    })
}

/// Validate SDK price/quantity objects without accepting lossy book data.
fn normalize_book_levels(levels: Option<&Value>, field: &str) -> Result<Vec<PolymarketBookLevel>> {
    let levels = levels
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Polymarket US market book {} must be a list", field))?;

    let mut normalized = Vec::with_capacity(levels.len());
    for level in levels {
        let price = amount_as_f64(&level["px"]);
        let quantity = amount_as_f64(&level["qty"]);
        let (Some(price), Some(quantity)) = (price, quantity) else {
            anyhow::bail!("Polymarket US market book {} contains an invalid level", field);
        };
        if !(0.0 < price && price < 1.0) {
            anyhow::bail!("Polymarket US market book {} contains an invalid price", field);
        }
        if quantity <= 0.0 {
            anyhow::bail!("Polymarket US market book {} contains an invalid quantity", field);
        }
        normalized.push(PolymarketBookLevel { price, quantity });
    }
    Ok(normalized)
}

/// Convert the SDK's market-keyed position map into a frontend-safe list.
fn normalize_positions(response: &Value) -> Result<Value> {
    let positions = response.get("positions");
    if let Some(Value::Array(items)) = positions {
        return Ok(Value::Array(items.clone()));
    }
    let Some(Value::Object(map)) = positions else {
        anyhow::bail!("Polymarket US positions response did not contain a position list or map");
    };

    let mut normalized = Vec::with_capacity(map.len());
    for (market_slug, position) in map {
        let Some(position) = position.as_object() else {
            continue;
        };
        let metadata = position.get("marketMetadata").and_then(Value::as_object);
        let title = metadata
            .and_then(|m| m.get("title"))
            .and_then(Value::as_str)
            .unwrap_or(market_slug);
        normalized.push(json!({
            "id": market_slug,
            "asset": market_slug,
            "conditionId": market_slug,
            "title": title,
            "size": position.get("netPosition").cloned().unwrap_or_else(|| json!("0")),
            "value": position.get("cashValue").and_then(amount_as_f64),
            // Total cost basis; with `cashValue` it yields avg entry, mark
            // and unrealized P&L without any extra API call.
            "cost": position.get("cost").and_then(amount_as_f64),
            "pnl": position.get("realized").and_then(amount_as_f64),
            "outcome": metadata.and_then(|m| m.get("outcome")).cloned().unwrap_or(Value::Null),
        }));
    }
    Ok(Value::Array(normalized))
}

/// Convert an HTTP error response into a concise message, keeping upstream
/// HTML/JSON error pages out of application logs.
fn concise_api_error(status: StatusCode, body_text: &str) -> String {
    let message = serde_json::from_str::<Value>(body_text)
        .ok()
        .and_then(|body| {
            body.get("message")
                .or_else(|| body.get("error"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| body_text.to_string());
    if message.contains("1015") || message.to_lowercase().contains("rate limited") {
        return "Polymarket US is rate limiting this IP. Retry after the temporary restriction expires."
            .to_string();
    }
    let mut truncated = message;
    truncated.truncate(500);
    format!("Polymarket US API error {}: {}", status, truncated)
}

/// Convert the raw `/v1/orders` execution envelope into an explicit order
/// result. Unlike the old Python order-service hop this replaced, a non-fill
/// logs the full outgoing request and raw
/// response at `warn!` - previously only a fill was logged anywhere, which
/// left rejections like `ORD_REJECT_REASON_EXCHANGE_OPTION` with no trace
/// of what was actually sent.
fn parse_polymarket_order_result(
    response: &Value,
    latency_ms: i64,
    request_body: &Value,
) -> PolymarketOrderResult {
    let executions = response.get("executions").and_then(Value::as_array);
    let Some(executions) = executions.filter(|executions| !executions.is_empty()) else {
        let upstream_error = ["error", "message", "detail", "orderRejectReason", "text"]
            .iter()
            .find_map(|field| response.get(*field).and_then(Value::as_str))
            .map(str::to_string);
        warn!(
            "Polymarket US order did not return an execution: request={} response={}",
            request_body, response
        );
        return PolymarketOrderResult {
            accepted: false,
            filled_contracts: 0,
            fill_quantity_valid: true,
            order_id: None,
            status: None,
            error: Some(upstream_error.unwrap_or_else(|| {
                "Polymarket US did not return an order execution".to_string()
            })),
            latency_ms: Some(latency_ms),
            average_fill_price: None,
        };
    };

    let execution = executions.last().expect("checked non-empty above");
    let Some(order) = execution.get("order").filter(|order| order.is_object()) else {
        warn!(
            "Polymarket US execution did not include order details: request={} response={}",
            request_body, response
        );
        return PolymarketOrderResult {
            accepted: false,
            filled_contracts: 0,
            fill_quantity_valid: true,
            order_id: None,
            status: None,
            error: Some("Polymarket US execution did not include order details".to_string()),
            latency_ms: Some(latency_ms),
            average_fill_price: None,
        };
    };

    let order_id = order.get("id").and_then(Value::as_str).map(str::to_string);
    let status = order
        .get("state")
        .or_else(|| order.get("status"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let filled = amount_as_f64(&order["cumQuantity"]).unwrap_or(0.0);

    if filled > 0.0 {
        let average_fill_price = extract_average_fill_price(response);
        let fill_quantity_valid = filled.is_finite()
            && filled >= 0.0
            && filled.fract() == 0.0
            && filled <= i32::MAX as f64;
        info!(
            "Polymarket US order FILLED: order_id={:?} market={:?} status={:?} filled={} avg_price={:?} latency_ms={}",
            order_id,
            order.get("marketSlug"),
            status,
            filled,
            average_fill_price,
            latency_ms
        );
        return PolymarketOrderResult {
            accepted: true,
            filled_contracts: if fill_quantity_valid { filled as i32 } else { 0 },
            fill_quantity_valid,
            order_id,
            status,
            error: if fill_quantity_valid {
                None
            } else {
                Some("Polymarket US returned a non-whole or invalid fill quantity".to_string())
            },
            latency_ms: Some(latency_ms),
            average_fill_price,
        };
    }

    let reject_code = execution.get("orderRejectReason").and_then(Value::as_str);
    let reject_text = execution.get("text").and_then(Value::as_str);
    let error = match (reject_code, reject_text) {
        (Some(code), Some(text)) if code != text => format!("{}: {}", code, text),
        (Some(code), _) => code.to_string(),
        (None, Some(text)) => text.to_string(),
        (None, None) => format!(
            "Order received no fill (state: {})",
            status.as_deref().unwrap_or("unknown")
        ),
    };
    warn!(
        "Polymarket US order not filled: order_id={:?} status={:?} error={} request={} response={}",
        order_id, status, error, request_body, response
    );
    PolymarketOrderResult {
        accepted: order_id.is_some(),
        filled_contracts: 0,
        fill_quantity_valid: true,
        order_id,
        status,
        error: Some(error),
        latency_ms: Some(latency_ms),
        average_fill_price: None,
    }
}

fn extract_average_fill_price(response: &Value) -> Option<f64> {
    let executions = response.get("executions")?.as_array()?;
    let order = executions.last()?.get("order")?;
    [
        "avgPx",
        "averageFillPrice",
        "averagePrice",
        "avgPrice",
        "avg_price",
    ]
    .iter()
    .find_map(|field| amount_as_f64(&order[*field]))
    .filter(|price| price.is_finite() && (0.0..1.0).contains(price))
}

fn amount_as_f64(value: &Value) -> Option<f64> {
    let value = value.get("value").unwrap_or(value);
    value.as_f64().or_else(|| value.as_str()?.parse().ok())
}

fn sport_code(sport: &Value) -> Option<&str> {
    ["sport", "code", "slug"]
        .iter()
        .find_map(|field| sport[*field].as_str())
}

fn sport_has_code(sport: &Value, expected_code: &str) -> bool {
    ["sport", "code", "slug"]
        .iter()
        .filter_map(|field| sport[*field].as_str())
        .any(|value| value.eq_ignore_ascii_case(expected_code))
}

fn sport_series_id(sport: &Value) -> Option<String> {
    ["series", "seriesId", "series_id"]
        .iter()
        .find_map(|field| match &sport[*field] {
            Value::String(value) if !value.is_empty() => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
}

fn parse_nba_market(
    event_title: &str,
    event_date: Option<DateTime<Utc>>,
    market_data: &Value,
) -> Option<(PolymarketEvent, PolymarketMarket)> {
    let question = market_data["question"].as_str().unwrap_or(event_title);
    if question != event_title {
        return None;
    }

    let (outcomes, sides) = parse_binary_outcomes_and_market_sides(market_data, |outcome| {
        if matches!(
            outcome.trim().to_ascii_lowercase().as_str(),
            "yes" | "no" | "over" | "under"
        ) {
            return None;
        }
        let normalized = normalize_team_name(outcome);
        (!normalized.is_empty()).then_some(normalized)
    })?;
    let team_a = outcomes[0].clone();
    let team_b = outcomes[1].clone();
    if team_a == team_b {
        return None;
    }

    let event_name = if team_a < team_b {
        format!("{team_a}-{team_b}")
    } else {
        format!("{team_b}-{team_a}")
    };
    build_competitor_market(
        market_data,
        event_name,
        team_a,
        team_b,
        sides[0],
        sides[1],
        event_date,
        "NBA",
    )
}

fn parse_tennis_match_winner_market(
    event_date: Option<DateTime<Utc>>,
    market_data: &Value,
) -> Option<(PolymarketEvent, PolymarketMarket)> {
    if market_data["sportsMarketType"].as_str() != Some("tennis_match_winner")
        || market_data["sportsMarketTypeV2"].as_str() != Some("SPORTS_MARKET_TYPE_MONEYLINE")
    {
        return None;
    }

    let (outcomes, sides) =
        parse_binary_outcomes_and_market_sides(market_data, normalize_competitor_name)?;
    let competitor_a = outcomes[0].clone();
    let competitor_b = outcomes[1].clone();
    let event_name = competitor_event_name(&competitor_a, &competitor_b)?;

    build_competitor_market(
        market_data,
        event_name,
        competitor_a,
        competitor_b,
        sides[0],
        sides[1],
        event_date,
        "TENNIS",
    )
}

#[derive(Debug, Clone, Copy)]
struct MarketSide {
    quote: f64,
    position_side: PolymarketPositionSide,
}

/// Parse a binary market and associate every competitor with its own gateway
/// market side. The outcome array supplies competitor names only; it never
/// determines a LONG or SHORT position.
fn parse_binary_outcomes_and_market_sides<F>(
    market_data: &Value,
    normalizer: F,
) -> Option<([String; 2], [MarketSide; 2])>
where
    F: Fn(&str) -> Option<String>,
{
    let outcomes = parse_json_array(&market_data["outcomes"])?
        .iter()
        .map(|value| value.as_str().and_then(|name| normalizer(name)))
        .collect::<Option<Vec<_>>>()?;
    let outcomes: [String; 2] = outcomes.try_into().ok()?;
    if outcomes[0] == outcomes[1] {
        return None;
    }

    let market_sides = market_data["marketSides"].as_array()?;
    let mut outcome_sides: [Option<MarketSide>; 2] = [None, None];
    for market_side in market_sides {
        let description = market_side["description"].as_str()?;
        let Some(competitor) = normalizer(description) else {
            continue;
        };
        let outcome_index = if competitor == outcomes[0] {
            0
        } else if competitor == outcomes[1] {
            1
        } else {
            continue;
        };

        let quote = market_side["quote"]["value"]
            .as_str()
            .and_then(|price| price.parse::<f64>().ok())
            .or_else(|| market_side["quote"]["value"].as_f64())?;
        if !quote.is_finite() || !(0.01..=0.99).contains(&quote) {
            return None;
        }
        let position_side = if market_side["long"].as_bool()? {
            PolymarketPositionSide::Long
        } else {
            PolymarketPositionSide::Short
        };

        // A competitor must map to exactly one side. Continuing would make
        // execution ambiguous and could choose the wrong position.
        if outcome_sides[outcome_index].is_some() {
            return None;
        }
        outcome_sides[outcome_index] = Some(MarketSide {
            quote,
            position_side,
        });
    }

    Some((outcomes, [outcome_sides[0]?, outcome_sides[1]?]))
}

fn parse_json_array(value: &Value) -> Option<Vec<Value>> {
    if let Some(serialized) = value.as_str() {
        serde_json::from_str(serialized).ok()
    } else {
        value.as_array().cloned()
    }
}

#[allow(clippy::too_many_arguments)]
fn build_competitor_market(
    market_data: &Value,
    event_name: String,
    first_competitor: String,
    second_competitor: String,
    first_side: MarketSide,
    second_side: MarketSide,
    start_time: Option<DateTime<Utc>>,
    category: &str,
) -> Option<(PolymarketEvent, PolymarketMarket)> {
    if first_competitor == second_competitor {
        return None;
    }

    let market_id = market_data["id"].as_str().unwrap_or("");
    let condition_id = market_data["conditionId"]
        .as_str()
        .or_else(|| market_data["condition_id"].as_str())
        .unwrap_or(market_id)
        .to_string();
    if condition_id.is_empty() {
        return None;
    }

    let market_slug = market_data["slug"].as_str()?.trim();
    if market_slug.is_empty() {
        return None;
    }

    let volume = market_data["volume"]
        .as_str()
        .and_then(|value| value.parse::<f64>().ok())
        .or_else(|| market_data["volume"].as_f64());

    let (team_a, team_b, side_a, side_b) = if first_competitor < second_competitor {
        (first_competitor, second_competitor, first_side, second_side)
    } else {
        (second_competitor, first_competitor, second_side, first_side)
    };

    let market = PolymarketMarket {
        market_id: condition_id.clone(),
        market_slug: market_slug.to_string(),
        event_name: event_name.clone(),
        team_a: team_a.clone(),
        team_b: team_b.clone(),
        price_a: side_a.quote,
        price_b: side_b.quote,
        team_a_position: side_a.position_side,
        team_b_position: side_b.position_side,
        start_time,
        volume,
    };
    let event = PolymarketEvent {
        event_id: condition_id,
        name: event_name,
        team_a,
        team_b,
        start_time,
        category: category.to_string(),
        market: Some(market.clone()),
    };

    Some((event, market))
}

/// Extract date from slug (e.g., "lakers-vs-grizzlies-2026-01-07" -> 2026-01-07)
fn extract_date_from_slug(slug: &str) -> Option<DateTime<Utc>> {
    let parts: Vec<&str> = slug.split('-').collect();
    if parts.len() >= 3 {
        let year_str = parts[parts.len() - 3];
        let month_str = parts[parts.len() - 2];
        let day_str = parts[parts.len() - 1];

        if let (Ok(year), Ok(month), Ok(day)) = (
            year_str.parse::<i32>(),
            month_str.parse::<u32>(),
            day_str.parse::<u32>(),
        ) {
            use chrono::NaiveDate;
            if let Some(naive_date) = NaiveDate::from_ymd_opt(year, month, day) {
                let naive_datetime = naive_date.and_hms_opt(12, 0, 0)?;
                return Some(DateTime::from_naive_utc_and_offset(naive_datetime, Utc));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(bids: Vec<(f64, f64)>, offers: Vec<(f64, f64)>) -> PolymarketMarketBook {
        PolymarketMarketBook {
            success: true,
            market_slug: "test-market".to_string(),
            state: "ACTIVE".to_string(),
            transact_time: None,
            fetched_at_ms: 0,
            bids: bids
                .into_iter()
                .map(|(price, quantity)| PolymarketBookLevel { price, quantity })
                .collect(),
            offers: offers
                .into_iter()
                .map(|(price, quantity)| PolymarketBookLevel { price, quantity })
                .collect(),
            minimum_trade_qty: None,
            price_tick_size: None,
        }
    }

    #[test]
    fn long_buy_depth_sums_native_offers_at_native_price() {
        let book = book(vec![(0.50, 10.0)], vec![(0.22, 4.0), (0.25, 6.5)]);
        let (usd, size) = book.buy_depth(PolymarketPositionSide::Long);
        // quantity floors to whole tokens: 4 + 6 = 10; usd = 4*0.22 + 6*0.25
        assert_eq!(size, 10.0);
        assert!((usd - (4.0 * 0.22 + 6.0 * 0.25)).abs() < 1e-9);
    }

    #[test]
    fn short_buy_depth_sums_native_bids_at_complement_price() {
        let book = book(vec![(0.70, 3.0)], vec![(0.22, 4.0)]);
        let (usd, size) = book.buy_depth(PolymarketPositionSide::Short);
        // Short buys consume native bids at 1 - price.
        assert_eq!(size, 3.0);
        assert!((usd - 3.0 * (1.0 - 0.70)).abs() < 1e-9);
    }

    #[test]
    fn buy_depth_ignores_non_positive_quantity_levels() {
        let book = book(vec![], vec![(0.30, 0.0), (0.40, 2.0)]);
        let (usd, size) = book.buy_depth(PolymarketPositionSide::Long);
        assert_eq!(size, 2.0);
        assert!((usd - 2.0 * 0.40).abs() < 1e-9);
    }

    #[test]
    fn parses_binary_tennis_match_winner_outcomes() {
        let market = json!({
            "id": "market-id",
            "conditionId": "condition-id",
            "slug": "atp-cerundolo-auger-2026-08-17",
            "sportsMarketType": "tennis_match_winner",
            "sportsMarketTypeV2": "SPORTS_MARKET_TYPE_MONEYLINE",
            "outcomes": "[\"Juan Manuel Cerundolo\", \"Felix Auger-Aliassime\"]",
            "marketSides": [
                {"description": "Juan Manuel Cerundolo", "long": false, "quote": {"value": "0.46"}},
                {"description": "Felix Auger-Aliassime", "long": true, "quote": {"value": "0.56"}}
            ]
        });

        let (event, parsed_market) = parse_tennis_match_winner_market(None, &market).unwrap();
        assert_eq!(event.name, "FELIX AUGER ALIASSIME VS JUAN MANUEL CERUNDOLO");
        assert_eq!(parsed_market.team_a, "FELIX AUGER ALIASSIME");
        assert_eq!(parsed_market.price_a, 0.56);
        assert_eq!(parsed_market.market_slug, "atp-cerundolo-auger-2026-08-17");
        assert_eq!(
            parsed_market
                .us_execution_for_competitor("Felix Auger-Aliassime")
                .unwrap()
                .position_side,
            PolymarketPositionSide::Long
        );
    }

    #[test]
    fn outcome_order_does_not_determine_position_side() {
        let market = json!({
            "id": "market-id",
            "conditionId": "condition-id",
            "slug": "atp-auger-cerundolo-2026-08-17",
            "sportsMarketType": "tennis_match_winner",
            "sportsMarketTypeV2": "SPORTS_MARKET_TYPE_MONEYLINE",
            "outcomes": "[\"Felix Auger-Aliassime\", \"Juan Manuel Cerundolo\"]",
            "marketSides": [
                {"description": "Juan Manuel Cerundolo", "long": true, "quote": {"value": "0.46"}},
                {"description": "Felix Auger-Aliassime", "long": false, "quote": {"value": "0.56"}}
            ]
        });

        let (_, parsed_market) = parse_tennis_match_winner_market(None, &market).unwrap();
        assert_eq!(
            parsed_market
                .us_execution_for_competitor("Felix Auger-Aliassime")
                .unwrap()
                .position_side,
            PolymarketPositionSide::Short
        );
        assert_eq!(
            parsed_market
                .us_execution_for_competitor("Juan Manuel Cerundolo")
                .unwrap()
                .position_side,
            PolymarketPositionSide::Long
        );
    }

    #[test]
    fn rejects_missing_ambiguous_or_slugless_side_mappings() {
        let base = json!({
            "id": "market-id",
            "conditionId": "condition-id",
            "slug": "atp-auger-cerundolo-2026-08-17",
            "sportsMarketType": "tennis_match_winner",
            "sportsMarketTypeV2": "SPORTS_MARKET_TYPE_MONEYLINE",
            "outcomes": "[\"Felix Auger-Aliassime\", \"Juan Manuel Cerundolo\"]",
            "marketSides": [
                {"description": "Felix Auger-Aliassime", "long": true, "quote": {"value": "0.56"}},
                {"description": "Juan Manuel Cerundolo", "long": false, "quote": {"value": "0.46"}}
            ]
        });

        let mut missing_slug = base.clone();
        missing_slug.as_object_mut().unwrap().remove("slug");
        assert!(parse_tennis_match_winner_market(None, &missing_slug).is_none());

        let mut missing_side = base.clone();
        missing_side["marketSides"] = json!([
            {"description": "Felix Auger-Aliassime", "long": true, "quote": {"value": "0.56"}}
        ]);
        assert!(parse_tennis_match_winner_market(None, &missing_side).is_none());

        let mut ambiguous = base;
        ambiguous["marketSides"] = json!([
            {"description": "Felix Auger-Aliassime", "long": true, "quote": {"value": "0.56"}},
            {"description": "Felix Auger-Aliassime", "long": false, "quote": {"value": "0.56"}},
            {"description": "Juan Manuel Cerundolo", "long": false, "quote": {"value": "0.46"}}
        ]);
        assert!(parse_tennis_match_winner_market(None, &ambiguous).is_none());
    }

    #[test]
    fn rejects_non_moneyline_tennis_markets() {
        let market = json!({
            "sportsMarketType": "tennis_tournament_winner",
            "sportsMarketTypeV2": "SPORTS_MARKET_TYPE_FUTURES",
            "outcomes": "[\"Juan Manuel Cerundolo\", \"Felix Auger-Aliassime\"]",
            "outcomePrices": "[\"0.45\", \"0.55\"]"
        });

        assert!(parse_tennis_match_winner_market(None, &market).is_none());
    }

    #[test]
    fn extracts_sdk_average_fill_price() {
        let response = json!({
            "executions": [{
                "order": {
                    "avgPx": {"value": "0.6250", "currency": "USD"}
                }
            }]
        });

        assert_eq!(extract_average_fill_price(&response), Some(0.625));
    }
}
