//! Criterion benchmarks for batch write throughput at varying batch sizes.
//!
//! Measures throughput at batch sizes 1, 8, 32, 128 for 4KB blocks.
//!
//! Run with: `cargo bench --bench throughput`

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use interfaces::Command;

/// Benchmark batch command construction throughput.
fn batch_construction_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_construction");

    for &batch_size in &[1usize, 8, 32, 128] {
        let bytes = batch_size as u64 * 4096;
        group.throughput(Throughput::Bytes(bytes));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &size| {
                b.iter(|| {
                    let ops: Vec<Command> = (0..size)
                        .map(|i| Command::WriteZeros {
                            ns_id: 1,
                            lba: i as u64,
                            num_blocks: 8,
                        })
                        .collect();
                    let _cmd = Command::BatchSubmit { ops };
                });
            },
        );
    }
    group.finish();
}

/// Benchmark write throughput with actual file IO.
fn write_throughput(c: &mut Criterion) {
    use block_device_filesys::BlockDeviceFilesysComponent;
    use interfaces::IBlockDevice;

    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("bench-throughput.img");
    let path_str = path.to_str().unwrap();

    // 16MB device (4096 × 4096-byte blocks)
    let comp = BlockDeviceFilesysComponent::create(path_str, 4096, 4096);
    comp.initialize().expect("init failed");
    let channels = comp.connect_client().expect("connect failed");

    let mut group = c.benchmark_group("write_throughput");

    for &num_blocks in &[1u32, 8, 32, 128] {
        let total_bytes = num_blocks as u64 * 4096;
        group.throughput(Throughput::Bytes(total_bytes));
        group.bench_with_input(
            BenchmarkId::from_parameter(num_blocks),
            &num_blocks,
            |b, &blocks| {
                b.iter(|| {
                    channels
                        .command_tx
                        .send(Command::WriteZeros {
                            ns_id: 1,
                            lba: 0,
                            num_blocks: blocks,
                        })
                        .unwrap();
                    let _completion = channels.completion_rx.recv().unwrap();
                });
            },
        );
    }

    group.finish();
    comp.shutdown().expect("shutdown failed");
}

criterion_group!(benches, batch_construction_throughput, write_throughput);
criterion_main!(benches);
