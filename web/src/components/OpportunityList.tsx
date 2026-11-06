import { useEffect, useRef, useState, useCallback } from 'react';
import { MatchedMarketData, ArbitrageExecuteRequest } from '../types';
import { executeArbitrage } from '../utils/api';

interface OpportunityListProps {
  matchedMarkets: MatchedMarketData[];
  onSelectMarket?: (market: MatchedMarketData) => void;
  apiBaseUrl?: string;
}

type SortOption = 'profit' | 'event' | 'team';

export function OpportunityList({ matchedMarkets, onSelectMarket, apiBaseUrl = '' }: OpportunityListProps) {
  // Track price changes for highlight animations.
  const [flashingCells, setFlashingCells] = useState<Set<string>>(new Set());
  const prevPricesRef = useRef<Map<string, { k_yes: number; k_no: number; p_yes: number; p_no: number }>>(new Map());
  
  // Sort option.
  const [sortBy, setSortBy] = useState<SortOption>('profit');
  
  // Execution state.
  const [executingKey, setExecutingKey] = useState<string | null>(null);
  const [lastResult, setLastResult] = useState<{ key: string; success: boolean; dryRun: boolean; message: string } | null>(null);
  
  // Excluded market state.
  const [excludedMarkets, setExcludedMarkets] = useState<Set<string>>(new Set());
  const [excludingKey, setExcludingKey] = useState<string | null>(null);
  
  // Fetch excluded markets.
  const fetchExcludedMarkets = useCallback(async () => {
    if (!apiBaseUrl) return;
    try {
      const res = await fetch(`${apiBaseUrl}/api/auto-trade/excluded`);
      const data = await res.json();
      setExcludedMarkets(new Set(data.excluded_markets || []));
    } catch (err) {
      console.error('Failed to fetch excluded markets:', err);
    }
  }, [apiBaseUrl]);
  
  // Initially load excluded markets.
  useEffect(() => {
    fetchExcludedMarkets();
  }, [fetchExcludedMarkets]);
  
  // Generate market key including game_date for accurate identification
  const getMarketKey = (market: MatchedMarketData): string => {
    const eventName = market.event_name.toUpperCase();
    const teamName = market.team_name.toUpperCase();
    if (market.game_date) {
      return `${eventName}_${market.game_date}_${teamName}`;
    }
    return `${eventName}_${teamName}`;
  };
  
  // Exclude or unexclude a market.
  const handleToggleExclude = async (market: MatchedMarketData, e: React.MouseEvent) => {
    e.stopPropagation();
    if (!apiBaseUrl) return;
    
    const key = getMarketKey(market);
    const isExcluded = excludedMarkets.has(key);
    
    setExcludingKey(key);
    
    try {
      const endpoint = isExcluded ? 'unexclude' : 'exclude';
      const res = await fetch(`${apiBaseUrl}/api/auto-trade/${endpoint}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          event_name: market.event_name,
          team_name: market.team_name,
          game_date: market.game_date || null
        })
      });
      
      if (res.ok) {
        // Update local state.
        setExcludedMarkets(prev => {
          const next = new Set(prev);
          if (isExcluded) {
            next.delete(key);
          } else {
            next.add(key);
          }
          return next;
        });
      }
    } catch (err) {
      console.error('Failed to update market exclusion:', err);
    } finally {
      setExcludingKey(null);
    }
  };
  
  // Execute arbitrage.
  const handleExecute = async (market: MatchedMarketData, executionKey: string, e: React.MouseEvent) => {
    e.stopPropagation();
    if (!market.has_opportunity || !apiBaseUrl) return;

    setExecutingKey(executionKey);
    setLastResult(null);

    try {
      // Determine the strategy type.
      // K↑ P↓ = Kalshi Yes + Polymarket No
      // K↓ P↑ = Kalshi No + Polymarket Yes
      const isKalshiYes = market.arbitrage_type?.includes('KalshiYes');

      const request: ArbitrageExecuteRequest = {
        event_name: market.event_name,
        team_name: market.team_name,
        kalshi_side: isKalshiYes ? 'yes' : 'no',
        polymarket_competitor: isKalshiYes
          ? market.polymarket_opponent_name
          : market.team_name,
        contracts: 10,
        // Live: sizing/pricing is computed against real depth and orders are
        // actually submitted to both exchanges.
        dry_run: false,
      };

      const result = await executeArbitrage(apiBaseUrl, request);
      const dryRun = result.status === 'dry_run';

      if (result.success) {
        const message = dryRun
          ? `DRY RUN: would trade ${result.contracts ?? '?'} contracts @ K ${result.kalshi_price != null ? (result.kalshi_price * 100).toFixed(0) + '¢' : '?'} / P ${result.polymarket_price != null ? (result.polymarket_price * 100).toFixed(0) + '¢' : '?'} (${result.profit_margin?.toFixed(2) ?? '?'}% margin) — nothing was submitted`
          : 'Arbitrage executed successfully!';
        setLastResult({ key: executionKey, success: true, dryRun, message });
      } else {
        const errors = [];
        if (result.kalshi && !result.kalshi.success) errors.push(`K: ${result.kalshi.error}`);
        if (result.polymarket && !result.polymarket.success) errors.push(`P: ${result.polymarket.error}`);
        setLastResult({ key: executionKey, success: false, dryRun, message: errors.join('; ') || result.error || 'Execution failed' });
      }
    } catch (err) {
      setLastResult({ key: executionKey, success: false, dryRun: false, message: err instanceof Error ? err.message : 'Execution failed' });
    } finally {
      setExecutingKey(null);
      // Keep the reason visible for a while instead of clearing it instantly on the next render.
      setTimeout(() => {
        setLastResult((current) => (current?.key === executionKey ? null : current));
      }, 8000);
    }
  };

  useEffect(() => {
    const newFlashing = new Set<string>();
    
    matchedMarkets.forEach((m) => {
      // Use a more unique key: kalshi_market_id + polymarket_market_id.
      const key = `${m.kalshi_market_id}_${m.polymarket_market_id}`;
      const prev = prevPricesRef.current.get(key);
      
      if (prev) {
        if (prev.k_yes !== m.kalshi_yes_price || prev.k_no !== m.kalshi_no_price) {
          newFlashing.add(`${key}_kalshi`);
        }
        if (prev.p_yes !== m.poly_yes_price || prev.p_no !== m.poly_no_price) {
          newFlashing.add(`${key}_poly`);
        }
      }
      
      // Update cache.
      prevPricesRef.current.set(key, {
        k_yes: m.kalshi_yes_price,
        k_no: m.kalshi_no_price,
        p_yes: m.poly_yes_price,
        p_no: m.poly_no_price
      });
    });
    
    if (newFlashing.size > 0) {
      setFlashingCells(newFlashing);
      // Clear flashing state.
      const timer = setTimeout(() => setFlashingCells(new Set()), 500);
      return () => clearTimeout(timer);
    }
  }, [matchedMarkets]);

  if (matchedMarkets.length === 0) {
    return (
      <div className="p-4 text-center h-full flex flex-col items-center justify-center">
        <div className="text-2xl mb-2">⏳</div>
        <div className="text-[--text-secondary] text-sm">Scanning markets...</div>
        <div className="text-[--text-muted] text-[10px] mt-1">Real-time opportunities will appear here</div>
      </div>
    );
  }

  // Count arbitrage opportunities.
  const oppCount = matchedMarkets.filter(m => m.has_opportunity).length;

  // Sort markets.
  const sortedMarkets = [...matchedMarkets].sort((a, b) => {
    switch (sortBy) {
      case 'profit':
        // Descending profit margin, with opportunities first.
        if (a.has_opportunity && !b.has_opportunity) return -1;
        if (!a.has_opportunity && b.has_opportunity) return 1;
        if (a.has_opportunity && b.has_opportunity) {
          return b.profit_margin - a.profit_margin;
        }
        return a.event_name.localeCompare(b.event_name);
      
      case 'event':
        // Alphabetically by event name.
        return a.event_name.localeCompare(b.event_name);
      
      case 'team':
        // Alphabetically by team name.
        return (a.team_name || '').localeCompare(b.team_name || '');
      
      default:
        return 0;
    }
  });

  return (
    <div className="overflow-hidden flex flex-col h-full text-xs">
      {/* Sort selector */}
      <div className="px-2 py-1 bg-[--bg-tertiary] border-b border-[--border-color] flex items-center justify-between flex-shrink-0">
        <span className="text-[10px] text-[--text-muted]">Sort:</span>
        <div className="flex gap-1">
          <button
            onClick={() => setSortBy('profit')}
            className={`px-2 py-0.5 rounded text-[10px] font-medium transition-colors ${
              sortBy === 'profit'
                ? 'bg-[--accent-green] text-white'
                : 'bg-[--bg-secondary] text-[--text-secondary] hover:bg-[--bg-primary]'
            }`}
          >
            💰 Profit
          </button>
          <button
            onClick={() => setSortBy('event')}
            className={`px-2 py-0.5 rounded text-[10px] font-medium transition-colors ${
              sortBy === 'event'
                ? 'bg-[--accent-purple] text-white'
                : 'bg-[--bg-secondary] text-[--text-secondary] hover:bg-[--bg-primary]'
            }`}
          >
            📅 Event
          </button>
          <button
            onClick={() => setSortBy('team')}
            className={`px-2 py-0.5 rounded text-[10px] font-medium transition-colors ${
              sortBy === 'team'
                ? 'bg-[--accent-yellow] text-white'
                : 'bg-[--bg-secondary] text-[--text-secondary] hover:bg-[--bg-primary]'
            }`}
          >
            🏀 Team
          </button>
        </div>
      </div>

      {/* Scrollable table container */}
      <div className="flex-1 overflow-y-auto overflow-x-hidden">
        <table className="w-full">
          <thead className="sticky top-0 bg-[--bg-secondary] z-10 text-[10px]">
            <tr>
              <th className="py-1 px-2 font-medium text-[--text-secondary]">Event</th>
              <th className="py-1 px-2 text-center font-medium text-[--text-secondary]">Team</th>
              <th className="py-1 px-2 text-center font-medium text-[--text-secondary]">Kalshi</th>
              <th className="py-1 px-2 text-center font-medium text-[--text-secondary]">Polymarket</th>
              <th className="py-1 px-2 text-center font-medium text-[--text-secondary]">Str.</th>
              <th className="py-1 px-2 text-right font-medium text-[--text-secondary]">Profit</th>
              <th className="py-1 px-2 text-center w-24 font-medium text-[--text-secondary]">Act</th>
            </tr>
          </thead>
          <tbody>
            {sortedMarkets.map((market) => {
              // Use kalshi_market_id + polymarket_market_id as a unique key.
              const uniqueKey = `${market.kalshi_market_id}_${market.polymarket_market_id}`;
              // Include the date in this key to differentiate games on different days.
              const marketKey = getMarketKey(market);
              const kalshiFlashing = flashingCells.has(`${uniqueKey}_kalshi`);
              const polyFlashing = flashingCells.has(`${uniqueKey}_poly`);
              
              return (
                <tr
                  key={uniqueKey}
                  onClick={() => onSelectMarket?.(market)}
                  className={`cursor-pointer border-b border-[--border-color] hover:bg-[--bg-secondary] transition-colors ${
                    excludedMarkets.has(marketKey) 
                      ? 'bg-[rgba(239,68,68,0.05)] opacity-60' 
                      : market.has_opportunity 
                        ? 'bg-[rgba(16,185,129,0.05)]' 
                        : ''
                  }`}
                >
                  {/* Event */}
                  <td className="py-1 px-2">
                    <div className="flex flex-col">
                      <span className="text-[--text-primary] font-medium truncate max-w-[100px] block" title={market.event_name}>
                        {market.event_name}
                      </span>
                      {market.game_date && (
                        <span className="text-[9px] text-[--text-muted]" title={`Game date: ${market.game_date}`}>
                          {market.game_date}
                        </span>
                      )}
                    </div>
                  </td>

                  {/* Team */}
                  <td className="text-center py-1 px-2">
                    <span className="px-1.5 py-0.5 rounded bg-[--bg-tertiary] text-[--accent-yellow] text-[10px] font-medium truncate max-w-[60px] inline-block" title={market.team_name}>
                      {market.team_name || '-'}
                    </span>
                  </td>

                  {/* Kalshi Prices */}
                  <td className="text-center py-1 px-2">
                    <span className={`price-tag price-kalshi tabular-nums text-[10px] ${kalshiFlashing ? 'flash-update' : ''} ${!market.kalshi_ready ? 'opacity-50' : ''}`}>
                      <span className="text-green-400">{formatCents(market.kalshi_yes_price)}</span>
                      <span className="text-[--text-muted] mx-0.5">/</span>
                      <span className="text-red-400">{formatCents(market.kalshi_no_price)}</span>
                    </span>
                  </td>

                  {/* Polymarket Prices */}
                  <td className="text-center py-1 px-2">
                    <span className={`price-tag price-poly tabular-nums text-[10px] ${polyFlashing ? 'flash-update' : ''} ${!market.poly_ready ? 'opacity-50' : ''}`}>
                      <span className="text-green-400">{formatCents(market.poly_yes_price)}</span>
                      <span className="text-[--text-muted] mx-0.5">/</span>
                      <span className="text-red-400">{formatCents(market.poly_no_price)}</span>
                    </span>
                  </td>

                  {/* Strategy */}
                  <td className="text-center py-1 px-2">
                    {market.has_opportunity ? (
                      <span className="text-[--text-secondary] text-[10px]">
                        {getStrategyShort(market.arbitrage_type || '')}
                      </span>
                    ) : (
                      <span className="text-[--text-muted] text-[10px]">-</span>
                    )}
                  </td>

                  {/* Profit (net) */}
                  <td className="text-right py-1 px-2">
                    {market.has_opportunity ? (
                      <div className={getProfitClass(market.profit_margin)}>
                        <span className="text-sm font-bold tabular-nums">{market.profit_margin.toFixed(2)}%</span>
                        <div className="text-[9px] opacity-70 leading-none" title="Net profit">${market.expected_profit.toFixed(2)}</div>
                      </div>
                    ) : (
                      <span className="text-[--text-muted] text-[10px]">-</span>
                    )}
                  </td>
                  
                  {/* Action */}
                  <td className="text-center py-1 px-2">
                    <div className="flex items-center justify-center gap-1">
                      {market.has_opportunity ? (
                        <div className="flex flex-col items-center gap-0.5 max-w-[110px]">
                          <button
                            onClick={(e) => handleExecute(market, marketKey, e)}
                            disabled={executingKey === marketKey || !apiBaseUrl}
                            title={lastResult?.key === marketKey ? lastResult.message : 'Execute this paired arbitrage'}
                            className={`w-full px-2 py-1 text-[10px] font-semibold rounded-md border transition-colors ${
                              executingKey === marketKey
                                ? 'bg-gray-500/20 text-gray-400 border-gray-500/30 cursor-wait'
                                : lastResult?.key === marketKey
                                  ? lastResult.dryRun
                                    ? 'bg-blue-500/20 text-blue-400 border-blue-500/40 hover:bg-blue-500/30'
                                    : lastResult.success
                                      ? 'bg-green-500/20 text-green-400 border-green-500/40 hover:bg-green-500/30'
                                      : 'bg-red-500/20 text-red-400 border-red-500/40 hover:bg-red-500/30'
                                  : 'bg-[--accent-green]/20 text-[--accent-green] border-[--accent-green]/40 hover:bg-[--accent-green]/30'
                            }`}
                          >
                            {executingKey === marketKey
                              ? '⏳ Sending…'
                              : lastResult?.key === marketKey
                                ? lastResult.dryRun ? '🧪 Dry Run' : lastResult.success ? '✓ Sent' : '✗ Blocked'
                                : '▶ Execute'}
                          </button>
                          {lastResult?.key === marketKey && (
                            <span
                              className={`text-[9px] leading-tight text-center ${
                                lastResult.dryRun ? 'text-blue-400' : lastResult.success ? 'text-green-400' : 'text-red-400'
                              }`}
                              title={lastResult.message}
                            >
                              {lastResult.message}
                            </span>
                          )}
                        </div>
                      ) : null}
                      {/* Exclude button */}
                      <button
                        onClick={(e) => handleToggleExclude(market, e)}
                        disabled={excludingKey === marketKey || !apiBaseUrl}
                        title={excludedMarkets.has(marketKey) ? 'Remove Exclusion' : 'Exclude This Market'}
                        className={`px-1 py-0.5 text-[9px] font-medium rounded transition-colors ${
                          excludingKey === marketKey
                            ? 'bg-gray-500/30 text-gray-400 cursor-wait'
                            : excludedMarkets.has(marketKey)
                              ? 'bg-orange-500/20 text-orange-400 hover:bg-orange-500/30'
                              : 'bg-gray-500/20 text-gray-400 hover:bg-red-500/20 hover:text-red-400'
                        }`}
                      >
                        {excludingKey === marketKey ? '...' : excludedMarkets.has(marketKey) ? '🚫' : '⊘'}
                      </button>
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      
      {/* Bottom statistics */}
      <div className="px-2 py-1 bg-[--bg-tertiary] border-t border-[--border-color] flex justify-between items-center text-[10px] text-[--text-muted] flex-shrink-0">
        <span>Total: {matchedMarkets.length}</span>
        <span className={oppCount > 0 ? 'text-[--accent-green]' : ''}>
          {oppCount > 0 ? `🔥 ${oppCount} Opps` : 'No Opps'}
        </span>
      </div>
    </div>
  );
}

function formatCents(price: number): string {
  return (price * 100).toFixed(0) + '¢';
}

function getProfitClass(margin: number): string {
  if (margin >= 5) return 'profit-high';
  if (margin >= 2) return 'profit-medium';
  return 'profit-low';
}

function getStrategyShort(type: string): string {
  if (type.includes('KalshiYes') && type.includes('PolymarketNo')) return 'K↑ P↓';
  if (type.includes('KalshiNo') && type.includes('PolymarketYes')) return 'K↓ P↑';
  // Try to shorten more if needed
  return type.replace('Kalshi', 'K').replace('Polymarket', 'P').replace('Yes', '↑').replace('No', '↓');
}
