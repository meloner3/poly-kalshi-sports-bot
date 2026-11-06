# Polytaoli - Prediction Market Arbitrage Scanner

A high-performance, real-time prediction market arbitrage opportunity monitoring system for **Kalshi** and **Polymarket**.

## Core Features

- **High performance**: Rust backend, React frontend, and real-time WebSocket communication
- **Real-time monitoring**: Synchronized market data from both platforms with intelligent event matching
- **Automated arbitrage**: Calculates profit margins and expected returns, with optional automated order execution
- **Data tracking**: Arbitrage history, position management, and performance monitoring
- **Telegram notifications**: Notifications for automated trades

## System Architecture

```text
+-------------------------------------------------------------+
|                    Frontend (React + TS)                    |
|                  http://localhost:5173                      |
|  - Live arbitrage opportunities - Position management       |
|  - Historical data analysis                                  |
+--------------------+----------------------------------------+
                     | WebSocket + REST API
+--------------------+----------------------------------------+
|                 Rust Backend (Axum + Tokio)                 |
|                  http://localhost:8000                      |
|  +-------------------------------------------------------+  |
|  | Core services                                         |  |
|  | - ArbitrageService: arbitrage calculation and control |  |
|  | - WebSocketManager: live data delivery                |  |
|  | - EventMatcher: intelligent market matching           |  |
|  | - ArbitrageCalculator: profit-margin calculation      |  |
|  | - Storage: SQLite persistence                         |  |
|  +-------------------------------------------------------+  |
|  +------------------+          +------------------------+ |
|  | Kalshi client    |          | Polymarket US client   | |
|  | - REST API       |          | - REST API             | |
|  | - WebSocket      |          | - Ed25519 signing      | |
|  | - RSA-PSS signing|          |                        | |
|  +------------------+          +------------------------+ |
+-------------------------------------------------------------+
```

Both exchanges are called directly from the Rust backend, each with its own
signed request path - there is no separate order-service process.

### Technology Stack

**Backend (Rust)**

- Axum 0.7 - web framework
- Tokio - asynchronous runtime
- SQLite - data storage
- Reqwest - HTTP client
- Alloy/RSA - cryptographic signing

**Frontend (React)**

- React 18 and TypeScript 5
- Vite 5 - build tool
- Tailwind CSS - styling
- Recharts - data visualization

## Quick Start

### 1. Prerequisites

- Rust 1.70+
- Node.js 16+

### 2. Configuration

```bash
cd rust-backend
cp config.example.toml config.toml
```

Edit `config.toml`:

```toml
[kalshi]
api_key = "your-kalshi-api-key"
api_secret = """-----BEGIN RSA PRIVATE KEY-----
YOUR_PRIVATE_KEY_HERE
-----END RSA PRIVATE KEY-----"""

[polymarket]
# Generate these at https://polymarket.us/developer.
# This local config.toml file is ignored by Git.
key_id = "your-polymarket-us-key-id"
secret_key = "your-polymarket-us-secret-key"

[auth]
username = "admin"
password = "admin123"
secret_key = "your-secret-key-min-32-chars"

[settings]
refresh_interval = 5          # Refresh interval (seconds)
min_profit_margin = 1.0       # Minimum profit margin (%)
default_bet_amount = 10.0     # Default bet amount
tracking_threshold = 2.0      # Tracking threshold (%)

[auto_trade]
# Auto-trade limits are persisted in SQLite and managed from the UI/API after
# first startup. These values document the initial safe defaults.
enabled = false               # Enable automatic trading
max_amount = 10.0             # Maximum amount per trade
max_trade_count = 2           # Maximum execution count
min_duration_ms = 500         # Minimum opportunity duration
neutralization_max_loss_cents = 5 # Maximum emergency-close loss per contract

[telegram]
enabled = false
bot_token = "YOUR_BOT_TOKEN"
chat_id = "YOUR_CHAT_ID"
```

### 3. Start All Services

```bash
./start_rust_stack.sh
```

The script starts:

- The Rust backend on port 8000
- The React frontend on port 5173

### 4. Access the Application

- **Frontend**: http://localhost:5173
- **Backend API**: http://localhost:8000
- **Health check**: http://localhost:8000/api/health

