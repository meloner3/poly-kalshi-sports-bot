import { useEffect, useState, useCallback } from 'react';

interface ProfitHistoryEntry {
  time: string;
  profit_margin: number;
  kalshi_price: number;
  polymarket_price: number;
}

interface HistoryRecord {
  id: number;
  event_name: string;
  team_name: string;
  kalshi_market_id: string;
  polymarket_market_id: string;
  kalshi_side: string;  // yes or no
  polymarket_side: string;  // yes or no
  start_time: string;
  end_time?: string;
  duration_seconds?: number;
  duration_ms?: number;  // duration in milliseconds
  max_profit_margin: number;
  max_profit_time?: string;
  profit_history?: ProfitHistoryEntry[];
  // Depth info
  poly_ask_depth?: number;  // Polymarket ask depth (USD = price * size)
  poly_ask_size?: number;   // Polymarket ask size (token count)
  kalshi_ask_depth?: number;  // Kalshi ask depth (contracts)
  // Price info
  kalshi_ask_price?: number;  // Kalshi ask price
  polymarket_ask_price?: number;  // Polymarket ask price
}

interface SearchResult {
  records: HistoryRecord[];
  total: number;
  limit: number;
  offset: number;
  has_more: boolean;
}

interface Statistics {
  total_records: number;
  avg_profit: number;
  max_profit: number;
  min_profit: number;
  avg_duration: number;
  avg_duration_ms: number;
  max_duration: number;
  max_duration_ms: number;
  min_duration: number;
  min_duration_ms: number;
  total_duration: number;
  duration_percentiles: {
    p50: number;
    p75: number;
    p90: number;
    p95: number;
    p99: number;
  };
  top_events: { event_name: string; count: number; avg_profit: number }[];
  top_teams: { team_name: string; count: number; avg_profit: number }[];
  profit_distribution: { range: string; count: number }[];
  duration_distribution: { range: string; count: number; avg_profit: number }[];
}

interface HistoryExplorerProps {
  apiBaseUrl: string;
  onClose: () => void;
}

