//! NUMA-aware throughput benchmarks.
//!
//! Measures messages-per-second for same-node and cross-node SPSC channel
//! configurations. A dedicated producer and consumer thread exchange a fixed
//! batch of messages, and throughput is derived from elapsed wall time.
//!
//! On single-NUMA systems the cross-node benchmarks are skipped gracefully.

use component_framework::channel::spsc::SpscChannel;
use component_framework::numa::{set_thread_affinity, CpuSet, NumaTopology};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::time::Instant;

const CAPACITY: usize = 1024;
const BATCH: u64 = 100_000;

/// Pick the first online CPU from a node's CPU set.
fn first_cpu(topo: &NumaTopology, node_id: usize) -> Option<usize> {
    topo.node(node_id).and_then(|n| n.cpus().iter().next())
}

fn numa_throughput(c: &mut Criterion) {
    let topo = match NumaTopology::discover() {
        Ok(t) => t,
        Err(_) => {
            eprintln!("NUMA topology unavailable — skipping NUMA throughput benchmarks");
            return;
        }
    };

    let node0_cpu = match first_cpu(&topo, 0) {
        Some(c) => c,
        None => return,
    };

    let mut group = c.benchmark_group("numa_throughput");
    group.throughput(Throughput::Elements(BATCH));
    group.sample_size(30);

    // --- Same-node ---
    group.bench_with_input(
        BenchmarkId::new("spsc", "same_node"),
        &node0_cpu,
        |b, &cpu| {
            b.iter_custom(|iters| {
                let ch = SpscChannel::<u64>::new(CAPACITY);
                let tx = ch.sender().unwrap();
                let rx = ch.receiver().unwrap();
                let total = iters * BATCH;

                let consumer = std::thread::spawn(move || {
                    let cs = CpuSet::from_cpu(cpu).unwrap();
                    let _ = set_thread_affinity(&cs);
                    let mut count = 0u64;
                    while count < total {
                        if rx.try_recv().is_ok() {
                            count += 1;
                        }
                    }
                });

                let cs = CpuSet::from_cpu(cpu).unwrap();
                let _ = set_thread_affinity(&cs);
                let start = Instant::now();
                for i in 0..total {
                    while tx.try_send(i).is_err() {
                        std::hint::spin_loop();
                    }
                }
                consumer.join().unwrap();
                start.elapsed()
            });
        },
    );

    // --- Same-node with new_numa ---
    group.bench_with_input(
        BenchmarkId::new("spsc_numa_alloc", "same_node"),
        &node0_cpu,
        |b, &cpu| {
            b.iter_custom(|iters| {
                let cs = CpuSet::from_cpu(cpu).unwrap();
                let _ = set_thread_affinity(&cs);

                let ch = SpscChannel::<u64>::new_numa(CAPACITY, 0);
                let tx = ch.sender().unwrap();
                let rx = ch.receiver().unwrap();
                let total = iters * BATCH;

                let consumer = std::thread::spawn(move || {
                    let cs = CpuSet::from_cpu(cpu).unwrap();
                    let _ = set_thread_affinity(&cs);
                    let mut count = 0u64;
                    while count < total {
                        if rx.try_recv().is_ok() {
                            count += 1;
                        }
                    }
                });

                let start = Instant::now();
                for i in 0..total {
                    while tx.try_send(i).is_err() {
                        std::hint::spin_loop();
                    }
                }
                consumer.join().unwrap();
                start.elapsed()
            });
        },
    );

    // --- Cross-node (requires >= 2 NUMA nodes) ---
    if topo.node_count() >= 2 {
        let node1_cpu = match first_cpu(&topo, 1) {
            Some(c) => c,
            None => {
                group.finish();
                return;
            }
        };

        group.bench_with_input(
            BenchmarkId::new("spsc", "cross_node"),
            &(node0_cpu, node1_cpu),
            |b, &(cpu0, cpu1)| {
                b.iter_custom(|iters| {
                    let ch = SpscChannel::<u64>::new(CAPACITY);
                    let tx = ch.sender().unwrap();
                    let rx = ch.receiver().unwrap();
                    let total = iters * BATCH;

                    let consumer = std::thread::spawn(move || {
                        let cs = CpuSet::from_cpu(cpu1).unwrap();
                        let _ = set_thread_affinity(&cs);
                        let mut count = 0u64;
                        while count < total {
                            if rx.try_recv().is_ok() {
                                count += 1;
                            }
                        }
                    });

                    let cs = CpuSet::from_cpu(cpu0).unwrap();
                    let _ = set_thread_affinity(&cs);
                    let start = Instant::now();
                    for i in 0..total {
                        while tx.try_send(i).is_err() {
                            std::hint::spin_loop();
                        }
                    }
                    consumer.join().unwrap();
                    start.elapsed()
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("spsc_numa_alloc", "cross_node"),
            &(node0_cpu, node1_cpu),
            |b, &(cpu0, cpu1)| {
                b.iter_custom(|iters| {
                    let cs = CpuSet::from_cpu(cpu0).unwrap();
                    let _ = set_thread_affinity(&cs);

                    let ch = SpscChannel::<u64>::new_numa(CAPACITY, 0);
                    let tx = ch.sender().unwrap();
                    let rx = ch.receiver().unwrap();
                    let total = iters * BATCH;

                    let consumer = std::thread::spawn(move || {
                        let cs = CpuSet::from_cpu(cpu1).unwrap();
                        let _ = set_thread_affinity(&cs);
                        let mut count = 0u64;
                        while count < total {
                            if rx.try_recv().is_ok() {
                                count += 1;
                            }
                        }
                    });

                    let start = Instant::now();
                    for i in 0..total {
                        while tx.try_send(i).is_err() {
                            std::hint::spin_loop();
                        }
                    }
                    consumer.join().unwrap();
                    start.elapsed()
                });
            },
        );
    } else {
        eprintln!("Single NUMA node — cross-node throughput benchmarks skipped");
    }

    group.finish();
}

criterion_group!(benches, numa_throughput);
criterion_main!(benches);
