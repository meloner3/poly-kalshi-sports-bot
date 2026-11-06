//! Polytaoli - Prediction Market Arbitrage Scanner
//!
//! A high-performance arbitrage scanner for Kalshi and Polymarket prediction markets.

use anyhow::Result;
use tracing::info;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

mod api;
mod clients;
mod config;
mod core;
mod models;
mod services;
mod utils;

use crate::config::Config;

#[tokio::main]
async fn main() -> Result<()> {
    // Create the log directory.
    std::fs::create_dir_all("logs")?;

    // Initialize the debug log path (using the current working directory).
    utils::init_debug_log_path(None);

    // File log appender - rotate daily.
    let file_appender = tracing_appender::rolling::daily("logs", "polytaoli.log");
    let (non_blocking_file, _guard) = tracing_appender::non_blocking(file_appender);

    // Console log layer - show only info and higher. tower_http=info (not
    // warn) so every completed HTTP request logs a line, not just failures.
    let console_layer = fmt::layer()
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_filter(EnvFilter::new("polytaoli=info,tower_http=info"));

    // File log layer - record info and higher.
    let file_layer = fmt::layer()
        .with_writer(non_blocking_file)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_line_number(true)
        .with_filter(EnvFilter::new("polytaoli=info,tower_http=info"));

    // Combine log layers.
    tracing_subscriber::registry()
        .with(console_layer)
        .with(file_layer)
        .init();

    info!("🚀 Starting Polytaoli - Prediction Market Arbitrage Scanner");
    info!("📝 Log file: logs/polytaoli.log");

    // Load configuration
    let config = Config::from_file("config.toml")?;
    info!("✅ Configuration file loaded");

    // Initialize and run the application
    let app = api::create_app(config).await?;

    // Start the server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8000").await?;
    info!("🌐 Server listening at: http://0.0.0.0:8000");

    axum::serve(listener, app).await?;

    Ok(())
}
