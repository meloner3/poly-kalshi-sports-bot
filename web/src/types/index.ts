export interface Market {
  platform: 'Kalshi' | 'Polymarket';
  event_id: string;
  event_name: string;
  yes_price: number;
  no_price: number;
  volume: number;
  team_name?: string;
  category: string;
  end_time?: string;
}

export interface ArbitrageOpportunity {
  kalshi_market: Market;
  polymarket_market: Market;
  arbitrage_type: ArbitrageType;
  profit_margin: number;
  optimal_bet: [number, number];
  expected_profit: number;
  match_confidence: number;
}

export type ArbitrageType = 
  | 'KalshiYesPolymarketNo'
  | 'KalshiNoPolymarketYes';

export interface ScanResponse {
  kalshi_markets: Market[];
  polymarket_markets: Market[];
  opportunities: ArbitrageOpportunity[];
  matched_count: number;
}

export interface MentionSignalRow {
  signal: {
    market_ticker: string;
    event_id?: string | null;
    event_day?: string | null;
    phrase?: string | null;
    p_hat: number | null;
    p_lo: number | null;
    p_hi: number | null;
    model_version?: string | null;
    status: string;
    reason?: string | null;
  };
  signal_fresh: boolean;
  yes_bid: number | null;
  yes_ask: number | null;
  no_bid: number | null;
  no_ask: number | null;
  net_edge_yes: number | null;
  net_edge_no: number | null;
}

export interface MentionStatus {
  mode: 'off' | 'paper' | 'live';
  kill_switch: boolean;
  loss_breaker: boolean;
  breaker_day?: string | null;
  today_realized_pnl?: number;
  realized_pnl?: number;
  today_realized_pnl_paper?: number;
  today_realized_pnl_live?: number;
  realized_pnl_paper?: number;
  realized_pnl_live?: number;
  open_positions?: number;
  open_exposure?: number;
  live_order_lookup_confirmed?: boolean;
  signal_service_healthy?: boolean;
  predictor_model_version?: string | null;
  strategy_model_version?: string | null;
  limits?: {
    min_net_edge: number;
    max_open_positions: number;
    live_gate_min_days: number;
    max_spread: number;
    max_signal_age_secs: number;
    pre_event_only: boolean;
    bankroll_usd: number;
  };
  gate?: {
    edge_estimate?: number | null;
    ci_low?: number | null;
    ci_high?: number | null;
    n_days?: number;
    as_of?: string | null;
    live_gate_passed?: boolean;
    status?: string;
  } | null;
  error?: string;
}

export interface MentionDecision {
  id: number;
  market_ticker: string;
  side?: string | null;
  net_edge?: number | null;
  decision: string;
  reason: string;
  created_at: string;
}

export interface MentionPosition {
  id: number;
  market_ticker: string;
  side: string;
  mode: 'paper' | 'live';
  contracts: number;
  remaining_contracts: number;
  entry_vwap: number;
  net_pnl: number | null;
  outcome_yes: number | boolean | null;
  status: string;
  created_at: string;
}

export interface WsMessage {
  type: 'opportunity' | 'opportunities' | 'opportunities_list' | 'matched_markets_list' | 'scan_started' | 'scan_completed' | 'log' | 'ping' | 'pong' | 'connected' | 'stats' | 'metrics' | 'mention_signals';
  data?: ArbitrageOpportunity | ArbitrageOpportunity[] | MatchedMarketData[] | MetricsReport | MentionSignalRow[];
  status?: MentionStatus;
  kalshi_count?: number;
  polymarket_count?: number;
  matched_count?: number;
  opportunities_count?: number;
  count?: number;
  level?: string;
  message?: string;
  timestamp?: string;
}

// Performance monitoring types
export interface OperationStats {
  name: string;
  count: number;
  avg_ms: number;
  max_ms: number;
  total_ms: number;
}

export interface ApiLatency {
  kalshi_ms?: number;
  polymarket_ms?: number;
}

export interface MetricsReport {
  operations: OperationStats[];
  api_latency: ApiLatency;
}

