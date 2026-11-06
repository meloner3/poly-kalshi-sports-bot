import { 
  Market, 
  ScanResponse, 
  OrderRequest, 
  OrderResponse, 
  OrdersResponse, 
  PositionsResponse,
  PolymarketOrderRequest,
  PolymarketOrderResponse,
  PolymarketOrdersResponse,
  PolymarketPosition,
  UnifiedPositionsResponse,
  ArbitrageExecuteRequest,
  ArbitrageExecuteResponse
} from '../types';

const API_BASE = '/api';

/**
 * Gets authentication headers.
 */
function getAuthHeaders(): HeadersInit {
  const token = localStorage.getItem('auth_token');
  const headers: HeadersInit = {
    'Content-Type': 'application/json',
  };
  
  if (token) {
    headers['Authorization'] = `Bearer ${token}`;
  }
  
  return headers;
}

export async function fetchKalshiMarkets(): Promise<Market[]> {
  const response = await fetch(`${API_BASE}/markets/kalshi`);
  if (!response.ok) {
    throw new Error('Failed to fetch Kalshi markets');
  }
  return response.json();
}

export async function fetchPolymarketMarkets(): Promise<Market[]> {
  const response = await fetch(`${API_BASE}/markets/polymarket`);
  if (!response.ok) {
    throw new Error('Failed to fetch Polymarket markets');
  }
  return response.json();
}

export async function fetchOpportunities(): Promise<ScanResponse> {
  const response = await fetch(`${API_BASE}/opportunities`);
  if (!response.ok) {
    throw new Error('Failed to fetch arbitrage opportunities');
  }
  return response.json();
}

export async function triggerScan(): Promise<ScanResponse> {
  const response = await fetch(`${API_BASE}/scan`, {
    method: 'POST',
  });
  if (!response.ok) {
    throw new Error('Failed to trigger scan');
  }
  return response.json();
}

export async function checkHealth(): Promise<{ status: string; version: string }> {
  const response = await fetch(`${API_BASE}/health`);
  if (!response.ok) {
    throw new Error('Health check failed');
  }
  return response.json();
}

// ==================== Trading APIs ====================

/**
 * Creates a Kalshi market order.
 */
export async function createKalshiOrder(
  baseUrl: string,
  request: OrderRequest
): Promise<OrderResponse> {
  const response = await fetch(`${baseUrl}/api/order/kalshi`, {
    method: 'POST',
    headers: getAuthHeaders(),
    body: JSON.stringify(request),
  });
  return response.json();
}

/**
 * Gets Kalshi orders.
 */
export async function getKalshiOrders(
  baseUrl: string,
  status?: string
): Promise<OrdersResponse> {
  const url = status 
    ? `${baseUrl}/api/orders/kalshi?status=${status}`
    : `${baseUrl}/api/orders/kalshi`;
  const response = await fetch(url);
  return response.json();
}

/**
 * Gets Kalshi positions.
 */
export async function getKalshiPositions(
  baseUrl: string
): Promise<PositionsResponse> {
  const response = await fetch(`${baseUrl}/api/positions/kalshi`);
  return response.json();
}

/**
 * Cancels a Kalshi order.
 */
export async function cancelKalshiOrder(
  baseUrl: string,
  orderId: string
): Promise<{ success: boolean; error?: string }> {
  const response = await fetch(`${baseUrl}/api/orders/kalshi/${orderId}`, {
    method: 'DELETE',
  });
  return response.json();
}

// ==================== Polymarket Trading APIs ====================

/**
 * Creates a Polymarket market order.
 */
export async function createPolymarketOrder(
  baseUrl: string,
  request: PolymarketOrderRequest
): Promise<PolymarketOrderResponse> {
  const response = await fetch(`${baseUrl}/api/order/polymarket`, {
    method: 'POST',
    headers: getAuthHeaders(),
    body: JSON.stringify(request),
  });
  return response.json();
}

/**
 * Gets Polymarket orders.
 */
export async function getPolymarketOrders(
  baseUrl: string
): Promise<PolymarketOrdersResponse> {
  const response = await fetch(`${baseUrl}/api/orders/polymarket`);
  return response.json();
}

/**
 * Cancels a Polymarket order.
 */
export async function cancelPolymarketOrder(
  baseUrl: string,
  orderId: string
): Promise<{ success: boolean; error?: string }> {
  const response = await fetch(`${baseUrl}/api/orders/polymarket/${orderId}`, {
    method: 'DELETE',
  });
  return response.json();
}

/**
 * Gets Polymarket positions.
 */
export async function getPolymarketPositions(
  baseUrl: string
): Promise<{ positions: PolymarketPosition[]; error?: string }> {
  const response = await fetch(`${baseUrl}/api/positions/polymarket`);
  return response.json();
}

/**
 * Gets positions grouped into hedged pairs and standalone legs.
 */
