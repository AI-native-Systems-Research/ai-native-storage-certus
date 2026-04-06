//! MPSC throughput benchmarks comparing all MPSC-capable backends.
//!
//! Backends: built-in MpscChannel, CrossbeamBounded, CrossbeamUnbounded,
//! Kanal, TokioMpsc.
//! rtrb excluded (SPSC only).

use component_framework::channel::crossbeam_bounded::CrossbeamBoundedChannel;
use component_framework::channel::crossbeam_unbounded::CrossbeamUnboundedChannel;
use component_framework::channel::kanal_bounded::KanalChannel;
use component_framework::channel::mpsc::MpscChannel;
use component_framework::channel::tokio_mpsc::TokioMpscChannel;
use component_framework::channel::{IReceiver, ISender};
use component_framework::iunknown::query;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::sync::Arc;
use std::thread;

const MSG_COUNT: u64 = 100_000;

// ---------------------------------------------------------------------------
// Helper: run MPSC throughput benchmark with N producers
// ---------------------------------------------------------------------------
fn run_mpsc_u64(
    ch_sender: impl Fn() -> Arc<dyn ISender<u64> + Send + Sync>,
    rx: Arc<dyn IReceiver<u64> + Send + Sync>,
    total: u64,
    producers: u64,
) {
    let per_producer = total / producers;
    let mut handles = Vec::new();
    for _ in 0..producers {
        let tx = ch_sender();
        handles.push(thread::spawn(move || {
            for i in 0..per_producer {
                tx.send(i).unwrap();
            }
        }));
    }
    let consumer = thread::spawn(move || {
        for _ in 0..total {
            rx.recv().unwrap();
        }
    });
    for h in handles {
        h.join().unwrap();
    }
    consumer.join().unwrap();
}

fn run_mpsc_vec(
    ch_sender: impl Fn() -> Arc<dyn ISender<Vec<u8>> + Send + Sync>,
    rx: Arc<dyn IReceiver<Vec<u8>> + Send + Sync>,
    total: u64,
    producers: u64,
) {
    let per_producer = total / producers;
    let mut handles = Vec::new();
    for _ in 0..producers {
        let tx = ch_sender();
        handles.push(thread::spawn(move || {
            for _ in 0..per_producer {
                tx.send(vec![0u8; 1024]).unwrap();
            }
        }));
    }
    let consumer = thread::spawn(move || {
        for _ in 0..total {
            rx.recv().unwrap();
        }
    });
    for h in handles {
        h.join().unwrap();
    }
    consumer.join().unwrap();
}