// Matched market data, including real-time prices
export interface MatchedMarketData {
  event_name: string;
  team_name: string;
  game_date?: string;               // Game date in YYYY-MM-DD format
  kalshi_market_id: string;
  polymarket_market_id: string;
  polymarket_market_slug: string;
  polymarket_team_position_side: 'long' | 'short';
  polymarket_opponent_position_side: 'long' | 'short';
  polymarket_opponent_name: string;
  kalshi_yes_price: number;
  kalshi_no_price: number;
  poly_yes_price: number;
  poly_no_price: number;
  kalshi_ready: boolean;
  poly_ready: boolean;
  both_ready: boolean;
  confidence: number;
  end_time?: string;
  // Arbitrage fields
  has_opportunity: boolean;
  profit_margin: number;
  expected_profit: number;  // Net profit after fees
  gross_profit?: number;    // Gross profit before fees
  kalshi_contracts?: number;  // Number of Kalshi contracts
  kalshi_fee?: number;      // Kalshi trading fee
  arbitrage_type?: string;
}

export interface LogEntry {
  time: string;
  level: 'info' | 'success' | 'warning' | 'error';
  message: string;
}

// Arbitrage tracking record
export interface ProfitHistoryEntry {
  time: string;
  profit_margin: number;
  kalshi_price: number;
  polymarket_price: number;
}

export interface TrackingRecord {
  event_name: string;
  team_name: string;
  kalshi_market_id: string;
  polymarket_market_id: string;
  start_time: string;
  end_time: string | null;
  duration_seconds: number | null;
  duration_ms: number | null;  // Duration in milliseconds
  max_profit_margin: number;
  max_profit_time: string | null;
  profit_history: ProfitHistoryEntry[];
  // Depth information
  poly_ask_depth: number;  // Polymarket ask depth (USD)
  kalshi_ask_depth: number;  // Kalshi ask depth (contracts)
}

export interface ActiveTracking {
  event_name: string;
  team_name: string;
  start_time: string;
  duration_seconds: number;
  max_profit_margin: number;
}

export interface TrackingStats {
  active_count: number;
  completed_count: number;
  active: ActiveTracking[];
  recent_completed: TrackingRecord[];
}

// Data coverage
export interface DataCoverage {
  total_markets: number;
  kalshi_ready: number;
  polymarket_ready: number;
  both_ready: number;
  kalshi_coverage: string;
  polymarket_coverage: string;
  full_coverage: string;
  kalshi_connected: boolean;
  polymarket_connected: boolean;
  kalshi_source: 'rest_polling';
  polymarket_source: 'rest_polling';
  kalshi_latency_ms?: number;
  polymarket_latency_ms?: number;
}

// Account balances
export interface PlatformBalance {
  available: boolean;
  balance?: number;
  portfolio_value?: number;
  error?: string;
  pnl?: string;
  trades?: number;
  positions?: number;
  updated_ts?: number;
}

export interface AccountBalance {
  kalshi: PlatformBalance;
  polymarket: PlatformBalance;
}

// ==================== Trading Types ====================

// Kalshi order
export interface KalshiOrder {
  order_id: string;
  user_id?: string;
  client_order_id?: string;
  ticker: string;
  side: 'yes' | 'no';
  action: 'buy' | 'sell';
  type: 'limit' | 'market';
  status: 'resting' | 'canceled' | 'executed' | 'pending';
  yes_price?: number;
  no_price?: number;
  fill_count: number;
  remaining_count: number;
  initial_count: number;
  taker_fees?: number;
  maker_fees?: number;
  taker_fill_cost?: number;
  maker_fill_cost?: number;
  created_time: string;
  last_update_time?: string;
}

// Kalshi position
export interface KalshiPosition {
  ticker: string;
  event_ticker?: string;
  /** Legacy normalized fields. */
  market_exposure?: number | string;
  position?: number | string;
  resting_orders_count?: number;
  fees_paid?: number | string;
  total_traded?: number | string;
  realized_pnl?: number | string;
  /** Native Kalshi v2 portfolio position fields. Values are denominated in USD. */
  market_exposure_dollars?: number | string;
  position_fp?: number | string;
  fees_paid_dollars?: number | string;
  total_traded_dollars?: number | string;
  realized_pnl_dollars?: number | string;
}

// Kalshi order request
export interface KalshiOrderRequest {
  ticker: string;
  side: 'yes' | 'no';
  action: 'buy' | 'sell';
  count: number;
}

// Backward compatibility
export type OrderRequest = KalshiOrderRequest;

