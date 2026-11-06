import { useEffect, useState } from 'react';
import { MentionDecision, MentionPosition, MentionSignalRow, MentionStatus } from '../types';

interface MentionSignalsPanelProps {
  apiBaseUrl: string;
  signals: MentionSignalRow[];
  status: MentionStatus | null;
}

const money = (value?: number | null) => value == null || !Number.isFinite(value)
  ? 'n/a'
  : `${value < 0 ? '-' : ''}$${Math.abs(value).toFixed(2)}`;

const probability = (value?: number | null) => value == null || !Number.isFinite(value)
  ? 'n/a'
  : `${(value * 100).toFixed(1)}%`;

const cents = (value?: number | null) => value == null || !Number.isFinite(value)
  ? 'n/a'
  : `${(value * 100).toFixed(1)}c`;

const pnlColor = (value?: number | null) => value == null || value === 0
  ? 'text-[--text-secondary]'
  : value < 0 ? 'text-red-400' : 'text-green-400';

const shortTime = (iso: string) => {
  const parsed = new Date(iso);
  return Number.isNaN(parsed.getTime()) ? '' : parsed.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
};

type Mode = 'off' | 'paper' | 'live';

const MODE_STYLE: Record<Mode, { frame: string; banner: string; text: string }> = {
  off: {
    frame: 'border-2 border-[--border-color]',
    banner: 'bg-[--bg-tertiary] text-[--text-muted]',
    text: 'OFF - no directional entries are evaluated for trading',
  },
  paper: {
    frame: 'border-2 border-dashed border-amber-400',
    banner: 'bg-amber-400 text-black',
    text: 'PAPER - hypothetical fills only, no orders are sent to Kalshi',
  },
  live: {
    frame: 'border-2 border-solid border-red-600',
    banner: 'bg-red-600 text-white animate-pulse',
    text: 'LIVE - REAL MONEY, orders are sent to Kalshi',
  },
};

