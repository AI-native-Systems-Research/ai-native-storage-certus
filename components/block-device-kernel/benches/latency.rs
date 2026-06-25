//! Criterion benchmarks for sync/async IO latency on a kernel block device.
//!
//! Run with: `cargo bench --bench latency -p block-device-kernel`
//!
//! Set `BENCH_DEVICE_PATH` to override the default /dev/nvme0n1:
//!   BENCH_DEVICE_PATH=/dev/sda cargo bench --bench latency -p block-device-kernel

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use interfaces::Command;

fn bench_device() -> String {
    std::env::var("BENCH_DEVICE_PATH").unwrap_or_else(|_| "/dev/nvme0n1".to_string())
}

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

/// Benchmark synchronous IO latency with a real block device.
fn sync_io_latency(c: &mut Criterion) {
    use block_device_kernel::BlockDeviceKernelComponent;
    use interfaces::{DmaBuffer, IBlockDevice};
    use std::sync::{Arc, Mutex};

    let device = bench_device();
    let comp = BlockDeviceKernelComponent::create(&device, 4096, 4096);
    comp.initialize().expect("init failed");
    let channels = comp.connect_client().expect("connect failed");

    let mut group = c.benchmark_group("sync_io_latency");

    // Write latency
    group.bench_function("write_4k", |b| {
        let buf = unsafe {
            let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            let ret = libc::posix_memalign(&mut ptr, 512, 4096);
            assert_eq!(ret, 0, "posix_memalign failed");
            std::ptr::write_bytes(ptr as *mut u8, 0xAB, 4096);
            DmaBuffer::from_raw(ptr, 4096, libc_free, -1).unwrap()
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
        let buf = unsafe {
            let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            let ret = libc::posix_memalign(&mut ptr, 512, 4096);
            assert_eq!(ret, 0, "posix_memalign failed");
            std::ptr::write_bytes(ptr as *mut u8, 0, 4096);
            DmaBuffer::from_raw(ptr, 4096, libc_free, -1).unwrap()
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
