//! Feature-gated telemetry for IO statistics collection.

#[cfg(feature = "telemetry")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "telemetry")]
use std::time::Instant;

#[cfg(feature = "telemetry")]
use interfaces::{NvmeBlockError, TelemetrySnapshot};

/// Telemetry statistics collector (only available with `telemetry` feature).
#[cfg(feature = "telemetry")]
pub struct TelemetryStats {
    total_ops: AtomicU64,
    min_latency_ns: AtomicU64,
    max_latency_ns: AtomicU64,
    total_latency_ns: AtomicU64,
    total_bytes: AtomicU64,
    start: Instant,
}

#[cfg(feature = "telemetry")]
impl TelemetryStats {
    pub fn new() -> Self {
        Self {
            total_ops: AtomicU64::new(0),
            min_latency_ns: AtomicU64::new(u64::MAX),
            max_latency_ns: AtomicU64::new(0),
            total_latency_ns: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            start: Instant::now(),
        }
    }

    pub fn record_op(&self, latency_ns: u64, bytes: u64) {
        self.total_ops.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ns
            .fetch_add(latency_ns, Ordering::Relaxed);
        self.total_bytes.fetch_add(bytes, Ordering::Relaxed);

        let mut current_min = self.min_latency_ns.load(Ordering::Relaxed);
        while latency_ns < current_min {
            match self.min_latency_ns.compare_exchange_weak(
                current_min,
                latency_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_min = actual,
            }
        }

        let mut current_max = self.max_latency_ns.load(Ordering::Relaxed);
        while latency_ns > current_max {
            match self.max_latency_ns.compare_exchange_weak(
                current_max,
                latency_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_max = actual,
            }
        }
    }

    pub fn snapshot(&self) -> Result<TelemetrySnapshot, NvmeBlockError> {
        let total_ops = self.total_ops.load(Ordering::Relaxed);
        let elapsed = self.start.elapsed();
        let elapsed_secs = elapsed.as_secs_f64();

        let min_latency_ns = if total_ops == 0 {
            0
        } else {
            self.min_latency_ns.load(Ordering::Relaxed)
        };
        let max_latency_ns = self.max_latency_ns.load(Ordering::Relaxed);
        let total_latency_ns = self.total_latency_ns.load(Ordering::Relaxed);
        let total_bytes = self.total_bytes.load(Ordering::Relaxed);

        let mean_latency_ns = if total_ops > 0 {
            total_latency_ns / total_ops
        } else {
            0
        };

        let mean_throughput_mbps = if elapsed_secs > 0.0 {
            (total_bytes as f64) / (1024.0 * 1024.0) / elapsed_secs
        } else {
            0.0
        };

        Ok(TelemetrySnapshot {
            total_ops,
            min_latency_ns,
            max_latency_ns,
            mean_latency_ns,
            mean_throughput_mbps,
            elapsed_secs,
        })
    }
}