export async function getUnifiedPositions(
  baseUrl: string
): Promise<UnifiedPositionsResponse> {
  const response = await fetch(`${baseUrl}/api/positions/unified`);
  if (!response.ok) {
    throw new Error(`Positions request failed (${response.status})`);
  }
  return response.json();
}

// ==================== Arbitrage Execution APIs ====================

/**
 * Executes an arbitrage trade by placing orders on both platforms.
 */
export async function executeArbitrage(
  baseUrl: string,
  request: ArbitrageExecuteRequest
): Promise<ArbitrageExecuteResponse> {
  const response = await fetch(`${baseUrl}/api/arbitrage/execute`, {
    method: 'POST',
    headers: getAuthHeaders(),
    body: JSON.stringify(request),
  });
  const contentType = response.headers.get('content-type') || '';
  if (!contentType.includes('application/json')) {
    throw new Error(await response.text());
  }
  return response.json();
}

// ==================== Automated Trading APIs ====================

/**
 * Automated trading status response type.
 */
export interface AutoTradeStatus {
  enabled: boolean;
  trade_count: number;
  max_trade_count: number;
  remaining: number;
  max_amount: number;
  min_duration_ms: number;
  /** Whether flexible order sizing is enabled. */
  flexible_mode: boolean;
  /** Maximum contracts per trade. */
  max_contracts: number;
  /** Minimum contracts required to trade. */
  min_contracts: number;
  /** Maximum adverse price allowed for an emergency close, in cents/contract. */
  neutralization_max_loss_cents: number;
  last_trade_time: string | null;
}

/**
 * Gets automated trading status.
 */
export async function getAutoTradeStatus(
  baseUrl: string
): Promise<AutoTradeStatus> {
  const response = await fetch(`${baseUrl}/api/auto-trade/status`);
  return response.json();
}

/**
 * Enables automated trading.
 */
export async function enableAutoTrade(
  baseUrl: string
): Promise<{ success: boolean; message?: string; error?: string }> {
  const response = await fetch(`${baseUrl}/api/auto-trade/enable`, {
    method: 'POST',
    headers: getAuthHeaders(),
  });
  return response.json();
}

/**
 * Disables automated trading.
 */
export async function disableAutoTrade(
  baseUrl: string
): Promise<{ success: boolean; message?: string; error?: string }> {
  const response = await fetch(`${baseUrl}/api/auto-trade/disable`, {
    method: 'POST',
    headers: getAuthHeaders(),
  });
  return response.json();
}

/**
 * Resets the automated trade count.
 */
export async function resetAutoTradeCount(
  baseUrl: string
): Promise<{ success: boolean; message?: string; error?: string }> {
  const response = await fetch(`${baseUrl}/api/auto-trade/reset`, {
    method: 'POST',
    headers: getAuthHeaders(),
  });
  return response.json();
}

/**
 * Updates automated trading settings.
 */
export async function updateAutoTradeSettings(
  baseUrl: string,
  settings: {
    max_amount?: number;
    min_duration_ms?: number;
    max_trade_count?: number;
    /** Whether flexible order sizing is enabled. */
    flexible_mode?: boolean;
    /** Maximum contracts per trade. */
    max_contracts?: number;
    /** Minimum contracts required. */
    min_contracts?: number;
    /** Maximum adverse price allowed for an emergency close, in cents/contract. */
    neutralization_max_loss_cents?: number;
  }
): Promise<{ success: boolean; message?: string; error?: string }> {
  const response = await fetch(`${baseUrl}/api/auto-trade/settings`, {
    method: 'PUT',
    headers: getAuthHeaders(),
    body: JSON.stringify(settings),
  });
  return response.json();
}

// ==================== Application Settings APIs ====================

/**
 * Application settings response type.
 */
export interface AppSettings {
  /** Data refresh interval in seconds. */
  refresh_interval: number;
  /** Minimum profit margin for displayed opportunities. */
  min_profit_margin: number;
  /** Default USD amount used for arbitrage calculations. */
  default_bet_amount: number;
  /** Profit margin threshold for starting tracking. */
  tracking_threshold: number;
  updated_at: string | null;
}

/**
 * Gets application settings.
 */
export async function getAppSettings(
  baseUrl: string
): Promise<AppSettings> {
  const response = await fetch(`${baseUrl}/api/settings`, {
    headers: getAuthHeaders(),
  });
  return response.json();
}

/**
 * Updates application settings.
 */
export async function updateAppSettings(
  baseUrl: string,
  settings: {
    refresh_interval?: number;
    min_profit_margin?: number;
    default_bet_amount?: number;
    tracking_threshold?: number;
  }
): Promise<{ success: boolean; message?: string; error?: string }> {
  const response = await fetch(`${baseUrl}/api/settings`, {
    method: 'PUT',
    headers: getAuthHeaders(),
    body: JSON.stringify(settings),
  });
  return response.json();
}
