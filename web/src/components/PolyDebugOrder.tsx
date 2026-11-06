import { useState } from 'react';
import { createPolymarketOrder } from '../utils/api';

interface PolyDebugOrderProps {
  apiBaseUrl: string;
  onClose: () => void;
}

interface OrderLog {
  timestamp: string;
  type: 'request' | 'response' | 'error';
  message: string;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  data?: any;
}

export function PolyDebugOrder({ apiBaseUrl, onClose }: PolyDebugOrderProps) {
  const [marketSlug, setMarketSlug] = useState('');
  const [positionSide, setPositionSide] = useState<'long' | 'short'>('long');
  const [side, setSide] = useState<'buy' | 'sell'>('buy');
  const [contracts, setContracts] = useState(1);
  const [price, setPrice] = useState(0);
  const [loading, setLoading] = useState(false);
  const [logs, setLogs] = useState<OrderLog[]>([]);

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const addLog = (type: OrderLog['type'], message: string, data?: any) => {
    const timestamp = new Date().toISOString().split('T')[1].slice(0, 12);
    setLogs(prev => [...prev, { timestamp, type, message, data }]);
  };

  const clearLogs = () => setLogs([]);

  const handleOrder = async () => {
    if (
      !marketSlug.trim()
      || !Number.isInteger(contracts)
      || contracts <= 0
      || !Number.isFinite(price)
      || price <= 0
      || price >= 1
    ) {
      addLog('error', 'A market slug, positive whole contract count, and reference price between $0.00 and $1.00 are required');
      return;
    }

    setLoading(true);
    clearLogs();

    const request = {
      market_slug: marketSlug.trim(),
      position_side: positionSide,
      side,
      contracts,
      price,
    };

    addLog('request', 'Sending order request', request);

    try {
      const startTime = Date.now();
      const response = await createPolymarketOrder(apiBaseUrl, request);
      const elapsed = Date.now() - startTime;

      addLog('response', `Response received (${elapsed}ms)`, response);

      if (response.success) {
        addLog('response', `✅ Order succeeded! order_id=${response.order_id || 'N/A'}`);
      } else {
        addLog('error', `❌ Order failed: ${response.error || 'Unknown error'}`);
      }
    } catch (e) {
      addLog('error', `❌ Request error: ${e instanceof Error ? e.message : 'Unknown error'}`);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/80 flex items-center justify-center z-50 p-4">
      <div className="bg-[--bg-secondary] rounded-lg border border-[--border-color] w-full max-w-2xl max-h-[90vh] flex flex-col">
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-[--border-color]">
          <div className="flex items-center gap-2">
            <span className="text-lg">🔧</span>
            <h2 className="text-sm font-medium text-[--text-primary]">Polymarket Manual Order Debug</h2>
          </div>
          <button
            onClick={onClose}
            className="text-[--text-muted] hover:text-[--text-secondary] text-xl"
          >
            ×
          </button>
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto p-4 space-y-4">
          {/* Market slug input */}
          <div className="space-y-2">
            <label className="text-xs text-[--text-muted]">Market Slug</label>
            <input
              type="text"
              value={marketSlug}
              onChange={(e) => setMarketSlug(e.target.value)}
              placeholder="Enter Polymarket US market slug..."
              className="w-full px-3 py-2 text-xs bg-[--bg-tertiary] border border-[--border-color] rounded text-[--text-primary] placeholder:text-[--text-muted] font-mono"
            />
            {marketSlug && (
              <div className="text-[10px] text-[--text-muted] font-mono break-all">
                {marketSlug}
              </div>
            )}
          </div>

          {/* Order parameters */}
          <div className="grid grid-cols-2 gap-4">
            <div className="space-y-2">
              <label className="text-xs text-[--text-muted]">Position</label>
              <div className="flex gap-2">
                {(['long', 'short'] as const).map((value) => (
                  <button
                    key={value}
                    onClick={() => setPositionSide(value)}
                    className={`flex-1 py-2 text-xs rounded ${
                      positionSide === value
                        ? 'bg-purple-500/20 text-purple-400 border border-purple-500'
                        : 'bg-[--bg-tertiary] text-[--text-secondary] border border-[--border-color]'
                    }`}
                  >
                    {value.toUpperCase()}
                  </button>
                ))}
              </div>

              <div className="space-y-2">
                <label className="text-xs text-[--text-muted]">Limit Price (USD)</label>
                <input
                  type="number"
                  min={0.01}
                  max={0.99}
                  step={0.01}
                  value={price || ''}
                  onChange={(e) => setPrice(parseFloat(e.target.value) || 0)}
                  placeholder="0.19"
                  className="w-full h-8 px-2 text-center text-xs bg-[--bg-tertiary] border border-[--border-color] rounded text-[--text-primary]"
                />
                <div className="text-[10px] text-[--text-muted]">
                  The order will rest at this limit price until filled or canceled.
                </div>
              </div>
            </div>

            {/* Side */}
            <div className="space-y-2">
              <label className="text-xs text-[--text-muted]">Side</label>
              <div className="flex gap-2">
                <button
                  onClick={() => setSide('buy')}
                  className={`flex-1 py-2 text-xs rounded ${
                    side === 'buy'
                      ? 'bg-green-500/20 text-green-400 border border-green-500'
                      : 'bg-[--bg-tertiary] text-[--text-secondary] border border-[--border-color]'
                  }`}
                >
                  BUY
                </button>
                <button
                  onClick={() => setSide('sell')}
                  className={`flex-1 py-2 text-xs rounded ${
                    side === 'sell'
                      ? 'bg-red-500/20 text-red-400 border border-red-500'
                      : 'bg-[--bg-tertiary] text-[--text-secondary] border border-[--border-color]'
                  }`}
                >
                  SELL
                </button>
              </div>
            </div>

            {/* Contract count */}
            <div className="space-y-2">
              <label className="text-xs text-[--text-muted]">Contracts</label>
              <div className="flex items-center gap-2">
                <button
                  onClick={() => setContracts(Math.max(1, contracts - 1))}
                  className="w-8 h-8 rounded bg-[--bg-tertiary] text-[--text-secondary] hover:bg-[--bg-primary] border border-[--border-color]"
                >
                  -
                </button>
                <input
                  type="number"
                  min={1}
                  step={1}
                  value={contracts}
                  onChange={(e) => setContracts(Math.max(1, parseInt(e.target.value) || 1))}
                  className="flex-1 h-8 px-2 text-center text-xs bg-[--bg-tertiary] border border-[--border-color] rounded text-[--text-primary]"
                />
                <button
                  onClick={() => setContracts(contracts + 1)}
                  className="w-8 h-8 rounded bg-[--bg-tertiary] text-[--text-secondary] hover:bg-[--bg-primary] border border-[--border-color]"
                >
                  +
                </button>
              </div>
              <div className="flex gap-1">
                {[1, 2, 5, 10, 25].map((v) => (
                  <button
                    key={v}
                    onClick={() => setContracts(v)}
                    className={`flex-1 py-1 text-[10px] rounded ${
                      contracts === v
                        ? 'bg-purple-500/20 text-purple-400'
                        : 'bg-[--bg-tertiary] text-[--text-muted] hover:text-[--text-secondary]'
                    }`}
                  >
                    {v}
                  </button>
                ))}
              </div>
            </div>
          </div>

          {/* Order button */}
          <button
            onClick={handleOrder}
            disabled={loading || !marketSlug.trim() || !Number.isInteger(contracts) || contracts <= 0 || price <= 0 || price >= 1}
            className="w-full py-3 text-sm font-medium rounded bg-gradient-to-r from-purple-500 to-violet-500 text-white hover:from-purple-600 hover:to-violet-600 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {loading ? 'Executing...' : `🚀 Execute ${side.toUpperCase()} ${contracts} Contract${contracts === 1 ? '' : 's'}`}
          </button>

          {/* Log area */}
          <div className="bg-[--bg-tertiary] rounded">
            <div className="flex items-center justify-between px-3 py-2 border-b border-[--border-color]">
              <span className="text-xs text-[--text-muted]">📋 Execution Log</span>
              <button
                onClick={clearLogs}
                className="text-[10px] text-[--text-muted] hover:text-[--text-secondary]"
              >
                Clear
              </button>
            </div>
            <div className="p-2 max-h-64 overflow-y-auto font-mono text-[10px] space-y-1">
              {logs.length === 0 ? (
                <div className="text-[--text-muted] text-center py-4">Waiting to execute...</div>
              ) : (
                logs.map((log, i) => (
                  <div key={i} className="space-y-1">
                    <div className="flex gap-2">
                      <span className="text-[--text-muted] flex-shrink-0">[{log.timestamp}]</span>
                      <span
                        className={`flex-shrink-0 ${
                          log.type === 'request'
                            ? 'text-blue-400'
                            : log.type === 'response'
                            ? 'text-green-400'
                            : 'text-red-400'
                        }`}
                      >
                        [{log.type}]
                      </span>
                      <span className="text-[--text-secondary]">{log.message}</span>
                    </div>
                    {log.data && (
                      <pre className="text-[--text-muted] ml-4 whitespace-pre-wrap break-all bg-[--bg-secondary] p-1 rounded">
                        {JSON.stringify(log.data, null, 2)}
                      </pre>
                    )}
                  </div>
                ))
              )}
            </div>
          </div>

          {/* Tips */}
          <div className="bg-yellow-500/10 border border-yellow-500/20 rounded p-3 text-xs text-yellow-400">
            <div className="font-medium mb-1">💡 Debug Tips</div>
            <ul className="list-disc list-inside space-y-0.5 text-[10px] text-yellow-400/80">
              <li>After placing an order, check the backend logs for detailed signing and request information.</li>
              <li>Use the native market slug and the gateway-provided LONG or SHORT position.</li>
              <li>Do not use CLOB token IDs or infer the position from an outcome index.</li>
              <li>Test with one contract first.</li>
            </ul>
          </div>
        </div>
      </div>
    </div>
  );
}
