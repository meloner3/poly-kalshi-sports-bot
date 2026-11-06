//! Polymarket US markets WebSocket (`wss://api.polymarket.us/v1/ws/markets`).
//!
//! Push-based replacement for waiting on the ~3s REST poll when detecting
//! arbitrage. Subscribes to `SUBSCRIPTION_TYPE_MARKET_DATA_LITE`, whose
//! `longQuote`/`shortQuote` are the same per-side quotes the gateway REST
//! feed exposes as `marketSides[].quote` (verified live). These are
//! detection prices only - execution depth still comes exclusively from
//! `/v1/markets/{slug}/book` at order time.
//!
//! Auth is the same Ed25519 scheme as REST: sign `{timestamp}GET{path}`.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::{
    connect_async, tungstenite::client::IntoClientRequest, tungstenite::Message,
};
use tracing::{debug, info, warn};

use super::polymarket::{build_market_book, PolymarketMarketBook};
use super::polymarket_auth;

const WS_URL: &str = "wss://api.polymarket.us/v1/ws/markets";
const WS_PATH: &str = "/v1/ws/markets";
/// Documented cap: 100 markets per subscription.
const MAX_SLUGS_PER_SUBSCRIPTION: usize = 100;
/// The server sends heartbeats; silence beyond this means a dead socket.
const SILENCE_TIMEOUT: Duration = Duration::from_secs(30);
const SLUG_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// A per-side quote update for one market slug.
#[derive(Debug, Clone, PartialEq)]
pub struct PolyWsQuote {
    pub slug: String,
    pub long_quote: f64,
    pub short_quote: f64,
}

/// One decoded stream message.
#[derive(Debug, Clone)]
pub enum PolyWsEvent {
    /// Lightweight per-side quotes (detection prices).
    Quote(PolyWsQuote),
    /// Full order book snapshot (display/tracking depth only - execution
    /// still fetches its own fresh book from REST).
    Book(PolymarketMarketBook),
}

fn price(value: &Value) -> Option<f64> {
    let v = value.get("value").unwrap_or(value);
    v.as_f64().or_else(|| v.as_str()?.parse().ok())
}

/// Parse a raw WS text frame. Returns `None` for heartbeats, acks, full
/// order-book messages, closed/expired markets, or anything missing a
/// quote - the caller then simply leaves the previous quote in place.
pub fn parse_lite_message(text: &str) -> Option<PolyWsQuote> {
    let root: Value = serde_json::from_str(text).ok()?;
    let lite = root.get("marketDataLite")?;
    if lite.get("state").and_then(Value::as_str) != Some("MARKET_STATE_OPEN") {
        return None;
    }
    let long_quote = price(lite.get("longQuote")?)?;
    let short_quote = price(lite.get("shortQuote")?)?;
    // Mirror the REST parser's validity window.
    if !(0.01..=0.99).contains(&long_quote) || !(0.01..=0.99).contains(&short_quote) {
        return None;
    }
    Some(PolyWsQuote {
        slug: lite.get("marketSlug")?.as_str()?.to_string(),
        long_quote,
        short_quote,
    })
}

/// Parse a full `marketData` book message. Each message is a complete
/// snapshot (verified live), so the caller replaces its stored book.
pub fn parse_book_message(text: &str) -> Option<PolymarketMarketBook> {
    let root: Value = serde_json::from_str(text).ok()?;
    let data = root.get("marketData")?;
    let slug = data.get("marketSlug")?.as_str()?;
    build_market_book(&json!({ "marketData": data }), slug).ok()
}

fn parse_event(text: &str) -> Option<PolyWsEvent> {
    parse_lite_message(text)
        .map(PolyWsEvent::Quote)
        .or_else(|| parse_book_message(text).map(PolyWsEvent::Book))
}

const SUBSCRIPTION_TYPES: [(&str, &str); 2] = [
    ("lite", "SUBSCRIPTION_TYPE_MARKET_DATA_LITE"),
    ("book", "SUBSCRIPTION_TYPE_MARKET_DATA"),
];

fn subscribe_messages(slugs: &[String], next_id: &mut u64) -> Vec<Message> {
    let mut messages = Vec::new();
    for chunk in slugs.chunks(MAX_SLUGS_PER_SUBSCRIPTION) {
        for (label, subscription_type) in SUBSCRIPTION_TYPES {
            *next_id += 1;
            messages.push(Message::Text(
                json!({"subscribe": {
                    "requestId": format!("{}-{}", label, next_id),
                    "subscriptionType": subscription_type,
                    "marketSlugs": chunk,
                }})
                .to_string(),
            ));
        }
    }
    messages
}

