//! Feature-gated telemetry collection for IO operations.
//!
//! When compiled with `--features telemetry`, the actor records
//! per-operation latency and throughput statistics using atomic counters.
//! When compiled without the feature, the telemetry API returns an error.
//!
//! The [`TelemetrySnapshot`] data type is defined in the `interfaces` crate.

use interfaces::{NvmeBlockError, TelemetrySnapshot};
#[cfg(feature = "telemetry")]
use interfaces::{ReadWriteStats, IO_SIZE_BUCKETS};

/// Internal telemetry collector using atomic counters.
///
/// Only available when the `telemetry` feature is enabled.
#[cfg(feature = "telemetry")]
pub(crate) struct TelemetryStats {
    total_ops: std::sync::atomic::AtomicU64,
    min_latency_ns: std::sync::atomic::AtomicU64,
    max_latency_ns: std::sync::atomic::AtomicU64,
    sum_latency_ns: std::sync::atomic::AtomicU64,
    total_bytes: std::sync::atomic::AtomicU64,
    start_time: std::time::Instant,
    // Per-direction counters. total_ops/total_bytes/sum_latency_ns above remain
    // the read+write aggregate used by snapshot(); these split the same events
    // so callers can account read vs write traffic (and per-direction latency).
    read_ops: std::sync::atomic::AtomicU64,
    read_bytes: std::sync::atomic::AtomicU64,
    read_latency_ns_sum: std::sync::atomic::AtomicU64,
    write_ops: std::sync::atomic::AtomicU64,
    write_bytes: std::sync::atomic::AtomicU64,
    write_latency_ns_sum: std::sync::atomic::AtomicU64,
    // Per-transfer-size histograms (one bucket per power-of-two size band; see
    // interfaces::IO_SIZE_BUCKETS). Each completed IO bumps exactly one bucket
    // on its direction's array, giving the on-device block-size distribution.
    read_size_buckets: [std::sync::atomic::AtomicU64; IO_SIZE_BUCKETS],
    write_size_buckets: [std::sync::atomic::AtomicU64; IO_SIZE_BUCKETS],
}

