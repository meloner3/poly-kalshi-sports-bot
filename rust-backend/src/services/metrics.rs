//! Performance Metrics Module
//!
//! Provides high-precision performance monitoring for the arbitrage system.
//! Tracks operation timings and API latencies with millisecond precision.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Operation types for performance tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Kalshi WebSocket price update processing
    KalshiWsProcess,
    /// Polymarket WebSocket price update processing
    PolyWsProcess,
    /// Single market arbitrage calculation
    ArbitrageCalc,
    /// Full market scan
    FullScan,
    /// Event/market matching
    MarketMatch,
}

impl Operation {
    fn name(&self) -> &'static str {
        match self {
            Operation::KalshiWsProcess => "Kalshi WebSocket Processing",
            Operation::PolyWsProcess => "Polymarket WebSocket Processing",
            Operation::ArbitrageCalc => "Arbitrage Calculation",
            Operation::FullScan => "Full Market Scan",
            Operation::MarketMatch => "Market Matching",
        }
    }
}

/// Atomic metric for a single operation type
#[derive(Default)]
struct AtomicMetric {
    /// Number of calls
    count: AtomicU64,
    /// Total time in nanoseconds
    total_ns: AtomicU64,
    /// Maximum time in nanoseconds
    max_ns: AtomicU64,
}

impl AtomicMetric {
    fn record(&self, duration_ns: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_ns.fetch_add(duration_ns, Ordering::Relaxed);

        // Update max using compare-and-swap loop
        let mut current_max = self.max_ns.load(Ordering::Relaxed);
        while duration_ns > current_max {
            match self.max_ns.compare_exchange_weak(
                current_max,
                duration_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_max = actual,
            }
        }
    }

    fn get_stats(&self) -> (u64, u64, u64) {
        let count = self.count.load(Ordering::Relaxed);
        let total_ns = self.total_ns.load(Ordering::Relaxed);
        let max_ns = self.max_ns.load(Ordering::Relaxed);
        (count, total_ns, max_ns)
    }

    fn reset(&self) {
        self.count.store(0, Ordering::Relaxed);
        self.total_ns.store(0, Ordering::Relaxed);
        self.max_ns.store(0, Ordering::Relaxed);
    }
}

/// Single operation statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationStats {
    pub name: String,
    pub count: u64,
    pub avg_ms: f64,
    pub max_ms: f64,
    pub total_ms: f64,
}

/// API latency information
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiLatency {
    pub kalshi_ms: Option<u64>,
    pub polymarket_ms: Option<u64>,
}

/// Complete metrics report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsReport {
    pub operations: Vec<OperationStats>,
    pub api_latency: ApiLatency,
}

/// Performance metrics collector
pub struct PerformanceMetrics {
    // Operation metrics
    kalshi_ws: AtomicMetric,
    poly_ws: AtomicMetric,
    arbitrage_calc: AtomicMetric,
    full_scan: AtomicMetric,
    market_match: AtomicMetric,

    // API latencies (milliseconds)
    kalshi_api_latency_ms: AtomicU64,
    polymarket_api_latency_ms: AtomicU64,

    // Flags to indicate if latency has been measured
    kalshi_latency_set: AtomicU64,
    polymarket_latency_set: AtomicU64,

    // Balance cache (stored as cents to avoid floating point issues)
    kalshi_balance_cents: AtomicU64,
    polymarket_balance_cents: AtomicU64,
    kalshi_balance_set: AtomicU64,
    polymarket_balance_set: AtomicU64,
}

impl Default for PerformanceMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl PerformanceMetrics {
    /// Create a new metrics collector
    pub fn new() -> Self {
        Self {
            kalshi_ws: AtomicMetric::default(),
            poly_ws: AtomicMetric::default(),
            arbitrage_calc: AtomicMetric::default(),
            full_scan: AtomicMetric::default(),
            market_match: AtomicMetric::default(),
            kalshi_api_latency_ms: AtomicU64::new(0),
            polymarket_api_latency_ms: AtomicU64::new(0),
            kalshi_latency_set: AtomicU64::new(0),
            polymarket_latency_set: AtomicU64::new(0),
            kalshi_balance_cents: AtomicU64::new(0),
            polymarket_balance_cents: AtomicU64::new(0),
            kalshi_balance_set: AtomicU64::new(0),
            polymarket_balance_set: AtomicU64::new(0),
        }
    }

    /// Get the metric for an operation
    fn get_metric(&self, op: Operation) -> &AtomicMetric {
        match op {
            Operation::KalshiWsProcess => &self.kalshi_ws,
            Operation::PolyWsProcess => &self.poly_ws,
            Operation::ArbitrageCalc => &self.arbitrage_calc,
            Operation::FullScan => &self.full_scan,
            Operation::MarketMatch => &self.market_match,
        }
    }

    /// Record a timing for an operation
    pub fn record(&self, op: Operation, duration: Duration) {
        let duration_ns = duration.as_nanos() as u64;
        self.get_metric(op).record(duration_ns);
    }

    /// Set API latency for Kalshi
    pub fn set_kalshi_latency(&self, latency_ms: u64) {
        self.kalshi_api_latency_ms
            .store(latency_ms, Ordering::Relaxed);
        self.kalshi_latency_set.store(1, Ordering::Relaxed);
    }

    /// Set API latency for Polymarket
    pub fn set_polymarket_latency(&self, latency_ms: u64) {
        self.polymarket_api_latency_ms
            .store(latency_ms, Ordering::Relaxed);
        self.polymarket_latency_set.store(1, Ordering::Relaxed);
    }