// Polymarket order request
export interface PolymarketOrderRequest {
  market_slug: string;
  position_side: 'long' | 'short';
  side: 'buy' | 'sell';
  contracts: number;
  price: number;
}

// Order response
export interface OrderResponse {
  success: boolean;
  order?: KalshiOrder;
  elapsed_ms?: number;
  error?: string;
}

// Polymarket order response
export interface PolymarketOrderResponse {
  success: boolean;
  order_id?: string;
  status?: string;
  filled_contracts?: number;
  taking_amount?: string;
  making_amount?: string;
  elapsed_ms?: number;
  error?: string;
}

// Polymarket order
export interface PolymarketOrder {
  id: string;
  status: string;
  owner: string;
  maker_address: string;
  market: string;
  asset_id: string;
  side: string;
  original_size: string;
  size_matched: string;
  price: string;
  outcome: string;
  created_at: number;
  order_type: string;
}

// Polymarket position
export interface PolymarketPosition {
  id?: string;
  asset?: string;
  conditionId?: string;
  outcomeIndex?: number;
  size?: string;
  avgPrice?: string;
  curPrice?: string;
  value?: string;
  pnl?: string;
  pnlPercent?: string;
  title?: string;
  // Additional fields may be handled dynamically from the API response.
  [key: string]: unknown;
}

// Unified position type for combined display
export interface UnifiedPosition {
  id: string;
  platform: 'kalshi' | 'polymarket';
  ticker: string;        // Kalshi ticker or Polymarket condition ID
  title?: string;        // Market title
  size: number;          // Position size
  side?: 'yes' | 'no' | 'buy' | 'sell';
  avgPrice?: number;     // Average price
  curPrice?: number;     // Current price
  value?: number;        // Position value
  pnl?: number;          // Profit and loss
  pnlPercent?: number;   // Profit and loss percentage
}

// Unified positions endpoint (/api/positions/unified)
export interface PositionLeg {
  platform: 'kalshi' | 'polymarket';
  market_id: string;
  title: string;
  /** Kalshi: yes/no. Polymarket: long/short. */
  side: string;
  contracts: number;
  entry_price: number | null;
  current_price: number | null;
  cost: number;
  value: number | null;
  unrealized_pnl: number | null;
  /** null = unknown (Polymarket exposes no fee field). */
  fees: number | null;
}

export interface PositionCard {
  kind: 'paired' | 'standalone';
  event_name: string | null;
  legs: PositionLeg[];
  total_cost: number;
  total_value: number | null;
  total_pnl: number | null;
  contracts_mismatch: boolean;
}

export interface UnifiedPositionsResponse {
  cards: PositionCard[];
  errors: string[];
}

// Orders list response
export interface OrdersResponse {
  orders: KalshiOrder[];
  error?: string;
}

// Polymarket orders list response
export interface PolymarketOrdersResponse {
  orders: PolymarketOrder[];
  error?: string;
}

// Positions list response
export interface PositionsResponse {
  positions: KalshiPosition[];
  error?: string;
}

// Arbitrage execution request
export interface ArbitrageExecuteRequest {
  event_name: string;
  team_name: string;
  kalshi_side: 'yes' | 'no';
  polymarket_competitor: string;
  contracts: number;
  /** When true, sizing/pricing is computed against real depth but nothing is submitted to either exchange. */
  dry_run: boolean;
}

// Arbitrage execution response
export interface ArbitrageExecuteResponse {
  success: boolean;
  status?: string;
  error?: string;
  contracts?: number;
  kalshi_price?: number;
  polymarket_price?: number;
  profit_margin?: number;
  kalshi?: {
    success: boolean;
    order?: Record<string, unknown>;
    elapsed_ms?: number;
    count?: number;
    order_id?: string;
    filled_contracts?: number;
    error?: string;
  };
  polymarket?: {
    success: boolean;
    order_id?: string;
    status?: string;
    elapsed_ms?: number;
    amount?: number;
    filled_contracts?: number;
    error?: string;
  };
}

// Order book depth
export interface SideDepth {
  price?: number;
  size?: number;
}

export interface PlatformDepthDual {
  yes: SideDepth;
  no: SideDepth;
}

export interface OrderbookDepthResponse {
  kalshi?: PlatformDepthDual;
  polymarket?: PlatformDepthDual;
}
