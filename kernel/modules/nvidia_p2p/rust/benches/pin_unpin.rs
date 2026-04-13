//! Criterion benchmarks for pin/unpin latency.
//!
//! Requires: NVIDIA GPU + CUDA runtime + loaded kernel module + root access.
//! Skips gracefully if prerequisites are missing.

use criterion::{criterion_group, criterion_main, Criterion};

mod cuda_helpers;

use cuda_helpers::check_prerequisites;
use nvidia_p2p_pin::NvP2pDevice;

const SIZE_64KB: usize = 65536;
const SIZE_1MB: usize = 1024 * 1024;
const SIZE_16MB: usize = 16 * 1024 * 1024;

fn bench_pin_latency(c: &mut Criterion) {
    let runtime = match check_prerequisites() {
        Ok(rt) => rt,
        Err(reason) => {
            eprintln!("Skipping pin benchmarks: {}", reason);
            return;
        }
    };

    let device = NvP2pDevice::open().expect("failed to open /dev/nvidia_p2p");

    let mut group = c.benchmark_group("pin_latency");

    for (label, size) in [("64KB", SIZE_64KB), ("1MB", SIZE_1MB), ("16MB", SIZE_16MB)] {
        let cuda_mem = runtime.malloc(size).expect("cudaMalloc failed");

        group.bench_function(label, |b| {
            b.iter(|| {
                let pinned = device
                    .pin_gpu_memory(cuda_mem.devptr(), size as u64)
                    .expect("pin failed");
                pinned.unpin().expect("unpin failed");
            });
        });

        drop(cuda_mem);
    }

    group.finish();
}

fn bench_unpin_latency(c: &mut Criterion) {
    let runtime = match check_prerequisites() {
        Ok(rt) => rt,
        Err(reason) => {
            eprintln!("Skipping unpin benchmarks: {}", reason);
            return;
        }
    };

    let device = NvP2pDevice::open().expect("failed to open /dev/nvidia_p2p");

    let mut group = c.benchmark_group("unpin_latency");

    for (label, size) in [("64KB", SIZE_64KB), ("1MB", SIZE_1MB), ("16MB", SIZE_16MB)] {
        let cuda_mem = runtime.malloc(size).expect("cudaMalloc failed");

        group.bench_function(label, |b| {
            b.iter_custom(|iters| {
                let mut total = std::time::Duration::ZERO;
                for _ in 0..iters {
                    let pinned = device
                        .pin_gpu_memory(cuda_mem.devptr(), size as u64)
                        .expect("pin failed");
                    let start = std::time::Instant::now();
                    pinned.unpin().expect("unpin failed");
                    total += start.elapsed();
                }
                total
            });
        });

        drop(cuda_mem);
    }

    group.finish();
}

fn bench_pin_unpin_roundtrip(c: &mut Criterion) {
    let runtime = match check_prerequisites() {
        Ok(rt) => rt,
        Err(reason) => {
            eprintln!("Skipping roundtrip benchmarks: {}", reason);
            return;
        }
    };

    let device = NvP2pDevice::open().expect("failed to open /dev/nvidia_p2p");
    let cuda_mem = runtime.malloc(SIZE_1MB).expect("cudaMalloc failed");

    c.bench_function("pin_unpin_roundtrip_1MB", |b| {
        b.iter(|| {
            let pinned = device
                .pin_gpu_memory(cuda_mem.devptr(), SIZE_1MB as u64)
                .expect("pin failed");
            pinned.unpin().expect("unpin failed");
        });
    });
}

criterion_group!(
    benches,
    bench_pin_latency,
    bench_unpin_latency,
    bench_pin_unpin_roundtrip
);
criterion_main!(benches);