/// One connection attempt. Returns when the socket dies or goes silent.
async fn run_once<S, Q>(
    key_id: &str,
    secret_key: &str,
    slugs: &S,
    on_quote: &Q,
) -> Result<()>
where
    S: Fn() -> Vec<String>,
    Q: Fn(PolyWsEvent),
{
    let headers = polymarket_auth::sign_request(key_id, secret_key, "GET", WS_PATH)?;
    let mut request = WS_URL.into_client_request()?;
    let h = request.headers_mut();
    h.insert("X-PM-Access-Key", headers.access_key.parse()?);
    h.insert("X-PM-Timestamp", headers.timestamp.parse()?);
    h.insert("X-PM-Signature", headers.signature.parse()?);

    let (stream, _) = connect_async(request)
        .await
        .context("Failed to connect to the Polymarket US WebSocket")?;
    let (mut write, mut read) = stream.split();

    let mut subscribed: HashSet<String> = HashSet::new();
    let mut next_id = 0u64;
    let mut refresh = tokio::time::interval(SLUG_REFRESH_INTERVAL);

    loop {
        tokio::select! {
            _ = refresh.tick() => {
                // Subscribe to newly matched slugs. There is no documented
                // unsubscribe; slugs that drop out stay subscribed until the
                // next reconnect, which is harmless (their updates are
                // ignored once no matched market carries the slug).
                let new: Vec<String> = slugs()
                    .into_iter()
                    .filter(|s| !subscribed.contains(s))
                    .collect();
                if !new.is_empty() {
                    for msg in subscribe_messages(&new, &mut next_id) {
                        write.send(msg).await.context("WS subscribe send failed")?;
                    }
                    info!("📡 Polymarket WS subscribed to {} new markets ({} total)",
                        new.len(), subscribed.len() + new.len());
                    subscribed.extend(new);
                }
            }
            frame = tokio::time::timeout(SILENCE_TIMEOUT, read.next()) => {
                let frame = frame.context("Polymarket WS silent for 30s")?;
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(event) = parse_event(&text) {
                            on_quote(event);
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        anyhow::bail!("Polymarket WS closed by server: {:?}", frame);
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e).context("Polymarket WS read error"),
                    None => anyhow::bail!("Polymarket WS stream ended"),
                }
            }
        }
    }
}

/// Run forever, reconnecting with exponential backoff (1s..30s). `slugs`
/// supplies the current desired subscription set; `on_quote` receives each
/// decoded quote or book event.
pub async fn run_markets_stream<S, Q>(key_id: String, secret_key: String, slugs: S, on_quote: Q)
where
    S: Fn() -> Vec<String>,
    Q: Fn(PolyWsEvent),
{
    let mut backoff = Duration::from_secs(1);
    loop {
        let started = std::time::Instant::now();
        match run_once(&key_id, &secret_key, &slugs, &on_quote).await {
            Ok(()) => debug!("Polymarket WS loop returned"),
            Err(e) => warn!("Polymarket WS disconnected: {:#}. Reconnecting in {:?}", e, backoff),
        }
        // A connection that stayed up a while earns a fresh backoff.
        if started.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(1);
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN: &str = r#"{"requestId":"r0","subscriptionType":"SUBSCRIPTION_TYPE_MARKET_DATA_LITE","marketDataLite":{"marketSlug":"s1","currentPx":{"value":"0.1050","currency":"USD"},"bestAsk":{"value":"0.1100","currency":"USD"},"bestBid":{"value":"0.1000","currency":"USD"},"longQuote":{"value":"0.1100","currency":"USD"},"shortQuote":{"value":"0.9","currency":"USD"},"state":"MARKET_STATE_OPEN"}}"#;

    #[test]
    fn parses_an_open_market_quote() {
        let q = parse_lite_message(OPEN).unwrap();
        assert_eq!(q.slug, "s1");
        assert!((q.long_quote - 0.11).abs() < 1e-9);
        assert!((q.short_quote - 0.9).abs() < 1e-9);
    }

    #[test]
    fn ignores_non_open_markets() {
        let expired = OPEN.replace("MARKET_STATE_OPEN", "MARKET_STATE_EXPIRED");
        assert!(parse_lite_message(&expired).is_none());
    }

    #[test]
    fn ignores_out_of_range_quotes() {
        assert!(parse_lite_message(&OPEN.replace(r#""0.9""#, r#""1.00""#)).is_none());
        assert!(parse_lite_message(&OPEN.replace(r#""0.1100","currency":"USD"},"shortQuote""#, r#""0.00","currency":"USD"},"shortQuote""#)).is_none());
    }

    #[test]
    fn ignores_heartbeats_acks_and_full_book_messages() {
        assert!(parse_lite_message(r#"{"heartbeat":{}}"#).is_none());
        assert!(parse_lite_message(r#"{"marketData":{"marketSlug":"s1","bids":[],"offers":[]}}"#).is_none());
        assert!(parse_lite_message("not json").is_none());
    }

    #[test]
    fn ignores_missing_quotes() {
        let no_quote = OPEN.replace("longQuote", "somethingElse");
        assert!(parse_lite_message(&no_quote).is_none());
    }

    #[test]
    fn chunks_subscriptions_at_100_slugs() {
        let slugs: Vec<String> = (0..250).map(|i| format!("s{i}")).collect();
        let mut id = 0;
        // Two subscription types (lite quotes + full book) per chunk.
        assert_eq!(subscribe_messages(&slugs, &mut id).len(), 6);
        assert_eq!(id, 6);
    }

    #[test]
    fn parses_a_full_book_snapshot_and_rejects_bad_levels() {
        let book = r#"{"marketData":{"marketSlug":"s1","bids":[{"px":{"value":"0.10"},"qty":"5"},{"px":{"value":"0.20"},"qty":"3"}],"offers":[{"px":{"value":"0.30"},"qty":"7"}],"state":"MARKET_STATE_OPEN"}}"#;
        let b = parse_book_message(book).unwrap();
        assert_eq!(b.market_slug, "s1");
        assert_eq!(b.bids[0].price, 0.20); // sorted best-first
        assert_eq!(b.offers[0].quantity, 7.0);
        // A lite message is not a book, and vice versa.
        assert!(parse_book_message(OPEN).is_none());
        assert!(parse_lite_message(book).is_none());
        let bad = book.replace(r#""qty":"7""#, r#""qty":"0""#);
        assert!(parse_book_message(&bad).is_none());
    }
}
