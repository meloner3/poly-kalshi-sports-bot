import { useState, useEffect, useCallback } from 'react';
import { PositionCard, PositionLeg } from '../types';
import { getUnifiedPositions, createKalshiOrder } from '../utils/api';

interface OrderPanelProps {
  apiBaseUrl: string;
}

const money = (v: number | null | undefined) =>
  v === null || v === undefined ? '-' : `$${v.toFixed(2)}`;

const cents = (v: number | null | undefined) =>
  v === null || v === undefined ? '-' : `${(v * 100).toFixed(1)}¢`;

const pnlText = (pnl: number | null | undefined, cost?: number) => {
  if (pnl === null || pnl === undefined) return '-';
  const sign = pnl >= 0 ? '+' : '';
  const pct = cost && cost > 0 ? ` (${sign}${((pnl / cost) * 100).toFixed(1)}%)` : '';
  return `${sign}$${pnl.toFixed(2)}${pct}`;
};

const pnlColor = (pnl: number | null | undefined) =>
  pnl === null || pnl === undefined
    ? 'text-[--text-muted]'
    : pnl >= 0
      ? 'text-green-400'
      : 'text-red-400';

function Field({
  label,
  value,
  className = 'text-[--text-secondary]',
  mono = false,
  title,
}: {
  label: string;
  value: string;
  className?: string;
  mono?: boolean;
  title?: string;
}) {
  return (
    <div className="flex flex-col leading-tight" title={title}>
      <span className="text-[8px] uppercase tracking-wide text-[--text-muted]">{label}</span>
      <span className={`text-[10px] ${mono ? 'font-mono' : ''} ${className}`}>{value}</span>
    </div>
  );
}

