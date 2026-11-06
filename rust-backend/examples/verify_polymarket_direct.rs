//! Throwaway live-API smoke test for the Polymarket US direct-API migration.
//! Hits only read-only endpoints (balance, positions, open orders, and a
//! market book from a real supported market) - never places an order.
//! Run with: cargo run --example verify_polymarket_direct

use polytaoli::clients::PolymarketClient;
use polytaoli::config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_file("config.toml")?;
    let client = PolymarketClient::new(config.polymarket)?;

    println!("== balance ==");
    match client.get_balance().await {
        Ok(balance) => println!("OK: {balance}"),
        Err(e) => println!("ERR: {e:#}"),
    }

    println!("== positions ==");
    match client.get_positions().await {
        Ok(positions) => println!("OK: {positions}"),
        Err(e) => println!("ERR: {e:#}"),
    }

    println!("== open orders ==");
    match client.get_open_orders().await {
        Ok(orders) => println!("OK: {orders}"),
        Err(e) => println!("ERR: {e:#}"),
    }

    println!("== market book (from a live supported market) ==");
    match client.get_supported_events_and_markets().await {
        Ok((_, markets)) => {
            if let Some(market) = markets.first() {
                println!("using slug: {}", market.market_slug);
                match client.get_market_book(&market.market_slug).await {
                    Ok(book) => println!(
                        "OK: state={} bids={} offers={} min_qty={:?} tick={:?}",
                        book.state,
                        book.bids.len(),
                        book.offers.len(),
                        book.minimum_trade_qty,
                        book.price_tick_size
                    ),
                    Err(e) => println!("ERR: {e:#}"),
                }
            } else {
                println!("no supported markets returned, skipping book check");
            }
        }
        Err(e) => println!("ERR fetching supported markets: {e:#}"),
    }

    Ok(())
}
