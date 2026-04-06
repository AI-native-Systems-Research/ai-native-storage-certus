//! NUMA-aware latency benchmarks.
//!
//! Measures per-message send-to-receive latency for same-node and cross-node
//! SPSC channel configurations. Each benchmark pins sender and receiver threads
//! to specific NUMA nodes and exchanges messages over a bounded channel.
//!
//! On single-NUMA systems the cross-node benchmarks are skipped gracefully.

use component_framework::channel::spsc::SpscChannel;
use component_framework::numa::{set_thread_affinity, CpuSet, NumaTopology};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

const CAPACITY: usize = 1024;
const ITERATIONS: u64 = 10_000;

/// Pick the first online CPU from a node's CPU set.
fn first_cpu(topo: &NumaTopology, node_id: usize) -> Option<usize> {
    topo.node(node_id).and_then(|n| n.cpus().iter().next())
}

/// Pick two distinct CPUs from the same NUMA node.
fn two_cpus(topo: &NumaTopology, node_id: usize) -> Option<(usize, usize)> {
    let mut it = topo.node(node_id)?.cpus().iter();
    let a = it.next()?;
    let b = it.next()?;
    Some((a, b))
}

fn numa_latency(c: &mut Criterion) {
    let topo = match NumaTopology::discover() {
        Ok(t) => t,
        Err(_) => {
            eprintln!("NUMA topology unavailable — skipping NUMA latency benchmarks");
            return;
        }
    };

    let node0_cpu = match first_cpu(&topo, 0) {
        Some(c) => c,
        None => return,
    };

    // Two distinct CPUs on node 0 for same-node benchmarks (sender != receiver).
    let (node0_cpu_a, node0_cpu_b) = match two_cpus(&topo, 0) {
        Some(pair) => pair,
        None => {
            eprintln!("Node 0 has fewer than 2 CPUs — skipping same-node benchmarks");
            return;
        }
    };

    let mut group = c.benchmark_group("numa_latency");
    group.sample_size(50);

    // --- Same-node ---
    group.bench_with_input(
        BenchmarkId::new("spsc", "same_node"),
        &(node0_cpu_a, node0_cpu_b),
        |b, &(cpu_send, cpu_recv)| {
            b.iter_custom(|iters| {
                let ch = SpscChannel::<u64>::new(CAPACITY);
                let tx = ch.sender().unwrap();
                let rx = ch.receiver().unwrap();

                let done = Arc::new(AtomicBool::new(false));
                let done2 = done.clone();
                let total_msgs = iters * ITERATIONS;

                let consumer = std::thread::spawn(move || {
                    let cs = CpuSet::from_cpu(cpu_recv).unwrap();
                    let _ = set_thread_affinity(&cs);
                    let mut count = 0u64;
                    while count < total_msgs {
                        if rx.try_recv().is_ok() {
                            count += 1;
                        }
                    }
                    done2.store(true, Ordering::Release);
                });

                let cs = CpuSet::from_cpu(cpu_send).unwrap();
                let _ = set_thread_affinity(&cs);
                let start = Instant::now();
                for _ in 0..total_msgs {
                    while tx.try_send(42).is_err() {
                        std::hint::spin_loop();
                    }
                }
                while !done.load(Ordering::Acquire) {
                    std::hint::spin_loop();
                }
                let elapsed = start.elapsed();
                consumer.join().unwrap();
                elapsed
            });
        },
    );

    // --- Same-node with new_numa ---
    group.bench_with_input(
        BenchmarkId::new("spsc_numa_alloc", "same_node"),
        &(node0_cpu_a, node0_cpu_b),
        |b, &(cpu_send, cpu_recv)| {
            b.iter_custom(|iters| {
                // Construct on a pinned thread for first-touch locality.
                let cs = CpuSet::from_cpu(cpu_send).unwrap();
                let _ = set_thread_affinity(&cs);

                let ch = SpscChannel::<u64>::new_numa(CAPACITY, 0);
                let tx = ch.sender().unwrap();
                let rx = ch.receiver().unwrap();

                let done = Arc::new(AtomicBool::new(false));
                let done2 = done.clone();
                let total_msgs = iters * ITERATIONS;

                let consumer = std::thread::spawn(move || {
                    let cs = CpuSet::from_cpu(cpu_recv).unwrap();
                    let _ = set_thread_affinity(&cs);
                    let mut count = 0u64;
                    while count < total_msgs {
                        if rx.try_recv().is_ok() {
                            count += 1;
                        }
                    }
                    done2.store(true, Ordering::Release);
                });

                let start = Instant::now();
                for _ in 0..total_msgs {
                    while tx.try_send(42).is_err() {
                        std::hint::spin_loop();
                    }
                }
                while !done.load(Ordering::Acquire) {
                    std::hint::spin_loop();
                }
                let elapsed = start.elapsed();
                consumer.join().unwrap();
                elapsed
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

                    let done = Arc::new(AtomicBool::new(false));
                    let done2 = done.clone();
                    let total_msgs = iters * ITERATIONS;

                    let consumer = std::thread::spawn(move || {
                        let cs = CpuSet::from_cpu(cpu1).unwrap();
                        let _ = set_thread_affinity(&cs);
                        let mut count = 0u64;
                        while count < total_msgs {
                            if rx.try_recv().is_ok() {
                                count += 1;
                            }
                        }
                        done2.store(true, Ordering::Release);
                    });

                    let cs = CpuSet::from_cpu(cpu0).unwrap();
                    let _ = set_thread_affinity(&cs);
                    let start = Instant::now();
                    for _ in 0..total_msgs {
                        while tx.try_send(42).is_err() {
                            std::hint::spin_loop();
                        }
                    }
                    while !done.load(Ordering::Acquire) {
                        std::hint::spin_loop();
                    }
                    let elapsed = start.elapsed();
                    consumer.join().unwrap();
                    elapsed
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("spsc_numa_alloc", "cross_node"),
            &(node0_cpu, node1_cpu),
            |b, &(cpu0, cpu1)| {
                b.iter_custom(|iters| {
                    // Build channel on node 0 thread.
                    let cs = CpuSet::from_cpu(cpu0).unwrap();
                    let _ = set_thread_affinity(&cs);

                    let ch = SpscChannel::<u64>::new_numa(CAPACITY, 0);
                    let tx = ch.sender().unwrap();
                    let rx = ch.receiver().unwrap();

                    let done = Arc::new(AtomicBool::new(false));
                    let done2 = done.clone();
                    let total_msgs = iters * ITERATIONS;

                    let consumer = std::thread::spawn(move || {
                        let cs = CpuSet::from_cpu(cpu1).unwrap();
                        let _ = set_thread_affinity(&cs);
                        let mut count = 0u64;
                        while count < total_msgs {
                            if rx.try_recv().is_ok() {
                                count += 1;
                            }
                        }
                        done2.store(true, Ordering::Release);
                    });

                    let start = Instant::now();
                    for _ in 0..total_msgs {
                        while tx.try_send(42).is_err() {
                            std::hint::spin_loop();
                        }
                    }
                    while !done.load(Ordering::Acquire) {
                        std::hint::spin_loop();
                    }
                    let elapsed = start.elapsed();
                    consumer.join().unwrap();
                    elapsed
                });
            },
        );
    } else {
        eprintln!("Single NUMA node — cross-node latency benchmarks skipped");
    }

    group.finish();
}

criterion_group!(benches, numa_latency);
criterion_main!(benches);
