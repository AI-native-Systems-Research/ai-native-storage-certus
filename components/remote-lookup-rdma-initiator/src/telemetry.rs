//! Optional telemetry for the outbound RDMA push path.
//!
//! Gated behind the `telemetry` feature. When the feature is disabled,
//! [`TelemetryCollector`] is a zero-sized type whose record/read methods are
//! inlined no-ops, so call sites need no `#[cfg]` and incur no cost.
//!
//! The metrics reflect this component's role as an initiator: outbound
//! connection establishment/repair, per-item push outcomes (mirroring
//! [`PushStatus`]), and bytes RDMA-written.

use interfaces::PushStatus;

#[cfg(feature = "telemetry")]
use std::sync::atomic::{AtomicU64, Ordering};

/// Telemetry collector for the remote request handler (push path).
///
/// All operations are lock-free (atomic counters).
#[cfg(feature = "telemetry")]
#[derive(Debug, Default)]
pub struct TelemetryCollector {
    connections_established: AtomicU64,
    connection_failures: AtomicU64,
    reconnects: AtomicU64,
    disconnects: AtomicU64,
    pushes: AtomicU64,
    items_success: AtomicU64,
    items_key_not_found: AtomicU64,
    items_size_mismatch: AtomicU64,
    items_unable_to_connect: AtomicU64,
    bytes_written: AtomicU64,
    total_push_duration_us: AtomicU64,
    // Per-phase connect timing (populated once per successful connect); used to
    // attribute cold-connect latency across resolve/route/handshake/MR-reg so we
    // can tell whether pool MR registration dominates.
    connect_samples: AtomicU64,
    resolve_addr_us: AtomicU64,
    resolve_route_us: AtomicU64,
    handshake_us: AtomicU64,
    mr_reg_us: AtomicU64,
}

#[cfg(feature = "telemetry")]
impl TelemetryCollector {
    /// Create a collector with all counters at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// An outbound connection was successfully established (initial or repair).
    pub fn record_connection_established(&self) {
        self.connections_established.fetch_add(1, Ordering::Relaxed);
    }