#[cfg(feature = "telemetry")]
impl TelemetryStats {
    /// Create a new telemetry collector.
    pub fn new() -> Self {
        Self {
            total_ops: std::sync::atomic::AtomicU64::new(0),
            min_latency_ns: std::sync::atomic::AtomicU64::new(u64::MAX),
            max_latency_ns: std::sync::atomic::AtomicU64::new(0),
            sum_latency_ns: std::sync::atomic::AtomicU64::new(0),
            total_bytes: std::sync::atomic::AtomicU64::new(0),
            start_time: std::time::Instant::now(),
            read_ops: std::sync::atomic::AtomicU64::new(0),
            read_bytes: std::sync::atomic::AtomicU64::new(0),
            read_latency_ns_sum: std::sync::atomic::AtomicU64::new(0),
            write_ops: std::sync::atomic::AtomicU64::new(0),
            write_bytes: std::sync::atomic::AtomicU64::new(0),
            write_latency_ns_sum: std::sync::atomic::AtomicU64::new(0),
            read_size_buckets: std::array::from_fn(|_| std::sync::atomic::AtomicU64::new(0)),
            write_size_buckets: std::array::from_fn(|_| std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Record a completed IO operation.
    ///
    /// `latency_ns` is the operation latency in nanoseconds.
    /// `bytes` is the number of bytes transferred.
    /// `is_read` routes the op/byte/latency counts to the read or write side.
    pub fn record(&self, latency_ns: u64, bytes: u64, is_read: bool) {
        use std::sync::atomic::Ordering::Relaxed;

        self.total_ops.fetch_add(1, Relaxed);
        self.sum_latency_ns.fetch_add(latency_ns, Relaxed);
        self.total_bytes.fetch_add(bytes, Relaxed);

        let bucket = ReadWriteStats::size_bucket(bytes);
        if is_read {
            self.read_ops.fetch_add(1, Relaxed);
            self.read_bytes.fetch_add(bytes, Relaxed);
            self.read_latency_ns_sum.fetch_add(latency_ns, Relaxed);
            self.read_size_buckets[bucket].fetch_add(1, Relaxed);
        } else {
            self.write_ops.fetch_add(1, Relaxed);
            self.write_bytes.fetch_add(bytes, Relaxed);
            self.write_latency_ns_sum.fetch_add(latency_ns, Relaxed);
            self.write_size_buckets[bucket].fetch_add(1, Relaxed);
        }

        // Update min with CAS loop.
        let mut current = self.min_latency_ns.load(Relaxed);
        while latency_ns < current {
            match self
                .min_latency_ns
                .compare_exchange_weak(current, latency_ns, Relaxed, Relaxed)
            {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }

        // Update max with CAS loop.
        let mut current = self.max_latency_ns.load(Relaxed);
        while latency_ns > current {
            match self
                .max_latency_ns
                .compare_exchange_weak(current, latency_ns, Relaxed, Relaxed)
            {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// Take a snapshot of the current telemetry state.
    pub fn snapshot(&self) -> TelemetrySnapshot {
        use std::sync::atomic::Ordering::Relaxed;

        let total_ops = self.total_ops.load(Relaxed);
        let min_latency_ns = self.min_latency_ns.load(Relaxed);
        let max_latency_ns = self.max_latency_ns.load(Relaxed);
        let sum_latency_ns = self.sum_latency_ns.load(Relaxed);
        let total_bytes = self.total_bytes.load(Relaxed);
        let elapsed = self.start_time.elapsed();
        let elapsed_secs = elapsed.as_secs_f64();

        let mean_latency_ns = if total_ops > 0 {
            sum_latency_ns / total_ops
        } else {
            0
        };

        let mean_throughput_mbps = if elapsed_secs > 0.0 {
            (total_bytes as f64) / (1024.0 * 1024.0) / elapsed_secs
        } else {
            0.0
        };

        // Normalize min when no operations recorded.
        let min_latency_ns = if total_ops == 0 { 0 } else { min_latency_ns };

        TelemetrySnapshot {
            total_ops,
            min_latency_ns,
            max_latency_ns,
            mean_latency_ns,
            mean_throughput_mbps,
            elapsed_secs,
        }
    }

    /// Snapshot the per-direction read/write byte, op, and latency counters.
    pub fn read_write_stats(&self) -> ReadWriteStats {
        use std::sync::atomic::Ordering::Relaxed;
        ReadWriteStats {
            read_ops: self.read_ops.load(Relaxed),
            read_bytes: self.read_bytes.load(Relaxed),
            read_latency_ns_sum: self.read_latency_ns_sum.load(Relaxed),
            write_ops: self.write_ops.load(Relaxed),
            write_bytes: self.write_bytes.load(Relaxed),
            write_latency_ns_sum: self.write_latency_ns_sum.load(Relaxed),
            read_size_buckets: std::array::from_fn(|i| self.read_size_buckets[i].load(Relaxed)),
            write_size_buckets: std::array::from_fn(|i| self.write_size_buckets[i].load(Relaxed)),
        }
    }
}

/// Return a telemetry snapshot, or an error if the feature is disabled.
///
/// This is the implementation behind `IBlockDevice::telemetry()`.
#[cfg(feature = "telemetry")]
pub(crate) fn get_telemetry(stats: &TelemetryStats) -> Result<TelemetrySnapshot, NvmeBlockError> {
    Ok(stats.snapshot())
}

/// Return an error when the telemetry feature is not compiled in.
#[cfg(not(feature = "telemetry"))]
pub(crate) fn telemetry_not_available() -> Result<TelemetrySnapshot, NvmeBlockError> {
    Err(NvmeBlockError::FeatureNotEnabled(
        "compile with --features telemetry to enable IO statistics collection".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_default_values() {
        let snap = TelemetrySnapshot {
            total_ops: 0,
            min_latency_ns: 0,
            max_latency_ns: 0,
            mean_latency_ns: 0,
            mean_throughput_mbps: 0.0,
            elapsed_secs: 0.0,
        };
        assert_eq!(snap.total_ops, 0);
        assert_eq!(snap.min_latency_ns, 0);
    }

    #[test]
    fn snapshot_clone() {
        let snap = TelemetrySnapshot {
            total_ops: 100,
            min_latency_ns: 500,
            max_latency_ns: 10_000,
            mean_latency_ns: 2_000,
            mean_throughput_mbps: 500.0,
            elapsed_secs: 1.0,
        };
        let snap2 = snap.clone();
        assert_eq!(snap2.total_ops, 100);
        assert_eq!(snap2.mean_latency_ns, 2_000);
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn stats_record_single_op() {
        let stats = TelemetryStats::new();
        stats.record(1000, 4096);
        let snap = stats.snapshot();
        assert_eq!(snap.total_ops, 1);
        assert_eq!(snap.min_latency_ns, 1000);
        assert_eq!(snap.max_latency_ns, 1000);
        assert_eq!(snap.mean_latency_ns, 1000);
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn stats_record_multiple_ops() {
        let stats = TelemetryStats::new();
        stats.record(1000, 4096);
        stats.record(3000, 4096);
        stats.record(2000, 4096);
        let snap = stats.snapshot();
        assert_eq!(snap.total_ops, 3);
        assert_eq!(snap.min_latency_ns, 1000);
        assert_eq!(snap.max_latency_ns, 3000);
        assert_eq!(snap.mean_latency_ns, 2000);
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn stats_empty_snapshot() {
        let stats = TelemetryStats::new();
        let snap = stats.snapshot();
        assert_eq!(snap.total_ops, 0);
        assert_eq!(snap.min_latency_ns, 0);
        assert_eq!(snap.max_latency_ns, 0);
        assert_eq!(snap.mean_latency_ns, 0);
    }

    #[cfg(not(feature = "telemetry"))]
    #[test]
    fn telemetry_not_available_returns_error() {
        let result = telemetry_not_available();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            NvmeBlockError::FeatureNotEnabled(_)
        ));
    }
}