export function HistoryExplorer({ apiBaseUrl, onClose }: HistoryExplorerProps) {
  // Filter criteria
  const [minProfit, setMinProfit] = useState<string>('');
  const [maxProfit, setMaxProfit] = useState<string>('');
  const [minDuration, setMinDuration] = useState<string>('');
  const [maxDuration, setMaxDuration] = useState<string>('');
  const [durationUnit, setDurationUnit] = useState<'s' | 'ms'>('s'); // seconds or milliseconds
  const [eventName, setEventName] = useState('');
  const [teamName, setTeamName] = useState('');
  const [sortBy, setSortBy] = useState('start_time');
  const [sortOrder, setSortOrder] = useState('desc');

  // Pagination
  const [page, setPage] = useState(1);
  const pageSize = 20;

  // Data
  const [result, setResult] = useState<SearchResult | null>(null);
  const [stats, setStats] = useState<Statistics | null>(null);
  const [loading, setLoading] = useState(true);
  const [selectedRecord, setSelectedRecord] = useState<HistoryRecord | null>(null);
  const [activeTab, setActiveTab] = useState<'list' | 'stats'>('list');

  // Search function
  const search = useCallback(async () => {
    setLoading(true);
    try {
      const params = new URLSearchParams();
      if (minProfit) params.set('min_profit', minProfit);
      if (maxProfit) params.set('max_profit', maxProfit);

      // Convert duration based on unit (API uses seconds)
      if (minDuration) {
        const minDurationSeconds = durationUnit === 'ms' 
          ? parseFloat(minDuration) / 1000 
          : parseFloat(minDuration);
        params.set('min_duration', minDurationSeconds.toString());
      }
      if (maxDuration) {
        const maxDurationSeconds = durationUnit === 'ms' 
          ? parseFloat(maxDuration) / 1000 
          : parseFloat(maxDuration);
        params.set('max_duration', maxDurationSeconds.toString());
      }
      
      if (eventName) params.set('event_name', eventName);
      if (teamName) params.set('team_name', teamName);
      params.set('sort_by', sortBy);
      params.set('sort_order', sortOrder);
      params.set('limit', pageSize.toString());
      params.set('offset', ((page - 1) * pageSize).toString());
      params.set('include_history', 'true');
      
      const response = await fetch(`${apiBaseUrl}/api/history/search?${params}`);
      if (response.ok) {
        const data = await response.json();
        setResult(data);
      }
    } catch (error) {
      console.error('Search failed:', error);
    } finally {
      setLoading(false);
    }
  }, [apiBaseUrl, minProfit, maxProfit, minDuration, maxDuration, durationUnit, eventName, teamName, sortBy, sortOrder, page]);

  // Fetch statistics
  const fetchStats = useCallback(async () => {
    try {
      const response = await fetch(`${apiBaseUrl}/api/history/statistics`);
      if (response.ok) {
        const data = await response.json();
        setStats(data);
      }
    } catch (error) {
      console.error('Failed to fetch stats:', error);
    }
  }, [apiBaseUrl]);

  useEffect(() => {
    search();
    fetchStats();
  }, [search, fetchStats]);

  // Reset filters
  const resetFilters = () => {
    setMinProfit('');
    setMaxProfit('');
    setMinDuration('');
    setMaxDuration('');
    setDurationUnit('s');
    setEventName('');
    setTeamName('');
    setSortBy('start_time');
    setSortOrder('desc');
    setPage(1);
  };

  const formatDuration = (seconds?: number) => {
    if (!seconds) return '-';
    if (seconds < 60) return `${Math.round(seconds)}s`;
    if (seconds < 3600) return `${Math.round(seconds / 60)}m ${Math.round(seconds % 60)}s`;
    return `${Math.floor(seconds / 3600)}h ${Math.round((seconds % 3600) / 60)}m`;
  };

  const formatDurationMs = (ms?: number) => {
    if (!ms && ms !== 0) return '-';
    if (ms < 1000) return `${Math.round(ms)}ms`;
    if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
    if (ms < 3600000) return `${Math.floor(ms / 60000)}m ${Math.round((ms % 60000) / 1000)}s`;
    return `${Math.floor(ms / 3600000)}h ${Math.round((ms % 3600000) / 60000)}m`;
  };

  const formatTime = (timeStr: string) => {
    const date = new Date(timeStr);
    return date.toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  };

  const formatDate = (timeStr: string) => {
    const date = new Date(timeStr);
    return date.toLocaleDateString('en-US', { year: 'numeric', month: '2-digit', day: '2-digit' });
  };

  const totalPages = result ? Math.ceil(result.total / pageSize) : 0;

  return (
    <div className="fixed inset-0 bg-black/80 flex items-center justify-center z-50 p-4">
      <div className="bg-[--bg-primary] rounded-xl w-full max-w-6xl max-h-[90vh] flex flex-col shadow-2xl border border-[--border-color]">
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-[--border-color]">
          <div className="flex items-center gap-4">
            <h2 className="text-xl font-bold text-[--text-primary]">📊 Arbitrage History Explorer</h2>
            {stats && (
              <span className="text-sm text-[--text-muted]">
                {stats.total_records} records total
              </span>
            )}
          </div>
          <button
            onClick={onClose}
            className="text-[--text-muted] hover:text-[--text-primary] text-xl"
          >
            ✕
          </button>
        </div>

        {/* Tabs */}
        <div className="flex border-b border-[--border-color]">
          <button
            className={`px-6 py-3 text-sm font-medium transition-colors ${
              activeTab === 'list'
                ? 'text-[--accent-purple] border-b-2 border-[--accent-purple] bg-[--bg-secondary]'
                : 'text-[--text-muted] hover:text-[--text-secondary]'
            }`}
            onClick={() => setActiveTab('list')}
          >
            📋 Record List
          </button>
          <button
            className={`px-6 py-3 text-sm font-medium transition-colors ${
              activeTab === 'stats'
                ? 'text-[--accent-green] border-b-2 border-[--accent-green] bg-[--bg-secondary]'
                : 'text-[--text-muted] hover:text-[--text-secondary]'
            }`}
            onClick={() => setActiveTab('stats')}
          >
            📈 Statistics
          </button>
        </div>

        {activeTab === 'list' ? (
          <>
            {/* Filter bar */}
            <div className="px-6 py-4 bg-[--bg-secondary] border-b border-[--border-color]">
              <div className="grid grid-cols-2 md:grid-cols-4 lg:grid-cols-6 gap-3">
                {/* Profit range */}
                <div>
                  <label className="block text-xs text-[--text-muted] mb-1">Min Profit %</label>
                  <input
                    type="number"
                    value={minProfit}
                    onChange={e => setMinProfit(e.target.value)}
                    placeholder="0"
                    className="w-full px-3 py-1.5 bg-[--bg-tertiary] border border-[--border-color] rounded text-sm text-[--text-primary] focus:border-[--accent-purple] focus:outline-none"
                  />
                </div>
                <div>
                  <label className="block text-xs text-[--text-muted] mb-1">Max Profit %</label>
                  <input
                    type="number"
                    value={maxProfit}
                    onChange={e => setMaxProfit(e.target.value)}
                    placeholder="100"
                    className="w-full px-3 py-1.5 bg-[--bg-tertiary] border border-[--border-color] rounded text-sm text-[--text-primary] focus:border-[--accent-purple] focus:outline-none"
                  />
                </div>

                {/* Duration */}
                <div>
                  <label className="block text-xs text-[--text-muted] mb-1">
                    Min Duration
                    <select
                      value={durationUnit}
                      onChange={e => setDurationUnit(e.target.value as 's' | 'ms')}
                      className="ml-1 bg-transparent text-[--accent-purple] cursor-pointer"
                    >
                      <option value="s">(sec)</option>
                      <option value="ms">(ms)</option>
                    </select>
                  </label>
                  <input
                    type="number"
                    value={minDuration}
                    onChange={e => setMinDuration(e.target.value)}
                    placeholder="0"
                    className="w-full px-3 py-1.5 bg-[--bg-tertiary] border border-[--border-color] rounded text-sm text-[--text-primary] focus:border-[--accent-purple] focus:outline-none"
                  />
                </div>
                <div>
                  <label className="block text-xs text-[--text-muted] mb-1">
                    Max Duration
                    <span className="ml-1 text-[--accent-purple]">({durationUnit === 's' ? 'sec' : 'ms'})</span>
                  </label>
                  <input
                    type="number"
                    value={maxDuration}
                    onChange={e => setMaxDuration(e.target.value)}
                    placeholder="∞"
                    className="w-full px-3 py-1.5 bg-[--bg-tertiary] border border-[--border-color] rounded text-sm text-[--text-primary] focus:border-[--accent-purple] focus:outline-none"
                  />
                </div>

                {/* Search */}
                <div>
                  <label className="block text-xs text-[--text-muted] mb-1">Event Name</label>
                  <input
                    type="text"
                    value={eventName}
                    onChange={e => setEventName(e.target.value)}
                    placeholder="Search events..."
                    className="w-full px-3 py-1.5 bg-[--bg-tertiary] border border-[--border-color] rounded text-sm text-[--text-primary] focus:border-[--accent-purple] focus:outline-none"
                  />
                </div>
                <div>
                  <label className="block text-xs text-[--text-muted] mb-1">Team Name</label>
                  <input
                    type="text"
                    value={teamName}
                    onChange={e => setTeamName(e.target.value)}
                    placeholder="Search teams..."
                    className="w-full px-3 py-1.5 bg-[--bg-tertiary] border border-[--border-color] rounded text-sm text-[--text-primary] focus:border-[--accent-purple] focus:outline-none"
                  />
                </div>
              </div>

              {/* Sort and actions */}
              <div className="flex items-center justify-between mt-3">
                <div className="flex items-center gap-3">
                  <select
                    value={sortBy}
                    onChange={e => setSortBy(e.target.value)}
                    className="px-3 py-1.5 bg-[--bg-tertiary] border border-[--border-color] rounded text-sm text-[--text-primary] focus:border-[--accent-purple] focus:outline-none"
                  >
                    <option value="start_time">By Time</option>
                    <option value="max_profit_margin">By Profit</option>
                    <option value="duration">By Duration</option>
                    <option value="event_name">By Event</option>
                  </select>
                  <select
                    value={sortOrder}
                    onChange={e => setSortOrder(e.target.value)}
                    className="px-3 py-1.5 bg-[--bg-tertiary] border border-[--border-color] rounded text-sm text-[--text-primary] focus:border-[--accent-purple] focus:outline-none"
                  >
                    <option value="desc">Descending</option>
                    <option value="asc">Ascending</option>
                  </select>
                </div>
                <div className="flex items-center gap-2">
                  <button
                    onClick={resetFilters}
                    className="px-3 py-1.5 text-sm text-[--text-muted] hover:text-[--text-primary] transition-colors"
                  >
                    Reset
                  </button>
                  <button
                    onClick={() => { setPage(1); search(); }}
                    className="px-4 py-1.5 bg-[--accent-purple] text-white rounded text-sm font-medium hover:bg-[--accent-purple]/80 transition-colors"
                  >
                    Search
                  </button>
                </div>
              </div>
            </div>

            {/* List content */}
            <div className="flex-1 overflow-y-auto">
              {loading ? (
                <div className="flex items-center justify-center py-12">
                  <div className="text-[--text-muted]">Loading...</div>
                </div>
              ) : result?.records.length === 0 ? (
                <div className="flex flex-col items-center justify-center py-12">
                  <div className="text-4xl mb-3">📭</div>
                  <div className="text-[--text-muted]">No matching records found</div>
                </div>
              ) : (
                <table className="w-full">
                  <thead className="bg-[--bg-secondary] sticky top-0">
                    <tr className="text-xs text-[--text-muted]">
                      <th className="text-left px-4 py-3 font-medium">Event</th>
                      <th className="text-left px-4 py-3 font-medium">Team</th>
                      <th className="text-right px-4 py-3 font-medium">Max Profit</th>
                      <th className="text-right px-4 py-3 font-medium">Duration</th>
                      <th className="text-right px-4 py-3 font-medium">Poly Depth</th>
                      <th className="text-right px-4 py-3 font-medium">Kalshi Depth</th>
                      <th className="text-left px-4 py-3 font-medium">Start Time</th>
                    </tr>
                  </thead>
                  <tbody>
                    {result?.records.map((record, idx) => (
                      <tr
                        key={record.id}
                        className={`border-b border-[--border-color] hover:bg-[--bg-secondary] cursor-pointer transition-colors ${
                          idx % 2 === 0 ? 'bg-[--bg-primary]' : 'bg-[--bg-secondary]/30'
                        }`}
                        onClick={() => setSelectedRecord(record)}
                      >
                        <td className="px-4 py-3">
                          <div className="text-sm text-[--text-primary] font-medium truncate max-w-[200px]">
                            {record.event_name}
                          </div>
                        </td>
                        <td className="px-4 py-3">
                          <span className="text-sm text-[--accent-yellow]">{record.team_name}</span>
                        </td>
                        <td className="px-4 py-3 text-right">
                          <span className={`text-sm font-bold tabular-nums ${
                            record.max_profit_margin >= 5 ? 'text-[--accent-green]' : 'text-[--accent-yellow]'
                          }`}>
                            {record.max_profit_margin.toFixed(2)}%
                          </span>
                        </td>
                        <td className="px-4 py-3 text-right">
                          <span className="text-sm text-[--text-secondary] tabular-nums" title={record.duration_ms ? `${record.duration_ms.toLocaleString()} ms` : ''}>
                            {record.duration_ms ? formatDurationMs(record.duration_ms) : formatDuration(record.duration_seconds)}
                          </span>
                        </td>
                        <td className="px-4 py-3 text-right">
                          <div className="flex flex-col items-end">
                            <span className="text-cyan-400 font-mono text-xs">
                              ×{record.poly_ask_size?.toFixed(0) ?? '-'}
                            </span>
                            <span className={`text-xs tabular-nums ${
                              (record.poly_ask_depth || 0) >= 10 ? 'text-yellow-400' : 'text-[--accent-red]'
                            }`}>
                              ${record.poly_ask_depth?.toFixed(0) ?? '-'}
                            </span>
                          </div>
                        </td>
                        <td className="px-4 py-3 text-right">
                          <div className="flex flex-col items-end">
                            <span className="text-cyan-400 font-mono text-xs">
                              ×{record.kalshi_ask_depth ?? '-'}
                            </span>
                            <span className={`text-xs tabular-nums ${
                              (record.kalshi_ask_depth || 0) >= 10 ? 'text-yellow-400' : 'text-[--accent-red]'
                            }`}>
                              ${record.kalshi_ask_depth && record.kalshi_ask_price 
                                ? (record.kalshi_ask_depth * record.kalshi_ask_price).toFixed(0) 
                                : '-'}
                            </span>
                          </div>
                        </td>
                        <td className="px-4 py-3">
                          <span className="text-xs text-[--text-muted]">
                            {formatDate(record.start_time)} {formatTime(record.start_time)}
                          </span>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>

            {/* Pagination */}
            {result && result.total > pageSize && (
              <div className="flex items-center justify-between px-6 py-3 border-t border-[--border-color] bg-[--bg-secondary]">
                <div className="text-sm text-[--text-muted]">
                  Showing {(page - 1) * pageSize + 1} - {Math.min(page * pageSize, result.total)} of {result.total}
                </div>
                <div className="flex items-center gap-2">
                  <button
                    onClick={() => setPage(p => Math.max(1, p - 1))}
                    disabled={page === 1}
                    className="px-3 py-1 text-sm rounded bg-[--bg-tertiary] text-[--text-secondary] disabled:opacity-50 disabled:cursor-not-allowed hover:bg-[--bg-tertiary]/80"
                  >
                    Previous
                  </button>
                  <span className="text-sm text-[--text-muted]">
                    {page} / {totalPages}
                  </span>
                  <button
                    onClick={() => setPage(p => Math.min(totalPages, p + 1))}
                    disabled={page >= totalPages}
                    className="px-3 py-1 text-sm rounded bg-[--bg-tertiary] text-[--text-secondary] disabled:opacity-50 disabled:cursor-not-allowed hover:bg-[--bg-tertiary]/80"
                  >
                    Next
                  </button>
                </div>
              </div>
            )}
          </>
        ) : (
          /* Statistics tab */
          <div className="flex-1 overflow-y-auto p-6">
            {stats ? (
              <div className="space-y-6">
                {/* Overview cards - profit */}
                <div>
                  <h3 className="text-sm font-semibold text-[--text-primary] mb-3">📈 Profit Statistics</h3>
                  <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
                    <div className="bg-[--bg-secondary] rounded-lg p-4 border border-[--border-color]">
                      <div className="text-xs text-[--text-muted] mb-1">Total Records</div>
                      <div className="text-2xl font-bold text-[--text-primary]">{stats.total_records}</div>
                    </div>
                    <div className="bg-[--bg-secondary] rounded-lg p-4 border border-[--border-color]">
                      <div className="text-xs text-[--text-muted] mb-1">Avg Profit</div>
                      <div className="text-2xl font-bold text-[--accent-green]">{stats.avg_profit}%</div>
                    </div>
                    <div className="bg-[--bg-secondary] rounded-lg p-4 border border-[--border-color]">
                      <div className="text-xs text-[--text-muted] mb-1">Max Profit</div>
                      <div className="text-2xl font-bold text-[--accent-yellow]">{stats.max_profit}%</div>
                    </div>
                    <div className="bg-[--bg-secondary] rounded-lg p-4 border border-[--border-color]">
                      <div className="text-xs text-[--text-muted] mb-1">Min Profit</div>
                      <div className="text-2xl font-bold text-[--text-secondary]">{stats.min_profit}%</div>
                    </div>
                  </div>
                </div>

                {/* Overview cards - duration */}
                <div>
                  <h3 className="text-sm font-semibold text-[--text-primary] mb-3">⏱️ Duration Statistics</h3>
                  <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
                    <div className="bg-[--bg-secondary] rounded-lg p-4 border border-[--border-color]">
                      <div className="text-xs text-[--text-muted] mb-1">Avg Duration</div>
                      <div className="text-xl font-bold text-[--accent-purple]">{formatDuration(stats.avg_duration)}</div>
                      <div className="text-xs text-[--text-muted] mt-1">{stats.avg_duration_ms?.toLocaleString()} ms</div>
                    </div>
                    <div className="bg-[--bg-secondary] rounded-lg p-4 border border-[--border-color]">
                      <div className="text-xs text-[--text-muted] mb-1">Max Duration</div>
                      <div className="text-xl font-bold text-[--accent-yellow]">{formatDuration(stats.max_duration)}</div>
                      <div className="text-xs text-[--text-muted] mt-1">{stats.max_duration_ms?.toLocaleString()} ms</div>
                    </div>
                    <div className="bg-[--bg-secondary] rounded-lg p-4 border border-[--border-color]">
                      <div className="text-xs text-[--text-muted] mb-1">Min Duration</div>
                      <div className="text-xl font-bold text-[--text-secondary]">{formatDuration(stats.min_duration)}</div>
                      <div className="text-xs text-[--text-muted] mt-1">{stats.min_duration_ms?.toLocaleString()} ms</div>
                    </div>
                    <div className="bg-[--bg-secondary] rounded-lg p-4 border border-[--border-color]">
                      <div className="text-xs text-[--text-muted] mb-1">Total Duration</div>
                      <div className="text-xl font-bold text-[--text-primary]">{formatDuration(stats.total_duration)}</div>
                    </div>
                  </div>
                </div>

                {/* Duration percentiles */}
                {stats.duration_percentiles && (
                  <div className="bg-[--bg-secondary] rounded-lg p-4 border border-[--border-color]">
                    <h3 className="text-sm font-semibold text-[--text-primary] mb-3">📊 Duration Percentiles</h3>
                    <div className="grid grid-cols-5 gap-3">
                      {[
                        { label: 'P50 (median)', value: stats.duration_percentiles.p50 },
                        { label: 'P75', value: stats.duration_percentiles.p75 },
                        { label: 'P90', value: stats.duration_percentiles.p90 },
                        { label: 'P95', value: stats.duration_percentiles.p95 },
                        { label: 'P99', value: stats.duration_percentiles.p99 },
                      ].map((p, idx) => (
                        <div key={idx} className="text-center">
                          <div className="text-xs text-[--text-muted] mb-1">{p.label}</div>
                          <div className="text-sm font-bold text-[--accent-purple]">
                            {formatDurationMs(p.value)}
                          </div>
                          <div className="text-[10px] text-[--text-muted]">
                            {p.value?.toLocaleString()} ms
                          </div>
                        </div>
                      ))}
                    </div>
                  </div>
                )}

                {/* Profit distribution */}
                <div className="bg-[--bg-secondary] rounded-lg p-4 border border-[--border-color]">
                  <h3 className="text-sm font-semibold text-[--text-primary] mb-4">💰 Profit Distribution</h3>
                  <div className="flex items-end gap-2 h-32">
                    {stats.profit_distribution.map((item, idx) => {
                      const maxCount = Math.max(...stats.profit_distribution.map(d => d.count));
                      const height = (item.count / maxCount) * 100;
                      return (
                        <div key={idx} className="flex-1 flex flex-col items-center">
                          <div className="w-full flex flex-col items-center">
                            <span className="text-xs text-[--text-muted] mb-1">{item.count}</span>
                            <div
                              className="w-full bg-gradient-to-t from-[--accent-green] to-[--accent-green]/50 rounded-t"
                              style={{ height: `${Math.max(height, 5)}%`, minHeight: '4px' }}
                            />
                          </div>
                          <span className="text-xs text-[--text-muted] mt-2">{item.range}</span>
                        </div>
                      );
                    })}
                  </div>
                </div>

                {/* Duration distribution */}
                {stats.duration_distribution && stats.duration_distribution.length > 0 && (
                  <div className="bg-[--bg-secondary] rounded-lg p-4 border border-[--border-color]">
                    <h3 className="text-sm font-semibold text-[--text-primary] mb-4">⏰ Duration Distribution</h3>
                    <div className="flex items-end gap-2 h-32">
                      {stats.duration_distribution.map((item, idx) => {
                        const maxCount = Math.max(...stats.duration_distribution.map(d => d.count));
                        const height = (item.count / maxCount) * 100;
                        return (
                          <div key={idx} className="flex-1 flex flex-col items-center">
                            <div className="w-full flex flex-col items-center">
                              <span className="text-xs text-[--text-muted] mb-1">{item.count}</span>
                              <div
                                className="w-full bg-gradient-to-t from-[--accent-purple] to-[--accent-purple]/50 rounded-t cursor-pointer hover:from-[--accent-yellow] hover:to-[--accent-yellow]/50 transition-colors"
                                style={{ height: `${Math.max(height, 5)}%`, minHeight: '4px' }}
                                title={`Avg profit: ${item.avg_profit?.toFixed(1)}%`}
                              />
                            </div>
                            <span className="text-[10px] text-[--text-muted] mt-2 whitespace-nowrap">{item.range}</span>
                          </div>
                        );
                      })}
                    </div>
                    <div className="text-xs text-[--text-muted] mt-3 text-center">
                      Hover to see the average profit for that duration range
                    </div>
                  </div>
                )}

                {/* Top events and teams */}
                <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                  <div className="bg-[--bg-secondary] rounded-lg p-4 border border-[--border-color]">
                    <h3 className="text-sm font-semibold text-[--text-primary] mb-3">🏆 Top 10 Events</h3>
                    <div className="space-y-2">
                      {stats.top_events.map((event, idx) => (
                        <div key={idx} className="flex items-center justify-between text-sm">
                          <span className="text-[--text-secondary] truncate flex-1">{event.event_name}</span>
                          <div className="flex items-center gap-3 ml-2">
                            <span className="text-[--text-muted]">{event.count}x</span>
                            <span className="text-[--accent-green] tabular-nums">{event.avg_profit?.toFixed(1)}%</span>
                          </div>
                        </div>
                      ))}
                    </div>
                  </div>
                  <div className="bg-[--bg-secondary] rounded-lg p-4 border border-[--border-color]">
                    <h3 className="text-sm font-semibold text-[--text-primary] mb-3">⭐ Top 10 Teams</h3>
                    <div className="space-y-2">
                      {stats.top_teams.map((team, idx) => (
                        <div key={idx} className="flex items-center justify-between text-sm">
                          <span className="text-[--accent-yellow]">{team.team_name}</span>
                          <div className="flex items-center gap-3">
                            <span className="text-[--text-muted]">{team.count}x</span>
                            <span className="text-[--accent-green] tabular-nums">{team.avg_profit?.toFixed(1)}%</span>
                          </div>
                        </div>
                      ))}
                    </div>
                  </div>
                </div>
              </div>
            ) : (
              <div className="flex items-center justify-center py-12">
                <div className="text-[--text-muted]">Loading statistics...</div>
              </div>
            )}
          </div>
        )}
      </div>

      {/* Detail modal */}
      {selectedRecord && (
        <div 
          className="fixed inset-0 bg-black/60 flex items-center justify-center z-[60]"
          onClick={() => setSelectedRecord(null)}
        >
          <div 
            className="bg-[--bg-secondary] rounded-lg p-6 max-w-lg w-full mx-4 max-h-[80vh] overflow-y-auto"
            onClick={e => e.stopPropagation()}
          >
            <div className="flex justify-between items-start mb-4">
              <div>
                <h3 className="text-lg font-semibold text-[--text-primary]">{selectedRecord.event_name}</h3>
                <span className="text-[--accent-yellow] text-sm">{selectedRecord.team_name}</span>
              </div>
              <button 
                className="text-[--text-muted] hover:text-[--text-primary]"
                onClick={() => setSelectedRecord(null)}
              >
                ✕
              </button>
            </div>

            <div className="grid grid-cols-2 gap-4 mb-4">
              <div className="bg-[--bg-tertiary] rounded p-3">
                <div className="text-xs text-[--text-muted] mb-1">Max Profit</div>
                <div className="text-xl font-bold text-[--accent-green] tabular-nums">
                  {selectedRecord.max_profit_margin.toFixed(2)}%
                </div>
              </div>
              <div className="bg-[--bg-tertiary] rounded p-3">
                <div className="text-xs text-[--text-muted] mb-1">Duration</div>
                <div className="text-xl font-bold text-[--text-primary] tabular-nums">
                  {selectedRecord.duration_ms ? formatDurationMs(selectedRecord.duration_ms) : formatDuration(selectedRecord.duration_seconds)}
                </div>
                {selectedRecord.duration_ms && (
                  <div className="text-xs text-[--text-muted] mt-1">
                    {selectedRecord.duration_ms.toLocaleString()} ms
                  </div>
                )}
              </div>
            </div>

            {/* Depth and price info */}
            <div className="grid grid-cols-2 gap-4 mb-4">
              <div className="bg-[--bg-tertiary] rounded p-3">
                <div className="text-xs text-[--text-muted] mb-1">Polymarket Depth</div>
                <div className="flex items-baseline gap-2">
                  <span className="text-cyan-400 font-mono text-lg">
                    ×{selectedRecord.poly_ask_size?.toFixed(0) ?? '-'}
                  </span>
                  <span className={`text-lg font-bold tabular-nums ${
                    (selectedRecord.poly_ask_depth || 0) >= 10 ? 'text-yellow-400' : 'text-[--accent-red]'
                  }`}>
                    ${selectedRecord.poly_ask_depth?.toFixed(0) ?? '-'}
                  </span>
                </div>
                <div className="text-xs text-[--text-muted] mt-1" title="Fetched from the real Polymarket CLOB book once, at the moment this opportunity started being tracked. Not a live/continuous reading — depth can move afterward.">
                  Token count × price = USD (real book, snapshot at tracking start)
                </div>
              </div>
              <div className="bg-[--bg-tertiary] rounded p-3">
                <div className="text-xs text-[--text-muted] mb-1">Kalshi Depth</div>
                <div className="flex items-baseline gap-2">
                  <span className="text-cyan-400 font-mono text-lg">
                    ×{selectedRecord.kalshi_ask_depth ?? '-'}
                  </span>
                  <span className={`text-lg font-bold tabular-nums ${
                    (selectedRecord.kalshi_ask_depth || 0) >= 10 ? 'text-yellow-400' : 'text-[--accent-red]'
                  }`}>
                    ${selectedRecord.kalshi_ask_depth && selectedRecord.kalshi_ask_price
                      ? (selectedRecord.kalshi_ask_depth * selectedRecord.kalshi_ask_price).toFixed(0)
                      : '-'}
                  </span>
                </div>
                <div className="text-xs text-[--text-muted] mt-1">
                  Contract count × price = USD
                </div>
              </div>
            </div>

            {/* Price info */}
            <div className="grid grid-cols-2 gap-4 mb-4">
              <div className="bg-[--bg-tertiary] rounded p-3">
                <div className="text-xs text-[--text-muted] mb-1">Polymarket Price</div>
                <div className="text-lg font-bold text-[--accent-purple] tabular-nums">
                  {selectedRecord.polymarket_ask_price ? `${(selectedRecord.polymarket_ask_price * 100).toFixed(1)}¢` : '-'}
                </div>
                <div className="text-xs text-[--text-muted] mt-1">
                  {selectedRecord.polymarket_side ? `Buy ${selectedRecord.polymarket_side.toUpperCase()}` : 'Ask Price'}
                </div>
              </div>
              <div className="bg-[--bg-tertiary] rounded p-3">
                <div className="text-xs text-[--text-muted] mb-1">Kalshi Price</div>
                <div className="text-lg font-bold text-[--accent-blue] tabular-nums">
                  {selectedRecord.kalshi_ask_price ? `${(selectedRecord.kalshi_ask_price * 100).toFixed(1)}¢` : '-'}
                </div>
                <div className="text-xs text-[--text-muted] mt-1">
                  {selectedRecord.kalshi_side ? `Buy ${selectedRecord.kalshi_side.toUpperCase()}` : 'Ask Price'}
                </div>
              </div>
            </div>

            <div className="space-y-2 text-sm">
              <div className="flex justify-between">
                <span className="text-[--text-muted]">Start Time</span>
                <span className="text-[--text-primary]">{formatDate(selectedRecord.start_time)} {formatTime(selectedRecord.start_time)}</span>
              </div>
              {selectedRecord.end_time && (
                <div className="flex justify-between">
                  <span className="text-[--text-muted]">End Time</span>
                  <span className="text-[--text-primary]">{formatDate(selectedRecord.end_time)} {formatTime(selectedRecord.end_time)}</span>
                </div>
              )}
              {selectedRecord.max_profit_time && (
                <div className="flex justify-between">
                  <span className="text-[--text-muted]">Peak Time</span>
                  <span className="text-[--accent-green]">{formatTime(selectedRecord.max_profit_time)}</span>
                </div>
              )}
            </div>

            {/* Profit history chart */}
            {selectedRecord.profit_history && selectedRecord.profit_history.length > 0 && (
              <div className="mt-4">
                <div className="text-xs text-[--text-muted] mb-2">Profit History ({selectedRecord.profit_history.length} points)</div>
                <div className="bg-[--bg-tertiary] rounded p-3 h-32 flex items-end gap-px">
                  {selectedRecord.profit_history.slice(-100).map((entry, i) => {
                    const maxProfit = Math.max(...selectedRecord.profit_history!.map(e => e.profit_margin));
                    const height = (entry.profit_margin / maxProfit) * 100;
                    return (
                      <div
                        key={i}
                        className="flex-1 bg-[--accent-green] min-w-[2px] rounded-t transition-all hover:bg-[--accent-yellow]"
                        style={{ height: `${Math.max(height, 5)}%` }}
                        title={`${entry.profit_margin.toFixed(2)}% at ${formatTime(entry.time)}`}
                      />
                    );
                  })}
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
