import { useState, useEffect } from 'react';
import { TrackingStats, ActiveTracking } from '../types';

interface TrackingPanelProps {
  apiBaseUrl: string;
}

export function TrackingPanel({ apiBaseUrl }: TrackingPanelProps) {
  const [trackingStats, setTrackingStats] = useState<TrackingStats | null>(null);

  // Fetch tracking data periodically.
  useEffect(() => {
    const fetchTracking = async () => {
      try {
        const response = await fetch(`${apiBaseUrl}/api/tracking`);
        if (response.ok) {
          const data = await response.json();
          setTrackingStats(data);
        }
      } catch (error) {
        console.error('Failed to fetch tracking data:', error);
      }
    };

    fetchTracking();
    const interval = setInterval(fetchTracking, 2000); // Refresh every two seconds.
    return () => clearInterval(interval);
  }, [apiBaseUrl]);

  if (!trackingStats) {
    return (
      <div className="p-4 text-center text-[--text-muted]">
        Loading tracking data...
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col">
      {/* Title */}
      <div className="px-4 py-2 border-b border-[--border-color]">
        <span className="text-sm font-medium text-[--accent-green]">
          🎯 Tracking ({trackingStats.active_count})
        </span>
      </div>

      {/* Content area */}
      <div className="flex-1 overflow-y-auto p-3">
        <ActiveTrackingList items={trackingStats.active || []} />
      </div>
    </div>
  );
}

// Active tracking list
function ActiveTrackingList({ items }: { items: ActiveTracking[] }) {
  if (!items || items.length === 0) {
    return (
      <div className="text-center py-8">
        <div className="text-3xl mb-2">😴</div>
        <div className="text-[--text-muted] text-sm">No arbitrage opportunities are currently being tracked</div>
        <div className="text-[--text-muted] text-xs mt-1">Tracking starts when profit exceeds 3%</div>
      </div>
    );
  }

  return (
    <div className="space-y-2">
      {items.map((item, index) => (
        <div 
          key={`${item.event_name}_${item.team_name}_${index}`}
          className="bg-[--bg-tertiary] rounded-lg p-3 border-l-4 border-[--accent-green] animate-pulse"
        >
          <div className="flex justify-between items-start mb-2">
            <div>
              <div className="text-sm font-medium text-[--text-primary]">
                {item.event_name}
              </div>
              <div className="text-xs text-[--accent-yellow]">{item.team_name}</div>
            </div>
            <div className="text-right">
              <div className="text-lg font-bold text-[--accent-green]">
                {item.max_profit_margin.toFixed(2)}%
              </div>
              <div className="text-xs text-[--text-muted]">Peak Profit</div>
            </div>
          </div>
          <div className="flex justify-between text-xs text-[--text-muted]">
            <span>⏱ Duration: {formatDuration(item.duration_seconds)}</span>
            <span>Started: {formatTime(item.start_time)}</span>
          </div>
        </div>
      ))}
    </div>
  );
}

// Format duration.
function formatDuration(seconds: number): string {
  if (seconds < 60) {
    return `${Math.round(seconds)}s`;
  } else if (seconds < 3600) {
    const mins = Math.floor(seconds / 60);
    const secs = Math.round(seconds % 60);
    return `${mins}m ${secs}s`;
  } else {
    const hours = Math.floor(seconds / 3600);
    const mins = Math.floor((seconds % 3600) / 60);
    return `${hours}h ${mins}m`;
  }
}

// Format time.
function formatTime(isoString: string): string {
  const date = new Date(isoString);
  return date.toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false
  });
}
