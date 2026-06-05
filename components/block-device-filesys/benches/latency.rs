//! Criterion benchmarks for sync/async IO latency.
//!
//! Measures command construction overhead and actual file-backed IO latency
//! at varying queue depths.
//!
//! Run with: `cargo bench --bench latency`

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use interfaces::Command;

/// Benchmark command construction at varying queue depths.
fn command_construction_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("command_construction");
    for &depth in &[1, 4, 16, 64] {
        group.bench_with_input(
            BenchmarkId::new("write_zeros", depth),
            &depth,
            |b, &_depth| {
                b.iter(|| {
                    let _cmd = Command::WriteZeros {
                        ns_id: 1,
                        lba: 0,
                        num_blocks: 8,
                    };
                });
            },
        );
    }
    group.finish();
}

/// Benchmark synchronous IO latency with a file-backed device.
fn sync_io_latency(c: &mut Criterion) {
    use block_device_filesys::BlockDeviceFilesysComponent;
    use interfaces::{DmaBuffer, IBlockDevice};
    use std::sync::{Arc, Mutex};

    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("bench-latency.img");
    let path_str = path.to_str().unwrap();

    let comp = BlockDeviceFilesysComponent::create(path_str, 4096, 4096);
    comp.initialize().expect("init failed");
    let channels = comp.connect_client().expect("connect failed");

    let mut group = c.benchmark_group("sync_io_latency");

    // Write latency
    group.bench_function("write_4k", |b| {
        let data = vec![0xABu8; 4096];
        let buf = unsafe {
            DmaBuffer::from_raw(
                Box::into_raw(data.into_boxed_slice()) as *mut std::ffi::c_void,
                4096,
                libc_free,
                -1,
            )
            .unwrap()
        };
        let buf_arc = Arc::new(buf);

        b.iter(|| {
            channels
                .command_tx
                .send(Command::WriteSync {
                    ns_id: 1,
                    lba: 0,
                    buf: Arc::clone(&buf_arc),
                })
                .unwrap();
            let _completion = channels.completion_rx.recv().unwrap();
        });
    });

    // Read latency
    group.bench_function("read_4k", |b| {
        let data = vec![0u8; 4096];
        let buf = unsafe {
            DmaBuffer::from_raw(
                Box::into_raw(data.into_boxed_slice()) as *mut std::ffi::c_void,
                4096,
                libc_free,
                -1,
            )
            .unwrap()
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
    });

    group.finish();
    comp.shutdown().expect("shutdown failed");
}

unsafe extern "C" fn libc_free(ptr: *mut std::ffi::c_void) {
    libc::free(ptr);
}

criterion_group!(benches, command_construction_latency, sync_io_latency);
criterion_main!(benches);