Default credentials: `admin` / `admin123`

## Production Deployment

### Linux

```bash
./build_linux.sh
scp polytaoli-linux-x86_64-*.tar.gz user@server:/path/
tar -xzf polytaoli-linux-x86_64-*.tar.gz
cd deploy
cp config.example.toml config.toml
# Edit the configuration, then start the application.
./start.sh
```

### Windows

```bash
./build_windows.sh
```

## Core Functionality

### Arbitrage Calculation

- Calculates profit margins in real time, including fees
- Automatically selects the best strategy: Yes-Yes, Yes-No, No-Yes, or No-No
- Analyzes order book depth

### Market Matching

- Exact matching by event name and question description
- Fuzzy matching by keywords and time range
- Specialized NBA handling that identifies game information intelligently

### Automated Trading

- Uses a fresh executable Kalshi WebSocket order book and a freshly-fetched
  Polymarket US market-book endpoint immediately before submission.
- Sizes only whole contracts supported by both books, using worst-case fill
  prices, Kalshi fees, the configured maximum amount, and the required profit
  margin.
- Uses IOC/FAK orders and treats a zero-fill acknowledgement as a failed leg.
- Hedges complementary outcomes only: Kalshi YES is paired with Polymarket NO
  for the tracked competitor, and Kalshi NO is paired with Polymarket YES.
  The native Polymarket LONG/SHORT direction is resolved from that selected
  competitor; it is never inferred from an outcome index.
- Persists intent before submitting either leg and records each acknowledgement,
  fill, cancellation, rejection, neutralization attempt, and residual exposure.
- For mismatched fills, attempts an IOC/FAK offset only within the configured
  `neutralization_max_loss_cents` bound (5 cents per contract by default).
  If it cannot safely close the residual, it disables auto-trading and sends
  the configured Telegram alert.
- Disables auto-trading at startup if an execution was interrupted while its
  status was `submitting`. Reconcile that record and every residual position
  before manually re-enabling the bot.

### Data Management

- SQLite storage for arbitrage history
- Real-time performance metric monitoring
- Position retrieval and management

## API Endpoints

| Endpoint | Method | Description |
| --- | --- | --- |
| `/api/health` | GET | Health check |
| `/api/auth/login` | POST | User authentication |
| `/api/settings` | GET/PUT | Settings management |
| `/api/auto-trade/status` | GET | Automated-trading state and limits |
| `/api/auto-trade/enable`, `/api/auto-trade/disable` | POST | Change automated-trading state |
| `/api/auto-trade/settings` | PUT | Update automated-trading limits |
| `/api/auto-trade/history` | GET | Submission, fill, recovery, and residual-exposure history |
| `/api/positions/kalshi` | GET | Kalshi positions |
| `/api/positions/polymarket` | GET | Polymarket positions |
| `/api/arbitrage-history` | GET | Arbitrage tracking history |
| `/api/order/kalshi` | POST | Submit a Kalshi order |
| `/api/order/polymarket` | POST | Submit a Polymarket US order |
| `/ws` | WebSocket | Live data delivery |

## Security Recommendations

1. Change the default password and JWT secret.
2. Never commit configuration files that contain real credentials.
3. Use HTTPS with an Nginx reverse proxy in production.
4. Restrict the sources that can access exposed ports.

## Logs

Logs are written to `rust-backend/logs/polytaoli.log.YYYY-MM-DD` and rotate daily.

```bash
tail -f rust-backend/logs/polytaoli.log
```

## Troubleshooting

- **The backend will not start**: Check the configuration-file syntax and API credentials.
- **WebSocket disconnects**: Confirm the backend is running and firewall settings allow access.
- **Data is not updating**: Verify API-key permissions and network connectivity.
- **Automated trades are not executing**: Check the history for a stale book,
  insufficient executable depth, worst-case profit/amount rejection, or a
  `submitting`/`exposed` record. Do not re-enable the bot while a residual
  position or interrupted submission requires reconciliation.
- **An execution is `exposed`**: The bot has halted after a one-leg or
  unneutralized partial fill. Resolve the named exchange position manually,
  verify the account position, and record the reconciliation before re-enabling.

---

**Disclaimer**: This software is for educational and research purposes only. You assume all trading risk.