export function OrderPanel({ apiBaseUrl }: OrderPanelProps) {
  const [cards, setCards] = useState<PositionCard[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [actionLoading, setActionLoading] = useState<string | null>(null);

  const loadData = useCallback(async () => {
    setLoading(true);
    try {
      const res = await getUnifiedPositions(apiBaseUrl);
      setCards(res.cards);
      setError(res.errors.length > 0 ? res.errors.join('; ') : null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Fetch failed');
    } finally {
      setLoading(false);
    }
  }, [apiBaseUrl]);

  useEffect(() => {
    loadData();
    const interval = setInterval(loadData, 60000); // Account endpoints are rate-limited.
    return () => clearInterval(interval);
  }, [loadData]);

  const handleSell = async (leg: PositionLeg) => {
    if (leg.platform !== 'kalshi') {
      // Portfolio records do not carry the native US market slug and explicit
      // LONG/SHORT mapping required for a safe close. Refuse rather than infer.
      alert(`Cannot sell ${leg.title}: native Polymarket US position mapping is unavailable.`);
      return;
    }
    setActionLoading(leg.market_id);
    try {
      const result = await createKalshiOrder(apiBaseUrl, {
        ticker: leg.market_id,
        side: leg.side as 'yes' | 'no',
        action: 'sell',
        count: leg.contracts,
      });
      if (result.success) {
        loadData();
      } else {
        alert(`Sell failed: ${result.error}`);
      }
    } catch (e) {
      alert(`Sell failed: ${e instanceof Error ? e.message : 'Unknown error'}`);
    } finally {
      setActionLoading(null);
    }
  };

  const renderLeg = (leg: PositionLeg) => (
    <div key={`${leg.platform}-${leg.market_id}`} className="flex items-center justify-between gap-2">
      <div className="flex items-center gap-2 min-w-0">
        <span
          className={`text-[9px] px-1.5 py-0.5 rounded font-medium ${
            leg.platform === 'kalshi' ? 'bg-blue-500/20 text-blue-400' : 'bg-purple-500/20 text-purple-400'
          }`}
        >
          {leg.platform === 'kalshi' ? 'K' : 'P'}
        </span>
        <span className="text-[9px] px-1 rounded bg-[--bg-secondary] text-[--text-secondary]">
          {leg.side.toUpperCase()}
        </span>
        <Field label="Qty" value={leg.contracts.toFixed(leg.platform === 'polymarket' ? 2 : 0)} mono />
        <Field label="Entry → Now" value={`${cents(leg.entry_price)} → ${cents(leg.current_price)}`} mono />
        <Field label="Cost → Value" value={`${money(leg.cost)} → ${money(leg.value)}`} />
        <Field
          label="P&L"
          value={pnlText(leg.unrealized_pnl, leg.cost)}
          className={pnlColor(leg.unrealized_pnl)}
        />
        <Field
          label="Fees"
          value={leg.fees === null ? 'n/a' : money(leg.fees)}
          title="Polymarket exposes no fee data"
        />
      </div>
      <button
        className="px-2 py-1 text-[10px] bg-red-500/20 text-red-400 rounded hover:bg-red-500/30 disabled:opacity-50"
        onClick={() => handleSell(leg)}
        disabled={actionLoading === leg.market_id}
      >
        {actionLoading === leg.market_id ? '...' : 'Sell'}
      </button>
    </div>
  );

  return (
    <div className="card h-full flex flex-col overflow-hidden">
      <div className="flex items-center justify-between border-b border-[--border-color] px-3 py-2 flex-shrink-0">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium text-[--text-primary]">💼 Positions</span>
          <span className="text-[10px] text-[--text-muted]">({cards.length})</span>
        </div>
        <button
          className="px-2 py-1 text-[--text-muted] hover:text-[--text-secondary] text-xs"
          onClick={loadData}
          disabled={loading}
          title="Refresh"
        >
          {loading ? '...' : '🔄'}
        </button>
      </div>

      <div className="flex-1 overflow-y-auto p-2">
        {loading && cards.length === 0 ? (
          <div className="flex items-center justify-center h-full text-[--text-muted] text-xs">Loading...</div>
        ) : error && cards.length === 0 ? (
          <div className="flex items-center justify-center h-full text-red-400 text-xs">{error}</div>
        ) : cards.length === 0 ? (
          <div className="flex items-center justify-center h-full text-[--text-muted] text-xs">No positions</div>
        ) : (
          <div className="space-y-1.5">
            {cards.map((card) => {
              const paired = card.kind === 'paired';
              const title = card.event_name || card.legs[0]?.title || '';
              return (
                <div
                  key={card.legs.map((l) => `${l.platform}-${l.market_id}-${l.side}`).join('|')}
                  className={`bg-[--bg-tertiary] rounded p-2 space-y-1 ${paired ? '' : 'border border-amber-500/40'}`}
                >
                  <div className="flex items-center justify-between gap-2">
                    <div className="flex items-center gap-2 min-w-0">
                      {paired ? (
                        <span className="text-[9px] px-1.5 py-0.5 rounded bg-green-500/20 text-green-400" title="Hedged pair opened together by this bot">
                          🔗 Paired
                        </span>
                      ) : (
                        <span className="text-[9px] px-1.5 py-0.5 rounded bg-amber-500/20 text-amber-400" title="No matching hedge leg held: one-sided exposure">
                          ⚠ Unhedged
                        </span>
                      )}
                      <span className="text-xs font-medium text-[--text-primary] truncate" title={title}>
                        {title}
                      </span>
                    </div>
                    <div className="text-[10px] whitespace-nowrap">
                      <span className="text-[--text-muted]">
                        Total {money(card.total_cost)} → {money(card.total_value)}{' '}
                      </span>
                      <span className={pnlColor(card.total_pnl)}>{pnlText(card.total_pnl, card.total_cost)}</span>
                    </div>
                  </div>
                  {card.contracts_mismatch && (
                    <div className="text-[9px] text-amber-400">⚠ Leg sizes differ: only partly hedged</div>
                  )}
                  {card.legs.map(renderLeg)}
                </div>
              );
            })}
          </div>
        )}
      </div>

      {error && cards.length > 0 && (
        <div className="border-t border-[--border-color] px-2 py-1 bg-yellow-500/10">
          <span className="text-[9px] text-yellow-400">⚠️ {error}</span>
        </div>
      )}
    </div>
  );
}
