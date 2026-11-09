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
use std::time::{Duration, Instant};

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

/// One latency sample: send `iters * ITERATIONS` messages from `cpu_send` to
/// a consumer pinned to `cpu_recv` and return the elapsed send-side time.
///
/// With `numa_alloc`, the sender pins itself first and builds the channel
/// with `SpscChannel::new_numa` (first-touch locality); otherwise it builds
/// the channel with `SpscChannel::new` and pins after spawning the consumer.
#[expect(
    clippy::unwrap_used,
    reason = "benchmark body: a failed channel split, CPU set or join is a bug that must abort the run"
)]
fn spsc_latency_sample(iters: u64, cpu_send: usize, cpu_recv: usize, numa_alloc: bool) -> Duration {
    if numa_alloc {
        // Construct on a pinned thread for first-touch locality.
        let cs = CpuSet::from_cpu(cpu_send).unwrap();
        let _ = set_thread_affinity(&cs);
    }

    let ch = if numa_alloc {
        SpscChannel::<u64>::new_numa(CAPACITY, 0)
    } else {
        SpscChannel::<u64>::new(CAPACITY)
    };
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

    if !numa_alloc {
        let cs = CpuSet::from_cpu(cpu_send).unwrap();
        let _ = set_thread_affinity(&cs);
    }
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
}

fn numa_latency(c: &mut Criterion) {
    let Ok(topo) = NumaTopology::discover() else {
        eprintln!("NUMA topology unavailable — skipping NUMA latency benchmarks");
        return;
    };

    let Some(node0_cpu) = first_cpu(&topo, 0) else {
        return;
    };

    // Two distinct CPUs on node 0 for same-node benchmarks (sender != receiver).
    let Some((node0_cpu_a, node0_cpu_b)) = two_cpus(&topo, 0) else {
        eprintln!("Node 0 has fewer than 2 CPUs — skipping same-node benchmarks");
        return;
    };

    let mut group = c.benchmark_group("numa_latency");
    group.sample_size(50);

    // --- Same-node ---
    group.bench_with_input(
        BenchmarkId::new("spsc", "same_node"),
        &(node0_cpu_a, node0_cpu_b),
        |b, &(cpu_send, cpu_recv)| {
            b.iter_custom(|iters| spsc_latency_sample(iters, cpu_send, cpu_recv, false));
        },
    );

    // --- Same-node with new_numa ---
    group.bench_with_input(
        BenchmarkId::new("spsc_numa_alloc", "same_node"),
        &(node0_cpu_a, node0_cpu_b),
        |b, &(cpu_send, cpu_recv)| {
            b.iter_custom(|iters| spsc_latency_sample(iters, cpu_send, cpu_recv, true));
        },
    );

    // --- Cross-node (requires >= 2 NUMA nodes) ---
    if topo.node_count() >= 2 {
        let Some(node1_cpu) = first_cpu(&topo, 1) else {
            group.finish();
            return;
        };

        group.bench_with_input(
            BenchmarkId::new("spsc", "cross_node"),
            &(node0_cpu, node1_cpu),
            |b, &(cpu0, cpu1)| {
                b.iter_custom(|iters| spsc_latency_sample(iters, cpu0, cpu1, false));
            },
        );

        group.bench_with_input(
            BenchmarkId::new("spsc_numa_alloc", "cross_node"),
            &(node0_cpu, node1_cpu),
            |b, &(cpu0, cpu1)| {
                // Build channel on node 0 thread.
                b.iter_custom(|iters| spsc_latency_sample(iters, cpu0, cpu1, true));
            },
        );
    } else {
        eprintln!("Single NUMA node — cross-node latency benchmarks skipped");
    }

    group.finish();
}

criterion_group!(benches, numa_latency);
criterion_main!(benches);
