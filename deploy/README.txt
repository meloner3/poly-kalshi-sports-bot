Polytaoli - Prediction Market Arbitrage Scanner
================================

Deployment steps:
1. Copy config.example.toml to config.toml.
2. Edit config.toml and enter your Kalshi API credentials.
3. Edit `config.toml` and set `polymarket.key_id` and `polymarket.secret_key`, generated at https://polymarket.us/developer.
4. Run: ./start.sh

Configuration:
- Rust backend port: 8000
- Logs: stored in the logs/ directory
- Frontend: visit http://your-server:8000

Service architecture:
- Rust backend: handles arbitrage scanning, WebSockets, the API, and both
  exchanges' order placement (Kalshi RSA-PSS-signed, Polymarket US
  Ed25519-signed) - there is no separate Python order service.
  poly-order-service/ is kept in this bundle for reference only.

Stop the application: Ctrl+C or kill the process.
