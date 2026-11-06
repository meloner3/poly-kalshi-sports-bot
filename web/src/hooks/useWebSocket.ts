import { useEffect, useRef, useState } from 'react';
import { WsMessage, LogEntry, DataCoverage, MatchedMarketData, MetricsReport, MentionSignalRow, MentionStatus } from '../types';

export function useWebSocket(url: string) {
  const [matchedMarkets, setMatchedMarkets] = useState<MatchedMarketData[]>([]); // All matched markets, including arbitrage information
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [isConnected, setIsConnected] = useState(false);
  const [isReceivingData, setIsReceivingData] = useState(false); // Whether real-time data is being received
  const [lastUpdateTime, setLastUpdateTime] = useState<Date | null>(null); // Last update time
  const [updateCount, setUpdateCount] = useState(0); // Update count used to trigger animations
  const [stats, setStats] = useState({
    kalshiCount: 0,
    polymarketCount: 0,
    matchedCount: 0,
    opportunitiesCount: 0,
  });
  const [dataCoverage, setDataCoverage] = useState<DataCoverage>({
    total_markets: 0,
    kalshi_ready: 0,
    polymarket_ready: 0,
    both_ready: 0,
    kalshi_coverage: '0/0',
    polymarket_coverage: '0/0',
    full_coverage: '0/0',
    kalshi_connected: false,
    polymarket_connected: false,
    kalshi_source: 'rest_polling',
    polymarket_source: 'rest_polling',
  });
  const [metrics, setMetrics] = useState<MetricsReport | null>(null);
  const [mentionSignals, setMentionSignals] = useState<MentionSignalRow[]>([]);
  const [mentionStatus, setMentionStatus] = useState<MentionStatus | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  // Track price changes for highlight animations.
  const prevPricesRef = useRef<Map<string, { k_yes: number; k_no: number; p_yes: number; p_no: number }>>(new Map());

  useEffect(() => {
    let disposed = false;
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined;

    // Keep addLog inside the effect to avoid stale closures.
    const addLog = (level: LogEntry['level'], message: string) => {
      const entry: LogEntry = {
        time: new Date().toISOString(),
        level,
        message,
      };
      setLogs((prev) => {
        const newLogs = [entry, ...prev];
        return newLogs.slice(0, 100); // Keep the 100 most recent entries
      });
    };

    // Keep handleMessage inside the effect to avoid stale closures.
    const handleMessage = (message: WsMessage) => {
      switch (message.type) {
        case 'opportunity':
          // The Rust backend sent a single arbitrage opportunity.
          if (message.data) {
            const opp = message.data as any;
            addLog('success', `Arbitrage opportunity found: ${opp.event_name} ${opp.team_name} - ${opp.profit_margin.toFixed(2)}%`);
          }
          break;

        case 'opportunities':
          // The Rust backend sent an arbitrage opportunity list.
          if (message.data && Array.isArray(message.data)) {
            setIsReceivingData(true);
            const opportunities = message.data as any[];
            
            // Update statistics.
            setStats((prev) => ({
              ...prev,
              opportunitiesCount: opportunities.length,
            }));
            
            setLastUpdateTime(new Date());
            setUpdateCount((prev) => prev + 1);
          }
          break;

        case 'stats':
          // The Rust backend sent system statistics.
          if (message.data) {
            const statsData = message.data as any;
            setStats({
              kalshiCount: statsData.total_kalshi_markets || 0,
              polymarketCount: statsData.total_polymarket_markets || 0,
              matchedCount: statsData.matched_markets || 0,
              opportunitiesCount: statsData.arbitrage_opportunities || 0,
            });
          }
          break;

        case 'log':
          if (message.level && message.message) {
            addLog(message.level as any, message.message);
          }
          break;

        case 'matched_markets_list':
          // Support the legacy Python backend message format, if present.
          if (message.data && Array.isArray(message.data)) {
            setIsReceivingData(true);
            const marketsData = message.data as MatchedMarketData[];
            
            // Detect price changes for highlight animations.
            // Use a more unique key: kalshi_market_id + polymarket_market_id.
            const newPricesMap = new Map<string, { k_yes: number; k_no: number; p_yes: number; p_no: number; changed: boolean }>();
            marketsData.forEach((m) => {
              const key = `${m.kalshi_market_id}_${m.polymarket_market_id}`;
              const prev = prevPricesRef.current.get(key);
              const changed = prev ? (
                prev.k_yes !== m.kalshi_yes_price ||
                prev.k_no !== m.kalshi_no_price ||
                prev.p_yes !== m.poly_yes_price ||
                prev.p_no !== m.poly_no_price
              ) : false;
              newPricesMap.set(key, {
                k_yes: m.kalshi_yes_price,
                k_no: m.kalshi_no_price,
                p_yes: m.poly_yes_price,
                p_no: m.poly_no_price,
                changed
              });
            });
            prevPricesRef.current = new Map(
              Array.from(newPricesMap.entries()).map(([k, v]) => [k, { k_yes: v.k_yes, k_no: v.k_no, p_yes: v.p_yes, p_no: v.p_no }])
            );
            
            setMatchedMarkets(marketsData);
            setLastUpdateTime(new Date());
            setUpdateCount((prev) => prev + 1);
            
            // Update statistics.
            if (message.count !== undefined) {
              setStats((prev) => ({
                ...prev,
                matchedCount: message.count!,
                opportunitiesCount: message.opportunities_count || prev.opportunitiesCount,
              }));
            }
          }
          break;

        case 'metrics':
          // Process performance metrics messages.
          if (message.data && typeof message.data === 'object' && 'api_latency' in message.data) {
            setMetrics(message.data as MetricsReport);
          }
          break;

        case 'mention_signals':
          if (Array.isArray(message.data)) {
            setMentionSignals(message.data as MentionSignalRow[]);
            if (message.status) setMentionStatus(message.status);
          }
          break;

        default:
          break;
      }
    };

    const connect = () => {
      const ws = new WebSocket(url);
      wsRef.current = ws;

      ws.onopen = () => {
        console.log('WebSocket connection established');
        setIsConnected(true);
        addLog('success', 'WebSocket connected');
      };

      ws.onmessage = (event) => {
        try {
          const message: WsMessage = JSON.parse(event.data);
          console.log('WebSocket message received:', message.type, message);
          handleMessage(message);
        } catch (error) {
          console.error('Failed to parse WebSocket message:', error);
        }
      };

      ws.onerror = (error) => {
        console.error('WebSocket error:', error);
        addLog('error', 'WebSocket connection error');
      };

      ws.onclose = () => {
        if (disposed) {
          return;
        }
        console.log('WebSocket connection closed');
        setIsConnected(false);
        addLog('warning', 'WebSocket disconnected; reconnecting in 5 seconds...');
        
        // Reconnect after five seconds.
        reconnectTimer = setTimeout(() => {
          if (!disposed) {
            connect();
          }
        }, 5000);
      };
    };

    connect();

    return () => {
      disposed = true;
      if (reconnectTimer !== undefined) {
        clearTimeout(reconnectTimer);
      }
      if (wsRef.current) {
        wsRef.current.close();
        wsRef.current = null;
      }
    };
  }, [url]);

  // Fetch data coverage periodically.
  useEffect(() => {
    const fetchCoverage = async () => {
      try {
        // Infer the API URL from the WebSocket URL.
        const apiUrl = url.replace('ws://', 'http://').replace('wss://', 'https://').replace('/ws', '');
        const response = await fetch(`${apiUrl}/api/data-coverage`);
        if (response.ok) {
          const data = await response.json();
          setDataCoverage(data);
        }
      } catch (error) {
        // Fail silently.
      }
    };

    // Fetch initially.
    fetchCoverage();
    // Refresh every three seconds.
    const interval = setInterval(fetchCoverage, 3000);
    return () => clearInterval(interval);
  }, [url]);

  return {
    matchedMarkets,
    logs,
    isConnected,
    isReceivingData,
    lastUpdateTime,
    updateCount,
    stats,
    dataCoverage,
    metrics,
    mentionSignals,
    mentionStatus,
  };
}
