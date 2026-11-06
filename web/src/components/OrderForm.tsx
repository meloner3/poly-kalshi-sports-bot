import { useState } from 'react';
import { MatchedMarketData } from '../types';
import { createKalshiOrder, createPolymarketOrder, executeArbitrage } from '../utils/api';

interface OrderFormProps {
  market: MatchedMarketData;
  apiBaseUrl: string;
  onOrderPlaced?: () => void;
}

type Side = 'yes' | 'no';
type Action = 'buy' | 'sell';

export function OrderForm({ market, apiBaseUrl, onOrderPlaced }: OrderFormProps) {
  const [count, setCount] = useState(1);
  const [polyContracts, setPolyContracts] = useState(1);
  const [loading, setLoading] = useState<string | null>(null);
  const [result, setResult] = useState<{ success: boolean; message: string; elapsed_ms?: number } | null>(null);
  const hasLivePolymarketQuote =
    market.poly_ready
    && Number.isFinite(market.poly_yes_price)
    && Number.isFinite(market.poly_no_price)
    && market.poly_yes_price > 0
    && market.poly_yes_price < 1
    && market.poly_no_price > 0
    && market.poly_no_price < 1;

  // Place a Kalshi order.
  const handleKalshiOrder = async (side: Side, action: Action) => {
    const loadingKey = `kalshi_${side}_${action}`;
    setLoading(loadingKey);
    setResult(null);

    try {
      const response = await createKalshiOrder(apiBaseUrl, {
        ticker: market.kalshi_market_id,
        side,
        action,
        count,
      });

      if (response.success) {
        setResult({
          success: true,
          message: `Kalshi ${action} ${side.toUpperCase()} succeeded! Filled ${response.order?.fill_count || 0}`,
          elapsed_ms: response.elapsed_ms,
        });
        onOrderPlaced?.();
      } else {
        setResult({
          success: false,
          message: response.error || 'Order failed',
        });
      }
    } catch (e) {
      setResult({
        success: false,
        message: e instanceof Error ? e.message : 'Order failed',
      });
    } finally {
      setLoading(null);
    }
  };

  // Place a Polymarket order.
  const handlePolyOrder = async (side: 'buy' | 'sell', competitor: 'team' | 'opponent') => {
    const loadingKey = `poly_${side}_${competitor}`;
    setLoading(loadingKey);
    setResult(null);

    try {
      const marketSlug = market.polymarket_market_slug;
      if (!marketSlug) {
        setResult({
          success: false,
          message: 'Polymarket US market slug not found',
        });
        return;
      }

      const response = await createPolymarketOrder(apiBaseUrl, {
        market_slug: marketSlug,
        position_side: competitor === 'team'
          ? market.polymarket_team_position_side
          : market.polymarket_opponent_position_side,
        side,
        contracts: polyContracts,
        price: competitor === 'team' ? market.poly_yes_price : market.poly_no_price,
      });

      if (response.success) {
        setResult({
          success: true,
          message: `Polymarket ${side.toUpperCase()} filled ${response.filled_contracts ?? 0} contract${response.filled_contracts === 1 ? '' : 's'}! Order ID: ${response.order_id?.slice(0, 8) || 'N/A'}`,
          elapsed_ms: response.elapsed_ms,
        });
        onOrderPlaced?.();
      } else {
        setResult({
          success: false,
          message: response.error || 'Order failed',
        });
      }
    } catch (e) {
      setResult({
        success: false,
        message: e instanceof Error ? e.message : 'Order failed',
      });
    } finally {
      setLoading(null);
    }
  };

  // Place both arbitrage orders.
  const handleArbitrageOrder = async () => {
    if (!market.has_opportunity || !market.arbitrage_type) {
      alert('No current arbitrage opportunity');
      return;
    }

    setLoading('arbitrage');
    setResult(null);

    try {
      // Determine the arbitrage strategy.
      const isKalshiYes = market.arbitrage_type.includes('KalshiYes');
      const kalshiSide: Side = isKalshiYes ? 'yes' : 'no';

      const response = await executeArbitrage(apiBaseUrl, {
        event_name: market.event_name,
        team_name: market.team_name,
        kalshi_side: kalshiSide,
        polymarket_competitor: isKalshiYes
          ? market.polymarket_opponent_name
          : market.team_name,
        contracts: polyContracts,
        // Live: sizing/pricing is computed against real depth and orders are
        // actually submitted to both exchanges.
        dry_run: false,
      });

      if (response.success) {
        const dryRun = response.status === 'dry_run';
        setResult({
          success: true,
          message: dryRun
            ? `DRY RUN: would trade ${response.contracts ?? '?'} contracts @ K ${response.kalshi_price != null ? (response.kalshi_price * 100).toFixed(0) + '¢' : '?'} / P ${response.polymarket_price != null ? (response.polymarket_price * 100).toFixed(0) + '¢' : '?'} — nothing was submitted`
            : `Arbitrage succeeded! Kalshi: ${response.kalshi?.success ? '✓' : '✗'}, Poly: ${response.polymarket?.success ? '✓' : '✗'}`,
          elapsed_ms: (response.kalshi?.elapsed_ms || 0) + (response.polymarket?.elapsed_ms || 0),
        });
        onOrderPlaced?.();
      } else {
        const errors = [];
        if (response.kalshi && !response.kalshi.success) errors.push(`Kalshi: ${response.kalshi.error || 'Failed'}`);
        if (response.polymarket && !response.polymarket.success) errors.push(`Poly: ${response.polymarket.error || 'Failed'}`);
        setResult({
          success: false,
          message: errors.join('; ') || response.error || 'Arbitrage failed',
        });
      }
    } catch (e) {
      setResult({
        success: false,
        message: e instanceof Error ? e.message : 'Order failed',
      });
    } finally {
      setLoading(null);
    }
  };

  return (
    <div className="space-y-3">
      {/* Size settings */}
      <div className="grid grid-cols-2 gap-2">
        {/* Kalshi contract count */}
        <div className="bg-[--bg-tertiary] rounded p-2">
          <div className="text-[10px] text-[--text-muted] mb-1">Kalshi Contracts</div>
          <div className="flex items-center gap-1">
            <button
              className="w-6 h-6 rounded bg-[--bg-secondary] text-[--text-secondary] hover:bg-[--bg-primary] disabled:opacity-50 text-xs"
              onClick={() => setCount(Math.max(1, count - 1))}
              disabled={count <= 1}
            >
              -
            </button>
            <input
              type="number"
              min={1}
              value={count}
              onChange={(e) => setCount(Math.max(1, parseInt(e.target.value) || 1))}
              className="flex-1 h-6 px-1 text-center text-xs bg-[--bg-secondary] border border-[--border-color] rounded text-[--text-primary]"
            />
            <button
              className="w-6 h-6 rounded bg-[--bg-secondary] text-[--text-secondary] hover:bg-[--bg-primary] text-xs"
              onClick={() => setCount(count + 1)}
            >
              +
            </button>
          </div>
        </div>

        {/* Polymarket contract count */}
        <div className="bg-[--bg-tertiary] rounded p-2">
          <div className="text-[10px] text-[--text-muted] mb-1">Poly Contracts</div>
          <div className="flex items-center gap-1">
            <button
              className="w-6 h-6 rounded bg-[--bg-secondary] text-[--text-secondary] hover:bg-[--bg-primary] disabled:opacity-50 text-xs"
              onClick={() => setPolyContracts(Math.max(1, polyContracts - 1))}
              disabled={polyContracts <= 1}
            >
              -
            </button>
            <input
              type="number"
              min={1}
              step={1}
              value={polyContracts}
              onChange={(e) => setPolyContracts(Math.max(1, parseInt(e.target.value) || 1))}
              className="flex-1 h-6 px-1 text-center text-xs bg-[--bg-secondary] border border-[--border-color] rounded text-[--text-primary]"
            />
            <button
              className="w-6 h-6 rounded bg-[--bg-secondary] text-[--text-secondary] hover:bg-[--bg-primary] text-xs"
              onClick={() => setPolyContracts(polyContracts + 1)}
            >
              +
            </button>
          </div>
        </div>
      </div>

      {/* Kalshi order */}
      <div className="bg-[--bg-tertiary] rounded p-2">
        <div className="flex items-center justify-between mb-2">
          <span className="text-[10px] text-blue-400 font-medium">Kalshi</span>
          <span className="text-[10px] text-[--text-muted] truncate max-w-[120px]">{market.kalshi_market_id}</span>
        </div>
        <div className="grid grid-cols-2 gap-1.5 mb-1.5">
          <button
            className="py-1.5 text-[10px] rounded bg-green-500/20 text-green-400 hover:bg-green-500/30 disabled:opacity-50"
            onClick={() => handleKalshiOrder('yes', 'buy')}
            disabled={loading !== null}
          >
            {loading === 'kalshi_yes_buy' ? '...' : `Buy YES Limit ${(market.kalshi_yes_price * 100).toFixed(0)}¢`}
          </button>
          <button
            className="py-1.5 text-[10px] rounded bg-red-500/20 text-red-400 hover:bg-red-500/30 disabled:opacity-50"
            onClick={() => handleKalshiOrder('no', 'buy')}
            disabled={loading !== null}
          >
            {loading === 'kalshi_no_buy' ? '...' : `Buy NO Limit ${(market.kalshi_no_price * 100).toFixed(0)}¢`}
          </button>
        </div>
        <div className="grid grid-cols-2 gap-1.5">
          <button
            className="py-1 text-[9px] rounded bg-green-500/10 text-green-400/70 hover:bg-green-500/20 disabled:opacity-50"
            onClick={() => handleKalshiOrder('yes', 'sell')}
            disabled={loading !== null}
          >
            {loading === 'kalshi_yes_sell' ? '...' : 'Sell YES Limit'}
          </button>
          <button
            className="py-1 text-[9px] rounded bg-red-500/10 text-red-400/70 hover:bg-red-500/20 disabled:opacity-50"
            onClick={() => handleKalshiOrder('no', 'sell')}
            disabled={loading !== null}
          >
            {loading === 'kalshi_no_sell' ? '...' : 'Sell NO Limit'}
          </button>
        </div>
      </div>

      {/* Polymarket order */}
      <div className="bg-[--bg-tertiary] rounded p-2">
        <div className="flex items-center justify-between mb-2">
          <span className="text-[10px] text-purple-400 font-medium">Polymarket</span>
          <span className="text-[10px] text-[--text-muted] truncate max-w-[120px]">{market.polymarket_market_slug.slice(0, 16)}...</span>
        </div>
        <div className="grid grid-cols-2 gap-1.5 mb-1.5">
          <button
            className="py-1.5 text-[10px] rounded bg-green-500/20 text-green-400 hover:bg-green-500/30 disabled:opacity-50"
            onClick={() => handlePolyOrder('buy', 'team')}
            disabled={loading !== null || !hasLivePolymarketQuote}
            title={hasLivePolymarketQuote ? undefined : 'Waiting for a current Polymarket quote'}
          >
            {loading === 'poly_buy_team' ? '...' : `Buy YES @ ${(market.poly_yes_price * 100).toFixed(0)}¢ (IOC)`}
          </button>
          <button
            className="py-1.5 text-[10px] rounded bg-red-500/20 text-red-400 hover:bg-red-500/30 disabled:opacity-50"
            onClick={() => handlePolyOrder('buy', 'opponent')}
            disabled={loading !== null || !hasLivePolymarketQuote}
            title={hasLivePolymarketQuote ? undefined : 'Waiting for a current Polymarket quote'}
          >
            {loading === 'poly_buy_opponent' ? '...' : `Buy NO @ ${(market.poly_no_price * 100).toFixed(0)}¢ (IOC)`}
          </button>
        </div>
        <div className="grid grid-cols-2 gap-1.5">
          <button
            className="py-1 text-[9px] rounded bg-green-500/10 text-green-400/70 hover:bg-green-500/20 disabled:opacity-50"
            onClick={() => handlePolyOrder('sell', 'team')}
            disabled={loading !== null || !hasLivePolymarketQuote}
            title={hasLivePolymarketQuote ? undefined : 'Waiting for a current Polymarket quote'}
          >
            {loading === 'poly_sell_team' ? '...' : 'Sell YES (IOC)'}
          </button>
          <button
            className="py-1 text-[9px] rounded bg-red-500/10 text-red-400/70 hover:bg-red-500/20 disabled:opacity-50"
            onClick={() => handlePolyOrder('sell', 'opponent')}
            disabled={loading !== null || !hasLivePolymarketQuote}
            title={hasLivePolymarketQuote ? undefined : 'Waiting for a current Polymarket quote'}
          >
            {loading === 'poly_sell_opponent' ? '...' : 'Sell NO (IOC)'}
          </button>
        </div>
      </div>

      {/* One-click arbitrage order */}
      {market.has_opportunity && (
        <button
          className="w-full py-2 text-xs font-medium rounded bg-gradient-to-r from-green-500 to-emerald-500 text-white hover:from-green-600 hover:to-emerald-600 disabled:opacity-50"
          onClick={handleArbitrageOrder}
          disabled={loading !== null}
        >
          {loading === 'arbitrage' ? 'Executing...' : `🚀 Execute Arbitrage (${market.profit_margin.toFixed(2)}%)`}
        </button>
      )}

      {/* Result message */}
      {result && (
        <div className={`p-2 rounded text-xs ${
          result.success 
            ? 'bg-green-500/20 text-green-400' 
            : 'bg-red-500/20 text-red-400'
        }`}>
          <div>{result.message}</div>
          {result.elapsed_ms && (
            <div className="text-[10px] opacity-70 mt-0.5">
              Elapsed: {result.elapsed_ms.toFixed(0)}ms
            </div>
          )}
        </div>
      )}
    </div>
  );
}