    /// An outbound connection attempt failed.
    pub fn record_connection_failed(&self) {
        self.connection_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// A queue pair in the error state (or a failed write) forced a reconnect.
    pub fn record_reconnect(&self) {
        self.reconnects.fetch_add(1, Ordering::Relaxed);
    }

    /// A live connection was torn down (via `disconnect`/`disconnect_all`).
    pub fn record_disconnect(&self) {
        self.disconnects.fetch_add(1, Ordering::Relaxed);
    }

    /// A `push` batch completed in `duration_us` microseconds.
    pub fn record_push(&self, duration_us: u64) {
        self.pushes.fetch_add(1, Ordering::Relaxed);
        self.total_push_duration_us
            .fetch_add(duration_us, Ordering::Relaxed);
    }

    /// Record the per-phase breakdown of one successful connect, in
    /// microseconds: rdma_cm address resolution, route resolution, the connect
    /// handshake (through `RDMA_CM_EVENT_ESTABLISHED`), and pool MR registration.
    pub fn record_connect_phases(
        &self,
        resolve_addr_us: u64,
        resolve_route_us: u64,
        handshake_us: u64,
        mr_reg_us: u64,
    ) {
        self.connect_samples.fetch_add(1, Ordering::Relaxed);
        self.resolve_addr_us
            .fetch_add(resolve_addr_us, Ordering::Relaxed);
        self.resolve_route_us
            .fetch_add(resolve_route_us, Ordering::Relaxed);
        self.handshake_us.fetch_add(handshake_us, Ordering::Relaxed);
        self.mr_reg_us.fetch_add(mr_reg_us, Ordering::Relaxed);
    }

    /// Record one item's outcome. `bytes` is the value size for a successful
    /// write, and ignored for every other status.
    pub fn record_item(&self, status: PushStatus, bytes: u64) {
        match status {
            PushStatus::Success => {
                self.items_success.fetch_add(1, Ordering::Relaxed);
                self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
            }
            PushStatus::KeyNotFound => {
                self.items_key_not_found.fetch_add(1, Ordering::Relaxed);
            }
            PushStatus::SizeMismatch => {
                self.items_size_mismatch.fetch_add(1, Ordering::Relaxed);
            }
            PushStatus::UnableToConnect => {
                self.items_unable_to_connect.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Successful outbound connection establishments (initial + repairs).
    pub fn connections_established(&self) -> u64 {
        self.connections_established.load(Ordering::Relaxed)
    }

    /// Failed outbound connection attempts.
    pub fn connection_failures(&self) -> u64 {
        self.connection_failures.load(Ordering::Relaxed)
    }

    /// Reconnects triggered by a queue-pair error or failed write.
    pub fn reconnects(&self) -> u64 {
        self.reconnects.load(Ordering::Relaxed)
    }

    /// Connections torn down via `disconnect`/`disconnect_all`.
    pub fn disconnects(&self) -> u64 {
        self.disconnects.load(Ordering::Relaxed)
    }

    /// Total `push` batches processed.
    pub fn pushes(&self) -> u64 {
        self.pushes.load(Ordering::Relaxed)
    }

    /// Items written successfully.
    pub fn items_success(&self) -> u64 {
        self.items_success.load(Ordering::Relaxed)
    }

    /// Items whose key was not present locally.
    pub fn items_key_not_found(&self) -> u64 {
        self.items_key_not_found.load(Ordering::Relaxed)
    }

    /// Items whose local size did not match the remote region length.
    pub fn items_size_mismatch(&self) -> u64 {
        self.items_size_mismatch.load(Ordering::Relaxed)
    }

    /// Items dropped because the host could not be connected.
    pub fn items_unable_to_connect(&self) -> u64 {
        self.items_unable_to_connect.load(Ordering::Relaxed)
    }

    /// Total bytes RDMA-written for successful items.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written.load(Ordering::Relaxed)
    }

    /// Average `push` batch duration in microseconds (0 if none yet).
    pub fn avg_push_duration_us(&self) -> u64 {
        let pushes = self.pushes.load(Ordering::Relaxed);
        if pushes == 0 {
            return 0;
        }
        self.total_push_duration_us.load(Ordering::Relaxed) / pushes
    }

    /// Bytes-written throughput over `elapsed_secs` (0.0 if non-positive).
    pub fn throughput_bytes_per_sec(&self, elapsed_secs: f64) -> f64 {
        if elapsed_secs <= 0.0 {
            return 0.0;
        }
        self.bytes_written.load(Ordering::Relaxed) as f64 / elapsed_secs
    }

    /// Number of connects with a recorded phase breakdown.
    pub fn connect_samples(&self) -> u64 {
        self.connect_samples.load(Ordering::Relaxed)
    }

    /// Average per-phase connect timing in microseconds
    /// `(resolve_addr, resolve_route, handshake, mr_reg)`; all 0 if no samples.
    pub fn avg_connect_phases_us(&self) -> (u64, u64, u64, u64) {
        let n = self.connect_samples.load(Ordering::Relaxed);
        if n == 0 {
            return (0, 0, 0, 0);
        }
        (
            self.resolve_addr_us.load(Ordering::Relaxed) / n,
            self.resolve_route_us.load(Ordering::Relaxed) / n,
            self.handshake_us.load(Ordering::Relaxed) / n,
            self.mr_reg_us.load(Ordering::Relaxed) / n,
        )
    }
}

/// No-op telemetry when the `telemetry` feature is disabled: a zero-sized type
/// whose methods compile away. The method surface matches the enabled variant
/// so call sites are identical regardless of feature.
#[cfg(not(feature = "telemetry"))]
#[derive(Debug, Default)]
pub struct TelemetryCollector;

#[cfg(not(feature = "telemetry"))]
#[allow(clippy::unused_self)]
impl TelemetryCollector {
    /// Create the (zero-sized) no-op collector.
    pub fn new() -> Self {
        Self
    }

    pub fn record_connection_established(&self) {}
    pub fn record_connection_failed(&self) {}
    pub fn record_reconnect(&self) {}
    pub fn record_disconnect(&self) {}
    pub fn record_push(&self, _duration_us: u64) {}
    pub fn record_item(&self, _status: PushStatus, _bytes: u64) {}
    pub fn record_connect_phases(
        &self,
        _resolve_addr_us: u64,
        _resolve_route_us: u64,
        _handshake_us: u64,
        _mr_reg_us: u64,
    ) {
    }

    pub fn connections_established(&self) -> u64 {
        0
    }
    pub fn connection_failures(&self) -> u64 {
        0
    }
    pub fn reconnects(&self) -> u64 {
        0
    }
    pub fn disconnects(&self) -> u64 {
        0
    }
    pub fn pushes(&self) -> u64 {
        0
    }
    pub fn items_success(&self) -> u64 {
        0
    }
    pub fn items_key_not_found(&self) -> u64 {
        0
    }
    pub fn items_size_mismatch(&self) -> u64 {
        0
    }
    pub fn items_unable_to_connect(&self) -> u64 {
        0
    }
    pub fn bytes_written(&self) -> u64 {
        0
    }
    pub fn avg_push_duration_us(&self) -> u64 {
        0
    }
    pub fn throughput_bytes_per_sec(&self, _elapsed_secs: f64) -> f64 {
        0.0
    }
    pub fn connect_samples(&self) -> u64 {
        0
    }
    pub fn avg_connect_phases_us(&self) -> (u64, u64, u64, u64) {
        (0, 0, 0, 0)
    }
}

#[cfg(test)]
#[cfg(feature = "telemetry")]
mod tests {
    use super::*;

    #[test]
    fn connection_counters() {
        let tc = TelemetryCollector::new();
        tc.record_connection_established();
        tc.record_connection_established();
        tc.record_connection_failed();
        tc.record_reconnect();
        tc.record_disconnect();
        assert_eq!(tc.connections_established(), 2);
        assert_eq!(tc.connection_failures(), 1);
        assert_eq!(tc.reconnects(), 1);
        assert_eq!(tc.disconnects(), 1);
    }

    #[test]
    fn item_outcomes_and_bytes() {
        let tc = TelemetryCollector::new();
        tc.record_item(PushStatus::Success, 4096);
        tc.record_item(PushStatus::Success, 2048);
        tc.record_item(PushStatus::KeyNotFound, 0);
        tc.record_item(PushStatus::SizeMismatch, 0);
        tc.record_item(PushStatus::UnableToConnect, 0);
        assert_eq!(tc.items_success(), 2);
        assert_eq!(tc.items_key_not_found(), 1);
        assert_eq!(tc.items_size_mismatch(), 1);
        assert_eq!(tc.items_unable_to_connect(), 1);
        assert_eq!(tc.bytes_written(), 6144);
    }

    #[test]
    fn connect_phase_averages() {
        let tc = TelemetryCollector::new();
        assert_eq!(tc.avg_connect_phases_us(), (0, 0, 0, 0));
        tc.record_connect_phases(10, 20, 30, 1000);
        tc.record_connect_phases(30, 40, 50, 3000);
        assert_eq!(tc.connect_samples(), 2);
        // Averages, with MR-reg dominating — the signal we care about.
        assert_eq!(tc.avg_connect_phases_us(), (20, 30, 40, 2000));
    }

    #[test]
    fn push_duration_and_throughput() {
        let tc = TelemetryCollector::new();
        tc.record_push(100);
        tc.record_push(200);
        tc.record_item(PushStatus::Success, 1_000_000);
        assert_eq!(tc.pushes(), 2);
        assert_eq!(tc.avg_push_duration_us(), 150);
        let tp = tc.throughput_bytes_per_sec(1.0);
        assert!((tp - 1_000_000.0).abs() < 0.1);
    }
}
