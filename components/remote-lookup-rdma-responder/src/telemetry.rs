//! Optional telemetry for the inbound RDMA accept path.
//!
//! Gated behind the `telemetry` feature. When the feature is disabled,
//! [`TelemetryCollector`] is a zero-sized type whose record/read methods are
//! inlined no-ops, so call sites need no `#[cfg]` and incur no cost.
//!
//! The metrics reflect this component's role as the passive accept side:
//! inbound connections accepted, how many carried an identifiable peer
//! (`node: Some`) versus not (`node: None`), teardowns (disconnect-acks
//! emitted), and non-fatal accept-loop errors. Mirrors the initiator
//! (`remote-lookup-rdma-initiator`) for cross-component consistency.

#[cfg(feature = "telemetry")]
use std::sync::atomic::{AtomicU64, Ordering};

/// Telemetry collector for the RDMA responder (accept path).
///
/// All operations are lock-free (atomic counters).
#[cfg(feature = "telemetry")]
#[derive(Debug, Default)]
pub struct TelemetryCollector {
    connections_accepted: AtomicU64,
    connections_identified: AtomicU64,
    connections_unidentified: AtomicU64,
    teardowns: AtomicU64,
    accept_loop_errors: AtomicU64,
}

#[cfg(feature = "telemetry")]
impl TelemetryCollector {
    /// Create a collector with all counters at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// An inbound connection was accepted (identified or not).
    pub fn record_connection_accepted(&self) {
        self.connections_accepted.fetch_add(1, Ordering::Relaxed);
    }

    /// An accepted connection carried a valid zyre UUID (`node: Some`).
    pub fn record_connection_identified(&self) {
        self.connections_identified.fetch_add(1, Ordering::Relaxed);
    }

    /// An accepted connection had absent/malformed `private_data` (`node: None`).
    pub fn record_connection_unidentified(&self) {
        self.connections_unidentified
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A teardown completed — one `DisconnectAck` was emitted.
    pub fn record_teardown(&self) {
        self.teardowns.fetch_add(1, Ordering::Relaxed);
    }

    /// A non-fatal error occurred in the accept loop.
    pub fn record_accept_loop_error(&self) {
        self.accept_loop_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Total inbound connections accepted.
    pub fn connections_accepted(&self) -> u64 {
        self.connections_accepted.load(Ordering::Relaxed)
    }

    /// Accepted connections with an identifiable peer (`node: Some`).
    pub fn connections_identified(&self) -> u64 {
        self.connections_identified.load(Ordering::Relaxed)
    }

    /// Accepted connections without an identifiable peer (`node: None`).
    pub fn connections_unidentified(&self) -> u64 {
        self.connections_unidentified.load(Ordering::Relaxed)
    }

    /// Teardowns performed (disconnect-acks emitted).
    pub fn teardowns(&self) -> u64 {
        self.teardowns.load(Ordering::Relaxed)
    }

    /// Non-fatal accept-loop errors.
    pub fn accept_loop_errors(&self) -> u64 {
        self.accept_loop_errors.load(Ordering::Relaxed)
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

    pub fn record_connection_accepted(&self) {}
    pub fn record_connection_identified(&self) {}
    pub fn record_connection_unidentified(&self) {}
    pub fn record_teardown(&self) {}
    pub fn record_accept_loop_error(&self) {}

    pub fn connections_accepted(&self) -> u64 {
        0
    }
    pub fn connections_identified(&self) -> u64 {
        0
    }
    pub fn connections_unidentified(&self) -> u64 {
        0
    }
    pub fn teardowns(&self) -> u64 {
        0
    }
    pub fn accept_loop_errors(&self) -> u64 {
        0
    }
}

#[cfg(test)]
#[cfg(feature = "telemetry")]
mod tests {
    use super::*;

    #[test]
    fn counters_advance() {
        let tc = TelemetryCollector::new();
        tc.record_connection_accepted();
        tc.record_connection_accepted();
        tc.record_connection_identified();
        tc.record_connection_unidentified();
        tc.record_teardown();
        tc.record_accept_loop_error();
        assert_eq!(tc.connections_accepted(), 2);
        assert_eq!(tc.connections_identified(), 1);
        assert_eq!(tc.connections_unidentified(), 1);
        assert_eq!(tc.teardowns(), 1);
        assert_eq!(tc.accept_loop_errors(), 1);
    }
}
