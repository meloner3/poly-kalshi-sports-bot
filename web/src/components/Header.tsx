import { useEffect, useState, useCallback } from 'react';
import { DataCoverage, AccountBalance, MetricsReport } from '../types';
import { 
  getAutoTradeStatus, 
  enableAutoTrade, 
  disableAutoTrade, 
  resetAutoTradeCount, 
  updateAutoTradeSettings,
  AutoTradeStatus,
  getAppSettings,
  updateAppSettings,
  AppSettings
} from '../utils/api';

interface HeaderProps {
  isConnected: boolean;
  stats: {
    kalshiCount: number;
    polymarketCount: number;
    matchedCount: number;
    opportunitiesCount: number;
  };
  totalProfit: number;
  lastUpdateTime: Date | null;
  updateCount: number;
  dataCoverage: DataCoverage;
  metrics: MetricsReport | null;
  apiBaseUrl: string;
  onLogout?: () => void;
  username?: string | null;
}

const AUTO_REFRESH_STORAGE_KEY = 'polytaoli_auto_refresh';
const MIN_AUTO_REFRESH_SECONDS = 5;

export function Header({ isConnected, stats, totalProfit, lastUpdateTime: _lastUpdateTime, updateCount, dataCoverage, metrics, apiBaseUrl, onLogout, username }: HeaderProps) {
  const [isFlashing, setIsFlashing] = useState(false);
  const [accountBalance, setAccountBalance] = useState<AccountBalance | null>(null);

  // Auto-refresh: reload the whole page on a user-configurable interval.
  // Persisted to localStorage since a page reload wipes React state.
  const [refreshSecondsInput, setRefreshSecondsInput] = useState('30');
  const [autoRefreshEnabled, setAutoRefreshEnabled] = useState(false);
  const [refreshCountdown, setRefreshCountdown] = useState<number | null>(null);

  // Load the saved auto-refresh preference once on mount.
  useEffect(() => {
    try {
      const saved = localStorage.getItem(AUTO_REFRESH_STORAGE_KEY);
      if (!saved) return;
      const parsed = JSON.parse(saved);
      if (typeof parsed.seconds === 'number' && parsed.seconds >= MIN_AUTO_REFRESH_SECONDS) {
        setRefreshSecondsInput(String(parsed.seconds));
      }
      if (parsed.enabled) {
        setAutoRefreshEnabled(true);
      }
    } catch {
      // Private browsing or blocked storage - auto-refresh just stays off.
    }
  }, []);

  // Persist the preference whenever it changes.
  useEffect(() => {
    try {
      const seconds = parseInt(refreshSecondsInput);
      localStorage.setItem(
        AUTO_REFRESH_STORAGE_KEY,
        JSON.stringify({
          enabled: autoRefreshEnabled,
          seconds: isNaN(seconds) ? 30 : seconds,
        })
      );
    } catch {
      // Ignore - not critical if the preference doesn't persist.
    }
  }, [autoRefreshEnabled, refreshSecondsInput]);

  // Drive the countdown and trigger the reload when it hits zero.
  useEffect(() => {
    if (!autoRefreshEnabled) {
      setRefreshCountdown(null);
      return;
    }
    const seconds = parseInt(refreshSecondsInput);
    if (isNaN(seconds) || seconds < MIN_AUTO_REFRESH_SECONDS) {
      return;
    }
    setRefreshCountdown(seconds);
    const tick = setInterval(() => {
      setRefreshCountdown((prev) => {
        if (prev === null) return null;
        if (prev <= 1) {
          window.location.reload();
          return 0;
        }
        return prev - 1;
      });
    }, 1000);
    return () => clearInterval(tick);
  }, [autoRefreshEnabled, refreshSecondsInput]);

  const handleManualRefresh = () => window.location.reload();

  const handleToggleAutoRefresh = () => {
    const seconds = parseInt(refreshSecondsInput);
    if (!autoRefreshEnabled && (isNaN(seconds) || seconds < MIN_AUTO_REFRESH_SECONDS)) {
      return;
    }
    setAutoRefreshEnabled((prev) => !prev);
  };
  
  // Automated trading state
  const [autoTradeStatus, setAutoTradeStatus] = useState<AutoTradeStatus | null>(null);
  const [showAutoTradeModal, setShowAutoTradeModal] = useState(false);
  const [autoTradeLoading, setAutoTradeLoading] = useState(false);
  const [durationInput, setDurationInput] = useState('500');
  const [maxTradeCountInput, setMaxTradeCountInput] = useState('2');
  const [maxContractsInput, setMaxContractsInput] = useState('100');
  const [minContractsInput, setMinContractsInput] = useState('10');
  const [neutralizationLossInput, setNeutralizationLossInput] = useState('5');
  const [autoTradeMessage, setAutoTradeMessage] = useState<{ text: string; type: 'success' | 'error' } | null>(null);

  // Application settings state
  const [appSettings, setAppSettings] = useState<AppSettings | null>(null);
  const [showSettingsModal, setShowSettingsModal] = useState(false);
  const [settingsLoading, setSettingsLoading] = useState(false);
  const [settingsMessage, setSettingsMessage] = useState<{ text: string; type: 'success' | 'error' } | null>(null);
  // Temporary values used for editing
  const [editRefreshInterval, setEditRefreshInterval] = useState('5');
  const [editMinProfit, setEditMinProfit] = useState('1.0');
  const [editDefaultBet, setEditDefaultBet] = useState('10');
  const [editTrackingThreshold, setEditTrackingThreshold] = useState('2.0');

  // Fetch automated trading status. This runs on a 2s poll for the live
  // read-only display (autoTradeStatus.*) - it must not also overwrite the
  // editable input fields below on every tick, or a poll landing mid-edit
  // silently reverts whatever the user is typing before they can submit it.
  // Only seed the inputs from the server on the very first load; after that,
  // an input only changes when the user edits it or a save round-trips.
  const fetchAutoTradeStatus = useCallback(async () => {
    try {
      const data = await getAutoTradeStatus(apiBaseUrl);
      setAutoTradeStatus((prev) => {
        if (prev === null) {
          setDurationInput(String(data.min_duration_ms));
          setMaxTradeCountInput(String(data.max_trade_count));
          setMaxContractsInput(String(data.max_contracts));
          setMinContractsInput(String(data.min_contracts));
          setNeutralizationLossInput(String(data.neutralization_max_loss_cents));
        }
        return data;
      });
    } catch (error) {
      console.error('Failed to fetch automated trading status:', error);
    }
  }, [apiBaseUrl]);

  // Fetch application settings.
  const fetchAppSettings = useCallback(async () => {
    try {
      const data = await getAppSettings(apiBaseUrl);
      setAppSettings(data);
      setEditRefreshInterval(String(data.refresh_interval));
      setEditMinProfit(String(data.min_profit_margin));
      setEditDefaultBet(String(data.default_bet_amount));
      setEditTrackingThreshold(String(data.tracking_threshold));
    } catch (error) {
      console.error('Failed to fetch application settings:', error);
    }
  }, [apiBaseUrl]);

  // Refresh automated trading status periodically.
  useEffect(() => {
    fetchAutoTradeStatus();
    const interval = setInterval(fetchAutoTradeStatus, 2000);
    return () => clearInterval(interval);
  }, [fetchAutoTradeStatus]);

  // Load application settings initially.
  useEffect(() => {
    fetchAppSettings();
  }, [fetchAppSettings]);

  // Show an automated trading message.
  const showAutoTradeMsg = (text: string, type: 'success' | 'error') => {
    setAutoTradeMessage({ text, type });
    setTimeout(() => setAutoTradeMessage(null), 3000);
  };

  // Show an application settings message.
  const showSettingsMsg = (text: string, type: 'success' | 'error') => {
    setSettingsMessage({ text, type });
    setTimeout(() => setSettingsMessage(null), 3000);
  };

  // Update application settings.
  const handleUpdateSettings = async () => {
    const refreshInterval = parseInt(editRefreshInterval);
    const minProfit = parseFloat(editMinProfit);
    const defaultBet = parseFloat(editDefaultBet);
    const trackingThreshold = parseFloat(editTrackingThreshold);

    if (isNaN(refreshInterval) || refreshInterval < 5) {
      showSettingsMsg('Refresh interval must be at least 5 seconds', 'error');
      return;
    }
    if (isNaN(minProfit) || minProfit < 0) {
      showSettingsMsg('Minimum profit margin cannot be negative', 'error');
      return;
    }
    if (isNaN(defaultBet) || defaultBet <= 0) {
      showSettingsMsg('Default amount must be greater than 0', 'error');
      return;
    }
    if (isNaN(trackingThreshold) || trackingThreshold < 0) {
      showSettingsMsg('Tracking threshold cannot be negative', 'error');
      return;
    }

    setSettingsLoading(true);
    try {
      const result = await updateAppSettings(apiBaseUrl, {
        refresh_interval: refreshInterval,
        min_profit_margin: minProfit,
        default_bet_amount: defaultBet,
        tracking_threshold: trackingThreshold,
      });
      if (result.success) {
        showSettingsMsg('Settings saved (fully applied after restart)', 'success');
        await fetchAppSettings();
      } else {
        showSettingsMsg(result.error || 'Failed to save settings', 'error');
      }
    } catch (error) {
      showSettingsMsg('Failed to save settings', 'error');
    }
    setSettingsLoading(false);
  };

  // Toggle automated trading.
  const handleAutoTradeToggle = async () => {
    if (!autoTradeStatus) return;
    setAutoTradeLoading(true);
    try {
      if (autoTradeStatus.enabled) {
        const result = await disableAutoTrade(apiBaseUrl);
        if (result.success) showAutoTradeMsg('Automated trading disabled', 'success');
        else showAutoTradeMsg(result.error || 'Operation failed', 'error');
      } else {
        const result = await enableAutoTrade(apiBaseUrl);
        if (result.success) showAutoTradeMsg('Automated trading enabled', 'success');
        else showAutoTradeMsg(result.error || 'Operation failed', 'error');
      }
      await fetchAutoTradeStatus();
    } catch (error) {
      showAutoTradeMsg('Operation failed', 'error');
    }
    setAutoTradeLoading(false);
  };

  // Reset the trade count.
  const handleAutoTradeReset = async () => {
    setAutoTradeLoading(true);
    try {
      const result = await resetAutoTradeCount(apiBaseUrl);
      if (result.success) showAutoTradeMsg('Trade count reset', 'success');
      else showAutoTradeMsg(result.error || 'Failed to reset trade count', 'error');
      await fetchAutoTradeStatus();
    } catch (error) {
      showAutoTradeMsg('Failed to reset trade count', 'error');
    }
    setAutoTradeLoading(false);
  };

  // Update the duration threshold.
  const handleUpdateDuration = async () => {
    const duration = parseInt(durationInput);
    if (isNaN(duration) || duration < 0) {
      showAutoTradeMsg('Enter a valid duration', 'error');
      return;
    }
    setAutoTradeLoading(true);
    try {
      const result = await updateAutoTradeSettings(apiBaseUrl, { min_duration_ms: duration });
      if (result.success) showAutoTradeMsg('Settings updated', 'success');
      else showAutoTradeMsg(result.error || 'Failed to update settings', 'error');
      await fetchAutoTradeStatus();
    } catch (error) {
      showAutoTradeMsg('Failed to update settings', 'error');
    }
    setAutoTradeLoading(false);
  };

  // Update the maximum number of trades.
  const handleUpdateMaxTradeCount = async () => {
    const count = parseInt(maxTradeCountInput);
    if (isNaN(count) || count < 1) {
      showAutoTradeMsg('Enter a valid trade count (≥1)', 'error');
      return;
    }
    setAutoTradeLoading(true);
    try {
      const result = await updateAutoTradeSettings(apiBaseUrl, { max_trade_count: count });
      if (result.success) showAutoTradeMsg('Maximum trade count updated', 'success');
      else showAutoTradeMsg(result.error || 'Failed to update settings', 'error');
      await fetchAutoTradeStatus();
    } catch (error) {
      showAutoTradeMsg('Failed to update settings', 'error');
    }
    setAutoTradeLoading(false);
  };

  // Toggle flexible order sizing.
  const handleToggleFlexibleMode = async () => {
    if (!autoTradeStatus) return;
    setAutoTradeLoading(true);
    try {
      const result = await updateAutoTradeSettings(apiBaseUrl, { 
        flexible_mode: !autoTradeStatus.flexible_mode 
      });
      if (result.success) {
        showAutoTradeMsg(
          autoTradeStatus.flexible_mode ? 'Switched to fixed mode' : 'Switched to flexible mode',
          'success'
        );
      } else {
        showAutoTradeMsg(result.error || 'Failed to switch mode', 'error');
      }
      await fetchAutoTradeStatus();
    } catch (error) {
      showAutoTradeMsg('Failed to switch mode', 'error');
    }
    setAutoTradeLoading(false);
  };

  // Update maximum contracts.
  const handleUpdateMaxContracts = async () => {
    const count = parseInt(maxContractsInput);
    if (isNaN(count) || count < 1) {
      showAutoTradeMsg('Enter a valid contract count (≥1)', 'error');
      return;
    }
    setAutoTradeLoading(true);
    try {
      const result = await updateAutoTradeSettings(apiBaseUrl, { max_contracts: count });
      if (result.success) showAutoTradeMsg('Maximum contracts updated', 'success');
      else showAutoTradeMsg(result.error || 'Failed to update settings', 'error');
      await fetchAutoTradeStatus();
    } catch (error) {
      showAutoTradeMsg('Failed to update settings', 'error');
    }
    setAutoTradeLoading(false);
  };

  // Update minimum contracts.
  const handleUpdateMinContracts = async () => {
    const count = parseInt(minContractsInput);
    if (isNaN(count) || count < 1) {
      showAutoTradeMsg('Enter a valid contract count (≥1)', 'error');
      return;
    }
    setAutoTradeLoading(true);
    try {
      const result = await updateAutoTradeSettings(apiBaseUrl, { min_contracts: count });
      if (result.success) showAutoTradeMsg('Minimum contracts updated', 'success');
      else showAutoTradeMsg(result.error || 'Failed to update settings', 'error');
      await fetchAutoTradeStatus();
    } catch (error) {
      showAutoTradeMsg('Failed to update settings', 'error');
    }
    setAutoTradeLoading(false);
  };

  const handleUpdateNeutralizationLoss = async () => {
    const cents = parseInt(neutralizationLossInput);
    if (isNaN(cents) || cents < 0 || cents > 99) {
      showAutoTradeMsg('Enter a close-loss bound from 0 to 99 cents', 'error');
      return;
    }
    setAutoTradeLoading(true);
    try {
      const result = await updateAutoTradeSettings(apiBaseUrl, {
        neutralization_max_loss_cents: cents,
      });
      if (result.success) showAutoTradeMsg('Close-loss bound updated', 'success');
      else showAutoTradeMsg(result.error || 'Failed to update close-loss bound', 'error');
      await fetchAutoTradeStatus();
    } catch (error) {
      showAutoTradeMsg('Failed to update close-loss bound', 'error');
    }
    setAutoTradeLoading(false);
  };
  
  // Flash when new data arrives.
  useEffect(() => {
    if (updateCount > 0) {
      setIsFlashing(true);
      const timer = setTimeout(() => setIsFlashing(false), 300);
      return () => clearTimeout(timer);
    }
  }, [updateCount]);

  // Fetch account balances periodically.
  useEffect(() => {
    const fetchBalance = async () => {
      try {
        const response = await fetch(`${apiBaseUrl}/api/account-balance`);
        if (response.ok) {
          const data = await response.json();
          setAccountBalance(data);
        }
      } catch (error) {
        // Fail silently.
      }
    };

    // Fetch initially.
    fetchBalance();
    // Account endpoints are rate-limited; refresh at a sustainable cadence.
    const interval = setInterval(fetchBalance, 60000);
    return () => clearInterval(interval);
  }, [apiBaseUrl]);

  // Refresh displayed time every second.
  const [, setTick] = useState(0);
  useEffect(() => {
    const timer = setInterval(() => setTick(t => t + 1), 1000);
    return () => clearInterval(timer);
  }, []);

  // Calculate coverage percentage.
  const getCoveragePercent = () => {
    if (dataCoverage.total_markets === 0) return 0;
    return Math.round((dataCoverage.both_ready / dataCoverage.total_markets) * 100);
  };

  const coveragePercent = getCoveragePercent();

  // Format displayed latency.
  const formatLatency = (latencyMs?: number) => {
    if (latencyMs === undefined || latencyMs === null) return '--';
    if (latencyMs < 1000) return `${latencyMs}ms`;
    return `${(latencyMs / 1000).toFixed(1)}s`;
  };

  // Get latency color.
  const getLatencyColor = (latencyMs?: number) => {
    if (latencyMs === undefined || latencyMs === null) return 'text-gray-500';
    if (latencyMs < 100) return 'text-green-400';
    if (latencyMs < 500) return 'text-yellow-400';
    if (latencyMs < 2000) return 'text-orange-400';
    return 'text-red-400';
  };

  // Format displayed balance.
  const formatBalance = (balance?: number) => {
    if (balance === undefined || balance === null) return '--';
    return balance >= 1000 ? `${(balance / 1000).toFixed(1)}k` : balance.toFixed(0);
  };

  return (
    <header className="border-b border-[--border-color] bg-[--bg-secondary]">
      <div className="px-2 h-10 flex items-center justify-between gap-2 overflow-x-auto">
        {/* Compact logo */}
        <div className="flex items-center gap-1 flex-shrink-0">
          <span className="text-sm">📊</span>
          <span className="font-semibold text-[--text-primary] text-xs">Arbitrage Scanner</span>
        </div>

        {/* Compact statistics */}
        <div className="flex items-center gap-2 text-xs flex-shrink-0">
          {/* Account balances */}
          {accountBalance && (
            <div className="flex items-center gap-1 px-2 py-0.5 rounded bg-[--bg-tertiary]" title={`Account Balances\nKalshi: ${accountBalance.kalshi.available ? `$${formatBalance(accountBalance.kalshi.balance)}` : 'Not configured or unavailable'}\nPolymarket: ${accountBalance.polymarket.available ? `$${formatBalance(accountBalance.polymarket.balance)}` : 'Not configured or unavailable'}`}>
              <span className="text-[10px] text-[--text-muted]">💰</span>
              <span className={`font-mono text-[10px] ${accountBalance.kalshi.available ? 'text-blue-400' : 'text-gray-500'}`} title={accountBalance.kalshi.error || ''}>
                K:${accountBalance.kalshi.available ? formatBalance(accountBalance.kalshi.balance) : '--'}
              </span>
              <span className={`font-mono text-[10px] ${accountBalance.polymarket.available ? 'text-purple-400' : 'text-gray-500'}`} title={accountBalance.polymarket.error || 'Polymarket balance'}>
                P:${accountBalance.polymarket.available ? formatBalance(accountBalance.polymarket.balance) : '--'}
              </span>
            </div>
          )}

          {/* Data coverage */}
          <div className="flex items-center gap-1 px-2 py-0.5 rounded bg-[--bg-tertiary]" title="Data coverage: authoritative Kalshi and Polymarket REST quote availability">
            <span className={`font-mono text-[10px] ${dataCoverage.kalshi_connected ? 'text-blue-400' : 'text-gray-500'}`}>
              K:REST {dataCoverage.kalshi_coverage}
            </span>
            <span className={`font-mono text-[10px] ${dataCoverage.polymarket_connected ? 'text-purple-400' : 'text-gray-500'}`}>
              P:REST {dataCoverage.polymarket_coverage}
            </span>
            <span className={`font-mono text-[10px] font-semibold ${coveragePercent >= 80 ? 'text-green-400' : coveragePercent >= 50 ? 'text-yellow-400' : 'text-red-400'}`}>
              ✓{dataCoverage.both_ready}/{dataCoverage.total_markets}
            </span>
          </div>

          {/* Platform latency */}
          <div className="flex items-center gap-1 px-2 py-0.5 rounded bg-[--bg-tertiary]" title="Platform data freshness">
            <span className="text-[10px] text-[--text-muted]">Feed:</span>
            <span className={`font-mono text-[10px] ${getLatencyColor(dataCoverage.kalshi_latency_ms)}`}>
              K:REST {formatLatency(dataCoverage.kalshi_latency_ms)}
            </span>
            <span className={`font-mono text-[10px] ${getLatencyColor(dataCoverage.polymarket_latency_ms)}`}>
              P:REST {formatLatency(dataCoverage.polymarket_latency_ms)}
            </span>
          </div>

          {/* API latency */}
          <div className="flex items-center gap-1 px-2 py-0.5 rounded bg-[--bg-tertiary]" title="API latency">
            <span className="text-[10px] text-[--text-muted]">API:</span>
            <span className={`font-mono text-[10px] ${getLatencyColor(metrics?.api_latency.kalshi_ms)}`}>
              K:{formatLatency(metrics?.api_latency.kalshi_ms)}
            </span>
            <span className={`font-mono text-[10px] ${getLatencyColor(metrics?.api_latency.polymarket_ms)}`}>
              P:{formatLatency(metrics?.api_latency.polymarket_ms)}
            </span>
          </div>

          <div className="h-3 w-px bg-[--border-color]" />
          
          <StatItem 
            label="Opps" 
            value={stats.opportunitiesCount} 
            color="text-green-400" 
            highlight={isFlashing}
          />
          <StatItem label="Profit" value={`$${totalProfit.toFixed(0)}`} color="text-emerald-400" />
          
          <div className="h-3 w-px bg-[--border-color]" />
          
          {/* Connection status */}
          <div className="flex items-center gap-1">
            <span className={`status-dot ${isConnected ? 'status-connected animate-pulse-dot' : 'status-disconnected'}`} />
            <span className={`text-[10px] ${isConnected ? 'text-green-400' : 'text-red-400'}`}>
              {isConnected ? '● Live' : 'Offline'}
            </span>
          </div>

          <div className="h-3 w-px bg-[--border-color]" />

          {/* Page refresh controls */}
          <div className="flex items-center gap-1">
            <button
              onClick={handleManualRefresh}
              className="px-1.5 py-0.5 rounded text-[10px] font-semibold bg-gray-500/20 text-gray-300 hover:bg-gray-500/30 transition-colors"
              title="Refresh the page now"
            >
              🔄
            </button>
            <input
              type="number"
              value={refreshSecondsInput}
              onChange={(e) => setRefreshSecondsInput(e.target.value)}
              min={MIN_AUTO_REFRESH_SECONDS}
              className="w-12 text-[10px] px-1 py-0.5 rounded bg-[--bg-tertiary] border border-[--border-color] text-[--text-primary] text-center focus:outline-none focus:border-blue-500"
              title={`Auto-refresh interval in seconds (min ${MIN_AUTO_REFRESH_SECONDS})`}
            />
            <button
              onClick={handleToggleAutoRefresh}
              className={`px-1.5 py-0.5 rounded text-[10px] font-semibold font-mono transition-colors ${
                autoRefreshEnabled
                  ? 'bg-green-500/20 text-green-400 hover:bg-green-500/30'
                  : 'bg-gray-500/20 text-gray-400 hover:bg-gray-500/30'
              }`}
              title={autoRefreshEnabled ? 'Auto-refresh is on — click to stop' : 'Click to reload the page automatically on this interval'}
            >
              {autoRefreshEnabled ? `⏱ ${refreshCountdown ?? refreshSecondsInput}s` : 'Auto: Off'}
            </button>
          </div>

          <div className="h-3 w-px bg-[--border-color]" />

          {/* Automated trading button */}
          {autoTradeStatus && (
            <button
              onClick={() => setShowAutoTradeModal(true)}
              className={`flex items-center gap-1 px-2 py-0.5 rounded text-[10px] font-semibold transition-colors ${
                autoTradeStatus.enabled
                  ? 'bg-green-500/20 text-green-400 hover:bg-green-500/30'
                  : 'bg-gray-500/20 text-gray-400 hover:bg-gray-500/30'
              }`}
              title="Automated Trading Settings"
            >
              <span>🤖</span>
              <span>{autoTradeStatus.enabled ? 'Auto' : 'Manual'}</span>
              <span className="font-mono">{autoTradeStatus.trade_count}/{autoTradeStatus.max_trade_count}</span>
            </button>
          )}

          {/* Settings button */}
          <button
            onClick={() => setShowSettingsModal(true)}
            className="flex items-center gap-1 px-2 py-0.5 rounded text-[10px] font-semibold bg-blue-500/20 text-blue-400 hover:bg-blue-500/30 transition-colors"
            title="Application Settings"
          >
            <span>⚙️</span>
            <span>Settings</span>
          </button>

          {/* User information */}
          {username && (
            <div className="flex items-center gap-1 pl-2 border-l border-[--border-color]">
              <span className="text-[10px] text-[--text-secondary]">👤 {username}</span>
              {onLogout && (
                <button
                  onClick={onLogout}
                  className="text-[10px] text-[--text-muted] hover:text-red-400 transition-colors px-1"
                  title="Log out"
                >
                  Log out
                </button>
              )}
            </div>
          )}
        </div>
      </div>

      {/* Automated trading settings modal */}
      {showAutoTradeModal && autoTradeStatus && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50" onClick={() => setShowAutoTradeModal(false)}>
          <div 
            className="bg-[--bg-secondary] border border-[--border-color] rounded-lg shadow-xl w-80 max-w-[90vw]"
            onClick={(e) => e.stopPropagation()}
          >
            {/* Modal header */}
            <div className="flex items-center justify-between px-4 py-3 border-b border-[--border-color]">
              <div className="flex items-center gap-2">
                <span className="text-base">🤖</span>
                <span className="font-semibold text-[--text-primary] text-sm">Automated Trading Settings</span>
              </div>
              <button
                onClick={() => setShowAutoTradeModal(false)}
                className="text-[--text-muted] hover:text-[--text-primary] text-lg leading-none"
              >
                ×
              </button>
            </div>

            {/* Modal content */}
            <div className="p-4 space-y-4">
              {/* Toggle state */}
              <div className="flex items-center justify-between">
                <span className="text-sm text-[--text-secondary]">Current Status</span>
                <div className="flex items-center gap-2">
                  <span className={`text-xs px-2 py-1 rounded font-semibold ${
                    autoTradeStatus.enabled
                      ? 'bg-green-500/20 text-green-400'
                      : 'bg-gray-500/20 text-gray-400'
                  }`}>
                    {autoTradeStatus.enabled ? 'Running' : 'Disabled'}
                  </span>
                  <button
                    onClick={handleAutoTradeToggle}
                    disabled={autoTradeLoading}
                    className={`text-xs px-3 py-1 rounded font-semibold transition-colors ${
                      autoTradeStatus.enabled
                        ? 'bg-red-500/20 text-red-400 hover:bg-red-500/30'
                        : 'bg-green-500/20 text-green-400 hover:bg-green-500/30'
                    } ${autoTradeLoading ? 'opacity-50 cursor-not-allowed' : ''}`}
                  >
                    {autoTradeStatus.enabled ? 'Disable' : 'Enable'}
                  </button>
                </div>
              </div>

              {/* Trade count */}
              <div className="flex items-center justify-between">
                <span className="text-sm text-[--text-secondary]">Trades Placed</span>
                <div className="flex items-center gap-2">
                  <span className={`text-sm font-mono font-semibold ${
                    autoTradeStatus.remaining > 0 ? 'text-green-400' : 'text-red-400'
                  }`}>
                    {autoTradeStatus.trade_count}
                  </span>
                  <span className="text-[--text-muted]">/</span>
                  <input
                    type="number"
                    value={maxTradeCountInput}
                    onChange={(e) => setMaxTradeCountInput(e.target.value)}
                    className="w-12 text-xs px-2 py-1 rounded bg-[--bg-primary] border border-[--border-color] text-[--text-primary] focus:outline-none focus:border-blue-500 text-center"
                    min="1"
                  />
                  <button
                    onClick={handleUpdateMaxTradeCount}
                    disabled={autoTradeLoading}
                    className="text-xs px-2 py-1 rounded bg-blue-500/20 text-blue-400 hover:bg-blue-500/30 transition-colors disabled:opacity-50"
                  >
                    Update
                  </button>
                  <button
                    onClick={handleAutoTradeReset}
                    disabled={autoTradeLoading}
                    className="text-xs px-2 py-1 rounded bg-orange-500/20 text-orange-400 hover:bg-orange-500/30 transition-colors disabled:opacity-50"
                  >
                    Reset
                  </button>
                </div>
              </div>

              {/* Duration threshold */}
              <div className="flex items-center justify-between">
                <span className="text-sm text-[--text-secondary]">Minimum Duration</span>
                <div className="flex items-center gap-2">
                  <input
                    type="number"
                    value={durationInput}
                    onChange={(e) => setDurationInput(e.target.value)}
                    className="w-20 text-xs px-2 py-1 rounded bg-[--bg-primary] border border-[--border-color] text-[--text-primary] focus:outline-none focus:border-blue-500 text-right"
                    min="0"
                    step="100"
                  />
                  <span className="text-xs text-[--text-muted]">ms</span>
                  <button
                    onClick={handleUpdateDuration}
                    disabled={autoTradeLoading}
                    className="text-xs px-2 py-1 rounded bg-blue-500/20 text-blue-400 hover:bg-blue-500/30 transition-colors disabled:opacity-50"
                  >
                    Update
                  </button>
                </div>
              </div>

              {/* Maximum amount per trade */}
              <div className="flex items-center justify-between">
                <span className="text-sm text-[--text-secondary]">Maximum Amount per Trade</span>
                <span className="text-sm font-mono text-[--text-primary]">${autoTradeStatus.max_amount}</span>
              </div>

              {/* Divider */}
              <div className="border-t border-[--border-color] my-2"></div>

              {/* Order mode */}
              <div className="flex items-center justify-between">
                <span className="text-sm text-[--text-secondary]">Order Mode</span>
                <div className="flex items-center gap-2">
                  <span className={`text-xs px-2 py-1 rounded font-semibold ${
                    autoTradeStatus.flexible_mode
                      ? 'bg-purple-500/20 text-purple-400'
                      : 'bg-blue-500/20 text-blue-400'
                  }`}>
                    {autoTradeStatus.flexible_mode ? 'Flexible Mode' : 'Fixed Mode'}
                  </span>
                  <button
                    onClick={handleToggleFlexibleMode}
                    disabled={autoTradeLoading}
                    className="text-xs px-2 py-1 rounded bg-gray-500/20 text-gray-300 hover:bg-gray-500/30 transition-colors disabled:opacity-50"
                  >
                    Switch
                  </button>
                </div>
              </div>

              {/* Maximum contracts */}
              <div className="flex items-center justify-between">
                <span className="text-sm text-[--text-secondary]">Maximum Contracts per Trade</span>
                <div className="flex items-center gap-2">
                  <input
                    type="number"
                    value={maxContractsInput}
                    onChange={(e) => setMaxContractsInput(e.target.value)}
                    className="w-16 text-xs px-2 py-1 rounded bg-[--bg-primary] border border-[--border-color] text-[--text-primary] focus:outline-none focus:border-blue-500 text-center"
                    min="1"
                  />
                  <span className="text-xs text-[--text-muted]">contracts</span>
                  <button
                    onClick={handleUpdateMaxContracts}
                    disabled={autoTradeLoading}
                    className="text-xs px-2 py-1 rounded bg-blue-500/20 text-blue-400 hover:bg-blue-500/30 transition-colors disabled:opacity-50"
                  >
                    Update
                  </button>
                </div>
              </div>

              {/* Minimum contracts */}
              <div className="flex items-center justify-between">
                <span className="text-sm text-[--text-secondary]">Minimum Contract Threshold</span>
                <div className="flex items-center gap-2">
                  <input
                    type="number"
                    value={minContractsInput}
                    onChange={(e) => setMinContractsInput(e.target.value)}
                    className="w-16 text-xs px-2 py-1 rounded bg-[--bg-primary] border border-[--border-color] text-[--text-primary] focus:outline-none focus:border-blue-500 text-center"
                    min="1"
                  />
                  <span className="text-xs text-[--text-muted]">contracts</span>
                  <button
                    onClick={handleUpdateMinContracts}
                    disabled={autoTradeLoading}
                    className="text-xs px-2 py-1 rounded bg-blue-500/20 text-blue-400 hover:bg-blue-500/30 transition-colors disabled:opacity-50"
                  >
                    Update
                  </button>
                </div>

                {/* Bounded exposure neutralization */}
                <div className="flex items-center justify-between">
                  <div>
                    <span className="text-sm text-[--text-secondary]">Emergency Close Loss Bound</span>
                    <p className="text-[10px] text-[--text-muted]">Maximum adverse price allowed for a partial-fill close</p>
                  </div>
                  <div className="flex items-center gap-2">
                    <input
                      type="number"
                      value={neutralizationLossInput}
                      onChange={(e) => setNeutralizationLossInput(e.target.value)}
                      className="w-12 text-xs px-2 py-1 rounded bg-[--bg-primary] border border-[--border-color] text-[--text-primary] focus:outline-none focus:border-blue-500 text-center"
                      min="0"
                      max="99"
                    />
                    <span className="text-xs text-[--text-muted]">¢</span>
                    <button
                      onClick={handleUpdateNeutralizationLoss}
                      disabled={autoTradeLoading}
                      className="text-xs px-2 py-1 rounded bg-blue-500/20 text-blue-400 hover:bg-blue-500/30 transition-colors disabled:opacity-50"
                    >
                      Update
                    </button>
                  </div>
                </div>
              </div>

              {/* Last trade time */}
              {autoTradeStatus.last_trade_time && (
                <div className="flex items-center justify-between">
                  <span className="text-sm text-[--text-secondary]">Last Trade</span>
                  <span className="text-xs text-[--text-muted]">
                    {new Date(autoTradeStatus.last_trade_time).toLocaleString()}
                  </span>
                </div>
              )}

              {/* Status message */}
              {autoTradeMessage && (
                <div className={`text-xs px-3 py-2 rounded ${
                  autoTradeMessage.type === 'success'
                    ? 'bg-green-500/20 text-green-400'
                    : 'bg-red-500/20 text-red-400'
                }`}>
                  {autoTradeMessage.text}
                </div>
              )}
            </div>

            {/* Modal footer */}
            <div className="px-4 py-3 border-t border-[--border-color] bg-[--bg-tertiary] rounded-b-lg">
              <p className="text-[10px] text-[--text-muted] leading-relaxed">
                💡 {autoTradeStatus.flexible_mode 
                  ? `Flexible mode: trade ${autoTradeStatus.min_contracts} contracts at depth 10–20, or half the depth at ≥20 (up to ${autoTradeStatus.max_contracts} contracts)`
                  : `Fixed mode: trade ${autoTradeStatus.min_contracts} contracts each time`
                }. Both platforms must have depth of at least {autoTradeStatus.min_contracts} contracts before trading.
                A partial or one-leg fill is closed only within the configured {autoTradeStatus.neutralization_max_loss_cents}¢ bound; otherwise auto-trading halts.
              </p>
            </div>
          </div>
        </div>
      )}

      {/* Application settings modal */}
      {showSettingsModal && appSettings && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50" onClick={() => setShowSettingsModal(false)}>
          <div 
            className="bg-[--bg-secondary] border border-[--border-color] rounded-lg shadow-xl w-96 max-w-[90vw]"
            onClick={(e) => e.stopPropagation()}
          >
            {/* Modal header */}
            <div className="flex items-center justify-between px-4 py-3 border-b border-[--border-color]">
              <div className="flex items-center gap-2">
                <span className="text-base">⚙️</span>
                <span className="font-semibold text-[--text-primary] text-sm">Application Settings</span>
              </div>
              <button
                onClick={() => setShowSettingsModal(false)}
                className="text-[--text-muted] hover:text-[--text-primary] text-lg leading-none"
              >
                ×
              </button>
            </div>

            {/* Modal content */}
            <div className="p-4 space-y-4">
              {/* Refresh interval */}
              <div className="flex items-center justify-between">
                <div>
                  <span className="text-sm text-[--text-secondary]">Data Refresh Interval</span>
                  <p className="text-[10px] text-[--text-muted]">Interval for scheduled market scans</p>
                </div>
                <div className="flex items-center gap-2">
                  <input
                    type="number"
                    value={editRefreshInterval}
                    onChange={(e) => setEditRefreshInterval(e.target.value)}
                    className="w-16 text-xs px-2 py-1 rounded bg-[--bg-primary] border border-[--border-color] text-[--text-primary] focus:outline-none focus:border-blue-500 text-right"
                    min="1"
                  />
                  <span className="text-xs text-[--text-muted]">seconds</span>
                </div>
              </div>

              {/* Minimum profit margin */}
              <div className="flex items-center justify-between">
                <div>
                  <span className="text-sm text-[--text-secondary]">Minimum Profit Margin</span>
                  <p className="text-[10px] text-[--text-muted]">Hide opportunities below this profit margin</p>
                </div>
                <div className="flex items-center gap-2">
                  <input
                    type="number"
                    value={editMinProfit}
                    onChange={(e) => setEditMinProfit(e.target.value)}
                    className="w-16 text-xs px-2 py-1 rounded bg-[--bg-primary] border border-[--border-color] text-[--text-primary] focus:outline-none focus:border-blue-500 text-right"
                    min="0"
                    step="0.1"
                  />
                  <span className="text-xs text-[--text-muted]">%</span>
                </div>
              </div>

              {/* Default bet amount */}
              <div className="flex items-center justify-between">
                <div>
                  <span className="text-sm text-[--text-secondary]">Default Bet Amount</span>
                  <p className="text-[10px] text-[--text-muted]">Default amount used for arbitrage calculations</p>
                </div>
                <div className="flex items-center gap-2">
                  <span className="text-xs text-[--text-muted]">$</span>
                  <input
                    type="number"
                    value={editDefaultBet}
                    onChange={(e) => setEditDefaultBet(e.target.value)}
                    className="w-16 text-xs px-2 py-1 rounded bg-[--bg-primary] border border-[--border-color] text-[--text-primary] focus:outline-none focus:border-blue-500 text-right"
                    min="1"
                    step="1"
                  />
                </div>
              </div>

              {/* Tracking threshold */}
              <div className="flex items-center justify-between">
                <div>
                  <span className="text-sm text-[--text-secondary]">Tracking Threshold</span>
                  <p className="text-[10px] text-[--text-muted]">Start tracking when this profit margin is exceeded</p>
                </div>
                <div className="flex items-center gap-2">
                  <input
                    type="number"
                    value={editTrackingThreshold}
                    onChange={(e) => setEditTrackingThreshold(e.target.value)}
                    className="w-16 text-xs px-2 py-1 rounded bg-[--bg-primary] border border-[--border-color] text-[--text-primary] focus:outline-none focus:border-blue-500 text-right"
                    min="0"
                    step="0.1"
                  />
                  <span className="text-xs text-[--text-muted]">%</span>
                </div>
              </div>

              {/* Save button */}
              <div className="flex justify-end pt-2">
                <button
                  onClick={handleUpdateSettings}
                  disabled={settingsLoading}
                  className="px-4 py-1.5 rounded bg-blue-500/20 text-blue-400 hover:bg-blue-500/30 transition-colors text-xs font-semibold disabled:opacity-50"
                >
                  {settingsLoading ? 'Saving...' : 'Save Settings'}
                </button>
              </div>

              {/* Status message */}
              {settingsMessage && (
                <div className={`text-xs px-3 py-2 rounded ${
                  settingsMessage.type === 'success'
                    ? 'bg-green-500/20 text-green-400'
                    : 'bg-red-500/20 text-red-400'
                }`}>
                  {settingsMessage.text}
                </div>
              )}
            </div>

            {/* Modal footer */}
            <div className="px-4 py-3 border-t border-[--border-color] bg-[--bg-tertiary] rounded-b-lg">
              <p className="text-[10px] text-[--text-muted] leading-relaxed">
                💡 Settings take effect immediately. Some settings, such as the refresh interval, require a service restart to be fully applied.
              </p>
            </div>
          </div>
        </div>
      )}
    </header>
  );
}

function StatItem({ label, value, color, highlight }: { label: string; value: string | number; color: string; highlight?: boolean }) {
  return (
    <div className="flex items-center gap-1">
      <span className="text-[--text-muted] text-[10px]">{label}</span>
      <span className={`${color} font-semibold tabular-nums text-[10px] transition-all duration-300 ${highlight ? 'scale-110 brightness-125' : ''}`}>
        {value}
      </span>
    </div>
  );
}