export function MentionSignalsPanel({ apiBaseUrl, signals, status }: MentionSignalsPanelProps) {
  const [decisions, setDecisions] = useState<MentionDecision[]>([]);
  const [positions, setPositions] = useState<MentionPosition[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    const load = async () => {
      try {
        const [decisionResponse, positionResponse] = await Promise.all([
          fetch(`${apiBaseUrl}/api/mention/decisions?limit=20`),
          fetch(`${apiBaseUrl}/api/mention/history?limit=10`),
        ]);
        if (!decisionResponse.ok) throw new Error(`Decision log unavailable (${decisionResponse.status})`);
        if (!positionResponse.ok) throw new Error(`Position history unavailable (${positionResponse.status})`);
        const decisionBody = await decisionResponse.json();
        const positionBody = await positionResponse.json();
        if (disposed) return;
        if (Array.isArray(decisionBody.records)) setDecisions(decisionBody.records);
        if (Array.isArray(positionBody.records)) setPositions(positionBody.records);
      } catch (reason) {
        if (!disposed) setError(reason instanceof Error ? reason.message : 'Mention data unavailable');
      }
    };
    void load();
    const timer = setInterval(() => void load(), 10000);
    return () => { disposed = true; clearInterval(timer); };
  }, [apiBaseUrl]);

  const command = async (method: 'POST' | 'PUT', path: string, body: unknown) => {
    setBusy(true);
    setError(null);
    try {
      const token = localStorage.getItem('auth_token');
      const response = await fetch(`${apiBaseUrl}/api/mention/${path}`, {
        method,
        headers: {
          'Content-Type': 'application/json',
          ...(token ? { Authorization: `Bearer ${token}` } : {}),
        },
        body: JSON.stringify(body),
      });
      const result = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(result.error || `Command failed (${response.status})`);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Command failed');
    } finally {
      setBusy(false);
    }
  };

  const currentMode: Mode = status?.mode ?? 'off';
  const style = MODE_STYLE[currentMode];
  const limits = status?.limits;
  const minEdge = limits?.min_net_edge ?? 0.03;
  const gateCiLow = status?.gate?.ci_low;
  const gatePassed = status?.gate?.live_gate_passed === true && gateCiLow != null && gateCiLow > 0;
  const lookupConfirmed = status?.live_order_lookup_confirmed === true;
  const liveAvailable = gatePassed && lookupConfirmed && !status?.kill_switch && !status?.loss_breaker;
  const liveBlockedReason = !lookupConfirmed
    ? 'order recovery has not been verified against the Kalshi API'
    : !gatePassed
      ? 'the forward-paper gate has not passed for the current strategy version'
      : status?.loss_breaker
        ? 'the daily loss breaker is tripped'
        : status?.kill_switch ? 'the kill switch is on' : '';

  const setMode = (mode: Mode) => {
    if (mode === 'live' && !window.confirm('Enable LIVE directional trading with real money?')) return;
    void command('PUT', 'mode', { mode });
  };

  const toggleKill = () => {
    const enable = !status?.kill_switch;
    if (!enable && !window.confirm('Turn the kill switch OFF and allow directional entries again?')) return;
    void command('POST', 'kill', { enabled: enable });
  };

  return (
    <section className={`flex h-full min-h-0 flex-col text-[--text-primary] ${style.frame}`}>
      <div className={`px-3 py-1.5 text-center text-[10px] font-bold tracking-wide ${style.banner}`}>
        {style.text}
      </div>
      <header className="flex flex-wrap items-center justify-between gap-2 border-b border-[--border-color] px-3 py-2">
        <div className="flex min-w-0 items-center gap-2">
          <h2 className="text-xs font-semibold">Mention Signals</h2>
          <span className={`rounded px-2 py-1 text-[10px] font-bold ${style.banner}`}>
            {currentMode.toUpperCase()}
          </span>
        </div>
        <div className="flex items-center gap-1">
          <button
            className={`rounded px-2 py-1 text-[10px] ${currentMode === 'off' ? 'bg-[--accent-blue] text-white' : 'bg-[--bg-tertiary] text-[--text-secondary]'}`}
            disabled={busy || currentMode === 'off'}
            onClick={() => setMode('off')}
          >Off</button>
          <button
            className={`rounded px-2 py-1 text-[10px] ${currentMode === 'paper' ? 'bg-amber-400 font-semibold text-black' : 'bg-[--bg-tertiary] text-[--text-secondary]'}`}
            disabled={busy || currentMode === 'paper'}
            onClick={() => setMode('paper')}
          >Paper</button>
          <button
            className={`rounded px-2 py-1 text-[10px] ${currentMode === 'live' ? 'bg-red-600 font-semibold text-white' : 'bg-[--bg-tertiary] text-[--text-secondary]'} disabled:opacity-40`}
            disabled={busy || currentMode === 'live' || !liveAvailable}
            title={liveAvailable ? 'Enable live trading' : `Live unavailable: ${liveBlockedReason}`}
            onClick={() => setMode('live')}
          >Live</button>
          <button
            className={`rounded px-2 py-1 text-[10px] font-semibold ${status?.kill_switch ? 'bg-red-600 text-white' : 'bg-red-500/15 text-red-400'}`}
            disabled={busy}
            title={status?.kill_switch ? 'Kill switch is ON. Click to allow entries again' : 'Immediately stop new directional entries'}
            onClick={toggleKill}
          >{status?.kill_switch ? 'Kill ON' : 'Kill switch'}</button>
        </div>
      </header>

      <div className="grid grid-cols-2 gap-x-4 gap-y-1 border-b border-[--border-color] px-3 py-2 text-[10px]">
        <div>Signal service <strong className={status?.signal_service_healthy ? 'text-green-400' : 'text-red-400'}>{status?.signal_service_healthy ? 'healthy' : 'unavailable'}</strong></div>
        <div>Live gate <strong className={gatePassed ? 'text-green-400' : 'text-amber-400'}>{gatePassed ? 'passed' : 'blocked'}</strong></div>
        <div>Gate days <span className="font-mono">{status?.gate?.n_days ?? 0}/{limits?.live_gate_min_days ?? 'n/a'}</span></div>
        <div>Gate CI low <span className="font-mono">{gateCiLow == null ? 'n/a' : money(gateCiLow)}</span></div>
        <div>Open risk <span className="font-mono">{money(status?.open_exposure)}</span></div>
        <div>Positions <span className="font-mono">{status?.open_positions ?? 0}/{limits?.max_open_positions ?? 'n/a'}</span></div>
        <div>Paper P&L today <span className={`font-mono ${pnlColor(status?.today_realized_pnl_paper)}`}>{money(status?.today_realized_pnl_paper)}</span></div>
        <div>Paper P&L total <span className={`font-mono ${pnlColor(status?.realized_pnl_paper)}`}>{money(status?.realized_pnl_paper)}</span></div>
        <div>Live P&L today <span className={`font-mono ${pnlColor(status?.today_realized_pnl_live)}`}>{money(status?.today_realized_pnl_live)}</span></div>
        <div>Live P&L total <span className={`font-mono ${pnlColor(status?.realized_pnl_live)}`}>{money(status?.realized_pnl_live)}</span></div>
      </div>
      {!liveAvailable && <div className="border-b border-amber-500/30 bg-amber-500/10 px-3 py-1.5 text-[10px] text-amber-300">LIVE DISABLED: {liveBlockedReason || 'unavailable'}.</div>}
      {status?.kill_switch && <div className="border-b border-red-500/30 bg-red-500/10 px-3 py-1.5 text-[10px] text-red-300">Kill switch is ON. No new directional entries.</div>}
      {status?.loss_breaker && <div className="border-b border-red-500/30 bg-red-500/10 px-3 py-1.5 text-[10px] text-red-300">Daily loss breaker tripped{status.breaker_day ? ` on ${status.breaker_day}` : ''}. Manual reset required.</div>}
      {status?.error && <div className="border-b border-red-500/30 px-3 py-1.5 text-[10px] text-red-300">Status error: {status.error}</div>}
      {error && <div className="border-b border-red-500/30 px-3 py-1.5 text-[10px] text-red-300">{error}</div>}

      <div className="min-h-0 flex-1 overflow-auto">
        <div className="grid grid-cols-[minmax(130px,1fr)_auto_auto_auto] gap-x-3 border-b border-[--border-color] px-3 py-1 text-[9px] uppercase text-[--text-muted]">
          <span>Market / phrase</span><span>p_hat / p_lo-p_hi</span><span>YES bid / ask</span><span>Net edge /contract Y, N</span>
        </div>
        {signals.length === 0 ? <div className="px-3 py-4 text-xs text-[--text-muted]">No current mention signals</div> : signals.map((row) => {
          const signal = row.signal;
          return (
            <div key={signal.market_ticker} className="grid grid-cols-[minmax(130px,1fr)_auto_auto_auto] items-center gap-x-3 border-b border-[--border-color]/60 px-3 py-2 text-[10px]">
              <div className="min-w-0">
                <div className="truncate font-mono" title={signal.market_ticker}>{signal.market_ticker}</div>
                <div className="truncate text-[--text-muted]" title={signal.phrase ?? ''}>{signal.phrase ?? 'Phrase unavailable'}</div>
                <div className={signal.status === 'ok' && row.signal_fresh ? 'text-green-400' : 'text-amber-400'}>{signal.status}{row.signal_fresh ? '' : ' (stale)'}{signal.reason ? ` · ${signal.reason}` : ''}</div>
              </div>
              <div className="text-right font-mono">
                <div>{probability(signal.p_hat)}</div>
                <div className="text-[--text-muted]">{probability(signal.p_lo)}–{probability(signal.p_hi)}</div>
              </div>
              <div className="text-right font-mono">
                <div>{cents(row.yes_bid)} / {cents(row.yes_ask)}</div>
                <div className="text-[--text-muted]">NO {cents(row.no_bid)} / {cents(row.no_ask)}</div>
              </div>
              <div className="text-right font-mono">
                <div className={(row.net_edge_yes ?? -1) >= minEdge ? 'text-green-400' : 'text-[--text-muted]'}>{money(row.net_edge_yes)}</div>
                <div className={(row.net_edge_no ?? -1) >= minEdge ? 'text-green-400' : 'text-[--text-muted]'}>{money(row.net_edge_no)}</div>
              </div>
            </div>
          );
        })}

        <div className="border-b border-[--border-color] px-3 py-2 text-[10px] font-semibold">Recent directional positions</div>
        {positions.length === 0 ? <div className="px-3 py-3 text-[10px] text-[--text-muted]">No directional positions</div> : positions.map((position) => (
          <div key={position.id} className="flex items-center justify-between gap-2 border-b border-[--border-color]/50 px-3 py-1.5 text-[9px]">
            <span className="flex min-w-0 items-center gap-2">
              <span className={`rounded px-1.5 py-0.5 font-bold ${position.mode === 'live' ? 'bg-red-600 text-white' : 'bg-amber-400 text-black'}`}>{position.mode.toUpperCase()}</span>
              <span className="truncate font-mono" title={position.market_ticker}>{position.side.toUpperCase()} {position.remaining_contracts}/{position.contracts} {position.market_ticker}</span>
            </span>
            <span className="whitespace-nowrap font-mono">
              <span className="text-[--text-muted]">{position.status} @ {cents(position.entry_vwap)} </span>
              <span className={pnlColor(position.net_pnl)}>{money(position.net_pnl)}</span>
            </span>
          </div>
        ))}

        <div className="border-b border-[--border-color] px-3 py-2 text-[10px] font-semibold">Recent directional decisions</div>
        {decisions.length === 0 ? <div className="px-3 py-3 text-[10px] text-[--text-muted]">No decisions recorded</div> : decisions.map((entry) => (
          <div key={entry.id} className="border-b border-[--border-color]/50 px-3 py-1.5 text-[9px]">
            <div className="flex items-center justify-between gap-2">
              <span className={entry.decision === 'entered' ? 'text-green-400' : 'text-[--text-secondary]'}>{entry.decision.toUpperCase()} · {entry.side?.toUpperCase() ?? '—'} · {entry.market_ticker}</span>
              <span className="font-mono text-[--text-muted]">{shortTime(entry.created_at)} {entry.net_edge == null ? '' : money(entry.net_edge)}</span>
            </div>
            <div className="text-[--text-muted]">{entry.reason}</div>
          </div>
        ))}
      </div>
      <footer className="flex items-center justify-between gap-2 border-t border-[--border-color] px-3 py-2">
        <span className="text-[9px] text-[--text-muted]">Single-leg directional bets with model risk. Not arbitrage.</span>
        {status?.loss_breaker && <button className="rounded bg-red-500/15 px-2 py-1 text-[10px] text-red-300 disabled:opacity-50" disabled={busy} onClick={() => void command('POST', 'reset-loss-breaker', {})}>Reset loss breaker</button>}
      </footer>
    </section>
  );
}
