//! Latency benchmarks: per-message send-to-receive time for all backends.
//!
//! Measures the time for a single send+recv round-trip in both SPSC and MPSC
//! configurations.

use component_framework::channel::crossbeam_bounded::CrossbeamBoundedChannel;
use component_framework::channel::crossbeam_unbounded::CrossbeamUnboundedChannel;
use component_framework::channel::kanal_bounded::KanalChannel;
use component_framework::channel::mpsc::MpscChannel;
use component_framework::channel::rtrb_spsc::RtrbChannel;
use component_framework::channel::spsc::SpscChannel;
use component_framework::channel::tokio_mpsc::TokioMpscChannel;
use component_framework::channel::{IReceiver, ISender};
use component_framework::iunknown::query;
use criterion::{criterion_group, criterion_main, Criterion};
use std::sync::Arc;

const CAPACITY: usize = 1024;

/// Measure single-message latency: send then recv on the same thread.
fn measure_latency(tx: &(dyn ISender<u64> + Send + Sync), rx: &(dyn IReceiver<u64> + Send + Sync)) {
    tx.send(42).unwrap();
    rx.recv().unwrap();
}

// ---------------------------------------------------------------------------
// SPSC latency
// ---------------------------------------------------------------------------
fn spsc_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("spsc_latency");

    group.bench_function("builtin", |b| {
        let ch = SpscChannel::<u64>::new(CAPACITY);
        let tx: Arc<dyn ISender<u64> + Send + Sync> =
            query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u64> + Send + Sync> =
            query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
        b.iter(|| measure_latency(&*tx, &*rx));
    });

    group.bench_function("crossbeam_bounded", |b| {
        let ch = CrossbeamBoundedChannel::<u64>::new(CAPACITY);
        let tx: Arc<dyn ISender<u64> + Send + Sync> =
            query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u64> + Send + Sync> =
            query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
        b.iter(|| measure_latency(&*tx, &*rx));
    });

    group.bench_function("crossbeam_unbounded", |b| {
        let ch = CrossbeamUnboundedChannel::<u64>::new();
        let tx: Arc<dyn ISender<u64> + Send + Sync> =
            query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u64> + Send + Sync> =
            query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
        b.iter(|| measure_latency(&*tx, &*rx));
    });

    group.bench_function("kanal", |b| {
        let ch = KanalChannel::<u64>::new(CAPACITY);
        let tx: Arc<dyn ISender<u64> + Send + Sync> =
            query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u64> + Send + Sync> =
            query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
        b.iter(|| measure_latency(&*tx, &*rx));
    });

    group.bench_function("rtrb", |b| {
        let ch = RtrbChannel::<u64>::new(CAPACITY);
        let tx: Arc<dyn ISender<u64> + Send + Sync> =
            query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u64> + Send + Sync> =
            query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
        b.iter(|| measure_latency(&*tx, &*rx));
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// MPSC latency (single-threaded send+recv, tests overhead of MPSC machinery)
// ---------------------------------------------------------------------------
fn mpsc_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("mpsc_latency");

    group.bench_function("builtin", |b| {
        let ch = MpscChannel::<u64>::new(CAPACITY);
        let tx: Arc<dyn ISender<u64> + Send + Sync> =
            query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u64> + Send + Sync> =
            query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
        b.iter(|| measure_latency(&*tx, &*rx));
    });

    group.bench_function("crossbeam_bounded", |b| {
        let ch = CrossbeamBoundedChannel::<u64>::new(CAPACITY);
        let tx: Arc<dyn ISender<u64> + Send + Sync> =
            query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u64> + Send + Sync> =
            query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
        b.iter(|| measure_latency(&*tx, &*rx));
    });

    group.bench_function("crossbeam_unbounded", |b| {
        let ch = CrossbeamUnboundedChannel::<u64>::new();
        let tx: Arc<dyn ISender<u64> + Send + Sync> =
            query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u64> + Send + Sync> =
            query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
        b.iter(|| measure_latency(&*tx, &*rx));
    });

    group.bench_function("kanal", |b| {
        let ch = KanalChannel::<u64>::new(CAPACITY);
        let tx: Arc<dyn ISender<u64> + Send + Sync> =
            query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u64> + Send + Sync> =
            query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
        b.iter(|| measure_latency(&*tx, &*rx));
    });

    group.bench_function("tokio", |b| {
        let ch = TokioMpscChannel::<u64>::new(CAPACITY);
        let tx: Arc<dyn ISender<u64> + Send + Sync> =
            query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
        let rx: Arc<dyn IReceiver<u64> + Send + Sync> =
            query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
        b.iter(|| measure_latency(&*tx, &*rx));
    });

    group.finish();
}

criterion_group!(benches, spsc_latency, mpsc_latency);
criterion_main!(benches);
