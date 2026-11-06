//! Throwaway read-only probe: compare Polymarket US WebSocket quotes with
//! the REST gateway feed for real sports markets.
//! Run: cargo run --example probe_polymarket_ws
use std::collections::HashMap;

use polytaoli::clients::polymarket::resolve_credentials;
use polytaoli::clients::polymarket_ws::{run_markets_stream, PolyWsEvent};
use polytaoli::clients::PolymarketClient;
use polytaoli::config::Config;
use polytaoli::models::PolymarketPositionSide;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_file("config.toml")?;
    let (key_id, secret) = resolve_credentials(&config.polymarket)?;
    let client = PolymarketClient::new(config.polymarket)?;
    let (_, markets) = client.get_supported_events_and_markets().await?;
    let mut sample: HashMap<String, (f64, f64, PolymarketPositionSide, PolymarketPositionSide)> = HashMap::new();
    for m in markets.iter().take(400) {
        sample.entry(m.market_slug.clone()).or_insert((m.price_a, m.price_b, m.team_a_position, m.team_b_position));
        if sample.len() >= 12 { break; }
    }
    let slugs: Vec<String> = sample.keys().cloned().collect();
    println!("subscribing to {} slugs", slugs.len());
    let seen = std::sync::Mutex::new(std::collections::HashSet::new());
    let seen_books = std::sync::Mutex::new(std::collections::HashSet::new());
    let sample2 = sample.clone();
    let stream = run_markets_stream(key_id, secret, move || slugs.clone(), move |event| {
        let q = match event {
            PolyWsEvent::Book(b) => {
                if seen_books.lock().unwrap().insert(b.market_slug.clone()) {
                    println!("BOOK {:<40} best bid {:?} best offer {:?} ({} / {} levels)", b.market_slug,
                        b.bids.first().map(|l| (l.price, l.quantity)), b.offers.first().map(|l| (l.price, l.quantity)), b.bids.len(), b.offers.len());
                }
                return;
            }
            PolyWsEvent::Quote(q) => q,
        };
        let (pa, pb, sa, sb) = sample2[&q.slug];
        let side = |s: PolymarketPositionSide| if s == PolymarketPositionSide::Long { q.long_quote } else { q.short_quote };
        if seen.lock().unwrap().insert(q.slug.clone()) {
            println!("{:<45} ws a={:.2} b={:.2} | rest a={:.2} b={:.2}", q.slug, side(sa), side(sb), pa, pb);
        }
    });
    let _ = tokio::time::timeout(std::time::Duration::from_secs(20), stream).await;
    for slug in sample.keys().take(3) {
        let b = client.get_market_book(slug).await?;
        println!("REST {:<40} best bid {:?} best offer {:?}", slug,
            b.bids.first().map(|l| (l.price, l.quantity)), b.offers.first().map(|l| (l.price, l.quantity)));
    }
    Ok(())
}
