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
use std::time::{Duration, Instant};

const CAPACITY: usize = 1024;
const BATCH: u64 = 100_000;

/// Pick the first online CPU from a node's CPU set.
fn first_cpu(topo: &NumaTopology, node_id: usize) -> Option<usize> {
    topo.node(node_id).and_then(|n| n.cpus().iter().next())
}

/// One throughput sample: stream `iters * BATCH` messages from `cpu_send` to
/// a consumer pinned to `cpu_recv` and return the elapsed time.
///
/// With `numa_alloc`, the sender pins itself first and builds the channel
/// with `SpscChannel::new_numa` (first-touch locality); otherwise it builds
/// the channel with `SpscChannel::new` and pins after spawning the consumer.
#[expect(
    clippy::unwrap_used,
    reason = "benchmark body: a failed channel split, CPU set or join is a bug that must abort the run"
)]
fn spsc_throughput_sample(
    iters: u64,
    cpu_send: usize,
    cpu_recv: usize,
    numa_alloc: bool,
) -> Duration {
    if numa_alloc {
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
    let total = iters * BATCH;

    let consumer = std::thread::spawn(move || {
        let cs = CpuSet::from_cpu(cpu_recv).unwrap();
        let _ = set_thread_affinity(&cs);
        let mut count = 0u64;
        while count < total {
            if rx.try_recv().is_ok() {
                count += 1;
            }
        }
    });

    if !numa_alloc {
        let cs = CpuSet::from_cpu(cpu_send).unwrap();
        let _ = set_thread_affinity(&cs);
    }
    let start = Instant::now();
    for i in 0..total {
        while tx.try_send(i).is_err() {
            std::hint::spin_loop();
        }
    }
    consumer.join().unwrap();
    start.elapsed()
}

fn numa_throughput(c: &mut Criterion) {
    let Ok(topo) = NumaTopology::discover() else {
        eprintln!("NUMA topology unavailable — skipping NUMA throughput benchmarks");
        return;
    };

    let Some(node0_cpu) = first_cpu(&topo, 0) else {
        return;
    };

    let mut group = c.benchmark_group("numa_throughput");
    group.throughput(Throughput::Elements(BATCH));
    group.sample_size(30);

    // --- Same-node ---
    group.bench_with_input(
        BenchmarkId::new("spsc", "same_node"),
        &node0_cpu,
        |b, &cpu| {
            b.iter_custom(|iters| spsc_throughput_sample(iters, cpu, cpu, false));
        },
    );

    // --- Same-node with new_numa ---
    group.bench_with_input(
        BenchmarkId::new("spsc_numa_alloc", "same_node"),
        &node0_cpu,
        |b, &cpu| {
            b.iter_custom(|iters| spsc_throughput_sample(iters, cpu, cpu, true));
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
                b.iter_custom(|iters| spsc_throughput_sample(iters, cpu0, cpu1, false));
            },
        );

        group.bench_with_input(
            BenchmarkId::new("spsc_numa_alloc", "cross_node"),
            &(node0_cpu, node1_cpu),
            |b, &(cpu0, cpu1)| {
                b.iter_custom(|iters| spsc_throughput_sample(iters, cpu0, cpu1, true));
            },
        );
    } else {
        eprintln!("Single NUMA node — cross-node throughput benchmarks skipped");
    }

    group.finish();
}

criterion_group!(benches, numa_throughput);
criterion_main!(benches);
