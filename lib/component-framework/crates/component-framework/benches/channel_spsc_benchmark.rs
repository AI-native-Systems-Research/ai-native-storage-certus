//! SPSC throughput benchmarks comparing all SPSC-capable backends.
//!
//! Backends: built-in SpscChannel, CrossbeamBounded, CrossbeamUnbounded,
//! Kanal, rtrb.

use component_framework::channel::crossbeam_bounded::CrossbeamBoundedChannel;
use component_framework::channel::crossbeam_unbounded::CrossbeamUnboundedChannel;
use component_framework::channel::kanal_bounded::KanalChannel;
use component_framework::channel::rtrb_spsc::RtrbChannel;
use component_framework::channel::spsc::SpscChannel;
use component_framework::channel::{IReceiver, ISender};
use component_framework::iunknown::query;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::sync::Arc;
use std::thread;

const MSG_COUNT: u64 = 100_000;

// ---------------------------------------------------------------------------
// Helper: run a SPSC throughput benchmark with ISender/IReceiver
// ---------------------------------------------------------------------------
fn run_spsc_u64(
    tx: Arc<dyn ISender<u64> + Send + Sync>,
    rx: Arc<dyn IReceiver<u64> + Send + Sync>,
    count: u64,
) {
    let producer = thread::spawn(move || {
        for i in 0..count {
            tx.send(i).unwrap();
        }
    });
    let consumer = thread::spawn(move || {
        for _ in 0..count {
            rx.recv().unwrap();
        }
    });
    producer.join().unwrap();
    consumer.join().unwrap();
}

fn run_spsc_vec(
    tx: Arc<dyn ISender<Vec<u8>> + Send + Sync>,
    rx: Arc<dyn IReceiver<Vec<u8>> + Send + Sync>,
    count: u64,
) {
    let producer = thread::spawn(move || {
        for _ in 0..count {
            tx.send(vec![0u8; 1024]).unwrap();
        }
    });
    let consumer = thread::spawn(move || {
        for _ in 0..count {
            rx.recv().unwrap();
        }
    });
    producer.join().unwrap();
    consumer.join().unwrap();
}

// ---------------------------------------------------------------------------
// SPSC throughput — small messages (u64)
// ---------------------------------------------------------------------------
fn spsc_throughput_small(c: &mut Criterion) {
    let mut group = c.benchmark_group("spsc_throughput_u64");
    group.throughput(Throughput::Elements(MSG_COUNT));

    for capacity in [64, 1024, 16384] {
        // Built-in SPSC
        group.bench_with_input(
            BenchmarkId::new("builtin", capacity),
            &capacity,
            |b, &cap| {
                b.iter(|| {
                    let ch = SpscChannel::<u64>::new(cap);
                    let tx = query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
                    let rx = query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
                    run_spsc_u64(tx, rx, MSG_COUNT);
                });
            },
        );

        // Crossbeam bounded
        group.bench_with_input(
            BenchmarkId::new("crossbeam_bounded", capacity),
            &capacity,
            |b, &cap| {
                b.iter(|| {
                    let ch = CrossbeamBoundedChannel::<u64>::new(cap);
                    let tx = query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
                    let rx = query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
                    run_spsc_u64(tx, rx, MSG_COUNT);
                });
            },
        );

        // Crossbeam unbounded (capacity parameter ignored)
        group.bench_with_input(
            BenchmarkId::new("crossbeam_unbounded", capacity),
            &capacity,
            |b, _cap| {
                b.iter(|| {
                    let ch = CrossbeamUnboundedChannel::<u64>::new();
                    let tx = query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
                    let rx = query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
                    run_spsc_u64(tx, rx, MSG_COUNT);
                });
            },
        );

        // Kanal
        group.bench_with_input(BenchmarkId::new("kanal", capacity), &capacity, |b, &cap| {
            b.iter(|| {
                let ch = KanalChannel::<u64>::new(cap);
                let tx = query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
                let rx = query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
                run_spsc_u64(tx, rx, MSG_COUNT);
            });
        });

        // rtrb
        group.bench_with_input(BenchmarkId::new("rtrb", capacity), &capacity, |b, &cap| {
            b.iter(|| {
                let ch = RtrbChannel::<u64>::new(cap);
                let tx = query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
                let rx = query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
                run_spsc_u64(tx, rx, MSG_COUNT);
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// SPSC throughput — large messages (Vec<u8> 1024 bytes)
// ---------------------------------------------------------------------------
fn spsc_throughput_large(c: &mut Criterion) {
    let mut group = c.benchmark_group("spsc_throughput_vec1024");
    group.throughput(Throughput::Elements(MSG_COUNT));

    for capacity in [64, 1024, 16384] {
        group.bench_with_input(
            BenchmarkId::new("builtin", capacity),
            &capacity,
            |b, &cap| {
                b.iter(|| {
                    let ch = SpscChannel::<Vec<u8>>::new(cap);
                    let tx = query::<dyn ISender<Vec<u8>> + Send + Sync>(&ch).unwrap();
                    let rx = query::<dyn IReceiver<Vec<u8>> + Send + Sync>(&ch).unwrap();
                    run_spsc_vec(tx, rx, MSG_COUNT);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("crossbeam_bounded", capacity),
            &capacity,
            |b, &cap| {
                b.iter(|| {
                    let ch = CrossbeamBoundedChannel::<Vec<u8>>::new(cap);
                    let tx = query::<dyn ISender<Vec<u8>> + Send + Sync>(&ch).unwrap();
                    let rx = query::<dyn IReceiver<Vec<u8>> + Send + Sync>(&ch).unwrap();
                    run_spsc_vec(tx, rx, MSG_COUNT);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("crossbeam_unbounded", capacity),
            &capacity,
            |b, _cap| {
                b.iter(|| {
                    let ch = CrossbeamUnboundedChannel::<Vec<u8>>::new();
                    let tx = query::<dyn ISender<Vec<u8>> + Send + Sync>(&ch).unwrap();
                    let rx = query::<dyn IReceiver<Vec<u8>> + Send + Sync>(&ch).unwrap();
                    run_spsc_vec(tx, rx, MSG_COUNT);
                });
            },
        );

        group.bench_with_input(BenchmarkId::new("kanal", capacity), &capacity, |b, &cap| {
            b.iter(|| {
                let ch = KanalChannel::<Vec<u8>>::new(cap);
                let tx = query::<dyn ISender<Vec<u8>> + Send + Sync>(&ch).unwrap();
                let rx = query::<dyn IReceiver<Vec<u8>> + Send + Sync>(&ch).unwrap();
                run_spsc_vec(tx, rx, MSG_COUNT);
            });
        });

        group.bench_with_input(BenchmarkId::new("rtrb", capacity), &capacity, |b, &cap| {
            b.iter(|| {
                let ch = RtrbChannel::<Vec<u8>>::new(cap);
                let tx = query::<dyn ISender<Vec<u8>> + Send + Sync>(&ch).unwrap();
                let rx = query::<dyn IReceiver<Vec<u8>> + Send + Sync>(&ch).unwrap();
                run_spsc_vec(tx, rx, MSG_COUNT);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, spsc_throughput_small, spsc_throughput_large);
criterion_main!(benches);
