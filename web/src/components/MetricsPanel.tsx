import { MetricsReport } from '../types';

interface MetricsPanelProps {
  metrics: MetricsReport | null;
}

export function MetricsPanel({ metrics }: MetricsPanelProps) {
  if (!metrics) {
    return (
      <div className="p-3 h-full flex flex-col items-center justify-center">
        <div className="text-2xl mb-2">⏳</div>
        <div className="text-xs text-[--text-muted]">Waiting for performance data...</div>
        <div className="text-[10px] text-[--text-muted] mt-1">Updates every 10 seconds</div>
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col">
      {/* Title */}
      <div className="px-3 py-2 border-b border-[--border-color] bg-[--bg-tertiary] flex-shrink-0">
        <h3 className="text-xs font-semibold text-[--text-muted] uppercase tracking-wider flex items-center gap-2">
          <span>📊</span>
          Performance Monitor
          <span className="text-[10px] font-normal text-[--text-muted]">(10-second statistics)</span>
        </h3>
      </div>

      {/* Operation statistics table */}
      <div className="flex-1 overflow-y-auto p-3">
        <div className="bg-[--bg-tertiary] rounded p-2.5">
          <div className="text-[10px] text-[--text-muted] mb-2 flex items-center gap-1">
            <span>⚡</span> Operation Statistics
          </div>
          
          {/* Table header */}
          <div className="grid grid-cols-5 gap-1 text-[9px] text-[--text-muted] pb-1 border-b border-[--border-color] mb-1">
            <div className="col-span-2">Operation</div>
            <div className="text-right">Count</div>
            <div className="text-right">Average</div>
            <div className="text-right">Maximum</div>
          </div>
          
          {/* Data rows */}
          <div className="space-y-0.5">
            {metrics.operations
              .filter(op => op.count > 0)
              .map((op, idx) => (
                <div 
                  key={op.name} 
                  className={`grid grid-cols-5 gap-1 text-[10px] py-1 ${idx % 2 === 0 ? 'bg-[--bg-secondary]/30' : ''} rounded`}
                >
                  <div className="col-span-2 text-[--text-secondary] truncate" title={op.name}>
                    {op.name}
                  </div>
                  <div className="text-right font-mono text-[--text-muted]">
                    {op.count}
                  </div>
                  <div className={`text-right font-mono ${op.avg_ms < 1 ? 'text-green-400' : op.avg_ms < 10 ? 'text-yellow-400' : 'text-orange-400'}`}>
                    {op.avg_ms < 0.01 ? '<0.01' : op.avg_ms.toFixed(2)}
                  </div>
                  <div className={`text-right font-mono ${op.max_ms < 5 ? 'text-green-400' : op.max_ms < 50 ? 'text-yellow-400' : 'text-orange-400'}`}>
                    {op.max_ms < 0.01 ? '<0.01' : op.max_ms.toFixed(2)}
                  </div>
                </div>
              ))}
            
            {metrics.operations.filter(op => op.count > 0).length === 0 && (
              <div className="text-center text-[10px] text-[--text-muted] py-2">
                No operation statistics available
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Latency description */}
      <div className="px-3 py-2 border-t border-[--border-color] bg-[--bg-tertiary] flex-shrink-0">
        <div className="text-[9px] text-[--text-muted] flex items-center justify-around">
          <div className="flex items-center gap-1">
            <span className="w-1.5 h-1.5 rounded-full bg-green-400"></span>
            <span>&lt;1ms</span>
          </div>
          <div className="flex items-center gap-1">
            <span className="w-1.5 h-1.5 rounded-full bg-yellow-400"></span>
            <span>&lt;10ms</span>
          </div>
          <div className="flex items-center gap-1">
            <span className="w-1.5 h-1.5 rounded-full bg-orange-400"></span>
            <span>&gt;10ms</span>
          </div>
        </div>
      </div>
    </div>
  );
}