    /// Set cached balance for Kalshi (in dollars)
    pub fn set_kalshi_balance(&self, balance: f64) {
        let cents = (balance * 100.0) as u64;
        self.kalshi_balance_cents.store(cents, Ordering::Relaxed);
        self.kalshi_balance_set.store(1, Ordering::Relaxed);
    }

    /// Set cached balance for Polymarket (in dollars)
    pub fn set_polymarket_balance(&self, balance: f64) {
        let cents = (balance * 100.0) as u64;
        self.polymarket_balance_cents
            .store(cents, Ordering::Relaxed);
        self.polymarket_balance_set.store(1, Ordering::Relaxed);
    }

    /// Get cached balances (kalshi, polymarket) in dollars
    /// Returns (None, None) if balances haven't been set yet
    pub fn get_cached_balances(&self) -> (Option<f64>, Option<f64>) {
        let kalshi = if self.kalshi_balance_set.load(Ordering::Relaxed) == 1 {
            Some(self.kalshi_balance_cents.load(Ordering::Relaxed) as f64 / 100.0)
        } else {
            None
        };

        let polymarket = if self.polymarket_balance_set.load(Ordering::Relaxed) == 1 {
            Some(self.polymarket_balance_cents.load(Ordering::Relaxed) as f64 / 100.0)
        } else {
            None
        };

        (kalshi, polymarket)
    }

    /// Get API latency information
    pub fn get_api_latency(&self) -> ApiLatency {
        let kalshi_ms = if self.kalshi_latency_set.load(Ordering::Relaxed) == 1 {
            Some(self.kalshi_api_latency_ms.load(Ordering::Relaxed))
        } else {
            None
        };

        let polymarket_ms = if self.polymarket_latency_set.load(Ordering::Relaxed) == 1 {
            Some(self.polymarket_api_latency_ms.load(Ordering::Relaxed))
        } else {
            None
        };

        ApiLatency {
            kalshi_ms,
            polymarket_ms,
        }
    }

    /// Generate a metrics report
    pub fn report(&self) -> MetricsReport {
        let operations = vec![
            Operation::KalshiWsProcess,
            Operation::PolyWsProcess,
            Operation::ArbitrageCalc,
            Operation::FullScan,
            Operation::MarketMatch,
        ]
        .into_iter()
        .map(|op| {
            let (count, total_ns, max_ns) = self.get_metric(op).get_stats();
            let total_ms = total_ns as f64 / 1_000_000.0;
            let avg_ms = if count > 0 {
                total_ms / count as f64
            } else {
                0.0
            };
            let max_ms = max_ns as f64 / 1_000_000.0;

            OperationStats {
                name: op.name().to_string(),
                count,
                avg_ms,
                max_ms,
                total_ms,
            }
        })
        .collect();

        MetricsReport {
            operations,
            api_latency: self.get_api_latency(),
        }
    }

    /// Reset all metrics (called after each report period)
    pub fn reset(&self) {
        self.kalshi_ws.reset();
        self.poly_ws.reset();
        self.arbitrage_calc.reset();
        self.full_scan.reset();
        self.market_match.reset();
    }

    /// Start timing an operation, returns a guard that records when dropped
    #[cfg(test)]
    pub fn start_timing(&self, op: Operation) -> TimingGuard<'_> {
        TimingGuard {
            metrics: self,
            operation: op,
            start: Instant::now(),
        }
    }
}

/// RAII guard for automatic timing
#[cfg(test)]
pub struct TimingGuard<'a> {
    metrics: &'a PerformanceMetrics,
    operation: Operation,
    start: Instant,
}

#[cfg(test)]
impl<'a> Drop for TimingGuard<'a> {
    fn drop(&mut self) {
        let duration = self.start.elapsed();
        self.metrics.record(self.operation, duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_record_timing() {
        let metrics = PerformanceMetrics::new();

        metrics.record(Operation::ArbitrageCalc, Duration::from_millis(10));
        metrics.record(Operation::ArbitrageCalc, Duration::from_millis(20));

        let report = metrics.report();
        let arb_stats = report
            .operations
            .iter()
            .find(|s| s.name == "Arbitrage Calculation")
            .unwrap();

        assert_eq!(arb_stats.count, 2);
        assert!(arb_stats.avg_ms >= 14.0 && arb_stats.avg_ms <= 16.0);
        assert!(arb_stats.max_ms >= 19.0 && arb_stats.max_ms <= 21.0);
    }

    #[test]
    fn test_timing_guard() {
        let metrics = PerformanceMetrics::new();

        {
            let _guard = metrics.start_timing(Operation::FullScan);
            thread::sleep(Duration::from_millis(5));
        }

        let report = metrics.report();
        let scan_stats = report
            .operations
            .iter()
            .find(|s| s.name == "Full Market Scan")
            .unwrap();

        assert_eq!(scan_stats.count, 1);
        assert!(scan_stats.avg_ms >= 4.0);
    }

    #[test]
    fn test_api_latency() {
        let metrics = PerformanceMetrics::new();

        // Initially no latency
        let latency = metrics.get_api_latency();
        assert!(latency.kalshi_ms.is_none());
        assert!(latency.polymarket_ms.is_none());

        // Set latencies
        metrics.set_kalshi_latency(45);
        metrics.set_polymarket_latency(120);

        let latency = metrics.get_api_latency();
        assert_eq!(latency.kalshi_ms, Some(45));
        assert_eq!(latency.polymarket_ms, Some(120));
    }

    #[test]
    fn test_reset() {
        let metrics = PerformanceMetrics::new();

        metrics.record(Operation::ArbitrageCalc, Duration::from_millis(10));
        metrics.reset();

        let report = metrics.report();
        let arb_stats = report
            .operations
            .iter()
            .find(|s| s.name == "Arbitrage Calculation")
            .unwrap();

        assert_eq!(arb_stats.count, 0);
    }
}
