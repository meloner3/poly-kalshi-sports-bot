import { useEffect, useMemo, useState, useCallback } from 'react';
import { Header } from './components/Header';
import { Login } from './components/Login';
import { OpportunityList } from './components/OpportunityList';
import { TrackingPanel } from './components/TrackingPanel';
import { ArbitrageHistory } from './components/ArbitrageHistory';
import { HistoryExplorer } from './components/HistoryExplorer';
import { OrderPanel } from './components/OrderPanel';
import { OrderForm } from './components/OrderForm';
import { MetricsPanel } from './components/MetricsPanel';
import { PolyDebugOrder } from './components/PolyDebugOrder';
import { MentionSignalsPanel } from './components/MentionSignalsPanel';
import { useWebSocket } from './hooks/useWebSocket';
import { MatchedMarketData, OrderbookDepthResponse } from './types';

function App() {
  // Authentication state
  const [isAuthenticated, setIsAuthenticated] = useState(false);
  const [currentUsername, setCurrentUsername] = useState<string | null>(null);

  // Check localStorage for a token on initialization.
  useEffect(() => {
    const token = localStorage.getItem('auth_token');
    const username = localStorage.getItem('username');
    if (token && username) {
      setIsAuthenticated(true);
      setCurrentUsername(username);
    }
  }, []);

  // Handle successful login.
  const handleLoginSuccess = (token: string, username: string) => {
    localStorage.setItem('auth_token', token);
    localStorage.setItem('username', username);
    setIsAuthenticated(true);
    setCurrentUsername(username);
  };

  // Log out.
  const handleLogout = () => {
    localStorage.removeItem('auth_token');
    localStorage.removeItem('username');
    setIsAuthenticated(false);
    setCurrentUsername(null);
  };

  const wsUrl = useMemo(() => {
    const devPorts = ['5175', '5176', '5177', '5173'];
    const isDev = devPorts.includes(window.location.port);
    return isDev 
      ? 'ws://localhost:8000/ws'
      : `${window.location.protocol === 'https:' ? 'wss:' : 'ws:'}//${window.location.host}/ws`;
  }, []);

  const apiBaseUrl = useMemo(() => {
    const devPorts = ['5175', '5176', '5177', '5173'];
    const isDev = devPorts.includes(window.location.port);
    return isDev 
      ? 'http://localhost:8000'
      : `${window.location.protocol}//${window.location.host}`;
  }, []);
  
  const { matchedMarkets, isConnected, stats, lastUpdateTime, updateCount, dataCoverage, metrics, mentionSignals, mentionStatus } = useWebSocket(wsUrl);
  // Calculate total profit from matched markets with arbitrage opportunities.
  const totalProfit = matchedMarkets
    .filter(m => m.has_opportunity)
    .reduce((sum, m) => sum + m.expected_profit, 0);
  const [selectedMarketKey, setSelectedMarketKey] = useState<string | null>(null);
  const selectedMarket = useMemo(() => {
    if (selectedMarketKey === null) {
      return null;
    }
    return matchedMarkets.find(
      (market) =>
        `${market.kalshi_market_id}_${market.polymarket_market_id}` === selectedMarketKey,
    ) ?? null;
  }, [matchedMarkets, selectedMarketKey]);
  const [rightPanelTab, setRightPanelTab] = useState<'detail' | 'tracking' | 'mention'>('detail');
  const [leftBottomTab, setLeftBottomTab] = useState<'positions' | 'history'>('positions');
  const [showHistoryExplorer, setShowHistoryExplorer] = useState(false);
  const [showPolyDebug, setShowPolyDebug] = useState(false);
  const [orderbookDepth, setOrderbookDepth] = useState<OrderbookDepthResponse | null>(null);

  // Fetch order book depth.
  const fetchOrderbookDepth = useCallback(async (market: MatchedMarketData) => {
    try {
      const params = new URLSearchParams();
      if (market.kalshi_market_id) {
        params.append('kalshi_ticker', market.kalshi_market_id);
      }
      const response = await fetch(`${apiBaseUrl}/api/orderbook/depth?${params}`);
      if (response.ok) {
        const data = await response.json();
        setOrderbookDepth(data);
      }
    } catch (error) {
      console.error('Failed to fetch order book depth:', error);
    }
  }, [apiBaseUrl]);

  // Fetch depth when the selected market changes.
  useEffect(() => {
    if (selectedMarket) {
      fetchOrderbookDepth(selectedMarket);
      // Refresh depth every three seconds.
      const interval = setInterval(() => fetchOrderbookDepth(selectedMarket), 3000);
      return () => clearInterval(interval);
    } else {
      setOrderbookDepth(null);
    }
  }, [selectedMarket, fetchOrderbookDepth]);

  // Show the login page when unauthenticated.
  if (!isAuthenticated) {
    return <Login onLoginSuccess={handleLoginSuccess} apiBaseUrl={apiBaseUrl} />;
  }

  return (
    <div className="h-screen flex flex-col overflow-hidden">
      <Header 
        isConnected={isConnected} 
        stats={stats} 
        totalProfit={totalProfit}
        lastUpdateTime={lastUpdateTime}
        updateCount={updateCount}
        dataCoverage={dataCoverage}
        metrics={metrics}
        apiBaseUrl={apiBaseUrl}
        onLogout={handleLogout}
        username={currentUsername}
      />
      
      <main className="flex-1 flex overflow-hidden gap-1 p-1 bg-[--bg-primary]">
        {/* Left: market list and positions/history */}
        <div className="flex-1 flex flex-col overflow-hidden gap-1">
          {/* Top: market list (55%) */}
          <div className="flex-1 min-h-0 overflow-hidden flex flex-col bg-[--bg-secondary] rounded border border-[--border-color] shadow-lg" style={{flexBasis: '55%'}}>
            <OpportunityList 
              matchedMarkets={matchedMarkets}
              onSelectMarket={(market) =>
                setSelectedMarketKey(
                  `${market.kalshi_market_id}_${market.polymarket_market_id}`,
                )
              }
              apiBaseUrl={apiBaseUrl}
            />
          </div>
          {/* Bottom: positions/history tabs (45%) */}
          <div className="flex-1 min-h-0 overflow-hidden flex flex-col bg-[--bg-secondary] rounded border border-[--border-color] shadow-lg" style={{flexBasis: '45%'}}>
            {/* Tab switcher */}
            <div className="flex border-b-2 border-[--border-color] flex-shrink-0 bg-[--bg-tertiary]">
              <button
                className={`flex-1 px-3 py-1.5 text-xs font-medium transition-colors border-b-2 ${
                  leftBottomTab === 'positions'
                    ? 'text-[--accent-blue] border-[--accent-blue] bg-[--bg-secondary]'
                    : 'text-[--text-muted] hover:text-[--text-secondary] border-transparent'
                }`}
                onClick={() => setLeftBottomTab('positions')}
              >
                💼 Positions
              </button>
              <button
                className={`flex-1 px-3 py-1.5 text-xs font-medium transition-colors border-b-2 ${
                  leftBottomTab === 'history'
                    ? 'text-[--accent-yellow] border-[--accent-yellow] bg-[--bg-secondary]'
                    : 'text-[--text-muted] hover:text-[--text-secondary] border-transparent'
                }`}
                onClick={() => setLeftBottomTab('history')}
              >
                📜 Arbitrage History
              </button>
            </div>
            
            {/* Tab content */}
            <div className="flex-1 overflow-hidden">
              {leftBottomTab === 'positions' ? (
                <OrderPanel apiBaseUrl={apiBaseUrl} />
              ) : (
                <ArbitrageHistory apiBaseUrl={apiBaseUrl} onOpenExplorer={() => setShowHistoryExplorer(true)} />
              )}
            </div>
          </div>
        </div>
        
        {/* Right: details/tracking and performance */}
        <aside className="w-96 flex-shrink-0 flex flex-col gap-1">
          {/* Top: tab area (70%) */}
          <div className="flex-1 min-h-0 flex flex-col bg-[--bg-secondary] rounded border border-[--border-color] shadow-lg" style={{flexBasis: '70%'}}>
            {/* Tab switcher */}
            <div className="flex border-b-2 border-[--border-color] flex-shrink-0 bg-[--bg-tertiary]">
              <button
                className={`flex-1 px-3 py-1.5 text-xs font-medium transition-colors border-b-2 ${
                  rightPanelTab === 'detail'
                    ? 'text-[--accent-purple] border-[--accent-purple] bg-[--bg-secondary]'
                    : 'text-[--text-muted] hover:text-[--text-secondary] border-transparent'
                }`}
                onClick={() => setRightPanelTab('detail')}
              >
                📋 Details
              </button>
              <button
                className={`flex-1 px-3 py-1.5 text-xs font-medium transition-colors border-b-2 ${
                  rightPanelTab === 'tracking'
                    ? 'text-[--accent-green] border-[--accent-green] bg-[--bg-secondary]'
                    : 'text-[--text-muted] hover:text-[--text-secondary] border-transparent'
                }`}
                onClick={() => setRightPanelTab('tracking')}
              >
                🎯 Tracking
              </button>
              <button
                className={`flex-1 px-2 py-1.5 text-[10px] font-medium transition-colors border-b-2 ${
                  rightPanelTab === 'mention'
                    ? 'text-amber-300 border-amber-300 bg-[--bg-secondary]'
                    : 'text-[--text-muted] hover:text-[--text-secondary] border-transparent'
                }`}
                onClick={() => setRightPanelTab('mention')}
              >
                Mention
              </button>
            </div>

            {/* Tab content */}
            <div className="flex-1 overflow-y-auto overflow-x-hidden">
              {rightPanelTab === 'detail' ? (
                selectedMarket ? (
                  <div className="p-2 space-y-2">
                      {/* Header */}
                      <div className="pb-1.5 border-b border-[--border-color]">
                        <h3 className="text-[10px] font-semibold text-[--text-muted] uppercase tracking-wider">Market Details</h3>
                      </div>
                      
                      {/* Event information */}
                      <div className="bg-[--bg-tertiary] rounded p-2">
                        <div className="text-[10px] text-[--text-muted] mb-1">Event</div>
                        <div className="text-xs text-[--text-primary] font-medium leading-tight">
                          {selectedMarket.event_name}
                        </div>
                        {selectedMarket.team_name && (
                          <div className="text-xs text-[--accent-yellow] mt-1 font-medium">
                            {selectedMarket.team_name}
                          </div>
                        )}
                        {selectedMarket.end_time && (
                          <div className="text-[10px] text-[--text-muted] mt-1.5 flex items-center gap-1">
                            <span>🕐</span>
                            <span>{formatDateTime(selectedMarket.end_time)}</span>
                          </div>
                        )}
                      </div>

                      {/* Data status and profit information */}
                      <div className="grid grid-cols-2 gap-1.5">
                        {/* Data status */}
                        <div className="bg-[--bg-tertiary] rounded p-1.5">
                          <div className="text-[10px] text-[--text-muted] mb-1.5">Data Status</div>
                          <div className="flex flex-col gap-1">
                            <span className={`px-1.5 py-0.5 rounded text-[10px] ${selectedMarket.kalshi_ready ? 'bg-blue-500/20 text-blue-400' : 'bg-gray-500/20 text-gray-400'}`}>
                              Kalshi {selectedMarket.kalshi_ready ? '✓' : '○'}
                            </span>
                            <span className={`px-1.5 py-0.5 rounded text-[10px] ${selectedMarket.poly_ready ? 'bg-purple-500/20 text-purple-400' : 'bg-gray-500/20 text-gray-400'}`}>
                              Poly {selectedMarket.poly_ready ? '✓' : '○'}
                            </span>
                          </div>
                        </div>

                        {/* Profit information */}
                        {selectedMarket.has_opportunity ? (
                          <div className="bg-gradient-to-br from-green-500/10 to-emerald-500/5 rounded p-1.5 border border-green-500/20">
                            <div className="text-[10px] text-[--text-muted] mb-1">Arbitrage Opportunity</div>
                            <div className="text-base font-bold text-[--accent-green] leading-tight">
                              {selectedMarket.profit_margin.toFixed(2)}%
                            </div>
                            <div className="text-[10px] text-green-400 mt-0.5">
                              Net Profit: ${selectedMarket.expected_profit.toFixed(2)}
                            </div>
                          </div>
                        ) : (
                          <div className="bg-[--bg-tertiary] rounded p-1.5 border border-gray-500/20">
                            <div className="text-[10px] text-[--text-muted] mb-1">Arbitrage Result</div>
                            <div className="text-base font-bold text-gray-400 leading-tight">
                              {selectedMarket.profit_margin.toFixed(2)}%
                            </div>
                            <div className="text-[10px] text-gray-400 mt-0.5">
                              Net Profit: ${selectedMarket.expected_profit.toFixed(2)}
                            </div>
                          </div>
                        )}
                      </div>

                      {/* Prices and depth side by side */}
                      <div className="grid grid-cols-2 gap-2">
                        {/* Kalshi */}
                        <div className="bg-[--bg-tertiary] rounded p-2">
                          <div className="text-[10px] text-blue-400 font-medium mb-1.5">Kalshi</div>
                          <div className="space-y-1">
                            {/* Yes */}
                            <div className="space-y-0.5">
                              <div className="flex items-center justify-between">
                                <span className="text-[9px] text-green-400 font-medium">Yes</span>
                                <span className="text-xs text-[--text-primary] font-mono font-semibold">
                                  {(selectedMarket.kalshi_yes_price * 100).toFixed(0)}¢
                                </span>
                              </div>
                              <div className="flex items-center justify-between text-[9px]">
                                <span className="text-cyan-400 font-mono">
                                  ×{orderbookDepth?.kalshi?.yes?.size?.toFixed(0) ?? '-'}
                                </span>
                                <span className="text-yellow-400 font-mono">
                                  ${orderbookDepth?.kalshi?.yes?.price != null && orderbookDepth?.kalshi?.yes?.size != null 
                                    ? (orderbookDepth.kalshi.yes.price * orderbookDepth.kalshi.yes.size).toFixed(0) 
                                    : '-'}
                                </span>
                              </div>
                            </div>
                            {/* No */}
                            <div className="space-y-0.5 pt-1 border-t border-[--border-color]">
                              <div className="flex items-center justify-between">
                                <span className="text-[9px] text-red-400 font-medium">No</span>
                                <span className="text-xs text-[--text-primary] font-mono font-semibold">
                                  {(selectedMarket.kalshi_no_price * 100).toFixed(0)}¢
                                </span>
                              </div>
                              <div className="flex items-center justify-between text-[9px]">
                                <span className="text-cyan-400 font-mono">
                                  ×{orderbookDepth?.kalshi?.no?.size?.toFixed(0) ?? '-'}
                                </span>
                                <span className="text-yellow-400 font-mono">
                                  ${orderbookDepth?.kalshi?.no?.price != null && orderbookDepth?.kalshi?.no?.size != null 
                                    ? (orderbookDepth.kalshi.no.price * orderbookDepth.kalshi.no.size).toFixed(0) 
                                    : '-'}
                                </span>
                              </div>
                            </div>
                          </div>
                        </div>

                        {/* Polymarket */}
                        <div className="bg-[--bg-tertiary] rounded p-2">
                          <div className="text-[10px] text-purple-400 font-medium mb-1.5">Polymarket</div>
                          <div className="space-y-1">
                            {/* Yes */}
                            <div className="space-y-0.5">
                              <div className="flex items-center justify-between">
                                <span className="text-[9px] text-green-400 font-medium">Yes</span>
                                <span className="text-xs text-[--text-primary] font-mono font-semibold">
                                  {(selectedMarket.poly_yes_price * 100).toFixed(0)}¢
                                </span>
                              </div>
                              <div className="flex items-center justify-between text-[9px]">
                                <span className="text-cyan-400 font-mono">
                                  ×{orderbookDepth?.polymarket?.yes?.size?.toFixed(0) ?? '-'}
                                </span>
                                <span className="text-yellow-400 font-mono">
                                  ${orderbookDepth?.polymarket?.yes?.price != null && orderbookDepth?.polymarket?.yes?.size != null 
                                    ? (orderbookDepth.polymarket.yes.price * orderbookDepth.polymarket.yes.size).toFixed(0) 
                                    : '-'}
                                </span>
                              </div>
                            </div>
                            {/* No */}
                            <div className="space-y-0.5 pt-1 border-t border-[--border-color]">
                              <div className="flex items-center justify-between">
                                <span className="text-[9px] text-red-400 font-medium">No</span>
                                <span className="text-xs text-[--text-primary] font-mono font-semibold">
                                  {(selectedMarket.poly_no_price * 100).toFixed(0)}¢
                                </span>
                              </div>
                              <div className="flex items-center justify-between text-[9px]">
                                <span className="text-cyan-400 font-mono">
                                  ×{orderbookDepth?.polymarket?.no?.size?.toFixed(0) ?? '-'}
                                </span>
                                <span className="text-yellow-400 font-mono">
                                  ${orderbookDepth?.polymarket?.no?.price != null && orderbookDepth?.polymarket?.no?.size != null 
                                    ? (orderbookDepth.polymarket.no.price * orderbookDepth.polymarket.no.size).toFixed(0) 
                                    : '-'}
                                </span>
                              </div>
                            </div>
                          </div>
                        </div>
                      </div>

                      {/* Arbitrage calculation details */}
                      <div className="bg-[--bg-tertiary] rounded p-2 space-y-1.5">
                        {selectedMarket.arbitrage_type && (
                          <div>
                            <div className="text-[10px] text-[--text-muted] mb-1">Arbitrage Strategy</div>
                            <div className={`text-xs font-mono ${selectedMarket.has_opportunity ? 'text-[--text-primary]' : 'text-gray-400'}`}>
                              {selectedMarket.arbitrage_type}
                            </div>
                          </div>
                        )}
                        
                        {/* Fee breakdown */}
                        <div className={selectedMarket.arbitrage_type ? 'pt-2 border-t border-[--border-color]' : ''}>
                          <div className="text-[10px] text-[--text-muted] mb-1.5">Arbitrage Calculation Details</div>
                          <div className="space-y-1 text-[10px]">
                            {selectedMarket.kalshi_contracts !== undefined && (
                              <div className="flex justify-between">
                                <span className="text-[--text-muted]">Kalshi Contracts</span>
                                <span className="text-blue-400 font-mono">{Math.round(selectedMarket.kalshi_contracts)}</span>
                              </div>
                            )}
                            {selectedMarket.kalshi_fee !== undefined && (
                              <div className="flex justify-between">
                                <span className="text-[--text-muted]">Kalshi Fee</span>
                                <span className="text-orange-400 font-mono">-${selectedMarket.kalshi_fee.toFixed(2)}</span>
                              </div>
                            )}
                            {selectedMarket.gross_profit !== undefined && (
                              <div className="flex justify-between">
                                <span className="text-[--text-muted]">Gross Profit</span>
                                <span className={`font-mono ${selectedMarket.gross_profit >= 0 ? 'text-[--text-secondary]' : 'text-red-400'}`}>
                                  ${selectedMarket.gross_profit.toFixed(2)}
                                </span>
                              </div>
                            )}
                            <div className="flex justify-between pt-1 border-t border-[--border-color]">
                              <span className="text-[--text-muted] font-medium">Net Profit</span>
                              <span className={`font-mono font-medium ${selectedMarket.expected_profit >= 0 ? (selectedMarket.has_opportunity ? 'text-green-400' : 'text-gray-400') : 'text-red-400'}`}>
                                ${selectedMarket.expected_profit.toFixed(2)}
                              </span>
                            </div>
                          </div>
                        </div>
                      </div>

                      {/* Order area */}
                      <div className="pt-1.5 border-t border-[--border-color]">
                        <div className="pb-1.5">
                          <h3 className="text-[10px] font-semibold text-[--text-muted] uppercase tracking-wider">Place Order</h3>
                        </div>
                        <OrderForm 
                          market={selectedMarket} 
                          apiBaseUrl={apiBaseUrl}
                        />
                      </div>
                  </div>
                ) : (
                  <div className="flex flex-col items-center justify-center h-full">
                    <div className="text-3xl mb-2">👆</div>
                    <div className="text-xs text-[--text-secondary]">Select a market</div>
                    <div className="text-[10px] text-[--text-muted] mt-1">View details and place trades</div>
                  </div>
                )
              ) : rightPanelTab === 'tracking' ? (
                <TrackingPanel apiBaseUrl={apiBaseUrl} />
              ) : (
                <MentionSignalsPanel apiBaseUrl={apiBaseUrl} signals={mentionSignals} status={mentionStatus} />
              )}
            </div>
          </div>
          
          {/* Bottom: performance monitoring (30%) */}
          <div className="flex-1 min-h-0 overflow-hidden flex flex-col bg-[--bg-secondary] rounded border border-[--border-color] shadow-lg" style={{flexBasis: '30%'}}>
            <MetricsPanel metrics={metrics} />
          </div>
        </aside>
      </main>

      {/* History explorer modal */}
      {showHistoryExplorer && (
        <HistoryExplorer 
          apiBaseUrl={apiBaseUrl} 
          onClose={() => setShowHistoryExplorer(false)} 
        />
      )}

      {/* Polymarket debug order modal */}
      {showPolyDebug && (
        <PolyDebugOrder
          apiBaseUrl={apiBaseUrl}
          onClose={() => setShowPolyDebug(false)}
        />
      )}

      {/* Debug button fixed to the bottom right */}
      <button
        onClick={() => setShowPolyDebug(true)}
        className="fixed bottom-4 right-4 w-12 h-12 rounded-full bg-purple-500 hover:bg-purple-600 text-white shadow-lg flex items-center justify-center text-xl z-40"
        title="Polymarket Manual Order Debug"
      >
        🔧
      </button>
    </div>
  );
}