// ---------------------------------------------------------------------------
// MPSC throughput — small messages (u64)
// ---------------------------------------------------------------------------
fn mpsc_throughput_small(c: &mut Criterion) {
    let mut group = c.benchmark_group("mpsc_throughput_u64");
    group.throughput(Throughput::Elements(MSG_COUNT));

    for producers in [2u64, 4, 8] {
        for capacity in [64usize, 1024, 16384] {
            let label = format!("{producers}p");

            // Built-in MPSC
            group.bench_with_input(
                BenchmarkId::new(format!("builtin/{label}"), capacity),
                &capacity,
                |b, &cap| {
                    b.iter(|| {
                        let ch = MpscChannel::<u64>::new(cap);
                        let rx = query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
                        run_mpsc_u64(
                            || query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap(),
                            rx,
                            MSG_COUNT,
                            producers,
                        );
                    });
                },
            );

            // Crossbeam bounded
            group.bench_with_input(
                BenchmarkId::new(format!("crossbeam_bounded/{label}"), capacity),
                &capacity,
                |b, &cap| {
                    b.iter(|| {
                        let ch = CrossbeamBoundedChannel::<u64>::new(cap);
                        let rx = query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
                        run_mpsc_u64(
                            || query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap(),
                            rx,
                            MSG_COUNT,
                            producers,
                        );
                    });
                },
            );

            // Crossbeam unbounded
            group.bench_with_input(
                BenchmarkId::new(format!("crossbeam_unbounded/{label}"), capacity),
                &capacity,
                |b, _cap| {
                    b.iter(|| {
                        let ch = CrossbeamUnboundedChannel::<u64>::new();
                        let rx = query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
                        run_mpsc_u64(
                            || query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap(),
                            rx,
                            MSG_COUNT,
                            producers,
                        );
                    });
                },
            );

            // Kanal
            group.bench_with_input(
                BenchmarkId::new(format!("kanal/{label}"), capacity),
                &capacity,
                |b, &cap| {
                    b.iter(|| {
                        let ch = KanalChannel::<u64>::new(cap);
                        let rx = query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
                        run_mpsc_u64(
                            || query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap(),
                            rx,
                            MSG_COUNT,
                            producers,
                        );
                    });
                },
            );

            // Tokio MPSC
            group.bench_with_input(
                BenchmarkId::new(format!("tokio/{label}"), capacity),
                &capacity,
                |b, &cap| {
                    b.iter(|| {
                        let ch = TokioMpscChannel::<u64>::new(cap);
                        let rx = query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();
                        run_mpsc_u64(
                            || query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap(),
                            rx,
                            MSG_COUNT,
                            producers,
                        );
                    });
                },
            );
        }
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// MPSC throughput — large messages (Vec<u8> 1024 bytes)
// ---------------------------------------------------------------------------
fn mpsc_throughput_large(c: &mut Criterion) {
    let mut group = c.benchmark_group("mpsc_throughput_vec1024");
    group.throughput(Throughput::Elements(MSG_COUNT));

    for producers in [2u64, 4, 8] {
        for capacity in [64usize, 1024, 16384] {
            let label = format!("{producers}p");

            group.bench_with_input(
                BenchmarkId::new(format!("builtin/{label}"), capacity),
                &capacity,
                |b, &cap| {
                    b.iter(|| {
                        let ch = MpscChannel::<Vec<u8>>::new(cap);
                        let rx = query::<dyn IReceiver<Vec<u8>> + Send + Sync>(&ch).unwrap();
                        run_mpsc_vec(
                            || query::<dyn ISender<Vec<u8>> + Send + Sync>(&ch).unwrap(),
                            rx,
                            MSG_COUNT,
                            producers,
                        );
                    });
                },
            );

            group.bench_with_input(
                BenchmarkId::new(format!("crossbeam_bounded/{label}"), capacity),
                &capacity,
                |b, &cap| {
                    b.iter(|| {
                        let ch = CrossbeamBoundedChannel::<Vec<u8>>::new(cap);
                        let rx = query::<dyn IReceiver<Vec<u8>> + Send + Sync>(&ch).unwrap();
                        run_mpsc_vec(
                            || query::<dyn ISender<Vec<u8>> + Send + Sync>(&ch).unwrap(),
                            rx,
                            MSG_COUNT,
                            producers,
                        );
                    });
                },
            );

            group.bench_with_input(
                BenchmarkId::new(format!("crossbeam_unbounded/{label}"), capacity),
                &capacity,
                |b, _cap| {
                    b.iter(|| {
                        let ch = CrossbeamUnboundedChannel::<Vec<u8>>::new();
                        let rx = query::<dyn IReceiver<Vec<u8>> + Send + Sync>(&ch).unwrap();
                        run_mpsc_vec(
                            || query::<dyn ISender<Vec<u8>> + Send + Sync>(&ch).unwrap(),
                            rx,
                            MSG_COUNT,
                            producers,
                        );
                    });
                },
            );

            group.bench_with_input(
                BenchmarkId::new(format!("kanal/{label}"), capacity),
                &capacity,
                |b, &cap| {
                    b.iter(|| {
                        let ch = KanalChannel::<Vec<u8>>::new(cap);
                        let rx = query::<dyn IReceiver<Vec<u8>> + Send + Sync>(&ch).unwrap();
                        run_mpsc_vec(
                            || query::<dyn ISender<Vec<u8>> + Send + Sync>(&ch).unwrap(),
                            rx,
                            MSG_COUNT,
                            producers,
                        );
                    });
                },
            );

            group.bench_with_input(
                BenchmarkId::new(format!("tokio/{label}"), capacity),
                &capacity,
                |b, &cap| {
                    b.iter(|| {
                        let ch = TokioMpscChannel::<Vec<u8>>::new(cap);
                        let rx = query::<dyn IReceiver<Vec<u8>> + Send + Sync>(&ch).unwrap();
                        run_mpsc_vec(
                            || query::<dyn ISender<Vec<u8>> + Send + Sync>(&ch).unwrap(),
                            rx,
                            MSG_COUNT,
                            producers,
                        );
                    });
                },
            );
        }
    }

    group.finish();
}

criterion_group!(benches, mpsc_throughput_small, mpsc_throughput_large);
criterion_main!(benches);
