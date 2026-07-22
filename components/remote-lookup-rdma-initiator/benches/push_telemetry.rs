//! Telemetry-overhead benchmark for the outbound push path (spec-002 SC-004).
//!
//! SC-004 requires that enabling the `telemetry` feature adds less than 5%
//! overhead to `push` versus the disabled build. Telemetry is a compile-time
//! feature (a zero-sized no-op when off), so the on/off comparison is done with
//! two runs and Criterion's baseline machinery rather than in a single process:
//!
//! ```bash
//! # Baseline: telemetry disabled (the no-op collector).
//! cargo bench -p remote-lookup-rdma-initiator --bench push_telemetry -- --save-baseline off
//!
//! # Candidate: telemetry enabled (atomic counters on the push path).
//! cargo bench -p remote-lookup-rdma-initiator --features telemetry --bench push_telemetry \
//!     -- --baseline off
//! ```
//!
//! The second run prints the percentage change per benchmark; SC-004 holds when
//! every `push/*` case is within +5%.
//!
//! The benchmark drives [`ConnectionTable::push`] over a bench-local mock
//! transport (the public `RdmaTransport`/`RdmaConn` seam), so it needs no RDMA
//! hardware and isolates the connection-table + telemetry work — which is where
//! every `record_*` call lives — from actual RDMA I/O.

use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use interfaces::ILogger;
use remote_lookup_rdma_initiator::connection::{
    ConnectTiming, ConnectionTable, ItemPlan, RdmaConn, RdmaError, RdmaTransport,
};
use remote_lookup_rdma_initiator::telemetry::TelemetryCollector;

const ENDPOINT: &str = "10.0.0.1:5000";

/// A connection whose write always succeeds without touching hardware.
struct BenchConn;

impl RdmaConn for BenchConn {
    fn qp_healthy(&self) -> bool {
        true
    }

    unsafe fn write(
        &self,
        _local: *const u8,
        _len: usize,
        _remote_addr: u64,
        _rkey: u32,
    ) -> Result<(), RdmaError> {
        Ok(())
    }
}

/// A transport that hands out [`BenchConn`]s without an `rdma_cm` round trip.
struct BenchTransport;

impl RdmaTransport for BenchTransport {
    fn connect(
        &self,
        _addr: &str,
        _port: u16,
    ) -> Result<(Box<dyn RdmaConn>, ConnectTiming), RdmaError> {
        Ok((Box::new(BenchConn), ConnectTiming::default()))
    }
}

/// Discards all log output (the push path only logs on failure anyway).
struct NullLogger;

impl ILogger for NullLogger {
    fn info(&self, _msg: &str) {}
    fn warn(&self, _msg: &str) {}
    fn error(&self, _msg: &str) {}
    fn debug(&self, _msg: &str) {}
}

/// Build a batch of `n` successful writes (the mock transport ignores `local`).
fn make_batch(n: usize) -> Vec<ItemPlan> {
    (0..n)
        .map(|i| ItemPlan::Write {
            local: 0x1000 as *const u8,
            len: 4096,
            remote_addr: 0x2000 + (i as u64) * 4096,
            rkey: 7,
        })
        .collect()
}

fn bench_push(c: &mut Criterion) {
    let mut group = c.benchmark_group("push");
    for &n in &[1usize, 16, 64] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let table = ConnectionTable::new(
                Box::new(BenchTransport),
                Arc::new(TelemetryCollector::new()),
            );
            let logger = NullLogger;
            // Warm the slot so timed pushes exercise the steady-state reuse path
            // rather than a (mocked) connect on the first call.
            let _ = table.push(ENDPOINT, make_batch(1), &logger);
            b.iter(|| {
                let out = table
                    .push(ENDPOINT, make_batch(n), &logger)
                    .expect("push succeeds");
                black_box(out);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_push);
criterion_main!(benches);