function formatDateTime(dateString: string): string {
  const date = new Date(dateString);
  const now = new Date();
  const diffMs = date.getTime() - now.getTime();
  const diffHours = Math.floor(diffMs / (1000 * 60 * 60));
  const diffMinutes = Math.floor((diffMs % (1000 * 60 * 60)) / (1000 * 60));
  
  // Format date and time.
  const dateStr = date.toLocaleDateString('en-US', {
    month: '2-digit', 
    day: '2-digit',
    weekday: 'short'
  });
  const timeStr = date.toLocaleTimeString('en-US', {
    hour: '2-digit', 
    minute: '2-digit',
    hour12: false
  });
  
  // Show a countdown for events within 24 hours.
  if (diffMs > 0 && diffHours < 24) {
    if (diffHours > 0) {
      return `${dateStr} ${timeStr} (in ${diffHours}h ${diffMinutes}m)`;
    } else if (diffMinutes > 0) {
      return `${dateStr} ${timeStr} (in ${diffMinutes}m)`;
    } else {
      return `${dateStr} ${timeStr} (starting soon)`;
    }
  }
  
  // Indicate events that have already started.
  if (diffMs < 0) {
    return `${dateStr} ${timeStr} (started)`;
  }
  
  return `${dateStr} ${timeStr}`;
}

export default App;
