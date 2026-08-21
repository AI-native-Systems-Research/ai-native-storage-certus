//! Benchmarks for channel throughput (SPSC and MPSC).
//!
//! For comprehensive multi-backend benchmarks comparing built-in, crossbeam,
//! kanal, rtrb, and tokio channels, see:
//! - `channel_spsc_benchmark.rs` (SPSC throughput)
//! - `channel_mpsc_benchmark.rs` (MPSC throughput)
//! - `channel_latency_benchmark.rs` (per-message latency)

use component_framework::channel::mpsc::MpscChannel;
use component_framework::channel::spsc::SpscChannel;
use criterion::{criterion_group, criterion_main, Criterion};
use std::thread;

fn spsc_throughput(c: &mut Criterion) {
    c.bench_function("spsc_1m_messages", |b| {
        b.iter(|| {
            let ch = SpscChannel::<u64>::new(4096);
            let tx = ch.sender().unwrap();
            let rx = ch.receiver().unwrap();

            let count = 100_000u64;

            let producer = thread::spawn(move || {
                for i in 0..count {
                    tx.send(i).unwrap();
                }
            });

            let consumer = thread::spawn(move || {
                let mut total = 0u64;
                for _ in 0..count {
                    total += rx.recv().unwrap();
                }
                total
            });

            producer.join().unwrap();
            let total = consumer.join().unwrap();
            assert_eq!(total, count * (count - 1) / 2);
        });
    });
}

fn mpsc_throughput(c: &mut Criterion) {
    c.bench_function("mpsc_8_producers_10k_each", |b| {
        b.iter(|| {
            let ch = MpscChannel::<u64>::new(4096);
            let rx = ch.receiver().unwrap();

            let mut handles = vec![];
            for pid in 0..8u64 {
                let tx = ch.sender().unwrap();
                handles.push(thread::spawn(move || {
                    for i in 0..10_000u64 {
                        tx.send(pid * 10_000 + i).unwrap();
                    }
                }));
            }

            let consumer = thread::spawn(move || {
                let mut count = 0u64;
                for _ in 0..80_000 {
                    let _ = rx.recv().unwrap();
                    count += 1;
                }
                count
            });

            for h in handles {
                h.join().unwrap();
            }

            let count = consumer.join().unwrap();
            assert_eq!(count, 80_000);
        });
    });
}

criterion_group!(benches, spsc_throughput, mpsc_throughput);
criterion_main!(benches);
