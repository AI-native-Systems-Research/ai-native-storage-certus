//! Telemetry-overhead benchmark for the inbound accept path (SC-006).
//!
//! SC-006 requires that enabling the `telemetry` feature adds less than 5%
//! overhead to the accept/disconnect path versus the disabled build. Telemetry
//! is a compile-time feature (a zero-sized no-op when off), so the on/off
//! comparison is done with two runs and Criterion's baseline machinery rather
//! than in a single process:
//!
//! ```bash
//! # Baseline: telemetry disabled (the no-op collector).
//! cargo bench -p remote-lookup-rdma-responder --bench connection_telemetry -- --save-baseline off
//!
//! # Candidate: telemetry enabled (atomic counters on the accept/disconnect path).
//! cargo bench -p remote-lookup-rdma-responder --features telemetry --bench connection_telemetry \
//!     -- --baseline off
//! ```
//!
//! The second run prints the percentage change per benchmark; SC-006 holds when
//! every case is within +5%.
//!
//! The benchmark drives [`ConnectionTable::accept`] + [`ConnectionTable::disconnect`]
//! over the public seam (a bench-local [`CmConnection`]), so it needs no RDMA
//! hardware and isolates the connection-table + telemetry work — which is where
//! every `record_*` call lives — from actual RDMA I/O. Mirrors the initiator's
//! `push_telemetry` benchmark.

use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use interfaces::PeerId;
use remote_lookup_rdma_responder::connection::{CmConnection, ConnectionTable};
use remote_lookup_rdma_responder::telemetry::TelemetryCollector;

/// A connection whose teardown does no hardware work.
struct BenchConn;

impl CmConnection for BenchConn {
    fn to_error(&self) {}
}

/// Run `n` accept→disconnect cycles against a fresh table.
fn accept_disconnect_cycle(n: usize) {
    let table_telemetry = Arc::new(TelemetryCollector::new());
    let mut table = ConnectionTable::new(table_telemetry);
    for i in 0..n {
        let node = format!("peer-{i}");
        let _ = table.accept(Some(node.clone().into_bytes()), Box::new(BenchConn));
        table.disconnect(&PeerId::new(node));
    }
}

fn bench_accept_disconnect(c: &mut Criterion) {
    let mut group = c.benchmark_group("accept_disconnect");
    for &n in &[1usize, 16, 64] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                accept_disconnect_cycle(black_box(n));
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_accept_disconnect);
criterion_main!(benches);
