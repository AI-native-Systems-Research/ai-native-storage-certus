//! Optional telemetry collection for connection rates and throughput.
//!
//! Gated behind the `telemetry` feature flag. When disabled, all metric
//! collection is compiled out with zero overhead.

#[cfg(feature = "telemetry")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "telemetry")]
use std::time::Instant;

/// Telemetry collector for the remote request handler.
///
/// Tracks connection rates, batch processing counts, and data transfer metrics.
/// All operations are lock-free (atomic counters).
#[cfg(feature = "telemetry")]
pub struct TelemetryCollector {
    connections_accepted: AtomicU64,
    connections_rejected: AtomicU64,
    batches_processed: AtomicU64,
    entries_resolved: AtomicU64,
    entries_failed: AtomicU64,
    bytes_transferred: AtomicU64,
    total_batch_duration_us: AtomicU64,
}

#[cfg(feature = "telemetry")]
impl TelemetryCollector {
    pub fn new() -> Self {
        Self {
            connections_accepted: AtomicU64::new(0),
            connections_rejected: AtomicU64::new(0),
            batches_processed: AtomicU64::new(0),
            entries_resolved: AtomicU64::new(0),
            entries_failed: AtomicU64::new(0),
            bytes_transferred: AtomicU64::new(0),
            total_batch_duration_us: AtomicU64::new(0),
        }
    }

    pub fn record_connection_accepted(&self) {
        self.connections_accepted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_connection_rejected(&self) {
        self.connections_rejected.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_batch(&self, entries_ok: u64, entries_err: u64, bytes: u64, duration_us: u64) {
        self.batches_processed.fetch_add(1, Ordering::Relaxed);
        self.entries_resolved
            .fetch_add(entries_ok, Ordering::Relaxed);
        self.entries_failed
            .fetch_add(entries_err, Ordering::Relaxed);
        self.bytes_transferred.fetch_add(bytes, Ordering::Relaxed);
        self.total_batch_duration_us
            .fetch_add(duration_us, Ordering::Relaxed);
    }

    pub fn connections_accepted(&self) -> u64 {
        self.connections_accepted.load(Ordering::Relaxed)
    }

    pub fn connections_rejected(&self) -> u64 {
        self.connections_rejected.load(Ordering::Relaxed)
    }

    pub fn batches_processed(&self) -> u64 {
        self.batches_processed.load(Ordering::Relaxed)
    }

    pub fn entries_resolved(&self) -> u64 {
        self.entries_resolved.load(Ordering::Relaxed)
    }

    pub fn entries_failed(&self) -> u64 {
        self.entries_failed.load(Ordering::Relaxed)
    }

    pub fn bytes_transferred(&self) -> u64 {
        self.bytes_transferred.load(Ordering::Relaxed)
    }

    pub fn avg_batch_duration_us(&self) -> u64 {
        let batches = self.batches_processed.load(Ordering::Relaxed);
        if batches == 0 {
            return 0;
        }
        self.total_batch_duration_us.load(Ordering::Relaxed) / batches
    }

    pub fn throughput_bytes_per_sec(&self, elapsed_secs: f64) -> f64 {
        if elapsed_secs <= 0.0 {
            return 0.0;
        }
        self.bytes_transferred.load(Ordering::Relaxed) as f64 / elapsed_secs
    }
}

#[cfg(feature = "telemetry")]
impl Default for TelemetryCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// No-op telemetry when feature is disabled.
#[cfg(not(feature = "telemetry"))]
#[derive(Default)]
pub struct TelemetryCollector;

#[cfg(not(feature = "telemetry"))]
impl TelemetryCollector {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(test)]
#[cfg(feature = "telemetry")]
mod tests {
    use super::*;

    #[test]
    fn counter_increments() {
        let tc = TelemetryCollector::new();
        tc.record_connection_accepted();
        tc.record_connection_accepted();
        tc.record_connection_rejected();
        assert_eq!(tc.connections_accepted(), 2);
        assert_eq!(tc.connections_rejected(), 1);
    }

    #[test]
    fn batch_metrics() {
        let tc = TelemetryCollector::new();
        tc.record_batch(10, 2, 4096, 100);
        tc.record_batch(8, 0, 2048, 200);
        assert_eq!(tc.batches_processed(), 2);
        assert_eq!(tc.entries_resolved(), 18);
        assert_eq!(tc.entries_failed(), 2);
        assert_eq!(tc.bytes_transferred(), 6144);
        assert_eq!(tc.avg_batch_duration_us(), 150);
    }

    #[test]
    fn throughput_calculation() {
        let tc = TelemetryCollector::new();
        tc.record_batch(10, 0, 1_000_000, 1000);
        let tp = tc.throughput_bytes_per_sec(1.0);
        assert!((tp - 1_000_000.0).abs() < 0.1);
    }
}
