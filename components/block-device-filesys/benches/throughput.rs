//! Criterion benchmarks for read/write throughput at varying block counts.
//!
//! Measures sequential read and write throughput at 1, 8, 32, 128 × 4KB blocks.
//!
//! Run with: `cargo bench --bench throughput`
//!
//! Set `BENCH_FILE_PATH` to use a specific backing file instead of a tempdir:
//!   BENCH_FILE_PATH=/mnt/nvme/bench.img cargo bench --bench throughput

use std::path::PathBuf;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use interfaces::Command;

fn bench_file_path(default_name: &str) -> (PathBuf, Option<tempfile::TempDir>) {
    if let Ok(p) = std::env::var("BENCH_FILE_PATH") {
        (PathBuf::from(p), None)
    } else {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join(default_name);
        (path, Some(dir))
    }
}

/// Benchmark write throughput with actual file IO.
fn write_throughput(c: &mut Criterion) {
    use block_device_filesys::BlockDeviceFilesysComponent;
    use interfaces::IBlockDevice;

    let (path, _dir) = bench_file_path("bench-throughput.img");
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

/// Benchmark read throughput with actual file IO.
fn read_throughput(c: &mut Criterion) {
    use block_device_filesys::BlockDeviceFilesysComponent;
    use interfaces::{DmaBuffer, IBlockDevice};
    use std::sync::{Arc, Mutex};

    let (path, _dir) = bench_file_path("bench-throughput-read.img");
    let path_str = path.to_str().unwrap();

    // 16MB device (4096 × 4096-byte blocks)
    let comp = BlockDeviceFilesysComponent::create(path_str, 4096, 4096);
    comp.initialize().expect("init failed");
    let channels = comp.connect_client().expect("connect failed");

    // Pre-fill the device so reads return real data
    channels
        .command_tx
        .send(Command::WriteZeros {
            ns_id: 1,
            lba: 0,
            num_blocks: 128,
        })
        .unwrap();
    let _ = channels.completion_rx.recv().unwrap();

    let mut group = c.benchmark_group("read_throughput");

    for &num_blocks in &[1u32, 8, 32, 128] {
        let total_bytes = num_blocks as u64 * 4096;
        group.throughput(Throughput::Bytes(total_bytes));
        group.bench_with_input(
            BenchmarkId::from_parameter(num_blocks),
            &num_blocks,
            |b, &blocks| {
                let buf_size = blocks as usize * 4096;
                let buf = unsafe {
                    let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
                    let ret = libc::posix_memalign(&mut ptr, 512, buf_size);
                    assert_eq!(ret, 0, "posix_memalign failed");
                    std::ptr::write_bytes(ptr as *mut u8, 0, buf_size);
                    DmaBuffer::from_raw(ptr, buf_size, libc_free, -1).unwrap()
                };
                let buf_arc = Arc::new(Mutex::new(buf));

                b.iter(|| {
                    channels
                        .command_tx
                        .send(Command::ReadSync {
                            ns_id: 1,
                            lba: 0,
                            buf: Arc::clone(&buf_arc),
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

unsafe extern "C" fn libc_free(ptr: *mut std::ffi::c_void) {
    libc::free(ptr);
}

criterion_group!(benches, write_throughput, read_throughput);
criterion_main!(benches);
